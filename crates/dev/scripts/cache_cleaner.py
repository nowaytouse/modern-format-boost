#!/usr/bin/env python3
"""Modern Format Boost - Cache Cleaner v1.3 (Python Edition)
Clears conversion/analysis caches only. Training corpora are never touched.

Full cache purge clears:
  1. PostgreSQL analysis cache tables (records, path_index, path_tree_snapshots, cache_metadata)
  2. PostgreSQL inference-log telemetry (not training sample tables)
  3. Local mfb_store.sqlite blob namespaces (path_tree, checkpoint, processed)
  4. Legacy image_analysis_v2*.db under MFB_HOME_ROOT/cache (if present)
  5. Batch resume state: ~/.mfb_progress/, MFB_HOME/tmp, stale locks

Never cleared by this tool:
  - PostgreSQL loop_samples / *_quality_samples / multi_scenario_metadata rows
  - Legacy or local training SQLite (e.g. gif_value_samples_*.db)
  - Training lane logs under the unified log root
  - cargo target/, dist/, __pycache__ (use other maintenance flows)

Cache purge modes require a live PostgreSQL connection and abort before local mutation
if the DB is unreachable. ``--purge-session-state`` clears session logs only (still skips
training lanes) and does not touch PostgreSQL analysis caches.
"""

import argparse
import fcntl
import json
import os
import shutil
import sqlite3
import subprocess
import sys
import time
from pathlib import Path

from fastmode_paths import default_mfb_state_root
from mfb_ui_tokens import pick_symbol

SCRIPT_DIR = Path(__file__).parent.resolve()
PROJECT_ROOT = SCRIPT_DIR.parent.parent.parent


MFB_STATE_ROOT = default_mfb_state_root()
MFB_PROGRESS_ROOT = Path.home() / ".mfb_progress"

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


def run_post_cleanup_rebuild(project_root: Path, *, force: bool = False) -> bool:
    if not shutil.which("cargo"):
        print(
            f"{RED}{pick_symbol('❌', ('[ERROR]'))} Error: cargo not found in PATH; cannot rebuild project.{RESET}"
        )
        return False

    print(
        f"{BOLD}\n{pick_symbol('📦', ('[PKG]'))} Verifying img/vid binaries after cache purge...{RESET}"
    )
    if force:
        print(f"{DIM}Rebuilding workspace tools with forced refresh...{RESET}\n")
    else:
        print(
            f"{DIM}Running smart_build (incremental if artifacts are current)...{RESET}\n"
        )

    release_smart_build = project_root / "target" / "release" / "smart_build"
    build_cmd = (
        [str(release_smart_build)]
        if release_smart_build.is_file()
        else [
            "cargo",
            "run",
            "--release",
            "--locked",
            "-p",
            "dev",
            "--bin",
            "smart_build",
            "--",
        ]
    )
    if force:
        build_cmd.append("--force")

    try:
        subprocess.run(
            build_cmd,
            cwd=project_root,
            check=True,
        )
        print(
            f"\n{GREEN}{pick_symbol('✅', ('[OK]'))} smart_build finished (img/vid binaries verified){RESET}"
        )
        return True
    except subprocess.CalledProcessError as exc:
        print(
            f"\n{RED}{pick_symbol('❌', ('[ERROR]'))} Error: Rebuild failed with exit code {exc.returncode}.{RESET}"
        )
    except FileNotFoundError as exc:
        print(
            f"\n{RED}{pick_symbol('❌', ('[ERROR]'))} Error: Rebuild failed to start: {exc}{RESET}"
        )

    print(
        f"{YELLOW}Please run 'cargo run --release --locked -p dev --bin smart_build -- --force' manually.{RESET}"
    )
    return False


# ---------------------------------------------------------------------------
# PostgreSQL helpers (primary cache backend)
# ---------------------------------------------------------------------------

PG_DBNAME = "modern_format_boost"
PG_DEFAULT_CONNSTR = f"host=localhost dbname={PG_DBNAME}"
PG_ANALYSIS_CACHE_TABLES = [
    "analysis_records",
    "quality_records",
    "video_records",
    "path_index",
    "path_tree_snapshots",
    "cache_metadata",
]

PG_INFERENCE_LOG_TABLES = [
    "loop_intent_inference_log",
    "image_quality_inference_log",
    "animated_image_quality_inference_log",
    "video_quality_inference_log",
]

# Backward-compatible alias used in tests/docs.
PG_TABLES = PG_ANALYSIS_CACHE_TABLES

# Training / corpus tables — must never be truncated or updated by this script.
PG_TRAINING_PROTECTED_TABLES = frozenset(
    {
        "loop_samples",
        "image_quality_samples",
        "animated_image_quality_samples",
        "video_quality_samples",
        "multi_scenario_metadata",
    }
)
ANIMATION_CACHE_EXTENSIONS = (
    "gif",
    "webp",
    "png",
    "apng",
    "avif",
    "heic",
    "heif",
    "jxl",
)


def _pg_connstr() -> str:
    return os.environ.get("MFB_PG_CONNSTR", PG_DEFAULT_CONNSTR)


def _pg_connect_error() -> str | None:
    """Return None if PostgreSQL is reachable; otherwise a short error reason."""
    try:
        import psycopg2
    except ImportError:
        return "psycopg2 is not installed (install psycopg2-binary)"
    try:
        conn = psycopg2.connect(_pg_connstr(), connect_timeout=3)
        conn.close()
        return None
    except (
        OSError,
        ValueError,
        RuntimeError,
        TypeError,
        KeyError,
        IndexError,
        AttributeError,
        UnicodeError,
    ) as exc:
        return str(exc)


def _require_postgres_for_cache_purge() -> None:
    """Abort cache cleanup if PostgreSQL (primary analysis cache) is not reachable."""
    err = _pg_connect_error()
    if err is None:
        return
    print(
        f"\n{RED}{pick_symbol('❌', ('[ERROR]'))} PostgreSQL is required for cache cleanup.{RESET}"
    )
    print(f"   {DIM}Reason: {err}{RESET}")
    print(f"   {DIM}Connection: {_pg_connstr()} (override with MFB_PG_CONNSTR){RESET}")
    print(
        f"   {DIM}Start the database service, then retry. "
        f"No analysis caches were modified.{RESET}\n"
    )
    sys.exit(1)


def _abort_cache_purge(message: str) -> None:
    print(f"\n{RED}{pick_symbol('❌', ('[ERROR]'))} {message}{RESET}")
    print(f"   {DIM}No further cache files were modified after this failure.{RESET}\n")
    sys.exit(1)


def _truncate_pg_cache_table(cur, table: str) -> None:
    if table in PG_TRAINING_PROTECTED_TABLES:
        raise RuntimeError(f"refusing to truncate protected training table: {table}")
    # No CASCADE: analysis cache tables have no FK links to training corpora.
    cur.execute(f"TRUNCATE TABLE {table} RESTART IDENTITY")


def purge_postgres_full():
    """TRUNCATE analysis + inference-log cache tables only (never training corpora)."""
    try:
        import psycopg2

        conn = psycopg2.connect(_pg_connstr())
        conn.autocommit = True
        cur = conn.cursor()
        for table in PG_ANALYSIS_CACHE_TABLES + PG_INFERENCE_LOG_TABLES:
            _truncate_pg_cache_table(cur, table)
        cur.close()
        conn.close()
        print(
            f"   {GREEN}{pick_symbol('✅', ('[OK]'))} PostgreSQL: analysis + inference-log caches "
            f"truncated (training tables untouched){RESET}"
        )
        return True
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
            f"   {RED}{pick_symbol('⚠️', '[WARN]')} PostgreSQL purge failed: {e}{RESET}"
        )
        return False


def purge_postgres_for_path(target_path: Path) -> int:
    """Delete rows matching target_path (or prefix) from PostgreSQL cache tables."""
    try:
        import psycopg2

        conn = psycopg2.connect(_pg_connstr())
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
                f"   {GREEN}{pick_symbol('✅', ('[OK]'))} PostgreSQL: removed {total} rows for {target_path.name}{RESET}"
            )
        return total
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
        _abort_cache_purge(f"PostgreSQL targeted purge failed: {e}")


def purge_postgres_animation_cache() -> int:
    """Delete cache rows for animation-capable image formats.

    This intentionally removes both positive and negative entries for these
    paths because old negative "static" verdicts cannot be distinguished
    safely without unpacking historical MessagePack payloads.
    """
    try:
        import psycopg2

        conn = psycopg2.connect(_pg_connstr())
        conn.autocommit = False
        cur = conn.cursor()
        patterns = [f"%.{ext}" for ext in ANIMATION_CACHE_EXTENSIONS]

        cur.execute(
            """
            CREATE TEMP TABLE mfb_animation_cache_purge AS
            SELECT DISTINCT content_hash
            FROM path_index
            WHERE lower(file_path) LIKE ANY(%s)
            """,
            (patterns,),
        )
        cur.execute("SELECT COUNT(*) FROM mfb_animation_cache_purge")
        hash_count = cur.fetchone()[0]

        total = 0
        for table in ("analysis_records", "quality_records", "video_records"):
            cur.execute(
                f"""
                DELETE FROM {table}
                WHERE content_hash IN (SELECT content_hash FROM mfb_animation_cache_purge)
                """
            )
            total += cur.rowcount

        cur.execute(
            """
            DELETE FROM path_index
            WHERE content_hash IN (SELECT content_hash FROM mfb_animation_cache_purge)
            """
        )
        total += cur.rowcount

        for table in PG_INFERENCE_LOG_TABLES:
            cur.execute(
                f"DELETE FROM {table} WHERE lower(source_path) LIKE ANY(%s)",
                (patterns,),
            )
            total += cur.rowcount

        conn.commit()
        cur.close()
        conn.close()

        print(
            f"   {GREEN}{pick_symbol('✅', ('[OK]'))} PostgreSQL: purged {total} rows across {hash_count} animation-capable content hashes{RESET}"
        )
        return total
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
        _abort_cache_purge(f"PostgreSQL animation-cache purge failed: {e}")


def draw_header(targeted=False):
    line = "─" * 60
    print(f"{BLUE}╭{line}╮{RESET}")
    mode_text = (
        f"{BOLD}{RED}{pick_symbol('🧹', ('[SWEEP]'))} TARGETED CACHE CLEANUP{RESET}"
        if targeted
        else f"{BOLD}{RED}{pick_symbol('🧹', ('[SWEEP]'))} CACHE & LOG CLEANUP UTILITY v1.1{RESET}"
    )
    print(f"{BLUE}│{RESET}  {mode_text:<62} {BLUE}│{RESET}")
    print(f"{BLUE}╰{line}╯{RESET}")
    if not targeted:
        print(
            f"   {RED}{pick_symbol('⚠️', '[WARN]')}  WARNING: Critical processing data will be permanently deleted.{RESET}\n"
        )


def get_dir_size(path):
    try:
        result = subprocess.run(
            ["du", "-sh", str(path)], capture_output=True, text=True
        )
        if result.returncode == 0:
            return result.stdout.split()[0]
        print(
            f"   {YELLOW}{pick_symbol('⚠️', '[WARN]')} size probe failed for {path}: exit {result.returncode}{RESET}",
            file=sys.stderr,
        )
        return None
    except (
        OSError,
        ValueError,
        RuntimeError,
        TypeError,
        KeyError,
        IndexError,
        AttributeError,
        UnicodeError,
    ) as exc:
        print(
            f"   {YELLOW}{pick_symbol('⚠️', '[WARN]')} size probe failed for {path}: {exc}{RESET}",
            file=sys.stderr,
        )
        return None


def _display_size(size: str | None) -> str:
    return size if size is not None else "N/A"


def show_stats(cache_dir, db_file, log_dir, mfb_progress_dir):
    print(f"{BOLD}Current Cache Status:{RESET}")

    if cache_dir.is_dir():
        size = _display_size(get_dir_size(cache_dir))
        print(f"   {pick_symbol('📂', ('[DIR]'))} Directory: {DIM}{cache_dir}{RESET}")
        print(
            f"   {pick_symbol('📦', ('[PKG]'))} Total Size: {BOLD}{GREEN}{size}{RESET}"
        )

        if db_file.is_file():
            db_size = _display_size(get_dir_size(db_file))
            print(
                f"   {pick_symbol('🗄️', ('[DB]'))}  Database:  {DIM}{db_file.name}{RESET} ({db_size})"
            )
    else:
        print(f"   {YELLOW}Empty: No cache directory found.{RESET}")

    log_size = _display_size(get_dir_size(log_dir)) if log_dir.is_dir() else "N/A"
    print(f"   {pick_symbol('📝', ('[LOG]'))} Logs:      {DIM}{log_size}{RESET}")

    if mfb_progress_dir.is_dir():
        prog_size = _display_size(get_dir_size(mfb_progress_dir))
        print(f"   {pick_symbol('🔄', ('~'))} Progress:  {DIM}{prog_size}{RESET}")

    target_dir = PROJECT_ROOT / "target"
    if target_dir.is_dir():
        target_size = _display_size(get_dir_size(target_dir))
        print(
            f"   {pick_symbol('🦀', ('[RUST]'))} Rust Build: {BOLD}{YELLOW}{target_size}{RESET}"
        )

    local_cache = PROJECT_ROOT / ".cache"
    if local_cache.is_dir():
        local_cache_size = _display_size(get_dir_size(local_cache))
        print(
            f"   {pick_symbol('⚡', ('[FAST]'))} Runtime:    {BOLD}{YELLOW}{local_cache_size}{RESET}"
        )

    fuzz_target = PROJECT_ROOT / "crates" / "dev" / "fuzz" / "target"
    if fuzz_target.is_dir():
        fuzz_size = _display_size(get_dir_size(fuzz_target))
        print(
            f"   {pick_symbol('🧪', ('[FUZZ]'))} Fuzz Build: {BOLD}{YELLOW}{fuzz_size}{RESET}"
        )

    dist_dir = PROJECT_ROOT / "dist"
    if dist_dir.is_dir():
        dist_size = _display_size(get_dir_size(dist_dir))
        print(
            f"   {pick_symbol('📦', ('[PKG]'))} Dist:       {BOLD}{BLUE}{dist_size}{RESET}"
        )

    lock_dir = MFB_STATE_ROOT / "locks"
    if lock_dir.is_dir():
        lock_count = len(list(lock_dir.glob("*.lock")))
        if lock_count > 0:
            print(
                f"   {pick_symbol('🔒', ('[LOCK]'))} Session Locks: {BOLD}{YELLOW}{lock_count} active/stale{RESET}"
            )
    print()


def _blob_crc32_i32(payload: bytes) -> int:
    import zlib

    crc = zlib.crc32(payload) & 0xFFFFFFFF
    return crc if crc < 2**31 else crc - 2**32


def _upsert_checkpoint_blob(
    conn: sqlite3.Connection, cache_key: str, payload_obj: dict
) -> None:
    import time

    payload = json.dumps(payload_obj, separators=(",", ":")).encode("utf-8")
    crc = _blob_crc32_i32(payload)
    updated_at = int(time.time())
    conn.execute(
        """INSERT INTO blob_store
           (namespace, cache_key, schema_version, root_path, payload, payload_crc32, updated_at)
           VALUES ('checkpoint', ?, 1, NULL, ?, ?, ?)
           ON CONFLICT(namespace, cache_key) DO UPDATE SET
              payload = excluded.payload,
              payload_crc32 = excluded.payload_crc32,
              updated_at = excluded.updated_at""",
        (cache_key, payload, crc, updated_at),
    )


def clean_mfb_progress(target_path: Path):
    """
    Cleans checkpoint resume state from mfb_store.sqlite (namespace checkpoint).
    Lock files under ~/.mfb_progress/ are removed when a tracker is deleted.
    """
    progress_dir = MFB_PROGRESS_ROOT
    target_abs = str(target_path.absolute())
    is_dir = target_path.is_dir()

    deleted_count = 0
    modified_count = 0

    store = _sqlite_store_path()
    if not store.is_file():
        return deleted_count, modified_count

    conn = sqlite3.connect(store)
    cur = conn.cursor()
    cur.execute(
        "SELECT cache_key, payload FROM blob_store WHERE namespace = 'checkpoint'",
    )
    rows = cur.fetchall()

    for cache_key, payload in rows:
        try:
            blob = json.loads(payload)
        except json.JSONDecodeError as e:
            raise RuntimeError(
                f"checkpoint blob {cache_key} is not valid JSON: {e}"
            ) from e

        header = blob.get("header") or {}
        entries = blob.get("entries") or {}
        target_dir = header.get("target_dir", "")

        if is_dir and (
            target_abs == target_dir or target_dir.startswith(target_abs + "/")
        ):
            cur.execute(
                "DELETE FROM blob_store WHERE namespace = 'checkpoint' AND cache_key = ?",
                (cache_key,),
            )
            deleted_count += 1
            lock_file = progress_dir / f"{cache_key}.lock"
            if lock_file.is_file():
                lock_file.unlink()
            print(
                f"   {GREEN}{pick_symbol('✅', ('[OK]'))} Removed checkpoint tracker: {DIM}{target_dir}{RESET}"
            )
            continue

        if not is_dir and (
            target_abs == target_dir or target_abs.startswith(target_dir + "/")
        ):
            if target_abs not in entries:
                continue
            del entries[target_abs]
            blob["entries"] = entries
            _upsert_checkpoint_blob(conn, cache_key, blob)
            modified_count += 1
            print(
                f"   {GREEN}{pick_symbol('✅', ('[OK]'))} Pruned file from checkpoint: {DIM}{target_path.name}{RESET}"
            )

    conn.commit()
    conn.close()

    if progress_dir.is_dir():
        for stale in progress_dir.glob("*.txt"):
            stale.unlink()
            deleted_count += 1
            print(
                f"   {GREEN}{pick_symbol('✅', ('[OK]'))} Removed orphan legacy progress file: {DIM}{stale.name}{RESET}"
            )

    return deleted_count, modified_count


def _sqlite_store_path() -> Path:
    return MFB_STATE_ROOT / "cache" / "mfb_store.sqlite"


def _purge_sqlite_blob_namespace_under(namespace: str, target_path: Path) -> int:
    """Delete blob_store rows under namespace scoped by root_path prefix."""
    store = _sqlite_store_path()
    if not store.is_file():
        return 0
    target_abs = str(target_path.absolute())
    pattern = target_abs.rstrip("/") + "/%"
    conn = sqlite3.connect(store)
    cur = conn.cursor()
    cur.execute(
        "DELETE FROM blob_store WHERE namespace = ? AND (root_path = ? OR root_path LIKE ?)",
        (namespace, target_abs, pattern),
    )
    deleted = cur.rowcount
    conn.commit()
    conn.close()
    return deleted


def _purge_sqlite_blob_namespace_all(namespace: str) -> int:
    store = _sqlite_store_path()
    if not store.is_file():
        return 0
    conn = sqlite3.connect(store)
    cur = conn.cursor()
    cur.execute("DELETE FROM blob_store WHERE namespace = ?", (namespace,))
    deleted = cur.rowcount
    conn.commit()
    conn.close()
    return deleted


PURGE_PATH_TREE_BIN = PROJECT_ROOT / "target" / "debug" / "purge_path_tree_cache"


def _invoke_purge_path_tree_cache(cli_args: list[str]) -> int:
    """Delegate path-tree purge to Rust SSOT (path_tree_cache.rs: PG + SQLite replica)."""
    env = os.environ.copy()
    env.setdefault("MFB_PG_CONNSTR", _pg_connstr())
    if PURGE_PATH_TREE_BIN.is_file():
        cmd = [str(PURGE_PATH_TREE_BIN), *cli_args]
    else:
        cmd = [
            "cargo",
            "run",
            "-p",
            "foundation",
            "--bin",
            "purge_path_tree_cache",
            "--",
            *cli_args,
        ]
    result = subprocess.run(
        cmd,
        cwd=PROJECT_ROOT,
        env=env,
        capture_output=True,
        text=True,
        check=False,
    )
    if result.returncode != 0:
        detail = (result.stderr or result.stdout or "").strip()
        raise RuntimeError(
            f"purge_path_tree_cache failed (exit {result.returncode}): {detail or 'no output'}"
        )
    lines = [
        line.strip() for line in (result.stdout or "").splitlines() if line.strip()
    ]
    if not lines:
        return 0
    try:
        return int(lines[-1])
    except ValueError as exc:
        raise RuntimeError(
            f"purge_path_tree_cache returned non-integer stdout: {result.stdout!r}"
        ) from exc


def clean_path_tree(target_path: Path):
    """Cleans path-tree snapshots from PostgreSQL (M213) and SQLite replica (M214)."""
    deleted_count = _invoke_purge_path_tree_cache(
        ["--under", str(target_path.absolute())]
    )
    if deleted_count > 0:
        print(
            f"   {GREEN}{pick_symbol('✅', ('[OK]'))} path_tree_snapshots (PG + SQLite): "
            f"removed {deleted_count} row(s) under {DIM}{target_path.name}{RESET}"
        )
    return deleted_count


def clean_all_path_tree():
    """Remove all path-tree snapshots (PostgreSQL + SQLite replica)."""
    deleted_count = _invoke_purge_path_tree_cache(["--all"])
    if deleted_count > 0:
        print(
            f"   {GREEN}{pick_symbol('✅', ('[OK]'))} Removed {deleted_count} path-tree "
            f"cache entries (PG + SQLite){RESET}"
        )
    return deleted_count


def _legacy_analysis_sqlite_paths() -> list[Path]:
    cache_dir = MFB_STATE_ROOT / "cache"
    return [
        cache_dir / "image_analysis_v2.db",
        cache_dir / "image_analysis_v2_main.db",
    ]


def remove_legacy_analysis_sqlite_files() -> int:
    """Delete retired pre-PostgreSQL analysis SQLite files (no migration)."""
    removed = 0
    for db_file in _legacy_analysis_sqlite_paths():
        if db_file.is_file():
            db_file.unlink()
            removed += 1
            print(
                f"   {GREEN}{pick_symbol('✅', ('[OK]'))} Removed legacy analysis DB: "
                f"{DIM}{db_file.name}{RESET}"
            )
    return removed


def purge_mfb_store_blob_namespaces_full() -> int:
    """Delete all blob_store cache namespaces before removing the store directory."""
    total = 0
    for namespace in ("path_tree", "checkpoint", "processed"):
        total += _purge_sqlite_blob_namespace_all(namespace)
    if total > 0:
        print(
            f"   {GREEN}{pick_symbol('✅', ('[OK]'))} mfb_store.sqlite: purged {total} "
            f"blob_store row(s) (path_tree/checkpoint/processed){RESET}"
        )
    return total


def _purge_postgres_inference_logs_for_path(cur, target_path: Path) -> int:
    target_abs = str(target_path.absolute())
    pattern = target_abs.rstrip("/") + "/%"
    total = 0
    for table in PG_INFERENCE_LOG_TABLES:
        if target_path.is_dir():
            cur.execute(
                f"DELETE FROM {table} WHERE source_path = %s OR source_path LIKE %s",
                (target_abs, pattern),
            )
        else:
            cur.execute(f"DELETE FROM {table} WHERE source_path = %s", (target_abs,))
        total += cur.rowcount
    return total


def purge_conversion_resume_state(
    progress_dir: Path, tmp_dir: Path, lock_dir: Path
) -> None:
    """Clear batch resume/progress artifacts (not training corpora or session logs)."""
    if progress_dir.is_dir():
        print(f"{DIM}   Removing MFB progress directory...{RESET}")
        shutil.rmtree(progress_dir, ignore_errors=True)
        print(f"   {GREEN}{pick_symbol('✅', ('[OK]'))} MFB progress purged{RESET}")

    if tmp_dir.is_dir():
        print(f"{DIM}   Purging isolated temp directory...{RESET}")
        shutil.rmtree(tmp_dir, ignore_errors=True)
        tmp_dir.mkdir(parents=True, exist_ok=True)
        print(
            f"   {GREEN}{pick_symbol('✅', ('[OK]'))} Isolated temp space cleared{RESET}"
        )

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
            print(
                f"   {GREEN}{pick_symbol('✅', ('[OK]'))} {deleted_locks} stale locks purged{RESET}"
            )
        if active_locks > 0:
            print(
                f"   {YELLOW}ℹ️  {active_locks} active sessions skipped (protected){RESET}"
            )


def perform_animation_cache_cleanup():
    _require_postgres_for_cache_purge()

    draw_header(targeted=True)
    print(
        f"   {BOLD}Target:{RESET} {DIM}animation-capable cache entries ({', '.join(ANIMATION_CACHE_EXTENSIONS)}){RESET}"
    )
    print(
        f"   {YELLOW}Purging cached static/unknown animation verdicts and path-tree routing snapshots...{RESET}\n"
    )

    purge_postgres_animation_cache()
    clean_all_path_tree()
    remove_legacy_analysis_sqlite_files()
    print(
        f"\n{GREEN}{pick_symbol('✅', ('[OK]'))} Animation Cache Cleanup Complete{RESET}\n"
    )


def _warn_cleanup_failure(entry: Path, exc: Exception) -> None:
    print(
        f"   {YELLOW}{pick_symbol('⚠️', '[WARN]')} cleanup failed for {entry}: {exc}{RESET}",
        file=sys.stderr,
    )


def _is_training_lane_log_dir(target: Path) -> bool:
    try:
        from mfb_log_paths import LEGACY_TRAINING_LOG_LANES, TRAINING_LOG_LANES

        protected = {*TRAINING_LOG_LANES, *LEGACY_TRAINING_LOG_LANES}
    except (
        OSError,
        ValueError,
        RuntimeError,
        TypeError,
        KeyError,
        IndexError,
        AttributeError,
        UnicodeError,
    ):
        protected = {
            "static_high",
            "static_low",
            "loop_high",
            "loop_low",
            "static",
            "all_high",
            "loop",
            "loop_video",
        }
    return target.name in protected


def _purge_log_dir_session_artifacts(target: Path) -> tuple[int, int]:
    """Remove conversion session logs/bundles (never training lane artifacts)."""
    removed_logs = 0
    removed_dirs = 0
    if not target.is_dir():
        return removed_logs, removed_dirs
    if _is_training_lane_log_dir(target):
        return removed_logs, removed_dirs

    for pattern in ("*.log", "*.jsonl", "diagnostic_report_*.txt"):
        for entry in target.glob(pattern):
            try:
                entry.unlink()
                removed_logs += 1
            except (
                OSError,
                ValueError,
                RuntimeError,
                TypeError,
                KeyError,
                IndexError,
                AttributeError,
                UnicodeError,
            ) as exc:
                _warn_cleanup_failure(entry, exc)

    for exact_name in ("deleted_offending_files.txt",):
        artifact = target / exact_name
        if artifact.is_file():
            try:
                artifact.unlink()
                removed_logs += 1
            except (
                OSError,
                ValueError,
                RuntimeError,
                TypeError,
                KeyError,
                IndexError,
                AttributeError,
                UnicodeError,
            ) as exc:
                _warn_cleanup_failure(artifact, exc)

    for pattern in ("replica_audit_*.jsonl",):
        for entry in target.glob(pattern):
            try:
                entry.unlink()
                removed_logs += 1
            except (
                OSError,
                ValueError,
                RuntimeError,
                TypeError,
                KeyError,
                IndexError,
                AttributeError,
                UnicodeError,
            ) as exc:
                _warn_cleanup_failure(entry, exc)

    for pattern in ("Bundle_*", "dev_verify"):
        for entry in target.glob(pattern):
            try:
                if entry.is_dir():
                    shutil.rmtree(entry, ignore_errors=True)
                    removed_dirs += 1
                elif entry.exists():
                    entry.unlink()
                    removed_logs += 1
            except (
                OSError,
                ValueError,
                RuntimeError,
                TypeError,
                KeyError,
                IndexError,
                AttributeError,
                UnicodeError,
            ) as exc:
                _warn_cleanup_failure(entry, exc)
    return removed_logs, removed_dirs


def purge_session_logs_only(log_dir: Path) -> None:
    """Clear conversion session logs from the main log root (training lanes are never touched)."""
    if not log_dir.is_dir():
        return
    print(
        f"{DIM}   Clearing conversion session logs from {log_dir} "
        f"(training lanes preserved)...{RESET}"
    )
    removed_logs, removed_dirs = _purge_log_dir_session_artifacts(log_dir)
    print(
        f"   {GREEN}{pick_symbol('✅', ('[OK]'))} Session logs cleared ({removed_logs} files, "
        f"{removed_dirs} directories){RESET}"
    )


def purge_session_artifacts(
    log_dir: Path, progress_dir: Path, tmp_dir: Path, lock_dir: Path
):
    purge_session_logs_only(log_dir)
    purge_conversion_resume_state(progress_dir, tmp_dir, lock_dir)


def perform_session_state_cleanup():
    from mfb_log_paths import unified_log_dir

    cache_dir = MFB_STATE_ROOT / "cache"
    log_dir = unified_log_dir()
    mfb_progress_dir = MFB_PROGRESS_ROOT
    mfb_tmp_dir = MFB_STATE_ROOT / "tmp"
    lock_dir = MFB_STATE_ROOT / "locks"

    clear_screen()
    draw_header(targeted=True)
    show_stats(cache_dir, _sqlite_store_path(), log_dir, mfb_progress_dir)
    print(
        f"   {BOLD}Target:{RESET} {DIM}session state only (logs, progress, temp, stale locks){RESET}\n"
    )

    if can_prompt_user():
        print(
            f"{YELLOW}{pick_symbol('⚠️', '[WARN]')}  CONFIRM: Clear session state artifacts only?{RESET}"
        )
        if input(
            f"   {CYAN}Type {GREEN}'yes'{CYAN} to proceed: {RESET}"
        ).strip().lower() not in ("y", "yes"):
            print(
                f"\n{RED}{pick_symbol('🚫', ('[BLOCKED]'))} Session-state cleanup cancelled by user.{RESET}\n"
            )
            return

    purge_session_artifacts(log_dir, mfb_progress_dir, mfb_tmp_dir, lock_dir)
    print(
        f"\n{GREEN}{pick_symbol('✅', ('[OK]'))} Session-State Cleanup Complete{RESET}\n"
    )


def perform_full_cleanup(*, skip_confirm: bool = False):
    from mfb_log_paths import unified_log_dir

    cache_dir = MFB_STATE_ROOT / "cache"
    lock_dir = MFB_STATE_ROOT / "locks"
    store_file = _sqlite_store_path()
    log_dir = unified_log_dir()
    mfb_progress_dir = MFB_PROGRESS_ROOT
    mfb_tmp_dir = MFB_STATE_ROOT / "tmp"

    clear_screen()
    draw_header()
    show_stats(cache_dir, store_file, log_dir, mfb_progress_dir)

    pg_err = _pg_connect_error()
    if pg_err:
        print(
            f"\n{RED}{pick_symbol('❌', ('[ERROR]'))} PostgreSQL is required before cache cleanup can run.{RESET}"
        )
        print(f"   {DIM}Reason: {pg_err}{RESET}")
        print(
            f"   {DIM}Connection: {_pg_connstr()} (override with MFB_PG_CONNSTR){RESET}\n"
        )
        time.sleep(2)
        return False, False

    print(
        f"{RED}{pick_symbol('⚠️', '[WARN]')}  The following caches will be PERMANENTLY cleared:{RESET}"
    )
    print(
        "   - PostgreSQL analysis cache (records, path_index, path_tree_snapshots, cache_metadata)"
    )
    print("   - PostgreSQL inference-log telemetry (loop/image/animated/video)")
    print("   - Legacy analysis SQLite files (image_analysis_v2*.db, if present)")
    print("   - mfb_store.sqlite (path-tree, checkpoint, processed blobs)")
    print("   - Batch resume state (~/.mfb_progress/, tmp/, stale locks)")
    print(
        f"   {GREEN}- Training corpora (loop_samples, *_quality_samples, metadata) are preserved{RESET}"
    )
    print(
        f"   {GREEN}- Training lane logs and local training SQLite are preserved{RESET}"
    )
    print()

    if not skip_confirm:
        print(
            f"\n{YELLOW}{pick_symbol('⚠️', '[WARN]')}  CONFIRM: Start full cache cleanup?{RESET}"
        )
        if input(
            f"   {CYAN}Type {GREEN}'yes'{CYAN} to proceed: {RESET}"
        ).strip().lower() not in ("y", "yes"):
            print(
                f"\n{RED}{pick_symbol('🚫', ('[BLOCKED]'))} Cleanup cancelled by user.{RESET}"
            )
            print(f"{DIM}   No action taken. Returning to menu...{RESET}")
            time.sleep(1.5)
            return False, False

    print(
        f"\n{YELLOW}{pick_symbol('🚀', ('[LAUNCH]'))} Executing full cache cleanup...{RESET}"
    )
    time.sleep(1.2)
    print(f"   {DIM}Initializing cleanup engine...{RESET}\n")
    time.sleep(0.8)

    _require_postgres_for_cache_purge()
    print(f"{DIM}   Purging PostgreSQL cache tables...{RESET}")
    if not purge_postgres_full():
        print(
            f"\n{RED}{pick_symbol('❌', ('[ERROR]'))} Cache cleanup aborted: PostgreSQL purge failed.{RESET}"
        )
        print(f"   {DIM}Local SQLite caches were not modified.{RESET}\n")
        return False, False

    remove_legacy_analysis_sqlite_files()
    purge_mfb_store_blob_namespaces_full()

    if cache_dir.is_dir():
        print(f"{DIM}   Clearing cache directory (preserving models)...{RESET}")
        for item in cache_dir.iterdir():
            if item.name == "models":
                continue
            try:
                if item.is_dir():
                    shutil.rmtree(item, ignore_errors=True)
                else:
                    item.unlink()
            except (
                OSError,
                ValueError,
                RuntimeError,
                TypeError,
                KeyError,
                IndexError,
                AttributeError,
                UnicodeError,
            ) as exc:
                _warn_cleanup_failure(item, exc)
        print(
            f"   {GREEN}{pick_symbol('✅', ('[OK]'))} Local cache directory cleared "
            f"(models preserved, mfb_store.sqlite + legacy files purged){RESET}"
        )

    purge_conversion_resume_state(mfb_progress_dir, mfb_tmp_dir, lock_dir)

    print(
        f"\n{GREEN}{pick_symbol('✅', ('[OK]'))} Full Cache Cleanup Complete{RESET}\n"
    )
    return True, True


def perform_targeted_cleanup(target_path: Path):
    if not target_path.exists():
        print(
            f"\n{RED}{pick_symbol('❌', ('[ERROR]'))} Error: Path does not exist: {target_path}{RESET}\n"
        )
        sys.exit(1)

    _require_postgres_for_cache_purge()

    draw_header(targeted=True)
    print(f"   {BOLD}Target:{RESET} {DIM}{target_path.absolute()}{RESET}")
    print(f"   {YELLOW}Scanning metadata associated with this path...{RESET}\n")

    # 0. PostgreSQL — primary cache backend (must be cleared first)
    purge_postgres_for_path(target_path)
    try:
        import psycopg2

        conn = psycopg2.connect(_pg_connstr())
        conn.autocommit = True
        cur = conn.cursor()
        removed = _purge_postgres_inference_logs_for_path(cur, target_path)
        cur.close()
        conn.close()
        if removed > 0:
            print(
                f"   {GREEN}{pick_symbol('✅', ('[OK]'))} PostgreSQL inference-log: "
                f"removed {removed} row(s) for {target_path.name}{RESET}"
            )
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
        _abort_cache_purge(f"PostgreSQL inference-log purge failed: {e}")

    # 1. Progress Tracker
    clean_mfb_progress(target_path)

    # 2. Path Tree Cache
    clean_path_tree(target_path)

    # 3. Processed-list blobs (anti-duplicate session cache)
    removed_processed = _purge_sqlite_blob_namespace_under("processed", target_path)
    if removed_processed > 0:
        print(
            f"   {GREEN}{pick_symbol('✅', ('[OK]'))} mfb_store processed blobs: "
            f"removed {removed_processed} row(s){RESET}"
        )

    print(f"\n{GREEN}{pick_symbol('✅', ('[OK]'))} Targeted Cleanup Complete{RESET}\n")


def main():
    import sys
    from pathlib import Path

    _scripts = Path(__file__).resolve().parent
    if str(_scripts) not in sys.path:
        sys.path.insert(0, str(_scripts))
    from mfb_entry_guard import guard_main

    guard_main("cache_cleaner.py")
    parser = argparse.ArgumentParser(description="Modern Format Boost Cache Cleaner")
    parser.add_argument(
        "--purge-animation-cache",
        action="store_true",
        help="Remove cache rows for animation-capable image formats (GIF/WebP/APNG/AVIF/HEIC/JXL/PNG)",
    )
    parser.add_argument(
        "--purge-session-state",
        action="store_true",
        help="Remove session logs, verification reports, progress trackers, temp files, and stale locks without touching analysis caches",
    )
    parser.add_argument(
        "path", nargs="?", help="Target file or directory for fine-grained cleanup"
    )
    parser.add_argument(
        "--yes",
        "-y",
        action="store_true",
        help="Skip interactive confirmation (e.g. when drag-and-drop already confirmed)",
    )
    args = parser.parse_args()

    if args.purge_animation_cache:
        perform_animation_cache_cleanup()
        return

    if args.purge_session_state:
        perform_session_state_cleanup()
        return

    if args.path:
        perform_targeted_cleanup(Path(args.path))
    else:
        cleanup_completed, _rebuild_after_full_cleanup = perform_full_cleanup(
            skip_confirm=args.yes
        )
        if not cleanup_completed:
            sys.exit(1)

        try:
            print(
                f"{DIM}Verifying img/vid binaries after cache cleanup (smart_build)...{RESET}"
            )
            if not run_post_cleanup_rebuild(PROJECT_ROOT, force=False):
                sys.exit(1)
        except (EOFError, KeyboardInterrupt):
            print(
                f"\n{YELLOW}Rebuild interrupted. Run: cargo run --locked --release -p dev --bin smart_build -- --force{RESET}"
            )
            sys.exit(130)


if __name__ == "__main__":
    main()
