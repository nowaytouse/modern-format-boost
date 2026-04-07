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


# ── Core helpers ───────────────────────────────────────────────────────────────

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
            f"  [yellow]Hint: '{cmd}' not found."
            f" Install: brew/npm/pip install {hint_pkg or cmd}[/yellow]"
        )
    return found


def has_cargo_subcommand(sub: str, verbose: bool = False) -> bool:
    found = _has_cargo_sub(sub)
    if not found and verbose:
        cprint(
            f"  [yellow]Hint: cargo-{sub} not found."
            f" Install: cargo install cargo-{sub}[/yellow]"
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


# ── Nightly toolchain probe ────────────────────────────────────────────────────

# Full set of nightly components this script knows about.
_NIGHTLY_COMPONENTS = ("clippy", "rustfmt", "miri", "rust-src", "llvm-tools")


@dataclass
class NightlyComponents:
    """Snapshot of which nightly rustup components are currently installed."""

    toolchain: bool = False  # nightly toolchain is present at all
    clippy: bool = False
    rustfmt: bool = False
    miri: bool = False  # Miri interpreter — UB / memory-safety checker
    rust_src: bool = False  # required by miri and rust-analyzer
    llvm_tools: bool = False  # required for coverage (cargo-llvm-cov)

    @property
    def any(self) -> bool:
        return self.toolchain

    def missing_components(self) -> list[str]:
        mapping = {
            "clippy": self.clippy,
            "rustfmt": self.rustfmt,
            "miri": self.miri,
            "rust-src": self.rust_src,
            "llvm-tools": self.llvm_tools,
        }
        return [k for k, v in mapping.items() if not v]

    def install_hint(self) -> str:
        missing = self.missing_components()
        if not missing:
            return ""
        components = " ".join(f"--component {c}" for c in missing)
        return f"rustup toolchain install nightly {components}"

    def rich_badge(self) -> str:
        if not self.toolchain:
            return "[red]nightly: ✗ not installed[/red]"
        parts = []
        for label, ok in [
            ("clippy", self.clippy),
            ("rustfmt", self.rustfmt),
            ("miri", self.miri),
            ("rust-src", self.rust_src),
            ("llvm-tools", self.llvm_tools),
        ]:
            color = "green" if ok else "yellow"
            parts.append(f"[{color}]{label}[/{color}]")
        return "nightly: " + " ".join(parts)


@lru_cache(maxsize=None)
def probe_nightly() -> NightlyComponents:
    """Detect nightly toolchain and all relevant components in one rustup call."""
    nc = NightlyComponents()

    if not _has_command("rustup"):
        return nc

    # 1. Is the nightly toolchain installed?
    r = subprocess.run(
        ["rustup", "run", "nightly", "rustc", "--version"],
        capture_output=True,
    )
    if r.returncode != 0:
        return nc
    nc.toolchain = True

    # 2. Which components are installed?
    r = subprocess.run(
        ["rustup", "component", "list", "--installed", "--toolchain", "nightly"],
        capture_output=True,
        text=True,
    )
    if r.returncode != 0:
        # Toolchain present but query failed — treat components as missing.
        return nc

    installed = r.stdout
    nc.clippy = "clippy" in installed
    nc.rustfmt = "rustfmt" in installed
    nc.miri = "miri" in installed
    nc.rust_src = "rust-src" in installed
    nc.llvm_tools = "llvm-tools" in installed
    return nc


def install_nightly(components: list[str] | None = None) -> bool:
    """
    Run `rustup toolchain install nightly --component ...`.
    If *components* is None, installs the full recommended set.
    Clears the probe cache so the next probe_nightly() reflects new state.
    Returns True on success.
    """
    target = components or list(_NIGHTLY_COMPONENTS)
    cmd = ["rustup", "toolchain", "install", "nightly"]
    for c in target:
        cmd += ["--component", c]

    cprint(f"  [dim]Running: {' '.join(cmd)}[/dim]")
    r = subprocess.run(cmd)
    probe_nightly.cache_clear()
    _has_command.cache_clear()
    return r.returncode == 0


# ── Tracker ────────────────────────────────────────────────────────────────────


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
        icon = ">" if kind == "required" else "*"
        cprint(
            f"\n[bold][{self.step_count}] {icon} {kind.upper()}: {escape(name)}[/bold]"
        )


# ── Step runners ───────────────────────────────────────────────────────────────


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
        cprint(f"  [green]OK[/green] ({duration})")
        return True

    if kind == "required":
        tracker.failed += 1
        tracker.failed_steps.append(name)
        cprint(f"  [red]FAIL[/red] (exit {proc.returncode}, {duration})")
    else:
        tracker.warned += 1
        tracker.warned_steps.append(name)
        cprint(f"  [yellow]WARN[/yellow] (exit {proc.returncode}, {duration})")
    return False


def skip_step(tracker: Tracker, name: str, reason: str) -> None:
    tracker.announce_step("optional", name)
    tracker.skipped += 1
    tracker.skipped_steps.append(f"{name} ({reason})")
    cprint(f"  [blue]SKIP[/blue] ({reason})")


# ── AI smell check ─────────────────────────────────────────────────────────────

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
        cprint("  [blue]SKIP[/blue] (neither 'claude' nor 'gemini' CLI found)")
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
        cprint("  [green]OK[/green]")
        return True

    tracker.warned += 1
    tracker.warned_steps.append("AI smell detection")
    cprint(f"  [yellow]WARN[/yellow] (exit {result.returncode})")
    return False


# ── Metadata checks ────────────────────────────────────────────────────────────


def check_changelog_sync(tracker: Tracker) -> bool:
    tracker.announce_step("required", "CHANGELOG version synchronization")
    root = get_repo_root()
    cargo_path = root / "Cargo.toml"
    changelog_path = root / "CHANGELOG.md"

    if not changelog_path.exists():
        tracker.failed += 1
        cprint("  [red]FAIL: CHANGELOG.md missing[/red]")
        return False

    try:
        cargo_content = cargo_path.read_text(encoding="utf-8")
        m = re.search(
            r'\[workspace\.package\]\s*version\s*=\s*"([^"]+)"', cargo_content
        )
        if not m:
            cprint(
                "  [yellow]Skipped: could not find workspace version in Cargo.toml[/yellow]"
            )
            return True

        version = m.group(1)
        changelog_content = changelog_path.read_text(encoding="utf-8")

        pattern = rf"##\s*\[v?{re.escape(version)}\]"
        if not re.search(pattern, changelog_content):
            tracker.failed += 1
            cprint(
                f"  [red]FAIL: Version '{version}' not found as a header in CHANGELOG.md[/red]"
            )
            return False

        tracker.passed += 1
        cprint(f"  [green]OK: {version} is documented[/green]")
        return True

    except Exception as e:
        tracker.failed += 1
        cprint(f"  [red]FAIL: Changelog check error: {e}[/red]")
        return False


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
        cprint(f"  [red]FAIL: Info.plist not found at {plist_path}[/red]")
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
                f"Version mismatch: Cargo.toml={workspace_version}"
                f" vs Info.plist={bundle_version}"
            )

        executable = pl.get("CFBundleExecutable")
        if executable != "Modern Format Boost":
            errors.append(
                f"Executable name mismatch: expected 'Modern Format Boost',"
                f" got '{executable}'"
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
                cprint(f"  [red]FAIL: {err}[/red]")
            return False

        tracker.passed += 1
        cprint(f"  [green]OK: Version {workspace_version} aligned[/green]")
        return True

    except Exception as e:
        tracker.failed += 1
        cprint(f"  [red]FAIL: Audit error: {e}[/red]")
        return False


# ── Summary ────────────────────────────────────────────────────────────────────


def print_summary(tracker: Tracker) -> None:
    if console:
        table = Table(title="\nCode Quality Summary", border_style="dim", expand=True)
        table.add_column("Category", style="cyan")
        table.add_column("Count", justify="right")
        table.add_column("Details", style="dim")
        table.add_row("OK", str(tracker.passed), "[green]All clear[/green]")
        table.add_row(
            "FAIL",
            str(tracker.failed),
            f"[red]{escape(', '.join(tracker.failed_steps))}[/red]"
            if tracker.failed_steps
            else "-",
        )
        table.add_row(
            "WARN",
            str(tracker.warned),
            f"[yellow]{escape(', '.join(tracker.warned_steps))}[/yellow]"
            if tracker.warned_steps
            else "-",
        )
        table.add_row(
            "SKIP",
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
            f"Passed: {tracker.passed}  Failed: {tracker.failed}"
            f"  Warned: {tracker.warned}  Skipped: {tracker.skipped}"
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


# ── Argument parsing ───────────────────────────────────────────────────────────


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
        "--no-expensive",
        action="store_true",
        help="Skip slow checks (bloat, hack, llvm-cov, mutants)",
    )
    parser.add_argument("--fix", action="store_true", help="Auto-fix formatting issues")
    parser.add_argument("--build", action="store_true", help="Run full release build")
    parser.add_argument(
        "--ai-smell",
        action="store_true",
        help="Run AI smell detection + CHANGELOG update via claude/gemini CLI",
    )
    parser.add_argument(
        "--miri",
        action="store_true",
        help=(
            "Run library tests under Miri (UB / memory-safety checker)."
            " Requires nightly + miri + rust-src components."
            " Very slow — expect 10-100× test runtime."
        ),
    )
    parser.add_argument(
        "--sanitizers",
        action="store_true",
        help=(
            "Run library tests with AddressSanitizer (nightly only)."
            " Catches heap/stack/global buffer overflows and use-after-free in"
            " unsafe code and FFI boundaries. Complements Miri for code Miri cannot reach."
        ),
    )
    parser.add_argument(
        "--mutants",
        action="store_true",
        help=(
            "Run cargo-mutants: mutation testing to gauge test suite quality."
            " Very slow (minutes–hours). Requires: cargo install cargo-mutants."
        ),
    )
    parser.add_argument(
        "--fuzz-list",
        action="store_true",
        help=(
            "Discover and list available fuzz targets via cargo-fuzz."
            " Fast (no actual fuzzing). Requires: cargo install cargo-fuzz + nightly."
        ),
    )
    parser.add_argument(
        "--install-nightly",
        action="store_true",
        help=(
            "Install/update nightly toolchain with all recommended components"
            " (clippy, rustfmt, miri, rust-src, llvm-tools) before running checks."
        ),
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


# ── Main ───────────────────────────────────────────────────────────────────────


def main() -> None:
    args = parse_args()
    repo_root = get_repo_root()
    os.chdir(repo_root)
    tracker = Tracker()

    # ── Optional: install/update nightly toolchain ─────────────────────────────
    if args.install_nightly:
        cprint("\n[bold cyan]Installing nightly toolchain + components…[/bold cyan]")
        ok = install_nightly()
        if ok:
            cprint("  [green]Nightly toolchain installed/updated successfully.[/green]")
        else:
            cprint(
                "  [red]rustup install failed — continuing with whatever is available.[/red]"
            )

    # ── Probe nightly once; all subsequent code reads from this snapshot ────────
    nc = probe_nightly()

    # ── Banner ──────────────────────────────────────────────────────────────────
    if console:
        console.print(
            Panel(
                f"Modern Quality Suite\n"
                f"[dim]Root: {repo_root}[/dim]\n"
                f"{nc.rich_badge()}",
                border_style="blue",
            )
        )
        if nc.toolchain and nc.missing_components():
            hint = nc.install_hint()
            console.print(
                f"[yellow]  Missing nightly components:"
                f" {', '.join(nc.missing_components())}[/yellow]\n"
                f"[dim]  Fix: {hint}[/dim]\n"
                f"[dim]  Or re-run with --install-nightly[/dim]"
            )
        elif not nc.toolchain:
            console.print(
                "[yellow]  Nightly toolchain not found.[/yellow]\n"
                "[dim]  Install: rustup toolchain install nightly"
                " --component clippy rustfmt miri rust-src llvm-tools[/dim]\n"
                "[dim]  Or re-run with --install-nightly[/dim]"
            )
    else:
        nc_status = (
            f"toolchain:{'OK' if nc.toolchain else 'MISSING'}"
            f" clippy:{'+' if nc.clippy else '-'}"
            f" rustfmt:{'+' if nc.rustfmt else '-'}"
            f" miri:{'+' if nc.miri else '-'}"
            f" rust-src:{'+' if nc.rust_src else '-'}"
            f" llvm-tools:{'+' if nc.llvm_tools else '-'}"
        )
        print(f"--- Modern Quality Suite ---\nRoot: {repo_root}\nNightly: {nc_status}")

    # ── Branch guard ────────────────────────────────────────────────────────────
    try:
        current_branch = subprocess.check_output(
            ["git", "rev-parse", "--abbrev-ref", "HEAD"], text=True
        ).strip()
        if not args.allow_non_nightly and current_branch != args.branch:
            print(
                f"Fatal: required branch '{args.branch}', current is '{current_branch}'. "
                "Use --allow-non-nightly or --branch <n>.",
                file=sys.stderr,
            )
            sys.exit(2)
    except subprocess.CalledProcessError as e:
        cprint(f"[yellow]Warning: could not determine git branch ({e})[/yellow]")

    # ── File discovery ──────────────────────────────────────────────────────────
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

    # ── Auto-fix phase ──────────────────────────────────────────────────────────
    if args.fix:
        # rustfmt: prefer nightly — supports `unstable_features` in rustfmt.toml.
        fmt_cmd = (
            ["cargo", "+nightly", "fmt", "--all"]
            if nc.rustfmt
            else ["cargo", "fmt", "--all"]
        )
        subprocess.run(fmt_cmd)

        # clippy --fix: prefer nightly for broader auto-fix coverage.
        clippy_fix = (
            ["cargo", "+nightly", "clippy"] if nc.clippy else ["cargo", "clippy"]
        )
        subprocess.run(
            clippy_fix
            + [
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

    # ══════════════════════════════════════════════════════════════════════════════
    # Required checks
    # ══════════════════════════════════════════════════════════════════════════════

    # ── rustfmt ──────────────────────────────────────────────────────────────────
    # Nightly rustfmt supports `unstable_features` in rustfmt.toml
    # (imports_granularity, group_imports, wrap_comments, etc.).
    if nc.rustfmt:
        run_step(
            tracker,
            "required",
            "cargo +nightly fmt --check",
            ["cargo", "+nightly", "fmt", "--all", "--check"],
        )
    else:
        run_step(
            tracker,
            "required",
            "cargo fmt --check (stable fallback)",
            ["cargo", "fmt", "--all", "--check"],
        )

    # ── cargo check ──────────────────────────────────────────────────────────────
    run_step(
        tracker,
        "required",
        "cargo check --workspace --all-features",
        ["cargo", "check", "--workspace", "--all-targets", "--all-features"],
    )

    check_changelog_sync(tracker)

    if py_files:
        run_step(
            tracker,
            "required",
            f"python3 syntax ({len(py_files)} files)",
            [sys.executable, "-m", "py_compile"] + py_files,
        )
    else:
        skip_step(tracker, "python syntax", "no scripts")

    # ── clippy (required, zero-warning policy) ───────────────────────────────────
    # Nightly catches more lints than stable; both enforce -D warnings.
    if nc.clippy:
        run_step(
            tracker,
            "required",
            "cargo +nightly clippy --workspace -D warnings",
            [
                "cargo",
                "+nightly",
                "clippy",
                "--workspace",
                "--all-targets",
                "--all-features",
                "--",
                "-D",
                "warnings",
            ],
        )
    else:
        if nc.toolchain:
            cprint(
                "  [yellow]Note: nightly clippy component missing;"
                " falling back to stable clippy.[/yellow]\n"
                f"  [dim]Fix: {nc.install_hint()}[/dim]"
            )
        run_step(
            tracker,
            "required",
            "cargo clippy --workspace -D warnings (stable fallback)",
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

    # ── tests ─────────────────────────────────────────────────────────────────────
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

    # ══════════════════════════════════════════════════════════════════════════════
    # Optional checks
    # ══════════════════════════════════════════════════════════════════════════════

    if not args.required_only:
        # ── nightly clippy: pedantic + nursery (second pass, warnings only) ───────
        # Separate from the required pass so noisy lints don't block CI.
        if nc.clippy:
            run_step(
                tracker,
                "optional",
                "cargo +nightly clippy (pedantic + nursery)",
                [
                    "cargo",
                    "+nightly",
                    "clippy",
                    "--workspace",
                    "--all-targets",
                    "--all-features",
                    "--",
                    "-W",
                    "clippy::pedantic",
                    "-W",
                    "clippy::nursery",
                    # Always-noisy lints — keep as allow to reduce false-positive volume.
                    "-A",
                    "clippy::module_name_repetitions",
                    "-A",
                    "clippy::missing_errors_doc",
                    "-A",
                    "clippy::missing_panics_doc",
                    "-A",
                    "clippy::must_use_candidate",
                ],
            )
        else:
            skip_step(
                tracker,
                "nightly clippy (pedantic + nursery)",
                "nightly clippy not installed",
            )

        # ── nightly rustfmt: unstable options ─────────────────────────────────────
        # Only meaningful when rustfmt.toml contains `unstable_features = true`.
        # Runs as optional because the stable fmt check already passed above.
        if nc.rustfmt:
            run_step(
                tracker,
                "optional",
                "cargo +nightly fmt --check (unstable options)",
                ["cargo", "+nightly", "fmt", "--all", "--check"],
            )
        else:
            skip_step(
                tracker,
                "nightly rustfmt (unstable options)",
                "nightly rustfmt not installed",
            )

        # ── Miri: UB / memory-safety (very slow, opt-in via --miri) ──────────────
        # Miri runs tests under its interpreter to catch:
        #   - use-after-free, out-of-bounds access
        #   - uninitialized memory reads
        #   - stacked/tree borrows violations
        #   - data races (with -Zmiri-preemption-rate > 0)
        #
        # Limitations: cannot call FFI, spawn processes, or do I/O beyond basic stdin/stdout.
        # Use --lib to target only unit tests that are Miri-compatible.
        if args.miri:
            if nc.miri and nc.rust_src:
                run_step(
                    tracker,
                    "optional",
                    "cargo +nightly miri test --lib",
                    [
                        "cargo",
                        "+nightly",
                        "miri",
                        "test",
                        "--workspace",
                        "--lib",
                    ],
                    env_vars={
                        # Strict provenance: catches most pointer provenance bugs.
                        # Switch to -Zmiri-tree-borrows if stacked-borrows gives
                        # false positives in unsafe code that is actually correct.
                        "MIRIFLAGS": "-Zmiri-strict-provenance",
                    },
                )
            else:
                missing = []
                if not nc.miri:
                    missing.append("miri")
                if not nc.rust_src:
                    missing.append("rust-src")
                skip_step(
                    tracker,
                    "miri",
                    f"missing: {', '.join(missing)} — run --install-nightly",
                )

        # ── AddressSanitizer: heap/stack/FFI memory errors (opt-in via --sanitizers)
        # Catches buffer overflows and use-after-free that Miri cannot reach
        # because Miri refuses to run FFI (libheif-rs, etc.).
        # aarch64-apple-darwin: ASAN works on nightly; TSAN has OS-level limitations.
        # Note: ASAN cannot run under Rosetta — requires native Apple Silicon shell.
        if args.sanitizers:
            if nc.toolchain and nc.rust_src:
                # Detect build target; default to host triple reported by rustc.
                try:
                    host_triple = subprocess.check_output(["rustc", "-vV"], text=True)
                    m = re.search(r"host:\s*(\S+)", host_triple)
                    build_target = m.group(1) if m else "aarch64-apple-darwin"
                except Exception:
                    build_target = "aarch64-apple-darwin"

                run_step(
                    tracker,
                    "optional",
                    f"cargo +nightly test --lib (AddressSanitizer, {build_target})",
                    [
                        "cargo",
                        "+nightly",
                        "test",
                        "--workspace",
                        "--lib",
                        "--target",
                        build_target,
                    ],
                    env_vars={
                        "RUSTFLAGS": "-Z sanitizer=address",
                        # Suppress known-benign leak in jemalloc/system allocator
                        # on macOS; set ASAN_OPTIONS explicitly to suppress false
                        # positives from Apple frameworks loaded at dyld time.
                        "ASAN_OPTIONS": "detect_leaks=0",
                    },
                )
            else:
                missing = []
                if not nc.toolchain:
                    missing.append("nightly toolchain")
                if not nc.rust_src:
                    missing.append("rust-src")
                skip_step(
                    tracker,
                    "AddressSanitizer",
                    f"missing: {', '.join(missing)} — run --install-nightly",
                )

        # ── llvm-cov coverage (optional, expensive) ───────────────────────────────
        # Requires: nightly llvm-tools component + cargo-llvm-cov installed.
        # Shows per-crate line coverage summary without writing HTML report.
        if not args.no_expensive and nc.llvm_tools:
            if has_cargo_subcommand("llvm-cov", verbose=args.verbose):
                run_step(
                    tracker,
                    "optional",
                    "cargo +nightly llvm-cov --summary-only",
                    [
                        "cargo",
                        "+nightly",
                        "llvm-cov",
                        "--workspace",
                        "--all-features",
                        "--summary-only",
                    ],
                )
            elif args.verbose:
                cprint(
                    "  [yellow]Hint: cargo-llvm-cov not found."
                    " Install: cargo install cargo-llvm-cov[/yellow]"
                )

        # ── Python quality ────────────────────────────────────────────────────────
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

        # ── Shell scripts ─────────────────────────────────────────────────────────
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

        # ── Markdown / JSON / YAML / TOML ─────────────────────────────────────────
        if md_files and has_command("markdownlint-cli2", verbose=args.verbose):
            config_path = os.path.join(
                repo_root, "scripts/config/.markdownlint-cli2.jsonc"
            )
            run_step(
                tracker,
                "optional",
                "markdownlint",
                ["markdownlint-cli2", "--config", config_path] + md_files,
            )

        targets = md_files + json_files + yaml_files
        if targets and has_command("prettier", verbose=args.verbose):
            run_step(
                tracker,
                "optional",
                "prettier check",
                ["prettier", "--check"] + targets,
            )

        if toml_files and has_cargo_subcommand("taplo", verbose=args.verbose):
            run_step(
                tracker,
                "optional",
                "taplo fmt check",
                ["cargo", "taplo", "fmt", "--check"],
            )

        # ── cargo doc (stable) ────────────────────────────────────────────────────
        run_step(
            tracker,
            "optional",
            "cargo doc",
            ["cargo", "doc", "--workspace", "--no-deps"],
        )

        # ── cargo +nightly doc -D warnings ───────────────────────────────────────
        # Nightly rustdoc enables extra lints (e.g. broken intra-doc links,
        # missing_docs, private_intra_doc_links) not gated on stable.
        # Runs as a second pass after the stable doc build so nightly-only
        # warning noise doesn't block the required path.
        if nc.toolchain:
            run_step(
                tracker,
                "optional",
                "cargo +nightly doc -D warnings (rustdoc lints)",
                ["cargo", "+nightly", "doc", "--workspace", "--no-deps"],
                env_vars={"RUSTDOCFLAGS": "-D warnings"},
            )
        else:
            skip_step(
                tracker,
                "nightly rustdoc -D warnings",
                "nightly toolchain not installed",
            )

        # ── Security audit ────────────────────────────────────────────────────────
        if has_cargo_subcommand("audit", verbose=args.verbose):
            run_step(tracker, "optional", "cargo audit", ["cargo", "audit"])

        # ── cargo deny: license + advisory + duplicate crate check ───────────────
        # More thorough than cargo-audit: also enforces license allowlists and
        # catches duplicate transitive dependency versions that bloat binary size.
        # Config: deny.toml at workspace root (create with `cargo deny init`).
        if has_cargo_subcommand("deny", verbose=args.verbose):
            run_step(
                tracker,
                "optional",
                "cargo deny check (licenses + advisories + bans)",
                ["cargo", "deny", "check"],
            )

        # ── Snapshot tests: insta ─────────────────────────────────────────────────
        # cargo-insta runs tests and fails if any snapshot has changed but not
        # been reviewed. Use `cargo insta review` to accept/reject diffs.
        # --unreferenced=reject fails if orphaned snapshots accumulate on disk.
        if has_cargo_subcommand("insta", verbose=args.verbose):
            run_step(
                tracker,
                "optional",
                "cargo insta test (snapshot regression check)",
                [
                    "cargo",
                    "insta",
                    "test",
                    "--workspace",
                    "--unreferenced=reject",
                ],
            )

        # ── Benchmark compile check ───────────────────────────────────────────────
        # Compiles all criterion benchmarks without running them.
        # Catches benchmark bitrot (benchmark code that no longer compiles after
        # internal API changes) without paying the full benchmark execution cost.
        # Only meaningful if the workspace actually has bench targets.
        bench_files = [f for f in git_files if "benches/" in f and f.endswith(".rs")]
        if bench_files:
            run_step(
                tracker,
                "optional",
                f"cargo bench --no-run (compile check, {len(bench_files)} bench file(s))",
                ["cargo", "bench", "--workspace", "--no-run"],
            )
        else:
            skip_step(tracker, "bench compile check", "no bench targets found")

        # ── Fuzz target discovery (fast, no actual fuzzing) ───────────────────────
        # Lists declared fuzz targets so regressions in the fuzz harness setup
        # surface in CI without the cost of running fuzzing at all.
        # Actual fuzzing is intentionally left as a local/scheduled workflow.
        if args.fuzz_list:
            if nc.toolchain and has_cargo_subcommand("fuzz", verbose=args.verbose):
                run_step(
                    tracker,
                    "optional",
                    "cargo fuzz list (fuzz target discovery)",
                    ["cargo", "+nightly", "fuzz", "list"],
                )
            else:
                missing_parts = []
                if not nc.toolchain:
                    missing_parts.append("nightly toolchain")
                if not _has_cargo_sub("fuzz"):
                    missing_parts.append("cargo-fuzz (cargo install cargo-fuzz)")
                skip_step(
                    tracker,
                    "cargo fuzz list",
                    f"missing: {', '.join(missing_parts)}",
                )

        # ── Expensive checks ──────────────────────────────────────────────────────
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

            # ── Mutation testing: cargo-mutants ───────────────────────────────────
            # Modifies source code one mutation at a time (flips operators, removes
            # return values, etc.) and checks whether the test suite catches it.
            # Measures test suite *quality* — complementary to coverage metrics.
            # Very slow: budget ~30-120 min for a medium workspace.
            # Opt-in via --mutants; also skipped by --no-expensive.
            if args.mutants:
                if has_cargo_subcommand("mutants", verbose=args.verbose):
                    run_step(
                        tracker,
                        "optional",
                        "cargo mutants (mutation testing, timeout 60s/mutant)",
                        [
                            "cargo",
                            "mutants",
                            "--workspace",
                            # Per-mutant timeout — prevents infinite-loop mutants
                            # from stalling the whole run.
                            "--timeout",
                            "60",
                            # Cap parallel workers to avoid starving the system
                            # when Antigravity IDE or other heavy tools are open.
                            "--jobs",
                            "2",
                        ],
                    )
                else:
                    skip_step(
                        tracker,
                        "cargo mutants",
                        "not installed — cargo install cargo-mutants",
                    )

        # ── AI smell ──────────────────────────────────────────────────────────────
        if args.ai_smell:
            check_ai_smell(tracker, repo_root)

    # ── Summary ───────────────────────────────────────────────────────────────────
    print_summary(tracker)

    if tracker.failed > 0:
        cprint(f"\n[bold red]Audit failed with {tracker.failed} error(s).[/bold red]")
        sys.exit(1)

    cprint("\n[bold green]🌟 Workspace is healthy![/bold green]")


if __name__ == "__main__":
    main()
