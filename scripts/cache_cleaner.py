#!/usr/bin/env python3
"""Modern Format Boost - Cache Cleaner v1.0 (Python Edition)
Cleans analysis and quality caches to free up space.
"""

import os
import sys
import subprocess
import shutil
from pathlib import Path

# ANSI Colors
if sys.stdout.isatty():
    RED = '\033[0;31m'
    GREEN = '\033[0;32m'
    YELLOW = '\033[1;33m'
    BLUE = '\033[0;34m'
    BOLD = '\033[1m'
    DIM = '\033[2m'
    RESET = '\033[0m'
else:
    RED = GREEN = YELLOW = BLUE = BOLD = DIM = RESET = ''

def clear_screen():
    print('\033[2J\033[H', end="")

def draw_header():
    line = '─' * 60
    print(f"{BLUE}╭{line}╮{RESET}")
    print(f"{BLUE}│{RESET}  {BOLD}{RED}🧹 CACHE & LOG CLEANUP UTILITY v1.0{RESET}                      {BLUE}│{RESET}")
    print(f"{BLUE}╰{line}╯{RESET}")
    print(f"   {RED}⚠️  WARNING: Critical processing data will be permanently deleted.{RESET}\n")

def get_dir_size(path):
    try:
        result = subprocess.run(["du", "-sh", str(path)], capture_output=True, text=True)
        if result.returncode == 0:
            return result.stdout.split()[0]
    except Exception:
        pass
    return "0B"

def show_stats(cache_dir, db_file, log_dir, mfb_progress_dir):
    print(f"{BOLD}Current Cache Status:{RESET}")
    
    if cache_dir.is_dir():
        size = get_dir_size(cache_dir)
        print(f"   📂 Directory: {DIM}{cache_dir}{RESET}")
        print(f"   📦 Total Size: {BOLD}{GREEN}{size}{RESET}")

        if db_file.is_file():
            db_size = get_dir_size(db_file)
            print(f"   🗄️  Database:  {DIM}image_analysis_v2.db{RESET} ({db_size})")
    else:
        print(f"   {YELLOW}Empty: No cache directory found.{RESET}")

    log_size = get_dir_size(log_dir) if log_dir.is_dir() else "0B"
    print(f"   📝 Logs:      {DIM}{log_size}{RESET}")

    if mfb_progress_dir.is_dir():
        prog_size = get_dir_size(mfb_progress_dir)
        print(f"   🔄 Progress:  {DIM}{prog_size}{RESET}")
    print("")

def main():
    script_dir = Path(__file__).parent.resolve()
    project_root = script_dir.parent

    cache_dir = project_root / ".cache"
    db_file = cache_dir / "image_analysis_v2.db"
    log_dir = project_root / "logs"
    mfb_progress_dir = Path.home() / ".mfb_progress"

    clear_screen()
    draw_header()
    show_stats(cache_dir, db_file, log_dir, mfb_progress_dir)

    print(f"{RED}⚠️  The following will be PERMANENTLY deleted:{RESET}")
    print(f"   - Image Analysis Database (Verification cache)")
    print(f"   - All Session Logs & Tool Debug Records")
    print(f"   - All Task Progress Trackers (Resume Capability)")
    print("")

    confirm = input(f"   {BOLD}Type 'yes' to confirm cleanup (yes/N): {RESET}")
    if confirm.lower() != 'yes':
        print(f"\n{GREEN}✅ Cleanup cancelled by user.{RESET}")
        return

    print(f"\n{YELLOW}🚀 Executing cleanup...{RESET}\n")

    # Vacuum database if sqlite3 is available
    if db_file.is_file() and shutil.which("sqlite3"):
        print(f"{DIM}   Vacuuming database...{RESET}")
        subprocess.run(["sqlite3", str(db_file), "VACUUM;"], stderr=subprocess.DEVNULL)
        print(f"   {GREEN}✅ Database vacuumed{RESET}")

    # Purge cache directory
    if cache_dir.is_dir():
        print(f"{DIM}   Removing cache directory...{RESET}")
        shutil.rmtree(cache_dir, ignore_errors=True)
        print(f"   {GREEN}✅ Cache purged{RESET}")

    # Clear logs (with safety check)
    if log_dir.is_dir() and str(log_dir) != "/":
        print(f"{DIM}   Clearing logs...{RESET}")
        for log_file in log_dir.glob("*.log"):
            try:
                log_file.unlink()
            except Exception:
                pass
        print(f"   {GREEN}✅ Logs cleared{RESET}")

    # Purge MFB progress directory
    if mfb_progress_dir.is_dir():
        print(f"{DIM}   Removing MFB progress directory...{RESET}")
        shutil.rmtree(mfb_progress_dir, ignore_errors=True)
        print(f"   {GREEN}✅ MFB progress purged{RESET}")

    print(f"\n{GREEN}✅ Cleanup Complete{RESET}\n")
    print(f"{DIM}Press Enter to return to menu...{RESET}")
    
    try:
        input()
    except (EOFError, KeyboardInterrupt):
        pass

if __name__ == "__main__":
    main()
