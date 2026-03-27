#!/usr/bin/env python3
"""Comprehensive code quality scanner and system diagnostic tool (Strict Parity Edition)"""

import os
import sys
import subprocess
import time
from pathlib import Path

# ANSI 颜色 (对应原版)
RED = '\033[0;31m'
GREEN = '\033[0;32m'
YELLOW = '\033[1;33m'
BLUE = '\033[0;34m'
CYAN = '\033[0;36m'
BOLD = '\033[1m'
NC = '\033[0m'

# 全局配置
REQUIRED_BRANCH = os.environ.get("CHECK_ALL_DEFAULT_BRANCH", "nightly")
ENFORCE_BRANCH = True
RUN_OPTIONAL = True
RUN_EXPENSIVE = True
RUN_DIAGNOSE = False
ALLOW_FETCH = False
AUTO_FIX = False
AUTO_FIX_YES = False

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

def show_help():
    help_text = f"""Usage: scripts/check_all.py [options]

Options:
  --allow-non-nightly    Do not enforce '{REQUIRED_BRANCH}' git branch.
  --required-only        Run only required checks (fmt/clippy/tests).
  --no-expensive         Skip expensive optional checks (udeps/bloat/hack/miri).
  --diagnose             Run system resource and process diagnostics.
  --fetch-advisory-db    Allow network fetch for cargo-audit/cargo-deny.
  --fix                  Auto-fix issues (cargo fmt, cargo clippy --fix). Requires --yes.
  --yes                  Skip confirmation prompt for --fix.
  -h, --help             Show this help.

Environment:
  CHECK_ALL_DEFAULT_BRANCH=<branch>  Override default required branch (default: nightly)."""
    print(help_text)

def has_cargo_subcommand(sub, pkg=None):
    if pkg is None: pkg = f"cargo-{sub}"
    result = subprocess.run(["cargo", sub, "--version"], capture_output=True)
    if result.returncode == 0:
        return True
    print(f"{YELLOW}Hint: '{sub}' requires {pkg}. Install with: cargo install {pkg}{NC}", file=sys.stderr)
    return False

def format_duration(ms):
    if ms < 1000:
        return f"{int(ms)}ms"
    return f"{ms/1000:.2f}s"

def print_step_header(tracker, kind, name):
    tracker.step += 1
    print(f"\n{BOLD}[{tracker.step}] {kind}: {name}{NC}", end="", flush=True)

def run_command(tracker, kind, name, cmd):
    """执行命令并实时输出到终端 (对齐 Bash 行为)"""
    print_step_header(tracker, kind, name)
    print("") # Bash 版在执行前有个换行
    
    start = time.time()
    # 采用直接运行而不 capture_output，使子进程的 stdout 连通终端
    result = subprocess.run(cmd)
    end = time.time()
    
    duration = format_duration((end - start) * 1000)
    
    if result.returncode == 0:
        tracker.passed += 1
        print(f" {GREEN}PASS{NC} ({duration})")
    else:
        if kind == "required":
            tracker.failed += 1
            tracker.failed_steps.append(f"{name} (exit {result.returncode})")
            print(f" {RED}FAIL{NC} (exit {result.returncode}, {duration})")
        else:
            tracker.warned += 1
            tracker.warned_steps.append(f"{name} (exit {result.returncode})")
            print(f" {YELLOW}WARN{NC} (exit {result.returncode}, {duration})")

def skip_optional(tracker, name, reason):
    print_step_header(tracker, "optional", name)
    tracker.skipped += 1
    tracker.skipped_steps.append(f"{name}: {reason}")
    print(f" {BLUE}SKIP{NC} ({reason})")

def run_diagnostics():
    print(f"\n{BOLD}{CYAN}--- System Diagnostics ({os.uname().nodename}) ---{NC}")
    
    print(f"{CYAN}--- Total System Threads ---{NC}")
    if sys.platform == "darwin":
        subprocess.run(["sysctl", "kern.num_threads"])
    else:
        subprocess.run(["cat", "/proc/loadavg"])
        
    print(f"{CYAN}--- Total File Descriptors (FD) ---{NC}")
    subprocess.run("lsof 2>/dev/null | wc -l", shell=True)
    
    print(f"{CYAN}--- img-hevc Thread Count ---{NC}")
    pids = subprocess.run("pgrep img-hevc | tr '\\n' ' '", shell=True, capture_output=True, text=True).stdout.strip()
    if not pids:
        print("No img-hevc processes found.")
    else:
        subprocess.run(f"ps -M -p {pids} 2>/dev/null | wc -l", shell=True)
        
    print(f"{CYAN}--- VTEncoderXPCService Instances ---{NC}")
    subprocess.run("ps aux | grep -c [V]TEncoderXPCService", shell=True)
    
    print(f"{CYAN}--- Top 10 Processes by FD Count ---{NC}")
    subprocess.run("lsof -u \"$USER\" 2>/dev/null | awk '{print $2}' | sort | uniq -c | sort -rn | head -10", shell=True)
    
    print(f"{CYAN}--- Zombie Processes ---{NC}")
    subprocess.run("ps aux | awk '$8==\"Z\"' | grep -v \"grep\"", shell=True)

def main():
    global ENFORCE_BRANCH, RUN_OPTIONAL, RUN_EXPENSIVE, RUN_DIAGNOSE, ALLOW_FETCH, AUTO_FIX, AUTO_FIX_YES
    
    script_dir = Path(__file__).parent.resolve()
    repo_root = script_dir.parent
    
    if not (repo_root / "Cargo.toml").is_file():
        print(f"Error: REPO_ROOT '{repo_root}' does not look like a Cargo workspace (no Cargo.toml found).", file=sys.stderr)
        sys.exit(2)
        
    os.chdir(repo_root)
    
    # Args Parsing
    args = sys.argv[1:]
    while args:
        arg = args.pop(0)
        if arg == "--allow-non-nightly": ENFORCE_BRANCH = False
        elif arg == "--required-only": RUN_OPTIONAL = False
        elif arg == "--no-expensive": RUN_EXPENSIVE = False
        elif arg == "--diagnose": RUN_DIAGNOSE = True
        elif arg == "--fetch-advisory-db": ALLOW_FETCH = True
        elif arg == "--fix": AUTO_FIX = True
        elif arg == "--yes": AUTO_FIX_YES = True
        elif arg in ("-h", "--help"):
            show_help()
            sys.exit(0)
        else:
            print(f"Unknown option: {arg}", file=sys.stderr)
            show_help()
            sys.exit(2)

    print(f"{BOLD}{CYAN}Starting Modern Format Boost Health Check{NC}")
    print(f"{BLUE}Repo: {repo_root}{NC}")

    # Branch Check
    if subprocess.run(["git", "rev-parse", "--is-inside-work-tree"], capture_output=True).returncode == 0:
        current_branch = subprocess.run(["git", "rev-parse", "--abbrev-ref", "HEAD"], capture_output=True, text=True).stdout.strip()
        if ENFORCE_BRANCH and current_branch != REQUIRED_BRANCH:
            print(f"{RED}Branch policy violation: expected '{REQUIRED_BRANCH}', got '{current_branch}'.{NC}", file=sys.stderr)
            sys.exit(2)

    # Auto Fix
    if AUTO_FIX:
        if not AUTO_FIX_YES:
            print(f"{YELLOW}--fix will modify source files in {repo_root}. Pass --yes to confirm.{NC}", file=sys.stderr)
            sys.exit(2)
        print(f"\n{BOLD}{CYAN}Running auto-fix{NC}")
        subprocess.run(["cargo", "fmt", "--all"])
        subprocess.run(["cargo", "fix", "--workspace", "--all-targets", "--all-features", "--allow-dirty", "--allow-staged"])
        subprocess.run(["cargo", "clippy", "--fix", "--workspace", "--all-targets", "--all-features", "--allow-dirty", "--allow-staged"])

    tracker = Tracker()

    # Required Checks
    run_command(tracker, "required", "cargo fmt --all --check", ["cargo", "fmt", "--all", "--check"])
    run_command(tracker, "required", "cargo clippy --workspace --all-targets --all-features -D warnings", 
                ["cargo", "clippy", "--workspace", "--all-targets", "--all-features", "--", "-D", "warnings"])
    
    if has_cargo_subcommand("nextest"):
        run_command(tracker, "required", "cargo nextest run --workspace --all-features", ["cargo", "nextest", "run", "--workspace", "--all-features"])
    else:
        run_command(tracker, "required", "cargo test --workspace --all-features", ["cargo", "test", "--workspace", "--all-features"])

    # Optional Checks
    if RUN_OPTIONAL:
        run_command(tracker, "optional", "cargo doc --workspace --no-deps", ["cargo", "doc", "--workspace", "--no-deps"])
        
        if has_cargo_subcommand("audit"):
            if ALLOW_FETCH:
                run_command(tracker, "optional", "cargo audit", ["cargo", "audit"])
            else:
                run_command(tracker, "optional", "cargo audit --no-fetch", ["cargo", "audit", "--no-fetch"])
                
        if RUN_EXPENSIVE and has_cargo_subcommand("bloat"):
            run_command(tracker, "optional", "cargo bloat --release --crates -n 10", ["cargo", "bloat", "--release", "--crates", "-n", "10"])

    # Diagnostics
    if RUN_DIAGNOSE:
        run_diagnostics()

    # Summary
    print(f"\n{BLUE}========================================{NC}")
    print(f"{BOLD}Summary{NC}")
    print(f"Passed:            {GREEN}{tracker.passed}{NC}")
    print(f"Required failures: {RED}{tracker.failed}{NC}")
    print(f"Optional warnings: {YELLOW}{tracker.warned}{NC}")
    print(f"Skipped:           {BLUE}{tracker.skipped}{NC}")

    if tracker.failed_steps:
        print(f"\n{RED}Failed:{NC}")
        for s in tracker.failed_steps: print(f"  {RED}✗{NC} {s}")
        
    if tracker.warned_steps:
        print(f"\n{YELLOW}Warnings:{NC}")
        for s in tracker.warned_steps: print(f"  {YELLOW}!{NC} {s}")
        
    if tracker.skipped_steps:
        print(f"\n{BLUE}Skipped:{NC}")
        for s in tracker.skipped_steps: print(f"  {BLUE}-{NC} {s}")

    if tracker.failed > 0:
        print(f"\n{RED}Health check completed with required failures.{NC}")
        sys.exit(1)

    print(f"\n{GREEN}Health check completed successfully.{NC}")

if __name__ == "__main__":
    main()#!/usr/bin/env python3
"""Comprehensive code quality scanner and system diagnostic tool (Strict Parity Edition)"""

import os
import sys
import subprocess
import time
from pathlib import Path

# ANSI 颜色 (对应原版)
RED = '\033[0;31m'
GREEN = '\033[0;32m'
YELLOW = '\033[1;33m'
BLUE = '\033[0;34m'
CYAN = '\033[0;36m'
BOLD = '\033[1m'
NC = '\033[0m'

# 全局配置
REQUIRED_BRANCH = os.environ.get("CHECK_ALL_DEFAULT_BRANCH", "nightly")
ENFORCE_BRANCH = True
RUN_OPTIONAL = True
RUN_EXPENSIVE = True
RUN_DIAGNOSE = False
ALLOW_FETCH = False
AUTO_FIX = False
AUTO_FIX_YES = False

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

def show_help():
    help_text = f"""Usage: scripts/check_all.py [options]

Options:
  --allow-non-nightly    Do not enforce '{REQUIRED_BRANCH}' git branch.
  --required-only        Run only required checks (fmt/clippy/tests).
  --no-expensive         Skip expensive optional checks (udeps/bloat/hack/miri).
  --diagnose             Run system resource and process diagnostics.
  --fetch-advisory-db    Allow network fetch for cargo-audit/cargo-deny.
  --fix                  Auto-fix issues (cargo fmt, cargo clippy --fix). Requires --yes.
  --yes                  Skip confirmation prompt for --fix.
  -h, --help             Show this help.

Environment:
  CHECK_ALL_DEFAULT_BRANCH=<branch>  Override default required branch (default: nightly)."""
    print(help_text)

def has_cargo_subcommand(sub, pkg=None):
    if pkg is None: pkg = f"cargo-{sub}"
    result = subprocess.run(["cargo", sub, "--version"], capture_output=True)
    if result.returncode == 0:
        return True
    print(f"{YELLOW}Hint: '{sub}' requires {pkg}. Install with: cargo install {pkg}{NC}", file=sys.stderr)
    return False

def format_duration(ms):
    if ms < 1000:
        return f"{int(ms)}ms"
    return f"{ms/1000:.2f}s"

def print_step_header(tracker, kind, name):
    tracker.step += 1
    print(f"\n{BOLD}[{tracker.step}] {kind}: {name}{NC}", end="", flush=True)

def run_command(tracker, kind, name, cmd):
    """执行命令并实时输出到终端 (对齐 Bash 行为)"""
    print_step_header(tracker, kind, name)
    print("") # Bash 版在执行前有个换行
    
    start = time.time()
    # 采用直接运行而不 capture_output，使子进程的 stdout 连通终端
    result = subprocess.run(cmd)
    end = time.time()
    
    duration = format_duration((end - start) * 1000)
    
    if result.returncode == 0:
        tracker.passed += 1
        print(f" {GREEN}PASS{NC} ({duration})")
    else:
        if kind == "required":
            tracker.failed += 1
            tracker.failed_steps.append(f"{name} (exit {result.returncode})")
            print(f" {RED}FAIL{NC} (exit {result.returncode}, {duration})")
        else:
            tracker.warned += 1
            tracker.warned_steps.append(f"{name} (exit {result.returncode})")
            print(f" {YELLOW}WARN{NC} (exit {result.returncode}, {duration})")

def skip_optional(tracker, name, reason):
    print_step_header(tracker, "optional", name)
    tracker.skipped += 1
    tracker.skipped_steps.append(f"{name}: {reason}")
    print(f" {BLUE}SKIP{NC} ({reason})")

def run_diagnostics():
    print(f"\n{BOLD}{CYAN}--- System Diagnostics ({os.uname().nodename}) ---{NC}")
    
    print(f"{CYAN}--- Total System Threads ---{NC}")
    if sys.platform == "darwin":
        subprocess.run(["sysctl", "kern.num_threads"])
    else:
        subprocess.run(["cat", "/proc/loadavg"])
        
    print(f"{CYAN}--- Total File Descriptors (FD) ---{NC}")
    subprocess.run("lsof 2>/dev/null | wc -l", shell=True)
    
    print(f"{CYAN}--- img-hevc Thread Count ---{NC}")
    pids = subprocess.run("pgrep img-hevc | tr '\\n' ' '", shell=True, capture_output=True, text=True).stdout.strip()
    if not pids:
        print("No img-hevc processes found.")
    else:
        subprocess.run(f"ps -M -p {pids} 2>/dev/null | wc -l", shell=True)
        
    print(f"{CYAN}--- VTEncoderXPCService Instances ---{NC}")
    subprocess.run("ps aux | grep -c [V]TEncoderXPCService", shell=True)
    
    print(f"{CYAN}--- Top 10 Processes by FD Count ---{NC}")
    subprocess.run("lsof -u \"$USER\" 2>/dev/null | awk '{print $2}' | sort | uniq -c | sort -rn | head -10", shell=True)
    
    print(f"{CYAN}--- Zombie Processes ---{NC}")
    subprocess.run("ps aux | awk '$8==\"Z\"' | grep -v \"grep\"", shell=True)

def main():
    global ENFORCE_BRANCH, RUN_OPTIONAL, RUN_EXPENSIVE, RUN_DIAGNOSE, ALLOW_FETCH, AUTO_FIX, AUTO_FIX_YES
    
    script_dir = Path(__file__).parent.resolve()
    repo_root = script_dir.parent
    
    if not (repo_root / "Cargo.toml").is_file():
        print(f"Error: REPO_ROOT '{repo_root}' does not look like a Cargo workspace (no Cargo.toml found).", file=sys.stderr)
        sys.exit(2)
        
    os.chdir(repo_root)
    
    # Args Parsing
    args = sys.argv[1:]
    while args:
        arg = args.pop(0)
        if arg == "--allow-non-nightly": ENFORCE_BRANCH = False
        elif arg == "--required-only": RUN_OPTIONAL = False
        elif arg == "--no-expensive": RUN_EXPENSIVE = False
        elif arg == "--diagnose": RUN_DIAGNOSE = True
        elif arg == "--fetch-advisory-db": ALLOW_FETCH = True
        elif arg == "--fix": AUTO_FIX = True
        elif arg == "--yes": AUTO_FIX_YES = True
        elif arg in ("-h", "--help"):
            show_help()
            sys.exit(0)
        else:
            print(f"Unknown option: {arg}", file=sys.stderr)
            show_help()
            sys.exit(2)

    print(f"{BOLD}{CYAN}Starting Modern Format Boost Health Check{NC}")
    print(f"{BLUE}Repo: {repo_root}{NC}")

    # Branch Check
    if subprocess.run(["git", "rev-parse", "--is-inside-work-tree"], capture_output=True).returncode == 0:
        current_branch = subprocess.run(["git", "rev-parse", "--abbrev-ref", "HEAD"], capture_output=True, text=True).stdout.strip()
        if ENFORCE_BRANCH and current_branch != REQUIRED_BRANCH:
            print(f"{RED}Branch policy violation: expected '{REQUIRED_BRANCH}', got '{current_branch}'.{NC}", file=sys.stderr)
            sys.exit(2)

    # Auto Fix
    if AUTO_FIX:
        if not AUTO_FIX_YES:
            print(f"{YELLOW}--fix will modify source files in {repo_root}. Pass --yes to confirm.{NC}", file=sys.stderr)
            sys.exit(2)
        print(f"\n{BOLD}{CYAN}Running auto-fix{NC}")
        subprocess.run(["cargo", "fmt", "--all"])
        subprocess.run(["cargo", "fix", "--workspace", "--all-targets", "--all-features", "--allow-dirty", "--allow-staged"])
        subprocess.run(["cargo", "clippy", "--fix", "--workspace", "--all-targets", "--all-features", "--allow-dirty", "--allow-staged"])

    tracker = Tracker()

    # Required Checks
    run_command(tracker, "required", "cargo fmt --all --check", ["cargo", "fmt", "--all", "--check"])
    run_command(tracker, "required", "cargo clippy --workspace --all-targets --all-features -D warnings", 
                ["cargo", "clippy", "--workspace", "--all-targets", "--all-features", "--", "-D", "warnings"])
    
    if has_cargo_subcommand("nextest"):
        run_command(tracker, "required", "cargo nextest run --workspace --all-features", ["cargo", "nextest", "run", "--workspace", "--all-features"])
    else:
        run_command(tracker, "required", "cargo test --workspace --all-features", ["cargo", "test", "--workspace", "--all-features"])

    # Optional Checks
    if RUN_OPTIONAL:
        run_command(tracker, "optional", "cargo doc --workspace --no-deps", ["cargo", "doc", "--workspace", "--no-deps"])
        
        if has_cargo_subcommand("audit"):
            if ALLOW_FETCH:
                run_command(tracker, "optional", "cargo audit", ["cargo", "audit"])
            else:
                run_command(tracker, "optional", "cargo audit --no-fetch", ["cargo", "audit", "--no-fetch"])
                
        if RUN_EXPENSIVE and has_cargo_subcommand("bloat"):
            run_command(tracker, "optional", "cargo bloat --release --crates -n 10", ["cargo", "bloat", "--release", "--crates", "-n", "10"])

    # Diagnostics
    if RUN_DIAGNOSE:
        run_diagnostics()

    # Summary
    print(f"\n{BLUE}========================================{NC}")
    print(f"{BOLD}Summary{NC}")
    print(f"Passed:            {GREEN}{tracker.passed}{NC}")
    print(f"Required failures: {RED}{tracker.failed}{NC}")
    print(f"Optional warnings: {YELLOW}{tracker.warned}{NC}")
    print(f"Skipped:           {BLUE}{tracker.skipped}{NC}")

    if tracker.failed_steps:
        print(f"\n{RED}Failed:{NC}")
        for s in tracker.failed_steps: print(f"  {RED}✗{NC} {s}")
        
    if tracker.warned_steps:
        print(f"\n{YELLOW}Warnings:{NC}")
        for s in tracker.warned_steps: print(f"  {YELLOW}!{NC} {s}")
        
    if tracker.skipped_steps:
        print(f"\n{BLUE}Skipped:{NC}")
        for s in tracker.skipped_steps: print(f"  {BLUE}-{NC} {s}")

    if tracker.failed > 0:
        print(f"\n{RED}Health check completed with required failures.{NC}")
        sys.exit(1)

    print(f"\n{GREEN}Health check completed successfully.{NC}")

if __name__ == "__main__":
    main()