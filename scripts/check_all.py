#!/usr/bin/env python3
"""
Comprehensive code quality scanner for Modern Format Boost.
A production-ready full-stack auditor for Rust, Python, Shell, and Documentation.
"""

import os
import sys
import subprocess
import shutil
import time
import argparse
from pathlib import Path
from dataclasses import dataclass, field
from typing import List, Optional
from functools import lru_cache

try:
    from rich.console import Console
    from rich.table import Table
    from rich.panel import Panel
    from rich.markup import escape
    console = Console()
except ImportError:
    console = None

# --- Configuration & Helpers ---

@lru_cache(maxsize=None)
def _has_command_internal(cmd: str) -> bool:
    """Internal cached check for command existence."""
    return shutil.which(cmd) is not None

@lru_cache(maxsize=None)
def _has_cargo_sub_internal(sub: str) -> bool:
    """Internal cached check for cargo subcommand."""
    try:
        res = subprocess.run(["cargo", sub, "--version"], capture_output=True)
        return res.returncode == 0
    except Exception:
        return False

def has_command(cmd: str, hint_pkg: Optional[str] = None, verbose: bool = False) -> bool:
    """UI wrapper for command check, preserves hints regardless of cache."""
    found = _has_command_internal(cmd)
    if not found and verbose:
        pkg = hint_pkg or cmd
        msg = f"  [yellow]Hint: '{cmd}' missing. Install: brew/npm/pip install {pkg}[/yellow]"
        if console: console.print(msg)
        else: print(msg)
    return found

def has_cargo_subcommand(sub: str, verbose: bool = False) -> bool:
    """UI wrapper for cargo sub check, preserves hints regardless of cache."""
    found = _has_cargo_sub_internal(sub)
    if not found and verbose:
        msg = f"  [yellow]Hint: cargo-{sub} missing. Install: cargo install cargo-{sub}[/yellow]"
        if console: console.print(msg)
        else: print(msg)
    return found

def get_repo_root() -> Path:
    """Identify project root using git, fallback to script parent's parent."""
    try:
        root = subprocess.check_output(["git", "rev-parse", "--show-toplevel"], 
                                       stderr=subprocess.STDOUT, text=True).strip()
        return Path(root)
    except Exception:
        return Path(__file__).parent.parent.resolve()

@dataclass
class Tracker:
    step_count: int = 0
    passed: int = 0
    failed: int = 0
    warned: int = 0
    skipped: int = 0
    failed_steps: List[str] = field(default_factory=list)
    warned_steps: List[str] = field(default_factory=list)
    skipped_steps: List[str] = field(default_factory=list)

    def announce_step(self, kind: str, name: str):
        self.step_count += 1
        if console:
            icon = "🔍" if kind == "required" else "💡"
            # Use escape() to prevent names with brackets from breaking rich markup
            console.print(f"\n[bold][{self.step_count}] {icon} {kind.upper()}: {escape(name)}[/bold]")
        else:
            print(f"\n[{self.step_count}] {kind.upper()}: {name}")

# --- Runner Engine ---

def format_duration(seconds: float) -> str:
    if seconds < 1:
        return f"{int(seconds * 1000)}ms"
    return f"{seconds:.2f}s"

def run_step(tracker: Tracker, kind: str, name: str, cmd: List[str], 
             env_vars: Optional[dict] = None) -> bool:
    tracker.announce_step(kind, name)
    
    start_time = time.time()
    env = os.environ.copy()
    if env_vars:
        env.update(env_vars)
        
    process = subprocess.Popen(cmd, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, 
                               text=True, env=env, bufsize=1)
    
    # NOTE: stdout/stderr must be fully consumed BEFORE calling wait() 
    # to avoid potential OS pipe deadlocks when buffers are full (approx 64KB).
    if process.stdout:
        for line in process.stdout:
            sys.stdout.write(line)
            sys.stdout.flush()
    
    process.wait()
    duration = format_duration(time.time() - start_time)
    
    if process.returncode == 0:
        tracker.passed += 1
        msg = f"  [green]✅ PASS[/green] ({duration})"
        if console: console.print(msg)
        else: print(msg)
        return True
    else:
        if kind == "required":
            tracker.failed += 1
            tracker.failed_steps.append(name)
            msg = f"  [red]❌ FAIL[/red] (exit {process.returncode}, {duration})"
        else:
            tracker.warned += 1
            tracker.warned_steps.append(name)
            msg = f"  [yellow]⚠️  WARN[/yellow] (exit {process.returncode}, {duration})"
        
        if console: console.print(msg)
        else: print(msg)
        return False

def skip_step(tracker: Tracker, name: str, reason: str):
    tracker.announce_step("optional", name)
    tracker.skipped += 1
    tracker.skipped_steps.append(f"{name} ({reason})")
    msg = f"  [blue]⏭️  SKIP[/blue] ({reason})"
    if console: console.print(msg)
    else: print(msg)

# --- Argument Parsing ---

def parse_args():
    # Use environment variables as defaults in argparse to maintain natural override order
    default_branch = os.environ.get("CHECK_ALL_DEFAULT_BRANCH", "nightly")
    
    parser = argparse.ArgumentParser(description="Modern Format Boost Multi-Language Auditor")
    parser.add_argument("--allow-non-nightly", action="store_true", help="Don't enforce branch check")
    parser.add_argument("--required-only", action="store_true", help="Skip optional checks")
    parser.add_argument("--no-expensive", action="store_true", help="Skip slow checks (bloat, hack)")
    parser.add_argument("--fix", action="store_true", help="Auto-fix formatting")
    parser.add_argument("--build", action="store_true", help="Run full release build")
    parser.add_argument("--branch", default=default_branch, help=f"Required branch (default: {default_branch})")
    parser.add_argument("--verbose", "-v", action="store_true", help="Show tool install hints")
    return parser.parse_args()

# --- Main Application ---

def main():
    args = parse_args()
    repo_root = get_repo_root()
    os.chdir(repo_root)
    tracker = Tracker()

    if console:
        console.print(Panel(f"[bold cyan]🚀 Modern Quality Suite[/bold cyan]\n[dim]Root: {repo_root}[/dim]", border_style="blue"))
    else:
        print(f"--- Modern Quality Suite ---\nRoot: {repo_root}")

    # --- 1. Environment Guard ---
    try:
        current_branch = subprocess.check_output(["git", "rev-parse", "--abbrev-ref", "HEAD"], text=True).strip()
        if not args.allow_non_nightly and current_branch != args.branch:
            print(f"Fatal: Required branch '{args.branch}', but current is '{current_branch}'. Use --allow-non-nightly or --branch <name>.")
            sys.exit(2)
    except Exception:
        pass

    # File Discovery
    try:
        git_files = subprocess.check_output(["git", "ls-files"], text=True).splitlines()
    except Exception:
        git_files = [str(p.relative_to(repo_root)) for p in repo_root.rglob("*") if p.is_file()]

    # Scoped buckets
    py_files = [f for f in git_files if f.endswith(".py")]
    shell_files = [f for f in git_files if f.endswith(".sh")]
    md_files = [f for f in git_files if f.endswith(".md")]
    json_files = [f for f in git_files if f.endswith(".json")]
    yaml_files = [f for f in git_files if f.endswith((".yml", ".yaml"))]
    toml_files = [f for f in git_files if f.endswith(".toml")]

    # --- 2. Auto-Fix Phase ---
    if args.fix:
        if console: console.print("\n[bold cyan]🔧 Running Auto-Fix Cycle...[/bold cyan]")
        
        # Rust fixes
        subprocess.run(["cargo", "fmt", "--all"])
        subprocess.run(["cargo", "clippy", "--fix", "--workspace", "--all-targets", "--all-features", "--allow-dirty", "--allow-staged"])
        
        # Python fixes
        if has_command("ruff"):
            subprocess.run(["ruff", "check", "--fix", "."], stderr=subprocess.DEVNULL)
            subprocess.run(["ruff", "format", "."], stderr=subprocess.DEVNULL)
        if has_command("pyupgrade") and py_files:
            subprocess.run(["pyupgrade", "--py311-plus"] + py_files)
            
        # Doc/Config fixes
        if has_command("prettier"):
            fmt_targets = md_files + json_files + yaml_files
            if fmt_targets:
                subprocess.run(["prettier", "--write"] + fmt_targets, stderr=subprocess.DEVNULL)
        
        if has_cargo_subcommand("taplo"):
            subprocess.run(["cargo", "taplo", "fmt"])

        if has_command("kondo"):
            kondo_cmd = ["kondo", "-a"]
            if sys.platform == "darwin":
                kondo_cmd.extend(["-I", "/Volumes", "-I", os.path.expanduser("~/Library")])
            subprocess.run(kondo_cmd + [str(repo_root)], stdout=subprocess.DEVNULL)

    # --- 3. Required Checks ---
    run_step(tracker, "required", "cargo fmt --check", ["cargo", "fmt", "--all", "--check"])
    run_step(tracker, "required", "cargo check --workspace --all-features", ["cargo", "check", "--workspace", "--all-targets", "--all-features"])
    
    if py_files:
        run_step(tracker, "required", f"python3 syntax ({len(py_files)} files)", [sys.executable, "-m", "py_compile"] + py_files)
    else:
        skip_step(tracker, "python syntax check", "no scripts")
    
    clippy_all = ["cargo", "clippy", "--workspace", "--all-targets", "--all-features", "--", "-D", "warnings"]
    run_step(tracker, "required", "cargo clippy (all-features)", clippy_all)

    # Optimized testing logic
    if has_cargo_subcommand("nextest"):
        run_step(tracker, "required", "cargo nextest run", ["cargo", "nextest", "run", "--workspace", "--all-features"])
    else:
        run_step(tracker, "required", "cargo test", ["cargo", "test", "--workspace", "--all-features"])

    if args.build:
        run_step(tracker, "required", "cargo build --release", ["cargo", "build", "--release", "--workspace"])

    # --- 4. Optional Checks ---
    if not args.required_only:
        # Python
        if py_files and has_command("ruff", verbose=args.verbose):
            run_step(tracker, "optional", "ruff linter", ["ruff", "check"] + py_files)
            run_step(tracker, "optional", "ruff format check", ["ruff", "format", "--check"] + py_files)
        elif not py_files:
            skip_step(tracker, "python quality", "no scripts")

        # Shell
        if shell_files:
            if has_command("shellcheck", verbose=args.verbose):
                run_step(tracker, "optional", "shellcheck security suite", ["shellcheck", "--severity=error"] + shell_files)
            if has_command("shfmt", verbose=args.verbose):
                run_step(tracker, "optional", "shfmt layout check", ["shfmt", "-d", "-i", "4"] + shell_files)
        
        # Docs & Config
        if md_files and has_command("markdownlint-cli2", verbose=args.verbose) :
            run_step(tracker, "optional", "markdown formatting", ["markdownlint-cli2"] + md_files)
        
        if has_command("prettier", verbose=args.verbose):
            targets = md_files + json_files + yaml_files
            if targets:
                run_step(tracker, "optional", "prettier styling check", ["prettier", "--check"] + targets)

        if toml_files and has_cargo_subcommand("taplo", verbose=args.verbose):
            run_step(tracker, "optional", "cargo toml formatting", ["cargo", "taplo", "fmt", "--check"])

        run_step(tracker, "optional", "cargo doc metadata", ["cargo", "doc", "--workspace", "--no-deps"])

        if not args.no_expensive:
            if has_cargo_subcommand("bloat", verbose=args.verbose):
                run_step(tracker, "optional", "binary size profile", ["cargo", "bloat", "--release", "--crates", "-n", "10"])
            
            if has_cargo_subcommand("hack", verbose=args.verbose):
                run_step(tracker, "optional", "feature matrix check", ["cargo", "hack", "check", "--workspace", "--each-feature", "--no-dev-deps"])

    # --- 5. Summary Generation ---
    if console:
        # Rich Terminal Output
        table = Table(title="\n📊 Code Quality Summary", border_style="dim", expand=True)
        table.add_column("Category", style="cyan")
        table.add_column("Count", justify="right")
        table.add_column("Details", style="dim")
        
        table.add_row("✅ Passed", str(tracker.passed), "[green]All clear[/green]")
        table.add_row("❌ Failed", str(tracker.failed), f"[red]{', '.join(tracker.failed_steps)}[/red]" if tracker.failed_steps else "-")
        table.add_row("⚠️  Warned", str(tracker.warned), f"[yellow]{', '.join(tracker.warned_steps)}[/yellow]" if tracker.warned_steps else "-")
        table.add_row("⏭️  Skipped", str(tracker.skipped), f"[blue]{len(tracker.skipped_steps)} items[/blue]" if tracker.skipped_steps else "-")
        
        console.print(table)
        
        if tracker.skipped_steps:
             console.print("\n[bold blue]Skipped Details:[/bold blue]")
             for s in tracker.skipped_steps: console.print(f"  [dim]• {s}[/dim]")
        
        if tracker.warned_steps:
             console.print("\n[bold yellow]Warnings Found In:[/bold yellow]")
             for s in tracker.warned_steps: console.print(f"  [yellow]! {s}[/yellow]")
    else:
        # Legacy/CLI Output
        print(f"\n{'='*40}\n{'Summary':^40}\n{'='*40}")
        print(f"Passed:   {tracker.passed}\nFailed:   {tracker.failed}\nWarned:   {tracker.warned}\nSkipped:  {tracker.skipped}")
        
        if tracker.failed_steps:
            print("\nREQUIRED FAILURES:")
            for s in tracker.failed_steps: print(f"  [X] {s}")
        if tracker.warned_steps:
            print("\nOPTIONAL WARNINGS:")
            for s in tracker.warned_steps: print(f"  [!] {s}")
        if tracker.skipped_steps:
            print("\nSKIPPED CHECKS:")
            for s in tracker.skipped_steps: print(f"  [-] {s}")

    if tracker.failed > 0:
        if console: console.print(f"\n[bold red]Audit failed with {tracker.failed} errors.[/bold red]")
        sys.exit(1)
    
    if console: console.print(f"\n[bold green]🌟 Workspace is healthy![/bold green]")

if __name__ == "__main__":
    main()
