#!/usr/bin/env python3
import os
import sys
import argparse
import subprocess
import time
import shutil
from pathlib import Path

# Color Definitions
if sys.stdout.isatty():
    RED = "\033[38;5;196m"
    GREEN = "\033[38;5;46m"
    YELLOW = "\033[38;5;226m"
    CYAN = "\033[38;5;51m"
    BOLD = "\033[1m"
    DIM = "\033[2m"
    NC = "\033[0m"
else:
    RED = GREEN = YELLOW = CYAN = BOLD = DIM = NC = ""

ALL_PROJECTS = {"crates/img": "img", "crates/vid": "vid"}

DEFAULT_PROJECTS = ["crates/img", "crates/vid"]

SCRIPT_DIR = Path(__file__).parent.resolve()
PROJECT_ROOT = SCRIPT_DIR.parent.parent.parent
os.chdir(PROJECT_ROOT)


def print_header():
    print()
    print(f"{CYAN}{BOLD}Smart Build System v0.11.2 (Python Edition){NC}")
    print(f"{DIM}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━{NC}")


def print_status(project, action, reason=""):
    if action == "skip":
        print(f"[OK] {BOLD}{project}{NC} {DIM}(up-to-date){NC}")
    elif action == "rebuild":
        print(f"[BUILD] {BOLD}{project}{NC} {DIM}({reason}){NC}")


def print_success(project):
    print(f"[OK] {BOLD}{project}{NC} - compiled")


def print_error(msg):
    print()
    print(f"{RED}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━{NC}")
    print(f"{RED}FAILURE: {msg}{NC}")
    print(f"{RED}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━{NC}")
    print()


def clean_old_binaries():
    print(f"{YELLOW}Cleaning old binaries...{NC}")
    cleaned = 0

    for binary_name in ALL_PROJECTS.values():
        for p in PROJECT_ROOT.rglob(binary_name):
            if p.is_file() and "target" not in p.parts:
                print(f"   {RED}Removing: {DIM}{p}{NC}")
                try:
                    p.unlink()
                    cleaned += 1
                except OSError:
                    pass

    if cleaned == 0:
        print(f"   [OK] {DIM}No old binaries found{NC}")
    else:
        print(f"   [OK] Cleaned {cleaned} old binary file(s){NC}")
    print()


def clean_with_kondo():
    if not shutil.which("kondo"):
        print(f"{DIM}kondo not found; skipping deep cleanup.{NC}")
        return
    print(f"{YELLOW}Project Deep Cleanup (kondo)...{NC}")
    subprocess.run(
        ["kondo", "-n", "-I", "/Volumes", "-I", os.path.expanduser("~/Library"), "."]
    )
    print()


def get_newest_source_mtime(project_dir):
    """
    Find the most recent modification time among all source files that
    could affect the build of the given project.
    """
    newest = 0.0
    # Common extensions that affect the build
    src_extensions = {".rs", ".sql", ".c", ".h", ".cpp", ".cc", ".proto", ".py", ".sh"}

    def check_file(p):
        nonlocal newest
        if p.is_file():
            try:
                mtime = p.stat().st_mtime
                if mtime > newest:
                    newest = mtime
            except OSError:
                pass

    # 1. Scan the project's own directory
    proj_path = PROJECT_ROOT / project_dir
    if proj_path.exists():
        for f in proj_path.rglob("*"):
            if f.suffix in src_extensions:
                check_file(f)
        check_file(proj_path / "Cargo.toml")

    # 2. Scan shared_utils (global dependency)
    shared_path = PROJECT_ROOT / "crates/shared_utils"
    if shared_path.exists() and project_dir != "crates/shared_utils":
        for f in shared_path.rglob("*"):
            if f.suffix in src_extensions:
                check_file(f)
        check_file(shared_path / "Cargo.toml")

    # 3. Scan workspace-level configuration
    check_file(PROJECT_ROOT / "Cargo.toml")
    check_file(PROJECT_ROOT / "Cargo.lock")
    check_file(PROJECT_ROOT / "rust-toolchain.toml")
    check_file(PROJECT_ROOT / "crates/dev/scripts/smart_build.py")  # Self-tracking

    return newest


def get_binary_mtime(binary_path):
    p = Path(binary_path)
    if not p.exists():
        return 0.0
    try:
        return p.stat().st_mtime
    except OSError:
        return 0.0


def get_binary_path(project_dir, binary_name):
    p = PROJECT_ROOT / "target/release" / binary_name
    return str(p)


def verify_binary_timestamp(binary_path, compile_start_time):
    p = Path(binary_path)
    if not p.exists():
        print(f"{RED}ERROR: TIMESTAMP VERIFICATION FAILED: Binary not found{NC}")
        print(f"{DIM}   Expected: {binary_path}{NC}")
        return False

    binary_mtime = get_binary_mtime(binary_path)
    if binary_mtime < compile_start_time:
        print(f"{RED}FAILURE: TIMESTAMP VERIFICATION FAILED{NC}")
        print(f"{YELLOW}Binary timestamp is older than compile time!{NC}")
        return False
    return True


def build_project(project_dir, binary_name, retry_count, args):
    compile_start_time = time.time()

    cmd = [
        "cargo",
        "build",
        "--release",
        "--manifest-path",
        f"{project_dir}/Cargo.toml",
    ]
    res = subprocess.run(cmd)
    if res.returncode != 0:
        print_error(project_dir)
        return False

    if args.verify_timestamps:
        binary_path = get_binary_path(project_dir, binary_name)
        time.sleep(1)  # wait for filesystem sync

        if not verify_binary_timestamp(binary_path, compile_start_time):
            if retry_count < 2:
                print(
                    f"{YELLOW}Retry {retry_count + 1}/2: Rebuilding with clean...{NC}"
                )
                shutil.rmtree("target/release/deps", ignore_errors=True)
                shutil.rmtree("target/release/.fingerprint", ignore_errors=True)
                return build_project(project_dir, binary_name, retry_count + 1, args)
            else:
                print(
                    f"{RED}FAILURE: Timestamp verification failed after 2 retries{NC}"
                )
                return False
    return True


def decide_build_action(project_dir, binary_name, args):
    binary_path = get_binary_path(project_dir, binary_name)
    if args.force:
        return "rebuild", "force"
    if not Path(binary_path).exists():
        return "rebuild", "binary-missing"

    source_mtime = get_newest_source_mtime(project_dir)
    binary_mtime = get_binary_mtime(binary_path)

    if source_mtime > binary_mtime:
        return "rebuild", "source-newer"

    return "skip", ""


def main():
    parser = argparse.ArgumentParser(
        description="Smart Build System v0.11.2 (Python Edition)"
    )
    parser.add_argument(
        "--force", "-f", action="store_true", help="Force rebuild all selected projects"
    )
    parser.add_argument(
        "--clean",
        "-c",
        action="store_true",
        help="Clean build artifacts before compiling",
    )
    parser.add_argument(
        "--verbose", "-v", action="store_true", help="Show detailed output"
    )
    parser.add_argument(
        "--no-clean-old",
        dest="clean_old",
        action="store_false",
        help="Don't clean old binary files",
    )
    parser.add_argument("--all", "-a", action="store_true", help="Build all projects")
    parser.add_argument("--img", action="store_true", help="Build image tools")
    parser.add_argument("--vid", action="store_true", help="Build video tools")
    parser.add_argument("--hevc", action="store_true", help="Support for HEVC codecs")
    parser.add_argument("--av1", action="store_true", help="Support for AV1 codecs")
    parser.add_argument(
        "--kondo", action="store_true", help="Perform deep project cleanup using kondo"
    )
    parser.add_argument(
        "--no-verify-timestamps",
        dest="verify_timestamps",
        action="store_false",
        help="Disable timestamp verification after build",
    )

    args = parser.parse_args()

    print_header()

    projects_to_build = []
    if args.all:
        projects_to_build = list(ALL_PROJECTS.keys())
    else:
        if args.img:
            projects_to_build.append("crates/img")
        if args.vid:
            projects_to_build.append("crates/vid")
        if args.hevc or args.av1:
            if "crates/img" not in projects_to_build:
                projects_to_build.append("crates/img")
            if "crates/vid" not in projects_to_build:
                projects_to_build.append("crates/vid")

    if not projects_to_build:
        projects_to_build = DEFAULT_PROJECTS

    print(f"{CYAN}Building:{NC} {BOLD}{' '.join(projects_to_build)}{NC}\n")

    if args.clean_old:
        clean_old_binaries()

    if args.clean:
        print(f"{YELLOW}Cleaning build artifacts...{NC}")
        for proj_dir in projects_to_build:
            shutil.rmtree(f"{proj_dir}/target/release/deps", ignore_errors=True)
            shutil.rmtree(f"{proj_dir}/target/release/.fingerprint", ignore_errors=True)
        shutil.rmtree("crates/shared_utils/target/release/deps", ignore_errors=True)
        print()
        clean_with_kondo()

    if args.kondo and not args.clean:
        clean_with_kondo()

    rebuilt = 0
    skipped = 0
    failed = 0

    for proj_dir in projects_to_build:
        binary_name = ALL_PROJECTS.get(proj_dir)
        if not binary_name:
            print(f"{RED}ERROR: Unknown project: {proj_dir}{NC}")
            failed += 1
            continue

        action, reason = decide_build_action(proj_dir, binary_name, args)

        if action == "skip":
            print_status(proj_dir, "skip")
            skipped += 1
        else:
            print_status(proj_dir, "rebuild", reason)
            if build_project(proj_dir, binary_name, 0, args):
                print_success(proj_dir)
                rebuilt += 1
            else:
                failed += 1

    print(f"\n{DIM}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━{NC}")

    if failed > 0:
        print(f"{RED}Build failed: {failed} project(s){NC}")
        sys.exit(1)

    if rebuilt == 0:
        print(f"{GREEN}OK: All binaries up-to-date (skipped {skipped}){NC}")
    else:
        print(f"{GREEN}OK: Built {rebuilt}, skipped {skipped}{NC}")

    if args.verbose or rebuilt > 0:
        print(f"\n{DIM}Binary info:{NC}")
        for proj_dir in projects_to_build:
            binary_name = ALL_PROJECTS.get(proj_dir)
            if not binary_name:
                continue

            p = get_binary_path(proj_dir, binary_name)
            if Path(p).exists():
                stat = Path(p).stat()
                sz_mb = stat.st_size / (1024 * 1024)
                mtime_str = time.strftime(
                    "%Y-%m-%d %H:%M", time.localtime(stat.st_mtime)
                )
                print(f"  {BOLD}{binary_name}{NC}: {sz_mb:.1f}M, {mtime_str}")


if __name__ == "__main__":
    main()
