#!/usr/bin/env python3
"""Modern Format Boost - Cache Cleaner v1.1 (Python Edition)
Cleans analysis and quality caches to free up space.
Supports full purge and targeted path cleanup.
"""

import sys
import subprocess
import shutil
import time
import fcntl
import json
import sqlite3
import argparse
from pathlib import Path

# ANSI Colors
if sys.stdout.isatty():
    RED = "\033[0;31m"
    GREEN = "\033[0;32m"
    YELLOW = "\033[1;33m"
    BLUE = "\033[0;34m"
    BOLD = "\033[1m"
    DIM = "\033[2m"
    RESET = "\033[0m"
else:
    RED = GREEN = YELLOW = BLUE = BOLD = DIM = RESET = ""


def clear_screen():
    print("\033[2J\033[H", end="")


def draw_header(targeted=False):
    line = "─" * 60
    print(f"{BLUE}╭{line}╮{RESET}")
    mode_text = (
        f"{BOLD}{RED}🧹 TARGETED CACHE CLEANUP{RESET}"
        if targeted
        else f"{BOLD}{RED}🧹 CACHE & LOG CLEANUP UTILITY v1.1{RESET}"
    )
    print(f"{BLUE}│{RESET}  {mode_text:<62} {BLUE}│{RESET}")
    print(f"{BLUE}╰{line}╯{RESET}")
    if not targeted:
        print(
            f"   {RED}⚠️  WARNING: Critical processing data will be permanently deleted.{RESET}\n"
        )


def get_dir_size(path):
    try:
        result = subprocess.run(
            ["du", "-sh", str(path)], capture_output=True, text=True
        )
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
            print(f"   🗄️  Database:  {DIM}{db_file.name}{RESET} ({db_size})")
    else:
        print(f"   {YELLOW}Empty: No cache directory found.{RESET}")

    log_size = get_dir_size(log_dir) if log_dir.is_dir() else "0B"
    print(f"   📝 Logs:      {DIM}{log_size}{RESET}")

    if mfb_progress_dir.is_dir():
        prog_size = get_dir_size(mfb_progress_dir)
        print(f"   🔄 Progress:  {DIM}{prog_size}{RESET}")

    lock_dir = Path.home() / ".modern_format_boost" / "locks"
    if lock_dir.is_dir():
        lock_count = len(list(lock_dir.glob("*.lock")))
        if lock_count > 0:
            print(
                f"   🔒 Session Locks: {BOLD}{YELLOW}{lock_count} active/stale{RESET}"
            )
    print("")


def clean_mfb_progress(target_path: Path):
    """
    Cleans entry from .mfb_progress.
    If target is directory, removes the entire .txt/.lock.
    If target is file, removes just that line from the parent's .txt.
    """
    progress_dir = Path.home() / ".mfb_progress"
    if not progress_dir.is_dir():
        return

    target_abs = str(target_path.absolute())
    is_dir = target_path.is_dir()

    deleted_count = 0
    modified_count = 0

    for pfile in progress_dir.glob("*.txt"):
        try:
            with open(pfile) as f:
                lines = f.readlines()

            if not lines:
                continue

            header_line = lines[0].strip()
            if not header_line.startswith("{"):
                continue

            header = json.loads(header_line)
            if header.get("kind") != "header":
                continue

            target_dir = header.get("target_dir", "")

            # Case 1: Target directory matches or is a parent of the tracker's target
            if is_dir and (
                target_abs == target_dir or target_dir.startswith(target_abs + "/")
            ):
                pfile.unlink()
                lock_file = pfile.with_suffix(".lock")
                if lock_file.exists():
                    lock_file.unlink()
                deleted_count += 1
                print(
                    f"   {GREEN}✅ Removed progress tracker: {DIM}{target_dir}{RESET}"
                )
                continue

            # Case 2: Target is a file inside this tracker
            if not is_dir and target_abs.startswith(target_dir):
                new_lines = [lines[0]]  # Keep header
                found = False
                for line in lines[1:]:
                    if target_abs in line:  # Simple match for now
                        try:
                            record = json.loads(line.strip())
                            if (
                                record.get("kind") == "entry"
                                and record.get("path") == target_abs
                            ):
                                found = True
                                continue
                        except:
                            pass
                    new_lines.append(line)

                if found:
                    with open(pfile, "w") as f:
                        f.writelines(new_lines)
                    modified_count += 1
                    print(
                        f"   {GREEN}✅ Pruned file from tracker: {DIM}{target_path.name}{RESET}"
                    )

        except Exception as e:
            print(f"   {RED}⚠️ Error processing {pfile.name}: {e}{RESET}")

    return deleted_count, modified_count


def clean_path_tree(target_path: Path):
    """Cleans matching records from ~/.modern_format_boost/cache/path_tree"""
    cache_dir = Path.home() / ".modern_format_boost" / "cache" / "path_tree"
    if not cache_dir.is_dir():
        return 0

    target_abs = str(target_path.absolute())
    deleted_count = 0

    for cfile in cache_dir.glob("*.json"):
        try:
            with open(cfile) as f:
                data = json.load(f)

            root = data.get("root", "")
            if root == target_abs or root.startswith(target_abs + "/"):
                cfile.unlink()
                deleted_count += 1
                print(f"   {GREEN}✅ Removed path-tree cache for: {DIM}{root}{RESET}")
        except:
            pass
    return deleted_count


def clean_sqlite_dbs(target_path: Path):
    """Cleans matching records from known SQLite databases"""
    cache_dir = Path.home() / ".modern_format_boost" / "cache"
    db_files = [
        cache_dir / "image_analysis_v2_main.db",
        Path.home() / ".modern_format_boost" / "gif_value_samples_v2.db",
    ]

    target_abs = str(target_path.absolute())
    total_deleted = 0

    for db_file in db_files:
        if not db_file.is_file():
            continue

        try:
            conn = sqlite3.connect(db_file)
            cursor = conn.cursor()

            # Try to find tables with 'path' or 'file_path' columns
            cursor.execute("SELECT name FROM sqlite_master WHERE type='table'")
            tables = [r[0] for r in cursor.fetchall()]

            for table in tables:
                cursor.execute(f"PRAGMA table_info({table})")
                cols = [c[1] for c in cursor.fetchall()]

                path_col = None
                if "file_path" in cols:
                    path_col = "file_path"
                elif "path" in cols:
                    path_col = "path"
                elif "source_path" in cols:
                    path_col = "source_path"

                if path_col:
                    if target_path.is_dir():
                        cursor.execute(
                            f"DELETE FROM {table} WHERE {path_col} LIKE ?",
                            (target_abs + "%",),
                        )
                    else:
                        cursor.execute(
                            f"DELETE FROM {table} WHERE {path_col} = ?", (target_abs,)
                        )

                    total_deleted += cursor.rowcount

            conn.commit()
            conn.close()
        except Exception as e:
            print(f"   {RED}⚠️ Database error ({db_file.name}): {e}{RESET}")

    if total_deleted > 0:
        print(
            f"   {GREEN}✅ Removed {total_deleted} records from analysis databases{RESET}"
        )
    return total_deleted


def perform_full_cleanup():
    script_dir = Path(__file__).parent.resolve()
    project_root = script_dir.parent

    cache_dir = Path.home() / ".modern_format_boost" / "cache"
    lock_dir = Path.home() / ".modern_format_boost" / "locks"
    db_file = cache_dir / "image_analysis_v2_main.db"
    log_dir = project_root / "logs"
    mfb_progress_dir = Path.home() / ".mfb_progress"
    mfb_tmp_dir = Path.home() / ".modern_format_boost" / "tmp"

    clear_screen()
    draw_header()
    show_stats(cache_dir, db_file, log_dir, mfb_progress_dir)

    print(f"{RED}⚠️  The following will be PERMANENTLY deleted:{RESET}")
    print("   - Image Analysis Database (Verification cache)")
    print("   - All Session Logs & Tool Debug Records")
    print("   - All Task Progress Trackers (Resume Capability)")
    print("   - All Isolated Temporary Files (Ghost Mode artifacts)")
    print("   - All STALE directory locks (Active locks will be skipped)")
    print("")

    confirm = input(
        f"   {BOLD}Type 'yes' to confirm cleanup (yes/N) [Default: N]: {RESET}"
    ).strip()
    if not confirm or confirm.lower() != "yes":
        print(f"\n{YELLOW}🚫 Cleanup cancelled by user.{RESET}")
        print(f"{DIM}   No action taken. Returning to menu...{RESET}")
        time.sleep(1.5)
        return

    print(f"\n{YELLOW}🚀 Executing full system cleanup...{RESET}\n")

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

    # Clear logs (with strict safety check)
    if log_dir.is_dir() and log_dir.name == "logs" and log_dir.parent == project_root:
        print(f"{DIM}   Clearing logs in {log_dir.name}...{RESET}")
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

    # Purge MFB temp directory
    if mfb_tmp_dir.is_dir():
        print(f"{DIM}   Purging isolated temp directory...{RESET}")
        shutil.rmtree(mfb_tmp_dir, ignore_errors=True)
        mfb_tmp_dir.mkdir(parents=True, exist_ok=True)
        print(f"   {GREEN}✅ Isolated temp space cleared{RESET}")

    # Purge stale session locks
    if lock_dir.is_dir():
        print(f"{DIM}   Scanning for stale session locks...{RESET}")
        deleted_locks = 0
        active_locks = 0
        for lock_file in lock_dir.glob("*.lock"):
            try:
                f = open(lock_file, "r+")
                fcntl.flock(f, fcntl.LOCK_EX | fcntl.LOCK_NB)
                f.close()
                lock_file.unlink()
                deleted_locks += 1
            except OSError:
                active_locks += 1

        if deleted_locks > 0:
            print(f"   {GREEN}✅ {deleted_locks} stale locks purged{RESET}")
        if active_locks > 0:
            print(
                f"   {YELLOW}ℹ️  {active_locks} active sessions skipped (protected){RESET}"
            )

    print(f"\n{GREEN}✅ Full Cleanup Complete{RESET}\n")


def perform_targeted_cleanup(target_path: Path):
    if not target_path.exists():
        print(f"\n{RED}❌ Error: Path does not exist: {target_path}{RESET}\n")
        return

    draw_header(targeted=True)
    print(f"   {BOLD}Target:{RESET} {DIM}{target_path.absolute()}{RESET}")
    print(f"   {YELLOW}Scanning metadata associated with this path...{RESET}\n")

    # 1. Progress Tracker
    clean_mfb_progress(target_path)

    # 2. Path Tree Cache
    clean_path_tree(target_path)

    # 3. Databases
    clean_sqlite_dbs(target_path)

    print(f"\n{GREEN}✅ Targeted Cleanup Complete{RESET}\n")


def main():
    parser = argparse.ArgumentParser(description="Modern Format Boost Cache Cleaner")
    parser.add_argument(
        "path", nargs="?", help="Target file or directory for fine-grained cleanup"
    )
    args = parser.parse_args()

    if args.path:
        perform_targeted_cleanup(Path(args.path))
    else:
        perform_full_cleanup()
        print(f"{DIM}Press Enter to exit...{RESET}")
        try:
            input()
        except (EOFError, KeyboardInterrupt):
            pass


if __name__ == "__main__":
    main()
