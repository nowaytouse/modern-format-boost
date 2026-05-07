#!/usr/bin/env python3
"""Modern Format Boost - Cache Cleaner v1.2 (Python Edition)
Cleans analysis and quality caches to free up space.
Supports full purge and targeted path cleanup.

Cache backends cleared:
  1. PostgreSQL (primary): analysis_records, quality_records, video_records, path_index
  2. SQLite (fallback): ~/.modern_format_boost/cache/image_analysis_v2.db
  3. Path-tree JSON: ~/.modern_format_boost/cache/path_tree/
  4. Progress trackers: ~/.mfb_progress/
  5. Temp/lock files: ~/.modern_format_boost/tmp|locks/
  6. Rust build artifacts: project_root/target/
  7. Runtime cache: project_root/.cache/
  8. Fuzzing targets: project_root/fuzz/target/
  9. Distribution artifacts: project_root/dist/
  10. Python bytecode: project_root/**/__pycache__/
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

SCRIPT_DIR = Path(__file__).parent.resolve()
PROJECT_ROOT = SCRIPT_DIR.parent.parent.parent

# ANSI Colors
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


def clear_screen():
    print("\033[2J\033[H", end="")


def can_prompt_user() -> bool:
    return sys.stdin.isatty() and sys.stdout.isatty()


def resolve_python_executable() -> str | None:
    if sys.executable:
        return sys.executable
    return shutil.which("python3") or shutil.which("python")


def run_post_cleanup_rebuild(project_root: Path) -> bool:
    smart_build = SCRIPT_DIR / "smart_build.py"
    if not smart_build.is_file():
        print(f"{RED}❌ Error: rebuild script not found: {smart_build}{RESET}")
        return False

    python_exe = resolve_python_executable()
    if not python_exe:
        print(f"{RED}❌ Error: no Python interpreter found for rebuild step.{RESET}")
        return False

    if not shutil.which("cargo"):
        print(f"{RED}❌ Error: cargo not found in PATH; cannot rebuild project.{RESET}")
        return False

    print(f"{BOLD}\n📦 Initializing Optimized Rebuild...{RESET}")
    print(
        f"{DIM}Build artifacts were purged. Rebuilding workspace tools with forced refresh...{RESET}\n"
    )

    try:
        subprocess.run(
            [python_exe, str(smart_build), "--force"],
            cwd=project_root,
            check=True,
        )
        print(f"\n{GREEN}✅ Project Rebuilt Successfully{RESET}")
        return True
    except subprocess.CalledProcessError as exc:
        print(
            f"\n{RED}❌ Error: Rebuild failed with exit code {exc.returncode}.{RESET}"
        )
    except FileNotFoundError as exc:
        print(f"\n{RED}❌ Error: Rebuild failed to start: {exc}{RESET}")

    print(f"{YELLOW}Please run '{python_exe} {smart_build} --force' manually.{RESET}")
    return False


# ---------------------------------------------------------------------------
# PostgreSQL helpers (primary cache backend)
# ---------------------------------------------------------------------------

PG_DBNAME = "modern_format_boost"
PG_TABLES = ["analysis_records", "quality_records", "video_records", "path_index"]


def _pg_available() -> bool:
    """Return True if psycopg2 is importable and the DB is reachable."""
    try:
        import psycopg2  # noqa: F401

        conn = psycopg2.connect(dbname=PG_DBNAME, connect_timeout=3)
        conn.close()
        return True
    except Exception:
        return False


def purge_postgres_full():
    """TRUNCATE all MFB cache tables in PostgreSQL."""
    try:
        import psycopg2

        conn = psycopg2.connect(dbname=PG_DBNAME)
        conn.autocommit = True
        cur = conn.cursor()
        for table in PG_TABLES:
            cur.execute(f"TRUNCATE TABLE {table} RESTART IDENTITY CASCADE")
        cur.close()
        conn.close()
        print(f"   {GREEN}✅ PostgreSQL: all cache tables truncated{RESET}")
        return True
    except Exception as e:
        print(f"   {RED}⚠️ PostgreSQL purge failed: {e}{RESET}")
        return False


def purge_postgres_for_path(target_path: Path):
    """Delete rows matching target_path (or prefix) from PostgreSQL cache tables."""
    try:
        import psycopg2

        conn = psycopg2.connect(dbname=PG_DBNAME)
        conn.autocommit = False
        cur = conn.cursor()
        target_abs = str(target_path.absolute())
        total = 0

        if target_path.is_dir():
            pattern = target_abs.rstrip("/") + "/%"
            # path_index is the entry point — delete cascades to records via FK,
            # but tables may not have FK constraints so delete all explicitly.
            cur.execute(
                "DELETE FROM path_index WHERE file_path = %s OR file_path LIKE %s",
                (target_abs, pattern),
            )
            total += cur.rowcount
            # content_hash orphan cleanup
            for table in ("analysis_records", "quality_records", "video_records"):
                cur.execute(
                    f"""DELETE FROM {table}
                        WHERE content_hash NOT IN (SELECT content_hash FROM path_index)"""
                )
                total += cur.rowcount
        else:
            cur.execute("DELETE FROM path_index WHERE file_path = %s", (target_abs,))
            total += cur.rowcount
            for table in ("analysis_records", "quality_records", "video_records"):
                cur.execute(
                    f"""DELETE FROM {table}
                        WHERE content_hash NOT IN (SELECT content_hash FROM path_index)"""
                )
                total += cur.rowcount

        conn.commit()
        cur.close()
        conn.close()
        if total > 0:
            print(
                f"   {GREEN}✅ PostgreSQL: removed {total} rows for {target_path.name}{RESET}"
            )
        return total
    except Exception as e:
        print(f"   {RED}⚠️ PostgreSQL targeted purge failed: {e}{RESET}")
        return 0


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

    target_dir = PROJECT_ROOT / "target"
    if target_dir.is_dir():
        target_size = get_dir_size(target_dir)
        print(f"   🦀 Rust Build: {BOLD}{YELLOW}{target_size}{RESET}")

    local_cache = PROJECT_ROOT / ".cache"
    if local_cache.is_dir():
        local_cache_size = get_dir_size(local_cache)
        print(f"   ⚡ Runtime:    {BOLD}{YELLOW}{local_cache_size}{RESET}")

    fuzz_target = PROJECT_ROOT / "crates" / "dev" / "fuzz" / "target"
    if fuzz_target.is_dir():
        fuzz_size = get_dir_size(fuzz_target)
        print(f"   🧪 Fuzz Build: {BOLD}{YELLOW}{fuzz_size}{RESET}")

    dist_dir = PROJECT_ROOT / "dist"
    if dist_dir.is_dir():
        dist_size = get_dir_size(dist_dir)
        print(f"   📦 Dist:       {BOLD}{BLUE}{dist_size}{RESET}")

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
            if not is_dir and (
                target_abs == target_dir or target_abs.startswith(target_dir + "/")
            ):
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
                        except Exception:
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
            if root == target_abs or root.startswith(target_abs.rstrip("/") + "/"):
                cfile.unlink()
                deleted_count += 1
                print(f"   {GREEN}✅ Removed path-tree cache for: {DIM}{root}{RESET}")
        except Exception:
            pass
    return deleted_count


def clean_sqlite_dbs(target_path: Path):
    """Cleans matching records from known SQLite databases"""
    cache_dir = Path.home() / ".modern_format_boost" / "cache"
    db_files = [
        cache_dir / "image_analysis_v2.db",  # SQLite fallback store
        cache_dir / "image_analysis_v2_main.db",  # legacy name, kept for safety
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
    cache_dir = Path.home() / ".modern_format_boost" / "cache"
    lock_dir = Path.home() / ".modern_format_boost" / "locks"
    db_file = cache_dir / "image_analysis_v2.db"
    log_dir = PROJECT_ROOT / "logs"
    mfb_progress_dir = Path.home() / ".mfb_progress"
    mfb_tmp_dir = Path.home() / ".modern_format_boost" / "tmp"
    rust_build_artifacts_present = (PROJECT_ROOT / "target").is_dir()

    pg_available = _pg_available()

    clear_screen()
    draw_header()
    show_stats(cache_dir, db_file, log_dir, mfb_progress_dir)

    print(f"{RED}⚠️  The following will be PERMANENTLY deleted:{RESET}")
    if pg_available:
        print(
            "   - PostgreSQL cache (analysis_records, quality_records, video_records, path_index)"
        )
    else:
        print(f"   {YELLOW}- PostgreSQL: NOT REACHABLE — will be skipped{RESET}")
    print("   - SQLite fallback database (image_analysis_v2.db)")
    print("   - Path-tree JSON cache")
    print("   - Project-local Runtime Cache (.cache/mfb_runtime)")
    print("   - All Session Logs & Tool Debug Records")
    print("   - Fuzzing Build Artifacts (fuzz/target)")
    print("   - Distribution Artifacts (dist/)")
    print("   - All Python bytecode (__pycache__ - recursive)")
    print("   - All Task Progress Trackers (Resume Capability)")
    print("   - All Isolated Temporary Files (Ghost Mode artifacts)")
    print("   - All Rust Build Artifacts (cargo clean - will free GBs of space)")
    print("   - All STALE directory locks (Active locks will be skipped)")
    print("")

    # Mandatory confirmation using 'y'
    print(f"\n{YELLOW}⚠️  CONFIRM: Start full system cleanup?{RESET}")
    if input(
        f"   {CYAN}Type {GREEN}'yes'{CYAN} to proceed: {RESET}"
    ).strip().lower() not in ("y", "yes"):
        print(f"\n{RED}🚫 Cleanup cancelled by user.{RESET}")
        print(f"{DIM}   No action taken. Returning to menu...{RESET}")
        time.sleep(1.5)
        return False, False

    print(f"\n{YELLOW}🚀 Executing full system cleanup...{RESET}")
    time.sleep(1.2)
    print(f"   {DIM}Initializing cleanup engine...{RESET}\n")
    time.sleep(0.8)

    # 1. PostgreSQL — primary cache backend (must be cleared first; SQLite is just a fallback)
    if pg_available:
        print(f"{DIM}   Purging PostgreSQL cache tables...{RESET}")
        purge_postgres_full()
    else:
        print(
            f"   {YELLOW}⚠️  PostgreSQL unavailable — skipping (cache hits may still occur if DB comes back online){RESET}"
        )

    # 2. SQLite fallback database
    if db_file.is_file() and shutil.which("sqlite3"):
        print(f"{DIM}   Vacuuming SQLite fallback database...{RESET}")
        subprocess.run(["sqlite3", str(db_file), "VACUUM;"], stderr=subprocess.DEVNULL)

    # Purge entire cache directory (covers SQLite db, path-tree JSON, etc.)
    if cache_dir.is_dir():
        print(f"{DIM}   Removing cache directory...{RESET}")
        shutil.rmtree(cache_dir, ignore_errors=True)
        print(f"   {GREEN}✅ SQLite cache & path-tree JSON purged{RESET}")

    # 3. Clear logs (with strict safety check)
    if log_dir.is_dir() and log_dir.name == "logs" and log_dir.parent == PROJECT_ROOT:
        print(f"{DIM}   Clearing logs in {log_dir.name}...{RESET}")
        for log_file in log_dir.glob("*.log"):
            try:
                log_file.unlink()
            except Exception:
                pass
        print(f"   {GREEN}✅ Logs cleared{RESET}")

    # 4. Purge MFB progress directory
    if mfb_progress_dir.is_dir():
        print(f"{DIM}   Removing MFB progress directory...{RESET}")
        shutil.rmtree(mfb_progress_dir, ignore_errors=True)
        print(f"   {GREEN}✅ MFB progress purged{RESET}")

    # 5. Purge MFB temp directory
    if mfb_tmp_dir.is_dir():
        print(f"{DIM}   Purging isolated temp directory...{RESET}")
        shutil.rmtree(mfb_tmp_dir, ignore_errors=True)
        mfb_tmp_dir.mkdir(parents=True, exist_ok=True)
        print(f"   {GREEN}✅ Isolated temp space cleared{RESET}")

    # 6. Cargo clean — mandatory for build artifact cleanup
    target_dir = PROJECT_ROOT / "target"
    if target_dir.is_dir():
        print(f"{DIM}   Running cargo clean in {PROJECT_ROOT.name}...{RESET}")
        try:
            subprocess.run(
                ["cargo", "clean"],
                cwd=PROJECT_ROOT,
                check=True,
                capture_output=True,
            )
            print(f"   {GREEN}✅ Rust build artifacts purged{RESET}")
        except Exception as e:
            print(f"   {RED}⚠️ Cargo clean failed: {e}{RESET}")

    # 7. Project-local .cache (mfb_runtime)
    local_cache = PROJECT_ROOT / ".cache"
    if local_cache.is_dir():
        print(f"{DIM}   Purging project-local runtime cache...{RESET}")
        shutil.rmtree(local_cache, ignore_errors=True)
        print(f"   {GREEN}✅ Runtime cache cleared{RESET}")

    # 8. Fuzzing targets: project_root/crates/dev/fuzz/target/
    fuzz_dir = PROJECT_ROOT / "crates" / "dev" / "fuzz"
    if (fuzz_dir / "target").is_dir():
        print(f"{DIM}   Running cargo clean in crates/dev/fuzz...{RESET}")
        try:
            subprocess.run(
                ["cargo", "clean"], cwd=fuzz_dir, check=True, capture_output=True
            )
            print(f"   {GREEN}✅ Fuzzing artifacts purged{RESET}")
        except Exception:
            # Fallback if cargo clean fails
            shutil.rmtree(fuzz_dir / "target", ignore_errors=True)
            print(f"   {GREEN}✅ Fuzzing artifacts purged (manual){RESET}")

    # 9. Dist folder
    dist_dir = PROJECT_ROOT / "dist"
    if dist_dir.is_dir():
        print(f"{DIM}   Removing dist directory...{RESET}")
        shutil.rmtree(dist_dir, ignore_errors=True)
        print(f"   {GREEN}✅ Distribution artifacts purged{RESET}")

    # 10. Recursive __pycache__ removal (Safe cleanup)
    print(f"{DIM}   Searching for __pycache__ directories...{RESET}")
    pycache_count = 0
    # Search project-wide, but be careful not to enter hidden dirs like .git or .venv
    for p in PROJECT_ROOT.rglob("__pycache__"):
        # Skip virtualenvs and git to avoid touching installed deps or history
        if ".venv" in p.parts or ".git" in (p.parts):
            continue
        try:
            shutil.rmtree(p, ignore_errors=True)
            pycache_count += 1
        except Exception:
            pass
    if pycache_count > 0:
        print(f"   {GREEN}✅ Removed {pycache_count} __pycache__ directories{RESET}")

    # 6. Purge stale session locks
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
    return True, rust_build_artifacts_present


def perform_targeted_cleanup(target_path: Path):
    if not target_path.exists():
        print(f"\n{RED}❌ Error: Path does not exist: {target_path}{RESET}\n")
        return

    draw_header(targeted=True)
    print(f"   {BOLD}Target:{RESET} {DIM}{target_path.absolute()}{RESET}")
    print(f"   {YELLOW}Scanning metadata associated with this path...{RESET}\n")

    # 0. PostgreSQL — primary cache backend (must be cleared first)
    if _pg_available():
        purge_postgres_for_path(target_path)
    else:
        print(
            f"   {YELLOW}⚠️  PostgreSQL unavailable — skipping (cache hits may persist if DB comes back online){RESET}"
        )

    # 1. Progress Tracker
    clean_mfb_progress(target_path)

    # 2. Path Tree Cache
    clean_path_tree(target_path)

    # 3. SQLite fallback databases
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
        cleanup_completed, rebuild_recommended = perform_full_cleanup()
        if not cleanup_completed:
            return
        if not rebuild_recommended:
            print(
                f"{DIM}No Rust build artifacts were removed; skipping rebuild step.{RESET}"
            )
            return

        try:
            print(f"{DIM}Triggering project rebuild automatically...{RESET}")
            run_post_cleanup_rebuild(PROJECT_ROOT)
        except (EOFError, KeyboardInterrupt):
            pass


if __name__ == "__main__":
    main()
