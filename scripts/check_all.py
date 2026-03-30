#!/usr/bin/env python3
"""check_all.py — Modern Format Boost workspace auditor."""

import os
import re
import sys
import subprocess
import shutil
import time
import argparse
import plistlib
from pathlib import Path
from dataclasses import dataclass, field
from functools import lru_cache

try:
    from rich.console import Console
    from rich.table import Table
    from rich.panel import Panel
    from rich.markup import escape

    console = Console()
except ImportError:
    console = None

    def escape(s: str) -> str:  # type: ignore[misc]
        return s


# ---------------------------------------------------------------------------
# Core helpers
# ---------------------------------------------------------------------------

_RICH_TAG = re.compile(r"\[/?[^\]]+\]")


def cprint(msg: str) -> None:
    if console:
        console.print(msg)
    else:
        print(_RICH_TAG.sub("", msg))


@lru_cache(maxsize=None)
def _has_command(cmd: str) -> bool:
    return shutil.which(cmd) is not None


@lru_cache(maxsize=None)
def _has_cargo_sub(sub: str) -> bool:
    try:
        return (
            subprocess.run(["cargo", sub, "--version"], capture_output=True).returncode
            == 0
        )
    except Exception:
        return False


def has_command(cmd: str, hint_pkg: str | None = None, verbose: bool = False) -> bool:
    found = _has_command(cmd)
    if not found and verbose:
        cprint(
            f"  [yellow]Hint: '{cmd}' not found. Install: brew/npm/pip install {hint_pkg or cmd}[/yellow]"
        )
    return found


def has_cargo_subcommand(sub: str, verbose: bool = False) -> bool:
    found = _has_cargo_sub(sub)
    if not found and verbose:
        cprint(
            f"  [yellow]Hint: cargo-{sub} not found. Install: cargo install cargo-{sub}[/yellow]"
        )
    return found


def get_repo_root() -> Path:
    try:
        root = subprocess.check_output(
            ["git", "rev-parse", "--show-toplevel"], stderr=subprocess.STDOUT, text=True
        ).strip()
        return Path(root)
    except Exception:
        return Path(__file__).parent.parent.resolve()


def format_duration(seconds: float) -> str:
    if seconds < 1:
        return f"{int(seconds * 1000)}ms"
    return f"{seconds:.2f}s"


# ---------------------------------------------------------------------------
# Tracker
# ---------------------------------------------------------------------------


@dataclass
class Tracker:
    step_count: int = 0
    passed: int = 0
    failed: int = 0
    warned: int = 0
    skipped: int = 0
    failed_steps: list[str] = field(default_factory=list)
    warned_steps: list[str] = field(default_factory=list)
    skipped_steps: list[str] = field(default_factory=list)

    def announce_step(self, kind: str, name: str) -> None:
        self.step_count += 1
        icon = "🔍" if kind == "required" else "💡"
        cprint(
            f"\n[bold][{self.step_count}] {icon} {kind.upper()}: {escape(name)}[/bold]"
        )


# ---------------------------------------------------------------------------
# Step runners
# ---------------------------------------------------------------------------


def run_step(
    tracker: Tracker,
    kind: str,
    name: str,
    cmd: list[str],
    env_vars: dict | None = None,
) -> bool:
    tracker.announce_step(kind, name)
    start = time.time()

    env = os.environ.copy()
    if env_vars:
        env.update(env_vars)

    # stdout/stderr merged and streamed; fully consumed before wait() to avoid pipe deadlock.
    proc = subprocess.Popen(
        cmd,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
        env=env,
        bufsize=1,
    )
    if proc.stdout:
        for line in proc.stdout:
            sys.stdout.write(line)
            sys.stdout.flush()
    proc.wait()
    duration = format_duration(time.time() - start)

    if proc.returncode == 0:
        tracker.passed += 1
        cprint(f"  [green]✅ PASS[/green] ({duration})")
        return True

    if kind == "required":
        tracker.failed += 1
        tracker.failed_steps.append(name)
        cprint(f"  [red]❌ FAIL[/red] (exit {proc.returncode}, {duration})")
    else:
        tracker.warned += 1
        tracker.warned_steps.append(name)
        cprint(f"  [yellow]⚠️  WARN[/yellow] (exit {proc.returncode}, {duration})")
    return False


def skip_step(tracker: Tracker, name: str, reason: str) -> None:
    tracker.announce_step("optional", name)
    tracker.skipped += 1
    tracker.skipped_steps.append(f"{name} ({reason})")
    cprint(f"  [blue]⏭️  SKIP[/blue] ({reason})")


# ---------------------------------------------------------------------------
# AI smell check
# ---------------------------------------------------------------------------

# Agentic task prompt for Claude Code / Gemini CLI.
# The agent has filesystem access and will enumerate + read .rs files itself.
AI_SMELL_PROMPT = """\
You are performing a focused code quality audit on the Rust workspace in the current directory.

Task: identify "AI smell" — patterns typical of AI-generated code that lacks genuine understanding
of this project's constraints and conventions.

Step 1: enumerate tracked Rust files with `git ls-files '*.rs'`, then read each one.

Patterns to flag:
  (a) redundant intermediate variables — `x = foo(); y = x; return y`
  (b) single-use constants that should be inlined
  (c) `.clone()` where a reference or move suffices
  (d) `Box<dyn Error>` when the crate already defines a typed error enum
  (e) over-defensive null/bounds checks where call-site invariants already hold
  (f) error handlers that swallow context (log + return None/default)
  (g) docstrings that only restate the function signature
  (h) section-banner comments: `// ======= SECTION =======`
  (i) Strategy/Factory abstraction for ≤2 cases
  (j) TODO comments that observe a problem without any plan or owner

Report format — one line per finding:
  <file>:<line>  [<category-letter>]  <what> → <concrete fix>

After findings (or the single line "No AI smell detected"), update CHANGELOG.md:
  - Prepend a new H2 section: `## [{date}] Code Quality`
  - Under `### Changed`, list each finding as a bullet (file:line, category, one-sentence description)
  - Do NOT modify any existing content below the new entry

Constraints:
  - No preamble, no closing remarks
  - Do not rewrite files other than CHANGELOG.md
  - If CHANGELOG.md does not exist, create it with only the new entry
""".format(date=time.strftime("%Y-%m-%d"))


def check_ai_smell(tracker: Tracker, repo_root: Path) -> bool:
    tracker.announce_step("optional", "AI smell detection + CHANGELOG update")

    agent = next((c for c in ("claude", "gemini") if _has_command(c)), None)
    if agent is None:
        tracker.skipped += 1
        tracker.skipped_steps.append("AI smell (no agent CLI)")
        cprint("  [blue]⏭️  SKIP[/blue] (neither 'claude' nor 'gemini' CLI found)")
        return True

    result = subprocess.run(
        [agent, "--print", AI_SMELL_PROMPT],
        capture_output=True,
        text=True,
        cwd=str(repo_root),
    )

    if result.stdout.strip():
        cprint("\n[dim]── AI Smell Report ──[/dim]")
        print(result.stdout)

    if result.returncode == 0:
        tracker.passed += 1
        cprint("  [green]✅ PASS[/green]")
        return True

    tracker.warned += 1
    tracker.warned_steps.append("AI smell detection")
    cprint(f"  [yellow]⚠️  WARN[/yellow] (exit {result.returncode})")
    return False


# ---------------------------------------------------------------------------
# macOS bundle metadata check
# ---------------------------------------------------------------------------


def check_bundle_metadata(tracker: Tracker) -> bool:
    tracker.announce_step("required", "macOS App bundle metadata")

    if sys.platform != "darwin":
        tracker.skipped += 1
        cprint("  [dim]Skipped: non-macOS platform[/dim]")
        return True

    root = get_repo_root()
    plist_path = root / "Modern Format Boost.app" / "Contents" / "Info.plist"
    cargo_path = root / "Cargo.toml"

    if not plist_path.exists():
        tracker.failed += 1
        cprint(f"  [red]❌ Info.plist not found at {plist_path}[/red]")
        return False

    try:
        cargo_content = cargo_path.read_text(encoding="utf-8")
        m = re.search(
            r'\[workspace\.package\]\s*version\s*=\s*"([^"]+)"', cargo_content
        )
        if not m:
            raise ValueError("Could not find [workspace.package] version in Cargo.toml")
        workspace_version = m.group(1)

        with open(plist_path, "rb") as f:
            pl = plistlib.load(f)

        errors: list[str] = []
        bundle_version = pl.get("CFBundleShortVersionString")
        if bundle_version != workspace_version:
            errors.append(
                f"Version mismatch: Cargo.toml={workspace_version} vs Info.plist={bundle_version}"
            )

        executable = pl.get("CFBundleExecutable")
        if executable != "Modern Format Boost":
            errors.append(
                f"Executable name mismatch: expected 'Modern Format Boost', got '{executable}'"
            )

        binary_path = (
            root
            / "Modern Format Boost.app"
            / "Contents"
            / "MacOS"
            / "Modern Format Boost"
        )
        if not binary_path.exists():
            errors.append(f"App binary wrapper missing at {binary_path}")

        if errors:
            tracker.failed += 1
            for err in errors:
                cprint(f"  [red]❌ {err}[/red]")
            return False

        tracker.passed += 1
        cprint(f"  [green]✅ Verified: Version {workspace_version} aligned[/green]")
        return True

    except Exception as e:
        tracker.failed += 1
        cprint(f"  [red]❌ Audit error: {e}[/red]")
        return False


# ---------------------------------------------------------------------------
# Summary
# ---------------------------------------------------------------------------


def print_summary(tracker: Tracker) -> None:
    if console:
        table = Table(
            title="\n📊 Code Quality Summary", border_style="dim", expand=True
        )
        table.add_column("Category", style="cyan")
        table.add_column("Count", justify="right")
        table.add_column("Details", style="dim")
        table.add_row("✅ Passed", str(tracker.passed), "[green]All clear[/green]")
        table.add_row(
            "❌ Failed",
            str(tracker.failed),
            f"[red]{escape(', '.join(tracker.failed_steps))}[/red]"
            if tracker.failed_steps
            else "-",
        )
        table.add_row(
            "⚠️  Warned",
            str(tracker.warned),
            f"[yellow]{escape(', '.join(tracker.warned_steps))}[/yellow]"
            if tracker.warned_steps
            else "-",
        )
        table.add_row(
            "⏭️  Skipped",
            str(tracker.skipped),
            f"[blue]{len(tracker.skipped_steps)} items[/blue]"
            if tracker.skipped_steps
            else "-",
        )
        console.print(table)

        if tracker.skipped_steps:
            console.print("\n[bold blue]Skipped:[/bold blue]")
            for s in tracker.skipped_steps:
                console.print(f"  [dim]• {s}[/dim]")
        if tracker.warned_steps:
            console.print("\n[bold yellow]Warnings in:[/bold yellow]")
            for s in tracker.warned_steps:
                console.print(f"  [yellow]! {s}[/yellow]")
    else:
        print(f"\n{'=' * 40}\nSummary\n{'=' * 40}")
        print(
            f"Passed: {tracker.passed}  Failed: {tracker.failed}  Warned: {tracker.warned}  Skipped: {tracker.skipped}"
        )
        for label, items, prefix in [
            ("REQUIRED FAILURES", tracker.failed_steps, "[X]"),
            ("OPTIONAL WARNINGS", tracker.warned_steps, "[!]"),
            ("SKIPPED CHECKS", tracker.skipped_steps, "[-]"),
        ]:
            if items:
                print(f"\n{label}:")
                for s in items:
                    print(f"  {prefix} {s}")


# ---------------------------------------------------------------------------
# Argument parsing
# ---------------------------------------------------------------------------


def parse_args() -> argparse.Namespace:
    default_branch = os.environ.get("CHECK_ALL_DEFAULT_BRANCH", "nightly")
    parser = argparse.ArgumentParser(
        description="Modern Format Boost Multi-Language Auditor"
    )
    parser.add_argument(
        "--allow-non-nightly", action="store_true", help="Don't enforce branch check"
    )
    parser.add_argument(
        "--required-only", action="store_true", help="Skip optional checks"
    )
    parser.add_argument(
        "--no-expensive", action="store_true", help="Skip slow checks (bloat, hack)"
    )
    parser.add_argument("--fix", action="store_true", help="Auto-fix formatting issues")
    parser.add_argument("--build", action="store_true", help="Run full release build")
    parser.add_argument(
        "--ai-smell",
        action="store_true",
        help="Run AI smell detection + CHANGELOG update via claude/gemini CLI",
    )
    parser.add_argument(
        "--branch",
        default=default_branch,
        help=f"Required branch (default: {default_branch})",
    )
    parser.add_argument(
        "--verbose", "-v", action="store_true", help="Show tool install hints"
    )
    return parser.parse_args()


# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------


def main() -> None:
    args = parse_args()
    repo_root = get_repo_root()
    os.chdir(repo_root)
    tracker = Tracker()

    if console:
        console.print(
            Panel(
                f"[bold cyan]🚀 Modern Quality Suite[/bold cyan]\n[dim]Root: {repo_root}[/dim]",
                border_style="blue",
            )
        )
    else:
        print(f"--- Modern Quality Suite ---\nRoot: {repo_root}")

    # Branch guard
    try:
        current_branch = subprocess.check_output(
            ["git", "rev-parse", "--abbrev-ref", "HEAD"], text=True
        ).strip()
        if not args.allow_non_nightly and current_branch != args.branch:
            print(
                f"Fatal: required branch '{args.branch}', current is '{current_branch}'. "
                "Use --allow-non-nightly or --branch <name>.",
                file=sys.stderr,
            )
            sys.exit(2)
    except subprocess.CalledProcessError as e:
        # git unavailable or not a repo — warn and continue
        cprint(f"[yellow]Warning: could not determine git branch ({e})[/yellow]")

    # File discovery via git index; fallback to recursive walk
    try:
        git_files = subprocess.check_output(["git", "ls-files"], text=True).splitlines()
    except Exception:
        git_files = [
            str(p.relative_to(repo_root)) for p in repo_root.rglob("*") if p.is_file()
        ]

    py_files = [f for f in git_files if f.endswith(".py")]
    shell_files = [f for f in git_files if f.endswith(".sh")]
    md_files = [f for f in git_files if f.endswith(".md")]
    json_files = [f for f in git_files if f.endswith(".json")]
    yaml_files = [f for f in git_files if f.endswith((".yml", ".yaml"))]
    toml_files = [f for f in git_files if f.endswith(".toml")]

    # Auto-fix phase
    if args.fix:
        cprint("\n[bold cyan]🔧 Running Auto-Fix Cycle...[/bold cyan]")

        subprocess.run(["cargo", "fmt", "--all"])
        subprocess.run(
            [
                "cargo",
                "clippy",
                "--fix",
                "--workspace",
                "--all-targets",
                "--all-features",
                "--allow-dirty",
                "--allow-staged",
            ]
        )

        if has_command("ruff"):
            subprocess.run(["ruff", "check", "--fix", "."], stderr=subprocess.DEVNULL)
            subprocess.run(["ruff", "format", "."], stderr=subprocess.DEVNULL)

        if has_command("pyupgrade") and py_files:
            subprocess.run(["pyupgrade", "--py311-plus"] + py_files)

        fmt_targets = md_files + json_files + yaml_files
        if fmt_targets and has_command("prettier"):
            subprocess.run(
                ["prettier", "--write"] + fmt_targets, stderr=subprocess.DEVNULL
            )

        if has_cargo_subcommand("taplo"):
            subprocess.run(["cargo", "taplo", "fmt"])

        if has_command("kondo"):
            kondo_cmd = ["kondo", "-a"]
            if sys.platform == "darwin":
                kondo_cmd.extend(
                    ["-I", "/Volumes", "-I", os.path.expanduser("~/Library")]
                )
            subprocess.run(kondo_cmd + [str(repo_root)], stdout=subprocess.DEVNULL)

    # Required checks
    run_step(
        tracker, "required", "cargo fmt --check", ["cargo", "fmt", "--all", "--check"]
    )
    run_step(
        tracker,
        "required",
        "cargo check --workspace --all-features",
        ["cargo", "check", "--workspace", "--all-targets", "--all-features"],
    )

    if py_files:
        run_step(
            tracker,
            "required",
            f"python3 syntax ({len(py_files)} files)",
            [sys.executable, "-m", "py_compile"] + py_files,
        )
    else:
        skip_step(tracker, "python syntax", "no scripts")

    run_step(
        tracker,
        "required",
        "cargo clippy (all-features)",
        [
            "cargo",
            "clippy",
            "--workspace",
            "--all-targets",
            "--all-features",
            "--",
            "-D",
            "warnings",
        ],
    )

    if has_cargo_subcommand("nextest"):
        run_step(
            tracker,
            "required",
            "cargo nextest run",
            ["cargo", "nextest", "run", "--workspace", "--all-features"],
        )
    else:
        run_step(
            tracker,
            "required",
            "cargo test",
            ["cargo", "test", "--workspace", "--all-features"],
        )

    if args.build:
        run_step(
            tracker,
            "required",
            "cargo build --release",
            ["cargo", "build", "--release", "--workspace"],
        )

    # Optional checks
    if not args.required_only:
        if py_files and has_command("ruff", verbose=args.verbose):
            run_step(tracker, "optional", "ruff linter", ["ruff", "check"] + py_files)
            run_step(
                tracker,
                "optional",
                "ruff format check",
                ["ruff", "format", "--check"] + py_files,
            )
        elif not py_files:
            skip_step(tracker, "python quality", "no scripts")

        if shell_files:
            if has_command("shellcheck", verbose=args.verbose):
                run_step(
                    tracker,
                    "optional",
                    "shellcheck",
                    ["shellcheck", "--severity=error"] + shell_files,
                )
            if has_command("shfmt", verbose=args.verbose):
                run_step(
                    tracker,
                    "optional",
                    "shfmt layout check",
                    ["shfmt", "-d", "-i", "4"] + shell_files,
                )

        check_bundle_metadata(tracker)

        if md_files and has_command("markdownlint-cli2", verbose=args.verbose):
            config_path = os.path.join(repo_root, "scripts/config/.markdownlint-cli2.jsonc")
            run_step(
                tracker,
                "optional",
                "markdownlint",
                ["markdownlint-cli2", "--config", config_path] + md_files,
            )

        targets = md_files + json_files + yaml_files
        if targets and has_command("prettier", verbose=args.verbose):
            run_step(
                tracker, "optional", "prettier check", ["prettier", "--check"] + targets
            )

        if toml_files and has_cargo_subcommand("taplo", verbose=args.verbose):
            run_step(
                tracker,
                "optional",
                "taplo fmt check",
                ["cargo", "taplo", "fmt", "--check"],
            )

        run_step(
            tracker,
            "optional",
            "cargo doc",
            ["cargo", "doc", "--workspace", "--no-deps"],
        )

        if has_cargo_subcommand("audit", verbose=args.verbose):
            run_step(tracker, "optional", "cargo audit", ["cargo", "audit"])

        if not args.no_expensive:
            if has_cargo_subcommand("bloat", verbose=args.verbose):
                run_step(
                    tracker,
                    "optional",
                    "cargo bloat",
                    ["cargo", "bloat", "--release", "--crates", "-n", "10"],
                )
            if has_cargo_subcommand("hack", verbose=args.verbose):
                run_step(
                    tracker,
                    "optional",
                    "cargo hack feature matrix",
                    [
                        "cargo",
                        "hack",
                        "check",
                        "--workspace",
                        "--each-feature",
                        "--no-dev-deps",
                    ],
                )

        if args.ai_smell:
            check_ai_smell(tracker, repo_root)

    # Summary
    print_summary(tracker)

    if tracker.failed > 0:
        cprint(f"\n[bold red]Audit failed with {tracker.failed} error(s).[/bold red]")
        sys.exit(1)

    cprint("\n[bold green]🌟 Workspace is healthy![/bold green]")


if __name__ == "__main__":
    main()
