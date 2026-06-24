#!/usr/bin/env python3
import argparse
import os
import shutil
import subprocess
import sys
import time
from pathlib import Path

from mfb_logger import setup_logger
from mfb_rust_toolchain import resolve_rust_toolchain

logger = setup_logger("mfb.smart_build")

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

BREW_MEDIA_FORMULAE = (
    "ffmpeg",
    "jpeg-xl",
    "exiftool",
    "imagemagick",
    "webp",
    "libheif",
    "libvmaf",
    "chromaprint",
    "pgvector",
)


ALL_PROJECTS = {"crates/img": "img", "crates/vid": "vid", "crates/dev": "verify"}

DEFAULT_PROJECTS = ["crates/img", "crates/vid", "crates/dev"]

SCRIPT_DIR = Path(__file__).parent.resolve()
PROJECT_ROOT = SCRIPT_DIR.parent.parent.parent
os.chdir(PROJECT_ROOT)


def print_header():
    print()
    print(f"{CYAN}{BOLD}Smart Build System v0.11.3 (Python Edition){NC}")
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


def _run_update_step(label: str, cmd: list[str], required: bool = False) -> bool:
    print(f"{DIM}   · {label}…{NC}", flush=True)
    try:
        result = subprocess.run(
            cmd,
            cwd=PROJECT_ROOT,
            capture_output=False,
            text=True,
            check=False,
        )
    except OSError as exc:
        if required:
            print(f"{RED}   ! {label} failed: {exc}{NC}", file=sys.stderr)
        return False

    if result.returncode != 0:
        print(f"{YELLOW}   ! {label} exited {result.returncode}{NC}", file=sys.stderr)
        return False
    return True


def bootstrap_macos_path() -> None:
    if sys.platform != "darwin":
        return
    extra = (
        "/opt/homebrew/bin",
        "/opt/homebrew/sbin",
        "/usr/local/bin",
        "/usr/local/sbin",
    )
    parts = os.environ.get("PATH", "").split(os.pathsep)
    for p in extra:
        if os.path.isdir(p) and p not in parts:
            parts.insert(0, p)
    os.environ["PATH"] = os.pathsep.join(parts)


def perform_updates():
    print(
        f"\n{CYAN}{BOLD}Running Dependency Updates (cargo update, brew, pip, etc.)…{NC}\n"
    )
    bootstrap_macos_path()

    if shutil.which("brew"):
        _run_update_step("brew update", ["brew", "update"])
        for formula in BREW_MEDIA_FORMULAE:
            _run_update_step(f"brew upgrade {formula}", ["brew", "upgrade", formula])

    requirements = PROJECT_ROOT / "crates/dev/scripts/requirements.txt"
    if requirements.is_file() and shutil.which(sys.executable):
        python_bin = sys.executable
        venv_python = PROJECT_ROOT / "crates/.modern_format_boost/.venv/bin/python"
        if venv_python.is_file():
            python_bin = str(venv_python)

        _run_update_step(
            "pip requirements",
            [python_bin, "-m", "pip", "install", "-U", "-r", str(requirements)],
        )

    rust_toolchain_file = PROJECT_ROOT / "rust-toolchain.toml"
    channel = None
    components = ["rustfmt", "clippy", "llvm-tools"]
    if rust_toolchain_file.is_file():
        import re

        text = rust_toolchain_file.read_text(encoding="utf-8")
        match = re.search(r'^\s*channel\s*=\s*"([^"]+)"', text, re.MULTILINE)
        if match:
            channel = match.group(1).strip()
        comp_match = re.search(r"components\s*=\s*\[([^\]]+)\]", text, re.DOTALL)
        if comp_match:
            components = re.findall(r'"([^"]+)"', comp_match.group(1))

    if channel and shutil.which("rustup"):
        ok = _run_update_step(
            f"rustup toolchain {channel}",
            ["rustup", "toolchain", "install", channel],
            required=True,
        )
        if ok:
            for component in components:
                _run_update_step(
                    f"rustup component {component}",
                    ["rustup", "component", "add", component, "--toolchain", channel],
                )

    if shutil.which("cargo"):
        cargo_cmd = ["rtk", "cargo"] if shutil.which("rtk") else ["cargo"]
        _run_update_step("cargo update", cargo_cmd + ["update"])
        _run_update_step(
            "cargo install kondo", cargo_cmd + ["install", "kondo", "--locked", "-q"]
        )

    print(f"\n{GREEN}{BOLD}Dependency updates finished.{NC}\n")


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

    # 2. Scan foundation (global dependency)
    shared_path = PROJECT_ROOT / "crates/foundation"
    if shared_path.exists() and project_dir != "crates/foundation":
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
    # Add a 2-second tolerance to account for filesystem sync delays and
    # Cargo reusing cached binaries with older timestamps.
    if binary_mtime < (compile_start_time - 2.0):
        print(f"{RED}FAILURE: TIMESTAMP VERIFICATION FAILED{NC}")
        print(f"{YELLOW}Binary timestamp is older than compile time!{NC}")
        return False
    return True


def build_project(project_dir, binary_name, retry_count, args):
    compile_start_time = time.time()

    toolchain = resolve_rust_toolchain()

    cmd = []
    if shutil.which("rtk"):
        cmd.append("rtk")
    cmd.extend(
        [
            str(toolchain.cargo),
            "build",
            "--release",
            "--manifest-path",
            f"{project_dir}/Cargo.toml",
        ]
    )

    env = os.environ.copy()
    env.update(toolchain.env())
    toolchain_name = toolchain.name

    logger.info("Executing cargo build for %s", project_dir)
    logger.debug("Cargo command: %s", " ".join(cmd))
    if toolchain_name:
        logger.debug("Using rustup toolchain: %s", toolchain_name)

    res = subprocess.run(cmd, env=env)
    if res.returncode != 0:
        logger.error(
            "Cargo compilation failed for %s with return code %s",
            project_dir,
            res.returncode,
        )
        print_error(project_dir)
        return False

    if True:  # Always verify timestamps (flag removed)
        binary_path = get_binary_path(project_dir, binary_name)
        time.sleep(1)  # wait for filesystem sync

        if not verify_binary_timestamp(binary_path, compile_start_time):
            if retry_count < 2:
                logger.warning(
                    "Timestamp verification failed for %s. Retrying (%s/2).",
                    binary_path,
                    retry_count + 1,
                )
                print(
                    f"{YELLOW}Retry {retry_count + 1}/2: Rebuilding with clean...{NC}"
                )
                logger.warning(
                    "[AUDIT] DESTRUCTIVE ACTION: Deleting target/release/deps"
                )
                shutil.rmtree("target/release/deps", ignore_errors=True)
                logger.warning(
                    "[AUDIT] DESTRUCTIVE ACTION: Deleting target/release/.fingerprint"
                )
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


def resolve_projects_to_build(args) -> list[str]:
    projects_to_build: list[str] = []
    if args.all:
        projects_to_build = ["crates/img", "crates/vid", "crates/dev"]
    else:
        if args.img:
            projects_to_build.append("crates/img")
        if args.vid:
            projects_to_build.append("crates/vid")

    if not projects_to_build:
        projects_to_build = list(DEFAULT_PROJECTS)
    return projects_to_build


def build_plan_needs_work(projects_to_build, args) -> bool:
    if args.force:
        return True
    for proj_dir in projects_to_build:
        binary_name = ALL_PROJECTS.get(proj_dir)
        if not binary_name:
            return True
        action, _reason = decide_build_action(proj_dir, binary_name, args)
        if action != "skip":
            return True
    return False


def sync_app_bundle():
    app_res_dir = PROJECT_ROOT / "Modern Format Boost.app" / "Contents" / "Resources"
    if not app_res_dir.exists():
        return
    print(f"\n{DIM}Syncing binaries to App Bundle...{NC}")
    binaries = [
        "img",
        "vid",
        "verify",
        "cache_cleaner",
        "database_manager",
        "collect_optimized",
        "merge_xmp",
        "icloud_import",
        "drag_and_drop_processor",
    ]
    target_release = PROJECT_ROOT / "target" / "release"
    for bin_name in binaries:
        src = target_release / bin_name
        if src.exists():
            try:
                shutil.copy2(src, app_res_dir / bin_name)
            except Exception as e:
                print(f"  {YELLOW}Warning: could not copy {bin_name}: {e}{NC}")
    print(f"{GREEN}App Bundle updated.{NC}")


def build_and_sync_gui():
    print(f"\n{BOLD}{CYAN} Building Tauri GUI...{NC}")
    vue_dir = os.path.join(PROJECT_ROOT, "crates", "dev", "src", "vue")

    result = subprocess.run(["npm", "run", "tauri", "build"], cwd=vue_dir)
    if result.returncode != 0:
        print(f"{RED}Tauri build failed.{NC}")
        sys.exit(1)

    print(f"{DIM}Syncing App bundle...{NC}")
    # Tauri build output is redirected to the workspace root target via
    # src-tauri/.cargo/config.toml (target-dir = "../../../../../target").
    src_bundle = os.path.join(
        PROJECT_ROOT,
        "target",
        "release",
        "bundle",
        "macos",
        "Modern Format Boost.app",
    )
    dest_bundle = os.path.join(PROJECT_ROOT, "Modern Format Boost.app")

    if os.path.exists(src_bundle):
        if os.path.exists(dest_bundle):
            shutil.rmtree(dest_bundle)
        shutil.copytree(src_bundle, dest_bundle)
        print(f"{GREEN}App bundle replaced successfully.{NC}")
    else:
        print(f"{RED}Built app bundle not found at {src_bundle}{NC}")
        sys.exit(1)

    sync_app_bundle()


def main():
    import sys
    from pathlib import Path

    _scripts = Path(__file__).resolve().parent
    if str(_scripts) not in sys.path:
        sys.path.insert(0, str(_scripts))
    from mfb_entry_guard import guard_main  # noqa: E402

    guard_main("smart_build.py")
    parser = argparse.ArgumentParser(
        description="Smart Build System — incremental Rust + Tauri builder",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog="""
Default (no flags): build img + vid + verify if sources are newer than binaries.

Examples:
  smart_build.py                  # incremental build img + vid + verify
  smart_build.py --all            # build everything including Tauri GUI
  smart_build.py --gui            # build Tauri GUI only
  smart_build.py --img --force    # force-rebuild img binary
  smart_build.py --clean --all    # clean + full rebuild
  smart_build.py --update         # update deps then build""",
    )
    parser.add_argument(
        "--force", "-f", action="store_true", help="Force rebuild even when up-to-date"
    )
    parser.add_argument(
        "--clean",
        "-c",
        action="store_true",
        help="Clean stale deps and run kondo before building",
    )
    parser.add_argument(
        "--verbose", "-v", action="store_true", help="Show binary size and mtime after build"
    )
    parser.add_argument(
        "--all", "-a", action="store_true", help="Build everything: img + vid + verify + GUI"
    )
    parser.add_argument("--img", action="store_true", help="Build image tools (img binary)")
    parser.add_argument("--vid", action="store_true", help="Build video tools (vid binary)")
    parser.add_argument(
        "--quiet",
        "-q",
        action="store_true",
        help="No output when all binaries are already up-to-date",
    )
    parser.add_argument(
        "--update",
        "-u",
        action="store_true",
        help="Update dependencies first (brew, cargo, pip, rustup)",
    )
    parser.add_argument(
        "--gui",
        action="store_true",
        help="Build the Tauri Vue GUI and sync the .app bundle",
    )

    args = parser.parse_args()

    projects_to_build = resolve_projects_to_build(args)

    if args.update:
        perform_updates()

    if args.quiet and not build_plan_needs_work(projects_to_build, args):
        return

    print_header()

    print(f"{CYAN}Building:{NC} {BOLD}{' '.join(projects_to_build)}{NC}\n")

    # Always remove stale binaries left over from previous build locations.
    clean_old_binaries()

    if args.clean:
        print(f"{YELLOW}Cleaning build artifacts...{NC}")
        for proj_dir in projects_to_build:
            shutil.rmtree(f"{proj_dir}/target/release/deps", ignore_errors=True)
            shutil.rmtree(f"{proj_dir}/target/release/.fingerprint", ignore_errors=True)
        shutil.rmtree("crates/foundation/target/release/deps", ignore_errors=True)
        print()
        clean_with_kondo()

    # GUI build: triggered by --gui flag or --all
    if args.gui or args.all:
        build_and_sync_gui()

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

    sync_app_bundle()


if __name__ == "__main__":
    main()
