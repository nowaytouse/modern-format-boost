#!/usr/bin/env python3
"""check_all.py — Modern Format Boost workspace auditor."""

from __future__ import annotations

import argparse
import os
import plistlib
import re
import shutil
import subprocess
import sys
import time
from dataclasses import dataclass, field
from functools import lru_cache
from pathlib import Path

try:
    from rich.console import Console
    from rich.markup import escape
    from rich.panel import Panel
    from rich.table import Table

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
    """Prefer toolchain ``cargo-<sub>`` binaries (not broken rustup shims)."""
    try:
        from mfb_rust_toolchain import has_cargo_component

        if has_cargo_component(sub):
            return True
    except (
        OSError,
        ValueError,
        RuntimeError,
        TypeError,
        KeyError,
        IndexError,
        AttributeError,
        UnicodeError,
    ):
        pass
    try:
        return (
            subprocess.run(["cargo", sub, "--version"], capture_output=True).returncode
            == 0
        )
    except (
        OSError,
        ValueError,
        RuntimeError,
        TypeError,
        KeyError,
        IndexError,
        AttributeError,
        UnicodeError,
    ):
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


def taplo_fmt_command(
    files: list[str], *args: str, verbose: bool = False
) -> list[str] | None:
    if not files:
        return None
    if has_cargo_subcommand("taplo"):
        return ["cargo", "taplo", "fmt", *args, *files]
    if has_command("taplo"):
        return ["taplo", "fmt", *args, *files]
    if verbose:
        cprint(
            "  [yellow]Hint: neither 'cargo taplo' nor 'taplo' was found."
            " Install: cargo install taplo-cli[/yellow]"
        )
    return None


def get_repo_root() -> Path:
    try:
        root = subprocess.check_output(
            ["git", "rev-parse", "--show-toplevel"], stderr=subprocess.STDOUT, text=True
        ).strip()
        return Path(root)
    except (
        OSError,
        ValueError,
        RuntimeError,
        TypeError,
        KeyError,
        IndexError,
        AttributeError,
        UnicodeError,
    ):
        return Path(__file__).parent.parent.parent.resolve()


def filter_existing_repo_files(repo_root: Path, files: list[str]) -> list[str]:
    """Drop tracked paths that no longer exist in the working tree."""
    return [f for f in files if (repo_root / f).is_file()]


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
def _rust_toolchain_channel_for_probe(repo_root: Path | None = None) -> str:
    """Match ``rust-toolchain.toml`` / active toolchain (not the generic ``nightly`` alias)."""
    root = repo_root or get_repo_root()
    toml_path = root / "rust-toolchain.toml"
    if toml_path.is_file():
        match = re.search(
            r'channel\s*=\s*"([^"]+)"',
            toml_path.read_text(encoding="utf-8"),
        )
        if match:
            return match.group(1)
    try:
        out = subprocess.check_output(
            ["rustup", "show", "active-toolchain"],
            text=True,
            stderr=subprocess.STDOUT,
        )
        first = out.strip().splitlines()[0].strip()
        if first:
            return first.split(maxsplit=1)[0]
    except (OSError, subprocess.CalledProcessError, IndexError):
        pass
    return "nightly"


def probe_nightly(repo_root: Path | None = None) -> NightlyComponents:
    """Detect pinned nightly toolchain and installed components."""
    nc = NightlyComponents()
    channel = _rust_toolchain_channel_for_probe(repo_root)

    if not _has_command("rustup"):
        return nc

    # 1. Is the toolchain installed?
    r = subprocess.run(
        ["rustup", "run", channel, "rustc", "--version"],
        capture_output=True,
    )
    if r.returncode != 0:
        return nc
    nc.toolchain = True

    # 2. Which components are installed?
    r = subprocess.run(
        [
            "rustup",
            "component",
            "list",
            "--installed",
            "--toolchain",
            channel,
        ],
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
    _rust_toolchain_channel_for_probe.cache_clear()
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
    fail_fast: bool = False
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


def abort_audit(tracker: Tracker, code: int = 1) -> None:
    """Print summary and exit immediately (CI fail-fast)."""
    print_summary(tracker)
    sys.exit(code)


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
        if tracker.fail_fast:
            abort_audit(tracker)
    else:
        tracker.warned += 1
        tracker.warned_steps.append(name)
        cprint(f"  [yellow]WARN[/yellow] (exit {proc.returncode}, {duration})")
        if tracker.fail_fast:
            abort_audit(tracker)
    return False


def skip_step(tracker: Tracker, name: str, reason: str) -> None:
    tracker.announce_step("optional", name)
    tracker.skipped += 1
    tracker.skipped_steps.append(f"{name} ({reason})")
    cprint(f"  [blue]SKIP[/blue] ({reason})")


def fail_required_step(tracker: Tracker, name: str, reason: str) -> None:
    tracker.announce_step("required", name)
    tracker.failed += 1
    tracker.failed_steps.append(name)
    cprint(f"  [red]FAIL[/red]: {reason}")
    if tracker.fail_fast:
        abort_audit(tracker)


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

After findings (or the single line "No AI smell detected"), update docs/CHANGELOG.md:
  - Prepend a new H2 section: `## [{date}] Code Quality`
  - Under `### Changed`, list each finding as a bullet (file:line, category, one-sentence description)
  - Do NOT modify any existing content below the new entry

Constraints:
  - No preamble, no closing remarks
  - Do not rewrite files other than docs/CHANGELOG.md
  - If docs/CHANGELOG.md does not exist, create it with only the new entry
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
    changelog_path = root / "docs" / "CHANGELOG.md"

    if not changelog_path.exists():
        tracker.failed += 1
        cprint("  [red]FAIL: docs/CHANGELOG.md missing[/red]")
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
                f"  [red]FAIL: Version '{version}' not found as a header in docs/CHANGELOG.md[/red]"
            )
            return False

        tracker.passed += 1
        cprint(f"  [green]OK: {version} is documented[/green]")
        return True

    except (
        OSError,
        ValueError,
        RuntimeError,
        TypeError,
        KeyError,
        IndexError,
        AttributeError,
        UnicodeError,
    ) as e:
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

    except (
        OSError,
        ValueError,
        RuntimeError,
        TypeError,
        KeyError,
        IndexError,
        AttributeError,
        UnicodeError,
    ) as e:
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


# ── GitHub Actions health-check profile (SSOT for ci-quality health-check job) ─

CI_CARGO_FEATURES: tuple[str, ...] = (
    "--all-features",
    "--features",
    "foundation/ci-static-build",
)


def apply_ci_runner_env() -> None:
    """Match ``ci-quality`` health-check: embedded libheif + strict clippy script."""
    os.environ.setdefault("GITHUB_ACTIONS", "true")
    os.environ.setdefault("LIBHEIF_STATIC", "1")
    os.environ.setdefault("LIBHEIF_SYS_STATIC", "1")
    os.environ.setdefault("NODE_OPTIONS", "--no-deprecation")
    existing = os.environ.get("RUSTFLAGS", "").strip()
    deny = "-D warnings"
    os.environ["RUSTFLAGS"] = f"{existing} {deny}".strip() if existing else deny


def ci_cargo_feature_argv(*cargo_args: str) -> list[str]:
    return [*cargo_args, *CI_CARGO_FEATURES]


def ensure_edge_test_media(tracker: Tracker, repo_root: Path) -> None:
    """Generate gitignored synthetic assets under ``crates/dev/src/tests/edge`` when missing."""
    marker = repo_root / "crates/dev/src/tests/edge/videos/test_h264_10s.mp4"
    if marker.is_file():
        skip_step(tracker, "generate edge test media", "already present")
        return
    script = repo_root / "crates/dev/scripts/generate_test_media.py"
    run_step(
        tracker,
        "required",
        "python3 generate_test_media.py (edge fixtures)",
        [sys.executable, str(script)],
    )


def run_ci_health_rust_tests(tracker: Tracker, rust_tc, repo_root: Path) -> None:
    """Same commands as ``.github/workflows/ci-quality.yml`` health-check (pre-check_all)."""
    from mfb_rust_toolchain import cargo_argv

    run_step(
        tracker,
        "required",
        "cargo test -p foundation --lib (serial, ci-static-build)",
        cargo_argv(
            rust_tc,
            "test",
            "-p",
            "foundation",
            "--lib",
            *ci_cargo_feature_argv(),
            "--no-fail-fast",
            "--",
            "--test-threads=1",
        ),
    )
    run_step(
        tracker,
        "required",
        "cargo test --workspace --lib (ci-static-build, exclude foundation)",
        cargo_argv(
            rust_tc,
            "test",
            "--workspace",
            "--lib",
            *ci_cargo_feature_argv(),
            "--exclude",
            "foundation",
            "--no-fail-fast",
        ),
    )
    run_step(
        tracker,
        "required",
        "cargo test -p dev test_real_silent_fallbacks (contract registry)",
        cargo_argv(
            rust_tc,
            "test",
            "-p",
            "dev",
            "--test",
            "test_real_silent_fallbacks",
            *ci_cargo_feature_argv(),
            "--no-fail-fast",
            "--",
            "--test-threads=1",
        ),
    )
    run_step(
        tracker,
        "required",
        "cargo test -p dev headless_gif_regression (ffmpeg/runtime probe regression)",
        cargo_argv(
            rust_tc,
            "test",
            "-p",
            "dev",
            "--test",
            "headless_gif_regression",
            *ci_cargo_feature_argv(),
            "--no-fail-fast",
            "--",
            "--test-threads=1",
        ),
    )
    run_step(
        tracker,
        "required",
        "cargo test -p dev runtime_probe_regression (WebP/APNG/HEIC/JXL/AVIF header preflight)",
        cargo_argv(
            rust_tc,
            "test",
            "-p",
            "dev",
            "--test",
            "runtime_probe_regression",
            *ci_cargo_feature_argv(),
            "--no-fail-fast",
            "--",
            "--test-threads=1",
        ),
    )
    run_step(
        tracker,
        "required",
        "cargo test -p dev comprehensive_weakness_audit (inventory + closure SSOT)",
        cargo_argv(
            rust_tc,
            "test",
            "-p",
            "dev",
            "--test",
            "comprehensive_weakness_audit",
            *ci_cargo_feature_argv(),
            "--no-fail-fast",
        ),
    )
    normalize_script = (
        repo_root / "crates/dev/scripts/normalize_stale_embed_measurement_slots.py"
    )
    if normalize_script.is_file():
        text = normalize_script.read_text(encoding="utf-8")
        if "EMBED_SLOT_INDICES" in text and "PGVECTOR_MISSING_MEASUREMENT" in text:
            run_step(
                tracker,
                "optional",
                "verify normalize_stale_embed_measurement_slots.py (DB sentinel backfill SSOT)",
                [
                    sys.executable,
                    "-c",
                    "import pathlib, sys; p=pathlib.Path(sys.argv[1]); "
                    "t=p.read_text(); "
                    "sys.exit(0 if 'EMBED_SLOT_INDICES' in t and 'PGVECTOR_MISSING_MEASUREMENT' in t else 1)",
                    str(normalize_script),
                ],
            )
        else:
            skip_step(
                tracker,
                "normalize_stale_embed_measurement_slots.py (incomplete SSOT markers)",
                "missing EMBED_SLOT_INDICES or PGVECTOR_MISSING_MEASUREMENT",
            )
    else:
        skip_step(
            tracker,
            "normalize_stale_embed_measurement_slots.py (missing DB backfill script)",
            "crates/dev/scripts/normalize_stale_embed_measurement_slots.py not found",
        )


def run_ci_health_coverage(tracker: Tracker, rust_tc, *, nc, repo_root: Path) -> None:
    """LCOV for ``foundation`` lib — artifact upload stays in the workflow YAML."""
    from mfb_rust_toolchain import cargo_argv

    if not has_cargo_subcommand("llvm-cov", verbose=False):
        fail_required_step(
            tracker,
            "cargo-llvm-cov availability (CI)",
            "cargo-llvm-cov not installed — workflow must run cargo install cargo-llvm-cov",
        )
        return

    if not nc.llvm_tools:
        channel = _rust_toolchain_channel_for_probe(repo_root)
        cprint(
            f"  [yellow]llvm-tools missing on {channel};"
            " installing for CI coverage…[/yellow]"
        )
        install = subprocess.run(
            ["rustup", "component", "add", "llvm-tools", "--toolchain", channel],
            capture_output=True,
            text=True,
        )
        if install.returncode != 0:
            detail = (install.stderr or install.stdout or "").strip()
            fail_required_step(
                tracker,
                f"rustup component add llvm-tools ({channel})",
                detail or "rustup component add failed",
            )
            return
        _rust_toolchain_channel_for_probe.cache_clear()
        nc.llvm_tools = True

    lcov_path = repo_root / "lcov.info"

    run_step(
        tracker,
        "required",
        "cargo llvm-cov -p foundation --lib --summary-only (CI)",
        cargo_argv(
            rust_tc,
            "llvm-cov",
            "-p",
            "foundation",
            "--lib",
            *ci_cargo_feature_argv(),
            "--summary-only",
            "--no-fail-fast",
        ),
    )
    run_step(
        tracker,
        "required",
        "cargo llvm-cov -p foundation --lib --lcov (CI)",
        cargo_argv(
            rust_tc,
            "llvm-cov",
            "-p",
            "foundation",
            "--lib",
            *ci_cargo_feature_argv(),
            "--no-fail-fast",
            "--lcov",
            "--output-path",
            str(lcov_path),
        ),
    )
    if not lcov_path.is_file() or lcov_path.stat().st_size == 0:
        tracker.failed += 1
        tracker.failed_steps.append("lcov.info missing after llvm-cov (CI)")
        cprint(f"  [red]FAIL[/red]: expected {lcov_path} for upload-artifact step")


def run_ci_health_rustdoc(tracker: Tracker, rust_tc) -> None:
    """``foundation`` crate docs with rustdoc warnings denied (CI log noise SSOT)."""
    from mfb_rust_toolchain import cargo_argv

    run_step(
        tracker,
        "required",
        "cargo doc -p foundation --no-deps (RUSTDOCFLAGS -D warnings)",
        cargo_argv(rust_tc, "doc", "-p", "foundation", "--no-deps"),
        env_vars={"RUSTDOCFLAGS": "-D warnings"},
    )


def assert_ci_lcov_artifact(repo_root: Path, tracker: Tracker) -> None:
    """Fail-closed: workflow upload-artifact requires non-empty ``lcov.info``."""
    lcov_path = repo_root / "lcov.info"
    if lcov_path.is_file() and lcov_path.stat().st_size > 0:
        return
    tracker.failed += 1
    tracker.failed_steps.append("lcov.info missing for CI artifact upload")
    cprint(
        f"  [red]FAIL[/red]: expected non-empty {lcov_path} "
        "(run_ci_health_coverage must succeed before upload-artifact)"
    )


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
        "--fuzz-smoke",
        action="store_true",
        help=(
            "Run each cargo-fuzz target briefly (-runs=1). CI treats missing "
            "cargo-fuzz or target failures as hard blockers."
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
    parser.add_argument(
        "--ci",
        action="store_true",
        help=(
            "GitHub Actions health-check profile: ci-static-build tests, contract "
            "registry (test_real_silent_fallbacks), required LCOV; sets GITHUB_ACTIONS "
            "for crates/dev/scripts/ci/clippy_strict.py. Use with expensive checks (default); "
            "pass --no-expensive to skip hack/bloat/extra llvm-cov only."
        ),
    )
    return parser.parse_args()


def bootstrap_macos_path() -> None:
    """Ensure Homebrew and common tool paths are in os.environ['PATH'] on macOS."""
    if sys.platform != "darwin":
        return

    extra_paths = [
        "/opt/homebrew/bin",
        "/opt/homebrew/sbin",
        "/usr/local/bin",
        "/usr/local/sbin",
    ]
    path_parts = os.environ.get("PATH", "").split(os.pathsep)
    added = []

    for p in extra_paths:
        if os.path.isdir(p) and p not in path_parts:
            path_parts.insert(0, p)
            added.append(p)

    if added:
        os.environ["PATH"] = os.pathsep.join(path_parts)
        # Also clean up duplicate separators
        os.environ["PATH"] = os.environ["PATH"].replace(
            f"{os.pathsep}{os.pathsep}", os.pathsep
        )


def main() -> None:
    _scripts = Path(__file__).resolve().parent
    if str(_scripts) not in sys.path:
        sys.path.insert(0, str(_scripts))
    from mfb_entry_guard import guard_main  # noqa: E402

    guard_main("check_all.py")
    bootstrap_macos_path()
    from mfb_rust_toolchain import (
        apply_toolchain_env,
        cargo_argv,
        cargo_component_argv,
    )

    args = parse_args()
    if args.ci:
        apply_ci_runner_env()
    repo_root = get_repo_root()
    os.chdir(repo_root)
    rust_tc = apply_toolchain_env()
    _has_cargo_sub.cache_clear()
    tracker = Tracker(fail_fast=False)

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
    nc = probe_nightly(repo_root)

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
    except (
        OSError,
        ValueError,
        RuntimeError,
        TypeError,
        KeyError,
        IndexError,
        AttributeError,
        UnicodeError,
    ):
        git_files = [
            str(p.relative_to(repo_root)) for p in repo_root.rglob("*") if p.is_file()
        ]

    git_files = filter_existing_repo_files(repo_root, git_files)

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
            cargo_component_argv(rust_tc, "fmt", "--all")
            if nc.rustfmt and rust_tc.rustfmt
            else cargo_argv(rust_tc, "fmt", "--all")
        )
        subprocess.run(fmt_cmd, env=os.environ.copy())

        clippy_fix = [
            sys.executable,
            str(repo_root / "crates" / "dev" / "scripts" / "ci" / "clippy_strict.py"),
            "--fix",
        ]
        subprocess.run(clippy_fix, env=os.environ.copy(), cwd=repo_root)

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

        taplo_cmd = taplo_fmt_command(toml_files)
        if taplo_cmd:
            subprocess.run(taplo_cmd)

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
    if nc.rustfmt and rust_tc.rustfmt:
        run_step(
            tracker,
            "required",
            f"cargo fmt --check ({rust_tc.name or 'toolchain'})",
            cargo_component_argv(rust_tc, "fmt", "--all", "--check"),
        )
    else:
        run_step(
            tracker,
            "required",
            "cargo fmt --check (stable fallback)",
            cargo_argv(rust_tc, "fmt", "--all", "--check"),
        )

    # ── cargo check ──────────────────────────────────────────────────────────────
    if args.ci:
        run_step(
            tracker,
            "required",
            "cargo check --workspace (ci-static-build)",
            cargo_argv(rust_tc, "check", "--workspace", *ci_cargo_feature_argv()),
        )
    else:
        run_step(
            tracker,
            "required",
            "cargo check --workspace --all-features",
            cargo_argv(
                rust_tc, "check", "--workspace", "--all-targets", "--all-features"
            ),
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

    # ── clippy (required, ultra-strict — same as CI crates/dev/scripts/ci/clippy_strict.py) ──
    if nc.clippy and rust_tc.clippy:
        run_step(
            tracker,
            "required",
            "crates/dev/scripts/ci/clippy_strict.py (workspace deny + pedantic/nursery/cargo)",
            [
                sys.executable,
                str(
                    repo_root / "crates" / "dev" / "scripts" / "ci" / "clippy_strict.py"
                ),
            ],
        )
    else:
        if nc.toolchain:
            cprint(
                "  [yellow]Note: nightly clippy component missing;"
                " falling back to workspace-lints-only clippy.[/yellow]\n"
                f"  [dim]Fix: {nc.install_hint()}[/dim]"
            )
        run_step(
            tracker,
            "required",
            "cargo clippy --workspace -D warnings (stable fallback)",
            cargo_argv(
                rust_tc,
                "clippy",
                "--workspace",
                "--all-targets",
                "--all-features",
                "--",
                "-D",
                "warnings",
            ),
        )

    # ── tests ─────────────────────────────────────────────────────────────────────
    if args.ci:
        run_ci_health_rust_tests(tracker, rust_tc, repo_root)
    else:
        ensure_edge_test_media(tracker, repo_root)
        if has_cargo_subcommand("nextest"):
            run_step(
                tracker,
                "required",
                "cargo nextest run",
                cargo_argv(
                    rust_tc,
                    "nextest",
                    "run",
                    "--workspace",
                    "--all-features",
                    "--no-fail-fast",
                ),
            )
        else:
            run_step(
                tracker,
                "required",
                "cargo test",
                cargo_argv(
                    rust_tc, "test", "--workspace", "--all-features", "--no-fail-fast"
                ),
            )

    if args.build:
        run_step(
            tracker,
            "required",
            "cargo build --release",
            cargo_argv(rust_tc, "build", "--release", "--workspace"),
        )

    if args.ci and not args.no_expensive:
        run_ci_health_coverage(tracker, rust_tc, nc=nc, repo_root=repo_root)
        run_ci_health_rustdoc(tracker, rust_tc)

    # ══════════════════════════════════════════════════════════════════════════════
    # Optional checks
    # ══════════════════════════════════════════════════════════════════════════════

    if not args.required_only:
        # ── nightly rustfmt: unstable options ─────────────────────────────────────
        # Only meaningful when rustfmt.toml contains `unstable_features = true`.
        # Runs as optional because the stable fmt check already passed above.
        if nc.rustfmt and rust_tc.rustfmt:
            run_step(
                tracker,
                "optional",
                "cargo fmt --check (unstable options)",
                cargo_component_argv(rust_tc, "fmt", "--all", "--check"),
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
                    "cargo miri test --lib",
                    cargo_component_argv(
                        rust_tc, "miri", "test", "--workspace", "--lib"
                    ),
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
                except (
                    OSError,
                    ValueError,
                    RuntimeError,
                    TypeError,
                    KeyError,
                    IndexError,
                    AttributeError,
                    UnicodeError,
                ):
                    build_target = "aarch64-apple-darwin"

                run_step(
                    tracker,
                    "optional",
                    f"cargo test --lib (AddressSanitizer, {build_target})",
                    cargo_argv(
                        rust_tc,
                        "test",
                        "--workspace",
                        "--lib",
                        "--target",
                        build_target,
                    ),
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

        # ── llvm-cov coverage (optional, expensive; --ci uses run_ci_health_coverage) ─
        if not args.ci and not args.no_expensive and nc.llvm_tools:
            if has_cargo_subcommand("llvm-cov", verbose=args.verbose):
                run_step(
                    tracker,
                    "optional",
                    "cargo llvm-cov --summary-only",
                    cargo_argv(
                        rust_tc,
                        "llvm-cov",
                        "--workspace",
                        "--all-features",
                        "--summary-only",
                    ),
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
                repo_root, "crates/dev/scripts/config/.markdownlint-cli2.jsonc"
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

        taplo_cmd = taplo_fmt_command(toml_files, "--check", verbose=args.verbose)
        if toml_files and taplo_cmd:
            run_step(
                tracker,
                "optional",
                "taplo fmt check",
                taplo_cmd,
            )

        # ── cargo doc (local audit only; --ci uses run_ci_health_rustdoc) ─────────
        if not args.ci:
            run_step(
                tracker,
                "optional",
                "cargo doc",
                cargo_argv(rust_tc, "doc", "--workspace", "--no-deps"),
            )

            # Nightly rustdoc enables extra lints (broken intra-doc links, etc.).
            if nc.toolchain:
                run_step(
                    tracker,
                    "optional",
                    "cargo doc -D warnings (rustdoc lints)",
                    cargo_argv(rust_tc, "doc", "--workspace", "--no-deps"),
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
                cargo_argv(rust_tc, "bench", "--workspace", "--no-run"),
            )
        else:
            skip_step(tracker, "bench compile check", "no bench targets found")

        # ── Fuzz target discovery / smoke ──────────────────────────────────────────
        # CI runs all fuzz targets briefly so harness build/link/runtime regressions
        # are hard blockers, not delayed to a separate scheduled workflow.
        if args.fuzz_list or args.fuzz_smoke:
            missing_parts = []
            if not nc.toolchain:
                missing_parts.append("nightly toolchain")
            if not _has_cargo_sub("fuzz"):
                missing_parts.append("cargo-fuzz (cargo install cargo-fuzz)")

            if missing_parts:
                message = f"missing: {', '.join(missing_parts)}"
                if args.ci:
                    fail_required_step(tracker, "cargo fuzz availability", message)
                else:
                    skip_step(tracker, "cargo fuzz", message)
            else:
                fuzz_list_cmd = cargo_argv(
                    rust_tc,
                    "fuzz",
                    "list",
                    "--fuzz-dir",
                    "crates/dev/src/fuzz",
                )
                run_step(
                    tracker,
                    "required" if args.ci else "optional",
                    "cargo fuzz list (fuzz target discovery)",
                    fuzz_list_cmd,
                )

                if args.fuzz_smoke:
                    fuzz_targets = sorted(
                        path.stem
                        for path in (repo_root / "crates/dev/src/fuzz/fuzz_targets").glob(
                            "*.rs"
                        )
                    )
                    if not fuzz_targets:
                        fail_required_step(
                            tracker,
                            "cargo fuzz smoke",
                            "no fuzz targets found under crates/dev/src/fuzz/fuzz_targets",
                        )
                    for target in fuzz_targets:
                        run_step(
                            tracker,
                            "required" if args.ci else "optional",
                            f"cargo fuzz run {target} (-runs=1)",
                            cargo_argv(
                                rust_tc,
                                "fuzz",
                                "run",
                                target,
                                "--fuzz-dir",
                                "crates/dev/src/fuzz",
                                "--",
                                "-runs=1",
                            ),
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
                        "cargo mutants (mutation testing, baseline-aware timeout)",
                        [
                            "cargo",
                            "mutants",
                            "--workspace",
                            # Some integration-heavy workspaces need more than 60s
                            # just to establish the unmutated baseline in a temp tree.
                            # Keep a firm per-command cap, but raise the floor so
                            # baseline compilation does not fail before mutation even begins.
                            "--timeout",
                            "180",
                            "--minimum-test-timeout",
                            "180",
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

    if args.ci and tracker.warned > 0:
        cprint(
            f"\n[bold red]CI profile failed: {tracker.warned} optional step(s)"
            " exited non-zero (treated as errors on GITHUB_ACTIONS).[/bold red]"
        )
        for step in tracker.warned_steps:
            cprint(f"  [red]• {escape(step)}[/red]")
        sys.exit(1)

    if args.ci and not args.no_expensive:
        assert_ci_lcov_artifact(repo_root, tracker)
        if tracker.failed > 0:
            cprint(
                f"\n[bold red]CI profile failed: {tracker.failed} error(s).[/bold red]"
            )
            sys.exit(1)

    cprint("\n[bold green]🌟 Workspace is healthy![/bold green]")


if __name__ == "__main__":
    main()
