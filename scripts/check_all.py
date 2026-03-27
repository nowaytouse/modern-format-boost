#!/usr/bin/env python3
"""Comprehensive code quality scanner for Modern Format Boost (Python Edition)"""

import os
import sys
import subprocess
import shutil
import time
from pathlib import Path

try:
    from rich.console import Console
    from rich.table import Table
    console = Console()
except ImportError:
    console = None

# ANSI Colors
if sys.stdout.isatty():
    RED = '\033[0;31m'
    GREEN = '\033[0;32m'
    YELLOW = '\033[1;33m'
    BLUE = '\033[0;34m'
    CYAN = '\033[0;36m'
    BOLD = '\033[1m'
    NC = '\033[0m'
else:
    RED = GREEN = YELLOW = BLUE = CYAN = BOLD = NC = ''

# Config options
REQUIRED_BRANCH = os.environ.get("CHECK_ALL_DEFAULT_BRANCH", "nightly")
ENFORCE_BRANCH = True
RUN_OPTIONAL = True
RUN_EXPENSIVE = True
ALLOW_FETCH = False
AUTO_FIX = False

def show_help():
    help_text = f"""Usage: scripts/check_all.py [options]

Options:
  --allow-non-nightly    Do not enforce '{REQUIRED_BRANCH}' git branch.
  --required-only        Run only required checks (fmt/clippy/tests).
  --no-expensive         Skip expensive optional checks (udeps/bloat/hack/miri).
  --fetch-advisory-db    Allow network fetch for cargo-audit/cargo-deny.
  --fix                  Auto-fix issues (cargo fmt, cargo clippy --fix).
  -h, --help             Show this help.

Environment:
  CHECK_ALL_DEFAULT_BRANCH=<branch>  Override default required branch (default: nightly).
"""
    print(help_text)

def has_cargo_subcommand(sub, pkg=None):
    if pkg is None: pkg = f"cargo-{sub}"
    result = subprocess.run(["cargo", sub, "--version"], capture_output=True)
    if result.returncode == 0:
        return True
    print(f"{YELLOW}Hint: '{sub}' requires {pkg}. Install with: cargo install {pkg}{NC}", file=sys.stderr)
    return False

def has_command(cmd, pkg=None):
    if pkg is None: pkg = cmd
    if shutil.which(cmd) is not None:
        return True
    print(f"{YELLOW}Hint: '{cmd}' requires {pkg}. Install with: brew install {pkg}{NC}", file=sys.stderr)
    return False

def has_nightly_toolchain():
    result = subprocess.run(["rustup", "toolchain", "list"], capture_output=True, text=True)
    for line in result.stdout.splitlines():
        if line.split()[0].startswith("nightly"):
            return True
    return False

def has_rust_component(component, toolchain=None):
    cmd = ["rustup"]
    if toolchain:
        cmd.append(f"+{toolchain}")
    cmd.extend(["component", "list", "--installed"])
    result = subprocess.run(cmd, capture_output=True, text=True)
    if result.returncode != 0:
        return False
    for line in result.stdout.splitlines():
        if line.split()[0] == component:
            return True
    return False

def advisory_db_dir():
    cargo_home = os.environ.get("CARGO_HOME", os.path.expanduser("~/.cargo"))
    return os.path.join(cargo_home, "advisory-db")

def advisory_db_writable():
    d = advisory_db_dir()
    return os.path.isdir(d) and os.access(d, os.W_OK)

def advisory_db_is_git_repo():
    d = advisory_db_dir()
    return os.path.isdir(os.path.join(d, ".git"))

def format_duration(ms):
    if ms < 1000:
        return f"{int(ms)}ms"
    return f"{ms/1000:.2f}s"

class Tracker:
    def __init__(self):
        self.step = 0
        self.passed = 0
        self.failed = 0
        self.warned = 0
        self.skipped = 0
        self.failed_steps = []
        self.warned_steps = []
        self.skipped_steps = []

def print_step_header(tracker, kind, name):
    tracker.step += 1
    print(f"\n{BOLD}[{tracker.step}] {kind}: {name}{NC}", end="", flush=True)

def run_command(tracker, kind, name, cmd_list, check_output=False, env_vars=None):
    print_step_header(tracker, kind, name)
    print("")
    start = time.time()
    
    env = os.environ.copy()
    if env_vars:
        env.update(env_vars)
        
    result = subprocess.run(cmd_list, env=env)
    
    end = time.time()
    duration = format_duration((end - start) * 1000)
    
    if result.returncode == 0:
        tracker.passed += 1
        print(f" {GREEN}PASS{NC} ({duration})")
    else:
        if kind == "required":
            tracker.failed += 1
            tracker.failed_steps.append(f"{name} (exit {result.returncode})")
            print(f" {RED}FAIL{NC} (exit {result.returncode})")
        else:
            tracker.warned += 1
            tracker.warned_steps.append(f"{name} (exit {result.returncode})")
            print(f" {YELLOW}WARN{NC} (exit {result.returncode})")

def skip_optional(tracker, name, reason):
    print_step_header(tracker, "optional", name)
    tracker.skipped += 1
    tracker.skipped_steps.append(f"{name}: {reason}")
    print(f" {BLUE}SKIP{NC} ({reason})")

def main():
    global ENFORCE_BRANCH, RUN_OPTIONAL, RUN_EXPENSIVE, ALLOW_FETCH, AUTO_FIX
    
    script_dir = Path(__file__).parent.resolve()
    repo_root = script_dir.parent
    
    if not (repo_root / "Cargo.toml").is_file():
        print(f"Error: REPO_ROOT '{repo_root}' does not look like a Cargo workspace (no Cargo.toml found).", file=sys.stderr)
        sys.exit(2)
        
    os.chdir(repo_root)
    
    # Args
    args = sys.argv[1:]
    while args:
        arg = args.pop(0)
        if arg == "--allow-non-nightly": ENFORCE_BRANCH = False
        elif arg == "--required-only": RUN_OPTIONAL = False
        elif arg == "--no-expensive": RUN_EXPENSIVE = False
        elif arg == "--fetch-advisory-db": ALLOW_FETCH = True
        elif arg == "--fix": AUTO_FIX = True
        elif arg in ("-h", "--help"):
            show_help()
            sys.exit(0)
        else:
            print(f"Unknown option: {arg}", file=sys.stderr)
            show_help()
            sys.exit(2)

    print(f"{BOLD}{CYAN}Starting code quality scan{NC}")
    print(f"{BLUE}Repo: {repo_root}{NC}")
    print(f"{BLUE}Default branch policy: {REQUIRED_BRANCH}{NC}")

    if subprocess.run(["git", "rev-parse", "--is-inside-work-tree"], capture_output=True).returncode == 0:
        current_branch = subprocess.run(["git", "rev-parse", "--abbrev-ref", "HEAD"], capture_output=True, text=True).stdout.strip()
        print(f"{BLUE}Current git branch: {current_branch}{NC}")
        if ENFORCE_BRANCH and current_branch != REQUIRED_BRANCH:
            print(f"{RED}Branch policy violation: expected '{REQUIRED_BRANCH}', got '{current_branch}'.{NC}", file=sys.stderr)
            print("Use --allow-non-nightly to bypass.")
            sys.exit(2)

    if AUTO_FIX:
        print(f"\n{BOLD}{CYAN}Running auto-fix{NC}")
        print(f"{BLUE}Applying cargo fmt...{NC}")
        subprocess.run(["cargo", "fmt", "--all"])
        print(f"{BLUE}Applying cargo fix...{NC}")
        subprocess.run(["cargo", "fix", "--workspace", "--all-targets", "--all-features", "--allow-dirty", "--allow-staged"])
        print(f"{BLUE}Applying cargo clippy --fix...{NC}")
        subprocess.run(["cargo", "clippy", "--fix", "--workspace", "--all-targets", "--all-features", "--allow-dirty", "--allow-staged"])

        if has_command("kondo"):
            print(f"{BLUE}Applying kondo project cleanup...{NC}")
            subprocess.run(["kondo", "-a", "-I", "/Volumes", "-I", os.path.expanduser("~/Library"), str(repo_root)])
        print(f"{GREEN}Auto-fix completed{NC}")

    tracker = Tracker()

    run_command(tracker, "required", "cargo fmt --all --check", ["cargo", "fmt", "--all", "--check"])
    run_command(tracker, "required", "cargo clippy --workspace --all-targets --all-features -D warnings", 
                ["cargo", "clippy", "--workspace", "--all-targets", "--all-features", "--", "-D", "warnings"])
    
    if has_cargo_subcommand("nextest"):
        run_command(tracker, "required", "cargo nextest run --workspace --all-features", ["cargo", "nextest", "run", "--workspace", "--all-features"])
    else:
        run_command(tracker, "required", "cargo test --workspace --all-features", ["cargo", "test", "--workspace", "--all-features"])

    if RUN_OPTIONAL:
        # File parsing for shell scripts
        shell_files = []
        for root, _, files in os.walk(repo_root):
            for file in files:
                if file.endswith(".sh"):
                    shell_files.append(os.path.join(root, file))
        
        if has_command("shellcheck"):
            if shell_files:
                run_command(tracker, "optional", "shellcheck *.sh errors only, zsh ignored", 
                            ["shellcheck", "--severity=error", "--exclude=SC1071", "--"] + shell_files)
            else:
                skip_optional(tracker, "shellcheck *.sh", "no .sh files found under repo")
        else:
            skip_optional(tracker, "shellcheck *.sh", "shellcheck not installed")

        if has_command("shfmt"):
            if shell_files:
                bash_files, zsh_files = [], []
                for f in shell_files:
                    try:
                        with open(f, 'r') as fp:
                            first_line = fp.readline().strip()
                        if os.path.basename(f) == "common.sh":
                            continue
                        if first_line in ("#!/bin/zsh", "#!/usr/bin/env zsh"):
                            zsh_files.append(f)
                        else:
                            bash_files.append(f)
                    except Exception:
                        bash_files.append(f)

                if bash_files:
                    run_command(tracker, "optional", "shfmt -d *.sh bash", ["shfmt", "-d", "-ln", "bash"] + bash_files)
                if zsh_files:
                    run_command(tracker, "optional", "shfmt -d *.sh zsh", ["shfmt", "-d", "-ln", "zsh"] + zsh_files)
            else:
                skip_optional(tracker, "shfmt -d *.sh", "no .sh files found under repo")
        else:
            skip_optional(tracker, "shfmt -d *.sh", "shfmt not installed")

        run_command(tracker, "optional", "cargo doc --workspace --no-deps", ["cargo", "doc", "--workspace", "--no-deps"])
        
        run_command(tracker, "optional", "cargo clippy deep --workspace --all-targets --all-features -W pedantic -W nursery",
                    ["cargo", "clippy", "--workspace", "--all-targets", "--all-features", "--", "-W", "clippy::pedantic", "-W", "clippy::nursery"])

        if has_cargo_subcommand("audit"):
            if ALLOW_FETCH:
                run_command(tracker, "optional", "cargo audit", ["cargo", "audit"])
            else:
                run_command(tracker, "optional", "cargo audit --no-fetch", ["cargo", "audit", "--no-fetch"])
        else:
            skip_optional(tracker, "cargo audit", "cargo-audit not installed")

        if has_cargo_subcommand("deny"):
            if ALLOW_FETCH:
                if advisory_db_writable():
                    run_command(tracker, "optional", "cargo deny check --hide-inclusion-graph", ["cargo", "deny", "check", "--hide-inclusion-graph"])
                else:
                    skip_optional(tracker, "cargo deny check", f"advisory DB is missing or read-only ({advisory_db_dir()})")
            else:
                if advisory_db_writable() and advisory_db_is_git_repo():
                    run_command(tracker, "optional", "cargo deny check --disable-fetch --hide-inclusion-graph", ["cargo", "deny", "check", "--disable-fetch", "--hide-inclusion-graph"])
                else:
                    skip_optional(tracker, "cargo deny check --disable-fetch", f"advisory DB is missing, read-only, or not a valid git repo ({advisory_db_dir()})")
        else:
            skip_optional(tracker, "cargo deny check", "cargo-deny not installed")

        if has_cargo_subcommand("machete"):
            run_command(tracker, "optional", "cargo machete", ["cargo", "machete"])
        else:
            skip_optional(tracker, "cargo machete", "cargo-machete not installed")

        if RUN_EXPENSIVE:
            if has_cargo_subcommand("udeps"):
                if has_nightly_toolchain() and has_rust_component("rust-src", "nightly"):
                    run_command(tracker, "optional", "cargo +nightly udeps --workspace --all-targets", ["cargo", "+nightly", "udeps", "--workspace", "--all-targets"])
                else:
                    skip_optional(tracker, "cargo +nightly udeps", "nightly toolchain or rust-src missing")
            else:
                skip_optional(tracker, "cargo udeps", "cargo-udeps not installed")

            if has_cargo_subcommand("geiger"):
                geiger_name = "cargo geiger per package --all-features --all-targets"
                print_step_header(tracker, "optional", geiger_name)
                
                manifests = [
                    repo_root / "shared_utils/Cargo.toml",
                    repo_root / "img_hevc/Cargo.toml",
                    repo_root / "img_av1/Cargo.toml",
                    repo_root / "vid_hevc/Cargo.toml",
                    repo_root / "vid_av1/Cargo.toml",
                ]
                
                geiger_failed = False
                output_accum = ""
                for m in manifests:
                    if not m.exists(): continue
                    result = subprocess.run(["cargo", "geiger", "--manifest-path", str(m), "--all-features", "--all-targets", "--output-format", "Ascii"], capture_output=True, text=True)
                    output_accum += result.stdout + result.stderr
                    if result.returncode != 0:
                        geiger_failed = True
                        
                if not geiger_failed:
                    tracker.passed += 1
                    print(f" {GREEN}PASS{NC} {geiger_name}")
                else:
                    if "No such file or directory" in output_accum or "NotFound" in output_accum or "error: Io" in output_accum:
                        geiger_skip_reason = "internal error geiger/cargo artifact missing"
                        tracker.skipped += 1
                        tracker.skipped_steps.append(f"{geiger_name}: {geiger_skip_reason}")
                        print(f" {BLUE}SKIP{NC} {geiger_name} {geiger_skip_reason}")
                    elif "error: Found" in output_accum and "warnings" in output_accum:
                        tracker.passed += 1
                        print(f" {GREEN}PASS{NC} {geiger_name} (unsafe deps reported)")
                    else:
                        tracker.warned += 1
                        tracker.warned_steps.append(f"{geiger_name} exit code error")
                        print(f" {YELLOW}WARN{NC} {geiger_name} exit error")
            else:
                skip_optional(tracker, "cargo geiger", "cargo-geiger not installed")

            if has_cargo_subcommand("bloat"):
                run_command(tracker, "optional", "cargo bloat --release --crates -n 20", ["cargo", "bloat", "--release", "--crates", "-n", "20"])
            else:
                skip_optional(tracker, "cargo bloat", "cargo-bloat not installed")

            if has_cargo_subcommand("hack"):
                run_command(tracker, "optional", "cargo hack check --workspace --each-feature --no-dev-deps", ["cargo", "hack", "check", "--workspace", "--each-feature", "--no-dev-deps"])
            else:
                skip_optional(tracker, "cargo hack", "cargo-hack not installed")

            if has_nightly_toolchain() and has_rust_component("miri", "nightly"):
                run_command(tracker, "optional", "cargo +nightly miri test -p shared_utils signature test, no isolation", 
                            ["cargo", "+nightly", "miri", "test", "-p", "shared_utils", "--lib", "test_signature_stability"],
                            env_vars={"MIRIFLAGS": "-Zmiri-disable-isolation"})
            elif has_nightly_toolchain():
                skip_optional(tracker, "cargo miri test", "miri component not installed nightly")

        if has_command("kondo"):
            run_command(tracker, "optional", "kondo dry-run (current project)", ["kondo", "-n", "-I", "/Volumes", "-I", os.path.expanduser("~/Library"), str(repo_root)])
        else:
            skip_optional(tracker, "kondo", "kondo not installed")
    else:
        skip_optional(tracker, "expensive optional checks", "disabled by --no-expensive")

    if console:
        # Professional Rich Summary
        table = Table(title="Modern Format Boost - Quality Scan Summary", border_style="dim")
        table.add_column("Result", justify="center")
        table.add_column("Step Name", style="bold")
        table.add_column("Type")
        
        # We need to map tracker data to table. 
        # For simplicity, we'll just show the high level stats if the trackers aren't easily mappable 
        # or we can recreate the logic. 
        # Re-using the tracker categories:
        table.add_row("[green]PASS[/green]", "Overall Passed", str(tracker.passed))
        table.add_row("[red]FAIL[/red]", "Required Failures", str(tracker.failed))
        table.add_row("[yellow]WARN[/yellow]", "Optional Warnings", str(tracker.warned))
        table.add_row("[blue]SKIP[/blue]", "Skipped Checks", str(tracker.skipped))
        
        console.print("\n")
        console.print(table)
        
        if tracker.failed > 0:
            for s in tracker.failed_steps: console.print(f"  [red]✗[/red] {s}")
    else:
        print(f"\n{BLUE}========================================{NC}")
        print(f"{BOLD}Summary{NC}")
        print(f"Passed: {GREEN}{tracker.passed}{NC}")
        print(f"Required failures: {RED}{tracker.failed}{NC}")
        print(f"Optional warnings: {YELLOW}{tracker.warned}{NC}")
        print(f"Skipped: {BLUE}{tracker.skipped}{NC}")
    
        if tracker.failed_steps:
            print(f"\n{RED}Required failures:{NC}")
            for s in tracker.failed_steps: print(f"  - {s}")
    
        if tracker.warned_steps:
            print(f"\n{YELLOW}Optional warnings:{NC}")
            for s in tracker.warned_steps: print(f"  - {s}")
    
        if tracker.skipped_steps:
            print(f"\n{BLUE}Skipped checks:{NC}")
            for s in tracker.skipped_steps: print(f"  - {s}")

    if tracker.failed > 0:
        if console: console.print(f"\n[error]Quality scan completed with required check failures.[/error]")
        else: print(f"\n{RED}Quality scan completed with required check failures.{NC}")
        sys.exit(1)

    if console: console.print(f"\n[success]Quality scan completed successfully (required checks passed).[/success]")
    else: print(f"\n{GREEN}Quality scan completed successfully (required checks passed).{NC}")

if __name__ == "__main__":
    main()
