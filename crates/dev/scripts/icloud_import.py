#!/usr/bin/env python3
"""Modern Format Boost - iCloud Import Tool
Uses osxphotos to import processed assets into Apple Photos/iCloud.

Two import modes:
  Mode 1 (Optimized): Imports with ✨ emoji prefix and organized album structure
  Mode 2 (Simple): Plain import without album organization
"""

import os
import sys
import subprocess
import time
from pathlib import Path

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


def find_osxphotos():
    """Find osxphotos in common locations and verify it works."""
    # Try common installation paths
    common_paths = [
        "/Users/nyamiiko/.local/bin/osxphotos",
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
    except (subprocess.CalledProcessError, FileNotFoundError, subprocess.TimeoutExpired):
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
        print(f"\n{BLUE}📱 iCloud Import Mode Selection{RESET}")
        print(f"{DIM}{'─' * 50}{RESET}")
        print(f"  {GREEN}1{RESET} - {BOLD}Optimized Import (Default){RESET}")
        print(f"     {DIM}• Auto-rename folder with ✨ emoji{RESET}")
        print(f"     {DIM}• Organize into ✨/✨/{{folder_name}} albums{RESET}")
        print(f"     {DIM}• Best for processed/final media{RESET}")
        print()
        print(f"  {GREEN}2{RESET} - {BOLD}Simple Import{RESET}")
        print(f"     {DIM}• Import directly, no renaming{RESET}")
        print(f"     {DIM}• No album organization{RESET}")
        print(f"     {DIM}• Quick import to main library{RESET}")
        print(f"{DIM}{'─' * 50}{RESET}")

        try:
            choice = input(f"{CYAN}Select mode (1 or 2) [{GREEN}default: 1{CYAN}]: {RESET}").strip()
            if choice == "" or choice == "1":
                return 1
            elif choice == "2":
                return 2
            else:
                print(f"{RED}❌ Invalid choice. Please enter 1 or 2.{RESET}")
                time.sleep(1)
        except KeyboardInterrupt:
            print(f"\n{YELLOW}⚠️ Import cancelled.{RESET}")
            sys.exit(0)


def rename_with_emoji(target_dir):
    """Add ✨ prefix to the folder name if not already present.
    
    Only renames the target folder itself, not parent directories.
    """
    path = Path(target_dir).resolve()
    
    # Check if folder name already has ✨ prefix
    if path.name.startswith("✨"):
        print(f"   {CYAN}ℹ️ Folder already has ✨ prefix: {path.name}{RESET}")
        return str(path)

    new_name = f"✨{path.name}"
    new_path = path.parent / new_name

    try:
        os.rename(path, new_path)
        print(f"   {GREEN}✨ Folder renamed: {path.name} -> {new_name}{RESET}")
        return str(new_path)
    except Exception as e:
        print(f"   {YELLOW}⚠️ Failed to rename folder with ✨ prefix: {e}{RESET}")
        return str(path)


def run_optimized_import(target_dir):
    """Mode 1: Import with ✨ prefix and organized album structure."""
    # Auto-rename feature
    target_dir = rename_with_emoji(target_dir)
    target_path = Path(target_dir).resolve()

    if not target_path.is_dir():
        print(f"{RED}❌ Error: {target_dir} is not a directory.{RESET}")
        return False

    print(f"\n{BLUE}🚀 Starting Optimized Import...{RESET}")
    print(f"   Target:     {CYAN}{target_path}{RESET}")
    print(f"   Mode:       {YELLOW}Organized (✨/✨/{{folder_name}}){RESET}")
    print(f"   Auto-Album: {YELLOW}Enabled{RESET}\n")

    osxphotos_path = find_osxphotos()

    cmd = [
        osxphotos_path,
        "import",
        str(target_path),
        "--walk",
        "--album",
        "✨/{filepath.parent.name}",
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
            print(f"\n{GREEN}✅ Optimized import completed successfully!{RESET}")
            return True
        else:
            print(
                f"\n{RED}❌ Import failed with exit code {process.returncode}.{RESET}"
            )
            return False

    except Exception as e:
        print(f"\n{RED}❌ An unexpected error occurred: {e}{RESET}")
        return False


def run_simple_import(target_dir):
    """Mode 2: Simple import without album organization."""
    target_path = Path(target_dir).resolve()

    if not target_path.is_dir():
        print(f"{RED}❌ Error: {target_dir} is not a directory.{RESET}")
        return False

    print(f"\n{BLUE}🚀 Starting Simple Import...{RESET}")
    print(f"   Target: {CYAN}{target_path}{RESET}")
    print(f"   Mode:   {YELLOW}Simple (no album organization){RESET}\n")

    osxphotos_path = find_osxphotos()

    cmd = [
        osxphotos_path,
        "import",
        str(target_path),
        "--walk",
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
            print(f"\n{GREEN}✅ Simple import completed successfully!{RESET}")
            return True
        else:
            print(
                f"\n{RED}❌ Import failed with exit code {process.returncode}.{RESET}"
            )
            return False

    except Exception as e:
        print(f"\n{RED}❌ An unexpected error occurred: {e}{RESET}")
        return False


def main():
    if len(sys.argv) < 2:
        print(f"{RED}Usage: {sys.argv[0]} <directory>{RESET}")
        sys.exit(1)

    target_dir = sys.argv[1]

    if not check_osxphotos():
        print(f"{RED}❌ Error: 'osxphotos' not found in system PATH or common locations.{RESET}")
        print(f"{YELLOW}   Tried: ~/.local/bin, /opt/homebrew/bin, /usr/local/bin{RESET}")
        print(f"{YELLOW}   Please install it first: {CYAN}pip install osxphotos{RESET}")
        print(f"{YELLOW}   Or if already installed, add its directory to PATH.{RESET}")
        sys.exit(1)

    # Show mode selection menu
    import_mode = select_import_mode()

    if import_mode == 1:
        success = run_optimized_import(target_dir)
    else:
        success = run_simple_import(target_dir)

    sys.exit(0 if success else 1)


if __name__ == "__main__":
    main()
