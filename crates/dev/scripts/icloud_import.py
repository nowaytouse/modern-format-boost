#!/usr/bin/env python3
"""Modern Format Boost - iCloud Import Tool
Uses osxphotos to import processed assets into Apple Photos/iCloud.
"""

import os
import sys
import subprocess
from pathlib import Path

# Add ANSI colors for terminal output
if sys.stdout.isatty():
    RED = "\033[0;31m"
    GREEN = "\033[0;32m"
    YELLOW = "\033[1;33m"
    BLUE = "\033[0;34m"
    CYAN = "\033[0;36m"
    BOLD = "\033[1m"
    RESET = "\033[0m"
else:
    RED = GREEN = YELLOW = BLUE = CYAN = BOLD = RESET = ""


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


def rename_with_emoji(target_dir):
    """Add ✨ prefix to the folder name if not already present."""
    path = Path(target_dir).resolve()
    if path.name.startswith("✨"):
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


def run_import(target_dir):
    """Run the osxphotos import command."""
    # Auto-rename feature
    target_dir = rename_with_emoji(target_dir)
    target_path = Path(target_dir).resolve()

    if not target_path.is_dir():
        print(f"{RED}❌ Error: {target_dir} is not a directory.{RESET}")
        return False

    print(f"\n{BLUE}🚀 Starting iCloud Import...{RESET}")
    print(f"   Target: {CYAN}{target_path}{RESET}")
    print(f"   Mode:   {YELLOW}Walk subdirectories, dynamic album naming{RESET}\n")

    # The command requested by the user:
    # osxphotos import /dir --walk --album "✨/✨/{filepath.parent.name}"
    # --split-folder /

    osxphotos_path = find_osxphotos()

    cmd = [
        osxphotos_path,
        "import",
        str(target_path),
        "--walk",
        "--album",
        "✨/✨/{filepath.parent.name}",
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
            print(f"\n{GREEN}✅ Import completed successfully!{RESET}")
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

    success = run_import(target_dir)
    sys.exit(0 if success else 1)


if __name__ == "__main__":
    main()
