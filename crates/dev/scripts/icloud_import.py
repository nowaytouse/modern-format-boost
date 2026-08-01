#!/usr/bin/env python3
"""Modern Format Boost - iCloud Import Tool
Uses osxphotos to import processed assets into Apple Photos/iCloud.

Two import modes:
  Mode 1 (Optimized): Imports with ✨ emoji prefix and organized album structure (✨/{folder_name})
  Mode 2 (Simple): Plain import without album organization
"""

import fcntl
import os
import subprocess
import sys
import time
from pathlib import Path

from fastmode_paths import default_mfb_state_root
from mfb_ui_tokens import pick_symbol

# Add ANSI colors for terminal output
if sys.stdout.isatty():
    RED = "\033[0;31m"
    GREEN = "\033[0;32m"
    YELLOW = "\033[1;33m"
    BLUE = "\033[0;34m"
    CYAN = "\033[0;36m"
    BOLD = "\033[1m"
    DIM = "\033[2m"
    RESET = "\033[0m"
else:
    RED = GREEN = YELLOW = BLUE = CYAN = BOLD = DIM = RESET = ""


def get_import_lock_path():
    """Get the path to the import lock file."""
    root = default_mfb_state_root()
    lock_dir = root / "locks"
    lock_dir.mkdir(parents=True, exist_ok=True)
    return lock_dir / "photos_import.lock"


def acquire_import_lock():
    """Acquire an exclusive lock to prevent concurrent imports.

    Returns:
        File object if lock acquired, None if already locked
    """
    lock_path = get_import_lock_path()
    try:
        lock_file = open(lock_path, "w")
        fcntl.flock(lock_file.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
        return lock_file
    except OSError:
        return None


def release_import_lock(lock_file):
    """Release the import lock."""
    if lock_file:
        try:
            fcntl.flock(lock_file.fileno(), fcntl.LOCK_UN)
            lock_file.close()
        except OSError:
            pass


def find_osxphotos():
    """Find osxphotos in common locations and verify it works."""
    # Try common installation paths
    home_local = Path.home() / ".local" / "bin" / "osxphotos"
    common_paths = [
        str(home_local),
        "/opt/homebrew/bin/osxphotos",
        "/usr/local/bin/osxphotos",
    ]

    # First try the system PATH
    try:
        subprocess.run(
            ["osxphotos", "--version"],
            capture_output=True,
            timeout=5,
            check=True,
        )
        return "osxphotos"
    except (
        subprocess.CalledProcessError,
        FileNotFoundError,
        subprocess.TimeoutExpired,
    ):
        pass

    # Try common paths
    for path in common_paths:
        if os.path.exists(path):
            try:
                subprocess.run(
                    [path, "--version"],
                    capture_output=True,
                    timeout=5,
                    check=True,
                )
                return path
            except (subprocess.CalledProcessError, subprocess.TimeoutExpired):
                continue

    return None


def check_osxphotos():
    """Verify osxphotos is installed and accessible."""
    return find_osxphotos() is not None


def select_import_mode():
    """Display mode selection menu and return user choice."""
    while True:
        print(
            f"\n{BLUE}{pick_symbol('📱', ('[PHONE]'))} iCloud Import Mode Selection{RESET}"
        )
        print(f"{DIM}{'─' * 50}{RESET}")
        print(f"  {GREEN}1{RESET} - {BOLD}Optimized Import (Default){RESET}")
        print(
            f"     {DIM}• Auto-rename folder with {pick_symbol('✨', ('[*]'))} emoji{RESET}"
        )
        print(
            f"     {DIM}• Organize into {pick_symbol('✨', ('[*]'))}/{{folder_name}} albums{RESET}"
        )
        print(f"     {DIM}• Best for processed/final media{RESET}")
        print()
        print(f"  {GREEN}2{RESET} - {BOLD}Simple Import{RESET}")
        print(f"     {DIM}• Basic album organization by folder name{RESET}")
        print(
            f"     {DIM}• Simpler than Mode 1, no {pick_symbol('✨', ('[*]'))} renaming{RESET}"
        )
        print(f"     {DIM}• Quick import with organized structure{RESET}")
        print(f"{DIM}{'─' * 50}{RESET}")

        try:
            choice = input(
                f"{CYAN}Select mode (1 or 2) [{GREEN}default: 1{CYAN}]: {RESET}"
            ).strip()
            if choice == "" or choice == "1":
                return 1
            elif choice == "2":
                return 2
            else:
                print(
                    f"{RED}{pick_symbol('❌', ('[ERROR]'))} Invalid choice. Please enter 1 or 2.{RESET}"
                )
                time.sleep(1)
        except KeyboardInterrupt:
            print(f"\n{YELLOW}{pick_symbol('⚠️', '[WARN]')} Import cancelled.{RESET}")
            sys.exit(0)


def strip_folder_suffix(folder_name):
    """Remove _optimized, _collected, and their combinations from folder name.

    Handles all possible combinations:
    - _optimized_collected
    - _collected_optimized
    - _optimized
    - _collected

    Returns the cleaned folder name.
    """
    # Remove all possible suffix combinations
    suffixes_to_remove = [
        "_optimized_collected",
        "_collected_optimized",
        "_optimized",
        "_collected",
    ]

    cleaned = folder_name
    for suffix in suffixes_to_remove:
        cleaned = cleaned.removesuffix(suffix)

    return cleaned


def get_album_name(target_dir):
    path = Path(target_dir).resolve()
    name = path.name
    if not name:
        raise ValueError("target path has no valid directory name")

    # In case the target already has emoji prepended by Mode 1 or earlier runs
    name = name.removeprefix("✨")
    name = name.removeprefix("[*]")
    return strip_folder_suffix(name).strip()


def optimized_album_template(target_dir):
    """Album template for Mode 1: emoji root plus emoji-prefixed child albums."""
    return f"✨/✨{get_album_name(target_dir)}"


def rename_with_emoji(target_dir):
    """Add ✨ prefix to the folder name if not already present.

    Only renames the target folder itself, not parent directories.
    """
    path = Path(target_dir).resolve()

    # Check if folder name already has ✨ prefix
    if path.name.startswith("✨"):
        print(
            f"   {CYAN}ℹ️ Folder already has {pick_symbol('✨', ('[*]'))} prefix: {path.name}{RESET}"
        )
        return str(path)

    new_name = f"{pick_symbol('✨', ('[*]'))}{path.name}"
    new_path = path.parent / new_name

    try:
        os.rename(path, new_path)
        print(
            f"   {GREEN}{pick_symbol('✨', ('[*]'))} Folder renamed: {path.name} -> {new_name}{RESET}"
        )
        return str(new_path)
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
        print(
            f"   {YELLOW}{pick_symbol('⚠️', '[WARN]')} Failed to rename folder with {pick_symbol('✨', ('[*]'))} prefix: {e}{RESET}"
        )
        return str(path)


def run_optimized_import(target_dir):
    """Mode 1: Import with ✨ prefix and organized album structure."""
    # Intentional delay for pacing
    print(f"\n{CYAN}⏳ Preparing for optimized import...{RESET}")
    time.sleep(1.2)

    album_template = optimized_album_template(target_dir)

    # Auto-rename feature
    target_dir = rename_with_emoji(target_dir)
    target_path = Path(target_dir).resolve()

    if not target_path.is_dir():
        print(
            f"{RED}{pick_symbol('❌', ('[ERROR]'))} Error: {target_dir} is not a directory.{RESET}"
        )
        return False

    print(
        f"\n{BLUE}{pick_symbol('🚀', ('[LAUNCH]'))} Starting Optimized Import...{RESET}"
    )
    print(f"   Target:     {CYAN}{target_path}{RESET}")
    print(
        f"   Mode:       {YELLOW}Organized ({pick_symbol('✨', ('[*]'))}/{{folder_name}}){RESET}"
    )
    print(f"   Auto-Album: {YELLOW}Enabled{RESET}")
    print(f"   Album Name: {YELLOW}Auto-strip suffix from folder names{RESET}")

    # Mandatory confirmation
    print(f"\n{YELLOW}{pick_symbol('⚠️', '[WARN]')}  READY TO IMPORT?{RESET}")
    if input(
        f"   {CYAN}Type {GREEN}'yes'{CYAN} to proceed: {RESET}"
    ).strip().lower() not in ("y", "yes"):
        print(
            f"\n{RED}{pick_symbol('❌', ('[ERROR]'))} Import cancelled by user.{RESET}"
        )
        time.sleep(1)
        return False

    print(f"\n{CYAN}{pick_symbol('⚙️', ('[GEAR]'))} Initializing osxphotos...{RESET}")
    time.sleep(1.5)
    print(f"   {DIM}Connecting to Apple Photos library...{RESET}")
    time.sleep(1)

    osxphotos_path = find_osxphotos()

    cmd = [
        osxphotos_path,
        "import",
        str(target_path),
        "--walk",
        "--album",
        album_template,
        "--split-folder",
        "/",
    ]

    try:
        # Run the command and pipe output to terminal
        process = subprocess.Popen(
            cmd, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, bufsize=1
        )

        for line in process.stdout:
            print(f"   {line.strip()}")

        process.wait()

        if process.returncode == 0:
            print(
                f"\n{GREEN}{pick_symbol('✅', ('[OK]'))} Optimized import completed successfully!{RESET}"
            )
            return True
        else:
            print(
                f"\n{RED}{pick_symbol('❌', ('[ERROR]'))} Import failed with exit code {process.returncode}.{RESET}"
            )
            return False

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
        print(
            f"\n{RED}{pick_symbol('❌', ('[ERROR]'))} An unexpected error occurred: {e}{RESET}"
        )
        return False


def run_simple_import(target_dir):
    """Mode 2: Simple import with basic album organization by folder name."""
    # Intentional delay for pacing
    print(f"\n{CYAN}⏳ Preparing for simple import...{RESET}")
    time.sleep(1.2)

    target_path = Path(target_dir).resolve()

    if not target_path.is_dir():
        print(
            f"{RED}{pick_symbol('❌', ('[ERROR]'))} Error: {target_dir} is not a directory.{RESET}"
        )
        return False

    osxphotos_path = find_osxphotos()
    if osxphotos_path is None:
        print(
            f"{RED}{pick_symbol('❌', ('[ERROR]'))} Error: 'osxphotos' not found in system PATH or common locations.{RESET}"
        )
        return False

    print(f"\n{BLUE}{pick_symbol('🚀', ('[LAUNCH]'))} Starting Simple Import...{RESET}")
    print(f"   Target:     {CYAN}{target_path}{RESET}")
    print(f"   Mode:       {YELLOW}Simple (organized by folder name){RESET}")
    print(f"   Album Name: {YELLOW}Auto-strip suffix from folder names{RESET}")

    # Mandatory confirmation
    print(f"\n{YELLOW}{pick_symbol('⚠️', '[WARN]')}  READY TO IMPORT?{RESET}")
    if input(
        f"   {CYAN}Type {GREEN}'yes'{CYAN} to proceed: {RESET}"
    ).strip().lower() not in ("y", "yes"):
        print(
            f"\n{RED}{pick_symbol('❌', ('[ERROR]'))} Import cancelled by user.{RESET}"
        )
        time.sleep(1)
        return False

    print(f"\n{CYAN}{pick_symbol('⚙️', ('[GEAR]'))} Initializing osxphotos...{RESET}")
    time.sleep(1.5)

    # Use chained removesuffix to strip folder name suffixes in album path
    # Order matters: remove longer suffixes first to handle all cases
    # Using get_album_name inline below
    album_template = get_album_name(target_dir)

    cmd = [
        osxphotos_path,
        "import",
        str(target_path),
        "--walk",
        "--album",
        album_template,
    ]

    try:
        # Run the command and pipe output to terminal
        process = subprocess.Popen(
            cmd, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, bufsize=1
        )

        for line in process.stdout:
            print(f"   {line.strip()}")

        process.wait()

        if process.returncode == 0:
            print(
                f"\n{GREEN}{pick_symbol('✅', ('[OK]'))} Simple import completed successfully!{RESET}"
            )
            return True
        else:
            print(
                f"\n{RED}{pick_symbol('❌', ('[ERROR]'))} Import failed with exit code {process.returncode}.{RESET}"
            )
            return False

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
        print(
            f"\n{RED}{pick_symbol('❌', ('[ERROR]'))} An unexpected error occurred: {e}{RESET}"
        )
        return False


def main():
    if len(sys.argv) < 2:
        print(f"{RED}Usage: {sys.argv[0]} <directory>{RESET}")
        sys.exit(1)

    target_dir = sys.argv[1]

    if not check_osxphotos():
        print(
            f"{RED}{pick_symbol('❌', ('[ERROR]'))} Error: 'osxphotos' not found in system PATH or common locations.{RESET}"
        )
        print(
            f"{YELLOW}   Tried: ~/.local/bin, /opt/homebrew/bin, /usr/local/bin{RESET}"
        )
        print(f"{YELLOW}   Please install it first: {CYAN}pip install osxphotos{RESET}")
        print(f"{YELLOW}   Or if already installed, add its directory to PATH.{RESET}")
        sys.exit(1)

    # Acquire import lock to prevent concurrent imports
    import_lock = acquire_import_lock()
    if not import_lock:
        print(
            f"\n{RED}{pick_symbol('❌', ('[ERROR]'))} Error: Another import operation is already in progress.{RESET}"
        )
        print(
            f"{YELLOW}   Please wait for the current import to complete before starting a new one.{RESET}"
        )
        print(
            f"{YELLOW}   If you believe this is an error, delete: {get_import_lock_path()}{RESET}"
        )
        sys.exit(1)

    try:
        # Show mode selection menu
        import_mode = select_import_mode()

        if import_mode == 1:
            success = run_optimized_import(target_dir)
        else:
            success = run_simple_import(target_dir)

        sys.exit(0 if success else 1)
    finally:
        # Always release the lock when exiting
        release_import_lock(import_lock)


if __name__ == "__main__":
    try:
        main()
    except KeyboardInterrupt:
        print(
            f"\n{YELLOW}{pick_symbol('⚠️', '[WARN]')}  Import interrupted by user.{RESET}"
        )
        sys.exit(0)
