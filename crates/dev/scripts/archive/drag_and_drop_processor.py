#!/usr/bin/env python3
"""Modern Format Boost - Drag & Drop Processor (Python Edition)
Usage: Drag folder onto this script or double-click to select
"""

import datetime
import importlib.util
import json
import os
import pty
import re
import shutil
import signal
import subprocess
import sys
import threading
import time
from dataclasses import dataclass
from pathlib import Path
from typing import Callable

from media_scope import (
    HANDOFF_PRESERVE_PHASE_POST_IMG_VID,
    MEDIA_EXTS,
    MediaProbeError,
    PIPELINE_IMAGE,
    PIPELINE_VIDEO,
    audit_handoff_blocked,
    classify_media_owner,
    detect_true_format,
    format_audit_event,
    format_bytes,
    format_session_audit_routed,
    list_handoff_preserve_candidates,
    preserve_handoff_gaps,
)
from mfb_entry_guard import guard_main, run_delegated
from mfb_log_paths import (
    append_jsonl_audit_record,
    archive_drag_drop_session_bundle,
    ensure_unified_log_dir,
    format_session_stamp,
    parse_session_stamp,
)
from mfb_logger import setup_logger
from mfb_ui_tokens import BRAND_BLUE, colors_enabled, pick_symbol

MFB_DEFAULT_HOME_DIRNAME = ".modern_format_boost"
FAST_IMG_FORCE_SMART_BUILD = False


def default_mfb_state_root() -> Path:
    env_root = (os.environ.get("MFB_HOME_ROOT") or "").strip()
    if env_root:
        return Path(env_root).expanduser()
    if home := os.environ.get("HOME"):
        return Path(home).expanduser() / MFB_DEFAULT_HOME_DIRNAME
    return Path.home() / MFB_DEFAULT_HOME_DIRNAME


def fast_img_output_dir_for_target(
    target_dir: Path,
    has_resume_marker: Callable[[Path], bool] | None = None,
) -> Path:
    target = target_dir.expanduser()
    base = target.with_name(f"{target.name}_optimized")
    candidate = base
    suffix = 2
    while (
        candidate.exists()
        and not (candidate / ".mfb_wc").exists()
        and not (has_resume_marker(candidate) if has_resume_marker else False)
    ):
        candidate = base.with_name(f"{base.name}_{suffix}")
        suffix += 1
    return candidate


def _unique_adjacent_dir(target_dir: Path, suffix_name: str) -> Path:
    target = target_dir.expanduser()
    base = target.with_name(f"{target.name}_{suffix_name}")
    candidate = base
    suffix = 2
    while candidate.exists():
        candidate = base.with_name(f"{base.name}_{suffix}")
        suffix += 1
    return candidate


def fast_img_restore_output_dir_for_target(target_dir: Path) -> Path:
    return _unique_adjacent_dir(target_dir, "restored_jpeg")


def fast_vid_output_dir_for_target(target_dir: Path) -> Path:
    return _unique_adjacent_dir(target_dir, "optimized")


logger = setup_logger("mfb.processor")
_FAST_IMG_MARKER_PROBE_RUN = subprocess.run

ICO_ERR = pick_symbol("❌", "[ERROR]")
ICO_OK = pick_symbol("✅", "[OK]")
ICO_WARN = pick_symbol("⚠️", "[WARN]")
ICO_WARN_ALT = pick_symbol("⚠", "[WARN]")
ICO_RETRY = pick_symbol("🔄", "~")
ICO_CLIP = pick_symbol("📋", "[CLIP]")
ICO_BLOCK = pick_symbol("🚫", "[BLOCKED]")
ICO_LAUNCH = pick_symbol("🚀", "[LAUNCH]")
ICO_DIR = pick_symbol("📁", "[DIR]")
ICO_IMG = pick_symbol("🖼️", "[IMG]")
ICO_VID = pick_symbol("🎬", "[VID]")
ICO_PKG = pick_symbol("📦", "[PKG]")
ICO_LOG = pick_symbol("📝", "[LOG]")
ICO_SEARCH = pick_symbol("🔍", "[SEARCH]")
ICO_STATS = pick_symbol("📊", "[STATS]")
ICO_CHART = pick_symbol("📈", "[CHART]")
ICO_CHECK = pick_symbol("🔎", "[CHECK]")
ICO_PLUS = pick_symbol("➕", "[+]")
ICO_FOLDER = pick_symbol("📂", "[DIR]")
ICO_TARGET = pick_symbol("🎯", "[TARGET]")
ICO_TEMP = pick_symbol("🌡️", "[TEMP]")
ICO_MSG = pick_symbol("💬", "[MSG]")

# Dependency Verification & Reliable Imports
try:
    import psutil

    # pyrefly: ignore [missing-import]
    from rich.console import Console

    # pyrefly: ignore [missing-import]
    from rich.panel import Panel

    # pyrefly: ignore [missing-import]
    from rich.table import Table

    # pyrefly: ignore [missing-import]
    from watchdog.events import FileSystemEventHandler

    # pyrefly: ignore [missing-import]
    from watchdog.observers import Observer
except ImportError:
    _REQ = ["psutil", "rich", "watchdog"]
    _MIS = [n for n in _REQ if importlib.util.find_spec(n) is None]

    class Console:
        def print(self, *args, **kwargs):
            print(*args)

    def _missing_dependency_error(*_args, **_kwargs):
        raise RuntimeError(f"Missing Python dependencies: {', '.join(_MIS)}")

    Panel = _missing_dependency_error
    Table = _missing_dependency_error

    class FileSystemEventHandler:
        pass

    class Observer:
        def __init__(self, *_args, **_kwargs):
            _missing_dependency_error()


def ensure_runtime_dependencies():
    if not globals().get("_MIS"):
        return
    print("\n" + "=" * 60)
    print(f"{ICO_ERR} CRITICAL ERROR: Missing Python Dependencies")
    print("=" * 60)
    print(f"Required libraries missing: {', '.join(_MIS)}")
    print(f"\n    pip install {' '.join(_MIS)}")
    print("=" * 60 + "\n")
    sys.exit(1)


class ReturnToHomeException(Exception):
    """Custom exception to trigger a return to the main selection menu."""

    pass


console = Console()

# Basic ANSI (honors NO_COLOR / MODERN_FORMAT_PLAIN_UI — U10)
if colors_enabled():
    RED = "\033[0;31m"
    GREEN = "\033[0;32m"
    YELLOW = "\033[1;33m"
    BLUE = "\033[0;34m"
    MAGENTA = "\033[0;35m"
    CYAN = "\033[0;36m"
    WHITE = "\033[0;37m"
    BOLD = "\033[1m"
    DIM = "\033[2m"
    RESET = "\033[0m"
else:
    RED = GREEN = YELLOW = BLUE = MAGENTA = CYAN = WHITE = BOLD = DIM = RESET = ""
    RED = GREEN = YELLOW = BLUE = MAGENTA = CYAN = WHITE = BOLD = DIM = RESET = ""

SCRIPT_DIR = Path(__file__).parent.resolve()
PROJECT_ROOT = SCRIPT_DIR.parent.parent.parent


IMGQUALITY_HEVC = PROJECT_ROOT / "target" / "release" / "img"
VIDQUALITY_HEVC = PROJECT_ROOT / "target" / "release" / "vid"
DRAG_DROP_RS_BIN = PROJECT_ROOT / "target" / "release" / "drag_and_drop_processor"


def launch_drag_drop_rs_cli(*cli_args: str) -> list[str]:
    """Prefer release Rust binary; fall back to cargo run during dev builds."""
    if DRAG_DROP_RS_BIN.is_file():
        return [str(DRAG_DROP_RS_BIN), *cli_args]
    return [
        "cargo",
        "run",
        "--locked",
        "-p",
        "dev",
        "--bin",
        "drag_and_drop_processor",
        "--",
        *cli_args,
    ]


def get_mfb_state_root() -> Path:
    return default_mfb_state_root()


# MFB Ghost Mode - Isolated Temporary Directory
# This prevents folder mtime updates by redirecting all intermediate IO away from source folders
MFB_STATE_ROOT = get_mfb_state_root()
MFB_TMP_ROOT = MFB_STATE_ROOT / "tmp"
MFB_TMP_ROOT.mkdir(parents=True, exist_ok=True)
os.environ["MFB_HOME_ROOT"] = str(MFB_STATE_ROOT)
os.environ["TMPDIR"] = str(MFB_TMP_ROOT)


def load_local_env():
    """Load local credentials from a non-git-tracked directory for privacy."""
    # Try loading from local_env.json first
    local_env_json = PROJECT_ROOT / "crates" / ".modern_format_boost" / "local_env.json"
    if local_env_json.exists():
        try:
            import json

            with open(local_env_json, "r") as f:
                config = json.load(f)
                for key, val in config.items():
                    os.environ[key] = str(val)
            return
        except Exception:
            pass

    # Fallback to local_env.sh for backwards compatibility
    local_env_file = PROJECT_ROOT / "crates" / ".modern_format_boost" / "local_env.sh"
    if local_env_file.exists():
        with open(local_env_file, "r") as f:
            for line in f:
                line = line.strip()
                if line and not line.startswith("#") and "export " in line:
                    # Simple shell export parser: export KEY="VALUE"
                    parts = line.replace("export ", "", 1).split("=", 1)
                    if len(parts) == 2:
                        key = parts[0].strip()
                        val = parts[1].strip().strip("\"'")
                        os.environ[key] = val


load_local_env()


def verify_database_mandatory():
    """Fail-fast if the mandatory PostgreSQL database is not reachable."""
    try:
        # Check tool existence first
        if not IMGQUALITY_HEVC.exists():
            return

        # Use db-health as the definitive check
        result = subprocess.run(
            [str(IMGQUALITY_HEVC), "db-health"],
            capture_output=True,
            text=True,
        )

        # If either db-health reports failure or the tool itself exits with error (due to our mandatory check)
        if result.returncode != 0:
            clear_screen()
            draw_header()
            print(f"{RED}{ICO_ERR} MANDATORY DATABASE CONNECTION FAILED{RESET}")
            print(
                f"{DIM}   Modern Format Boost now requires a PostgreSQL backend for full forensic accuracy.{RESET}\n"
            )
            print(f"{YELLOW}   HOW TO FIX:{RESET}")
            print("   1. Ensure PostgreSQL is running locally.")
            print("   2. Run the private setup helper:")
            print(f"      {CYAN}cargo run -p dev --bin setup_private_db{RESET}")
            print("   3. Or manually create the local environment file:")
            print(
                f"      {CYAN}{PROJECT_ROOT}/.modern_format_boost/local_env.json{RESET}"
            )
            print("   4. Add your connection string inside:")
            print(
                f'      {BOLD}{{"MFB_PG_CONNSTR": "postgresql://user:pass@localhost/db_name"}}{RESET}\n'
            )

            # Print stderr if available, otherwise stdout
            err = result.stderr.strip() or result.stdout.strip()
            if err:
                print(f"{DIM}   Diagnostic Output: {err}{RESET}")
            sys.exit(1)
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
        logger.exception("Mandatory database verification preflight failed")
        print(f"{RED}{ICO_ERR} DATABASE HEALTH PRECHECK FAILED{RESET}")
        print(
            f"{DIM}   Database verification raised an unexpected error before processing started.{RESET}"
        )
        print(f"{DIM}   Diagnostic Output: {exc}{RESET}")
        sys.exit(1)


OUTPUT_MODE = "adjacent"
TARGET_DIR = ""
OUTPUT_DIR = ""
ULTIMATE_MODE = True
VERBOSE_MODE = True
RESUME_MODE = False
PROCESSING_MODE = "both"  # Options: "both", "images_only", "videos_only"
FAST_IMG_ACTION = "shortest_path"
FAST_IMG_SHORTEST_PATH = False
FAST_IMG_OUTPUT_CLEANED = False
FAST_VID_SHORTEST_PATH = False

DRAG_DROP_ERROR_MODE_ENV = "MFB_DRAG_DROP_ERROR_MODE"
DRAG_DROP_FAIL_FAST_ENV = "MFB_DRAG_DROP_FAIL_FAST"
DRAG_DROP_ERROR_MODE_FAIL_FAST = "fail-fast"
DRAG_DROP_ERROR_MODE_LOG_AND_CONTINUE = "log-and-continue"

IMG_SUCCEEDED = 0
IMG_SKIPPED = 0
IMG_IGNORED = 0
IMG_FAILED = 0
VID_SUCCEEDED = 0
VID_SKIPPED = 0
VID_IGNORED = 0
VID_FAILED = 0
ROUTED_IMAGE_REL_PATHS = set()
ROUTED_VIDEO_REL_PATHS = set()
LAST_VERIFY_WARNINGS = None
LAST_VERIFY_SUMMARY = ""
LAST_VERIFY_ISSUE_COUNT = 0
SIZE_SUMMARY_AFTER_OVERRIDE = None

spinner_event = threading.Event()
spinner_thread = None

# Set in init_log() via ensure_unified_log_dir(); see docs/LOGGING_LAYOUT.md.
LOG_DIR: Path | None = None
LOG_FILE = ""
VERBOSE_LOG_FILE = ""
SESSION_AUDIT_FILE = ""
SESSION_START_TIME = ""
WATCH_MODE = False
BRANCH_TYPE = "NIGHTLY"

# Pipeline phase gate: handoff preserve may run ONLY in ``post_img_vid``.
_PIPELINE_PHASE = "idle"  # idle | img_vid_running | post_img_vid | adjacent_finalized
_IMG_VID_PIPELINE_RAN = False


@dataclass(frozen=True)
class ProcessorRunResult:
    returncode: int
    succeeded: int
    skipped: int
    ignored: int
    failed: int


def _truthy_env(value: str) -> bool:
    return value.strip().lower() in {"1", "true", "yes", "on"}


def drag_drop_error_mode() -> str:
    fail_fast_legacy = os.environ.get(DRAG_DROP_FAIL_FAST_ENV, "")
    if _truthy_env(fail_fast_legacy):
        return DRAG_DROP_ERROR_MODE_FAIL_FAST

    raw = os.environ.get(DRAG_DROP_ERROR_MODE_ENV, "").strip().lower()
    normalized = raw.replace("_", "-").replace(" ", "-")
    if normalized in {"fail-fast", "failfast", "abort", "strict"}:
        return DRAG_DROP_ERROR_MODE_FAIL_FAST
    if normalized in {
        "",
        "continue",
        "log-and-continue",
        "batch-report",
        "report",
        "normal",
    }:
        return DRAG_DROP_ERROR_MODE_LOG_AND_CONTINUE

    audit_session(
        format_audit_event(
            "DRAG_DROP_ERROR_MODE_UNKNOWN",
            value=raw,
            fallback=DRAG_DROP_ERROR_MODE_LOG_AND_CONTINUE,
        ),
        echo=True,
    )
    return DRAG_DROP_ERROR_MODE_LOG_AND_CONTINUE


def drag_drop_fail_fast_enabled() -> bool:
    return drag_drop_error_mode() == DRAG_DROP_ERROR_MODE_FAIL_FAST


def _ci_rsync_exclude(ext_with_dot: str) -> str:
    """Build a case-insensitive rsync exclude glob for one extension."""
    ext = ext_with_dot.lstrip(".")
    ci = "".join(f"[{c.lower()}{c.upper()}]" if c.isalpha() else c for c in ext)
    return f"--exclude=*.{ci}"


# Threading & Control
stats_lock = threading.Lock()
watch_timer = None
is_processing = False
watch_debounce_seconds = 2.0
active_child_lock = threading.Lock()
active_child_process = None


def _set_active_child_process(proc):
    global active_child_process
    with active_child_lock:
        active_child_process = proc


def _clear_active_child_process(proc):
    global active_child_process
    with active_child_lock:
        if active_child_process is proc:
            active_child_process = None


def _active_child_is_running() -> bool:
    with active_child_lock:
        return active_child_process is not None and active_child_process.poll() is None


def _handle_sigint(signum, frame):
    if _active_child_is_running():
        return
    signal.default_int_handler(signum, frame)


def _handle_sigterm(signum, frame):
    if _active_child_is_running():
        return
    raise SystemExit(128 + signum)


def install_runtime_signal_handlers():
    signal.signal(signal.SIGINT, _handle_sigint)
    signal.signal(signal.SIGTERM, _handle_sigterm)


def reset_runtime_session_state(clear_processing_stats: bool = False):
    global LAST_VERIFY_WARNINGS, LAST_VERIFY_SUMMARY, LAST_VERIFY_ISSUE_COUNT
    global _PIPELINE_PHASE, _IMG_VID_PIPELINE_RAN
    LAST_VERIFY_WARNINGS = None
    LAST_VERIFY_SUMMARY = ""
    LAST_VERIFY_ISSUE_COUNT = 0
    _PIPELINE_PHASE = "idle"
    _IMG_VID_PIPELINE_RAN = False

    if clear_processing_stats:
        global SIZE_SUMMARY_AFTER_OVERRIDE
        SIZE_SUMMARY_AFTER_OVERRIDE = None
        with stats_lock:
            global IMG_SUCCEEDED, IMG_SKIPPED, IMG_IGNORED, IMG_FAILED
            global VID_SUCCEEDED, VID_SKIPPED, VID_IGNORED, VID_FAILED
            IMG_SUCCEEDED = IMG_SKIPPED = IMG_IGNORED = IMG_FAILED = 0
            VID_SUCCEEDED = VID_SKIPPED = VID_IGNORED = VID_FAILED = 0


def hide_cursor():
    sys.stdout.write("\033[?25l")
    sys.stdout.flush()


def resize_terminal(rows=45, cols=223):
    """
    Update terminal window size to wide format for better progress visibility.
    """
    if sys.stdout.isatty():
        sys.stdout.write(f"\033[8;{rows};{cols}t")
        sys.stdout.flush()


def show_cursor():
    sys.stdout.write("\033[?25h")
    sys.stdout.flush()


def clear_screen():
    sys.stdout.write("\033[2J\033[H")
    sys.stdout.flush()


def drain_stdin():
    """Flush stdin buffer to prevent accidental menu triggers"""
    import termios

    try:
        termios.tcflush(sys.stdin.fileno(), termios.TCIFLUSH)
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
        pass


def _fmt_elapsed(t):
    t = max(0, t)
    s = t % 60
    m = (t // 60) % 60
    h = (t // 3600) % 24
    d = (t // 86400) % 7
    w = (t // (7 * 86400)) % 4
    mo = (t // (30 * 86400)) % 12
    y = t // (365 * 86400)

    if y > 0:
        return (
            f"{y:02d}Y   {mo:02d}M   {w:02d}W   {d:02d}D   {h:02d}h  {m:02d}m{s:02d}s"
        )
    if mo > 0:
        return f"{mo:02d}M   {w:02d}W   {d:02d}D   {h:02d}h  {m:02d}m{s:02d}s"
    if w > 0:
        return f"{w:02d}W   {d:02d}D   {h:02d}h  {m:02d}m{s:02d}s"
    if d > 0:
        return f"{d:02d}D   {h:02d}h  {m:02d}m{s:02d}s"
    if h > 0:
        return f"{h:02d}h  {m:02d}m{s:02d}s"
    if m > 0:
        return f"{m:02d}m{s:02d}s"
    return f"{s:02d}s"


def spinner_run():
    start = time.time()
    while not spinner_event.is_set():
        elapsed = int(time.time() - start)
        if sys.stdout.isatty():
            sys.stdout.write(f"\033]0;{_fmt_elapsed(elapsed)}\007")
            sys.stdout.flush()
        time.sleep(0.15)

    elapsed = int(time.time() - start)
    print(f"   Total time: {_fmt_elapsed(elapsed)}")
    if sys.stdout.isatty():
        sys.stdout.write("\033]0;\007")
        sys.stdout.flush()


def start_elapsed_spinner():
    global spinner_thread
    spinner_event.clear()
    spinner_thread = threading.Thread(target=spinner_run, daemon=True)
    spinner_thread.start()


def stop_elapsed_spinner():
    if spinner_thread and spinner_thread.is_alive():
        spinner_event.set()
        spinner_thread.join(timeout=1.0)


def _pty_session_log_includes_progress() -> bool:
    raw = (os.environ.get("MFB_LOG_PTY_PROGRESS") or "").strip().lower()
    return raw in ("1", "true", "yes")


def init_log():
    global SESSION_START_TIME, LOG_FILE, VERBOSE_LOG_FILE, SESSION_AUDIT_FILE
    reset_runtime_session_state(clear_processing_stats=True)
    # Resize terminal to 40x100 (Legacy behavior)
    if sys.stdout.isatty():
        # Set terminal dimensions to wide format (45x223)
        sys.stdout.write("\033[8;45;223t")
        sys.stdout.flush()
        # Lock terminal dimensions for subprocess progress bars (indicatif/console)
        os.environ["COLUMNS"] = "223"
        os.environ["LINES"] = "45"

    global LOG_DIR
    SESSION_START_TIME = format_session_stamp()
    os.environ["MFB_SESSION_ID"] = SESSION_START_TIME
    LOG_DIR = ensure_unified_log_dir()
    # Start with a generic name, we will rename it to the project name later
    LOG_FILE = LOG_DIR / f"MFB_Session_{SESSION_START_TIME}.log"
    VERBOSE_LOG_FILE = LOG_DIR / f"verbose_{SESSION_START_TIME}.log"
    SESSION_AUDIT_FILE = LOG_DIR / f"session_audit_{SESSION_START_TIME}.jsonl"
    os.environ.setdefault("RUST_LOG", "trace")
    append_session_audit(
        format_audit_event(
            "SESSION_STARTED", session_id=SESSION_START_TIME, pid=os.getpid()
        )
    )


def append_session_audit(line: str) -> None:
    """Append one forensic line to verbose + structured session audit logs."""
    stamp = datetime.datetime.now().isoformat(timespec="seconds")
    if VERBOSE_LOG_FILE:
        try:
            with open(VERBOSE_LOG_FILE, "a", encoding="utf-8") as audit_f:
                audit_f.write(f"{stamp} {line}\n")
        except OSError as exc:
            logger.warning(
                "Session audit append failed for %s: %s", VERBOSE_LOG_FILE, exc
            )
    if SESSION_AUDIT_FILE:
        try:
            record = {"ts": stamp, "event": line}
            with open(SESSION_AUDIT_FILE, "a", encoding="utf-8") as audit_f:
                audit_f.write(json.dumps(record, ensure_ascii=False) + "\n")
        except OSError as exc:
            logger.warning(
                "Structured session audit append failed for %s: %s",
                SESSION_AUDIT_FILE,
                exc,
            )


def audit_session(line: str, *, echo: bool = False) -> None:
    """Write session audit line; optionally mirror to terminal for transparency."""
    append_session_audit(line)
    if echo:
        print(f"   {DIM}[session] {line}{RESET}")


def rename_log_to_project():
    """Rename the current session log to include the project/folder name"""
    global LOG_FILE
    if not TARGET_DIR or not LOG_FILE.exists():
        return

    project_name = Path(TARGET_DIR).name
    new_name = LOG_DIR / f"MFB_{project_name}_{SESSION_START_TIME}.log"

    try:
        # If the file is already named correctly, skip
        if LOG_FILE == new_name:
            return

        # Rename the physical file
        os.rename(LOG_FILE, new_name)
        LOG_FILE = new_name
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
        logger.warning(
            "Failed to rename session log from %s to %s: %s", LOG_FILE, new_name, exc
        )
        append_session_audit(
            format_audit_event(
                "LOG_RENAME_FAILED",
                source=str(LOG_FILE),
                dest=str(new_name),
                error=f"{type(exc).__name__}: {exc}",
            )
        )


def get_branch_tag():
    if BRANCH_TYPE == "NIGHTLY":
        return f" {BOLD}{MAGENTA}[NIGHTLY]{RESET}"
    elif BRANCH_TYPE == "MAIN":
        return f" {BOLD}{CYAN}[MAIN]{RESET}"
    return ""


def draw_header():
    width = 70
    tag = get_branch_tag()

    version = "x.x.x"
    try:
        with open(PROJECT_ROOT / "Cargo.toml") as f:
            for line in f:
                if line.startswith("version ="):
                    version = line.split('"')[1]
                    break
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
        pass

    title = f"MODERN FORMAT BOOST v{version}"

    if "Panel" in globals():
        console.print(
            Panel(
                f"[bold #ffffff]{title}[/bold #ffffff]{tag}\n"
                f"[#888888]PREMIUM MEDIA OPTIMIZER[/#888888]\n"
                f"[#00ff00]- [/#00ff00] [#aaaaaa]No Data Loss[/#aaaaaa]   [#00ff00]- [/#00ff00] [#aaaaaa]Smart Conversion[/#aaaaaa]   [#00ff00]- [/#00ff00] [#aaaaaa]Auto-Repair[/#aaaaaa]",
                title=f"[bold {BRAND_BLUE}]Modern Format Boost[/bold {BRAND_BLUE}]",
                subtitle="[dim]Secure & High-Precision Pipeline[/dim]",
                expand=False,
                padding=(0, 4),
                border_style="#444444",
            )
        )
    else:
        padding = (width - len(title)) // 2
        print(f"\n{BLUE}╭{'─' * 70}╮{RESET}")
        print(
            f"{BLUE}│{RESET}{' ' * padding}{BOLD}{WHITE}{title}{RESET}{tag}{' ' * ((width - len(title) - 8) // 2)}{BLUE}│{RESET}"
        )
        print(f"{BLUE}│{'─' * 70}│{RESET}")
        print(
            f"{BLUE}│{RESET}  {DIM}PREMIUM MEDIA OPTIMIZER{RESET}{' ' * (69 - 25)}{BLUE}│{RESET}"
        )
        print(
            f"{BLUE}│{RESET}  {GREEN}-{RESET} {DIM}No Data Loss{RESET}   {GREEN}-{RESET} {DIM}Smart Conversion{RESET}   {GREEN}-{RESET} {DIM}Auto-Repair{' ' * (69 - 58)}{BLUE}│{RESET}"
        )
        print(f"{BLUE}╰{'─' * 70}╯{RESET}")
    print(
        f"   {RED}WARNING: Always keep a backup of your original media before optimization.{RESET}\n"
    )


def _pipeline_needs_binaries() -> bool:
    """True when the current mode will invoke img and/or vid workers."""
    if OUTPUT_MODE in ("fast_img", "fast_vid"):
        return True
    if PROCESSING_MODE in ("both", "images_only") and IMG_COUNT > 0:
        return True
    if PROCESSING_MODE in ("both", "videos_only") and VID_COUNT > 0:
        return True
    return False


def ensure_tools_ready(*, force: bool = False, quiet: bool = False) -> None:
    """Refresh local tools and run smart_build to ensure binaries are up to date."""
    cmd = []
    if shutil.which("rtk"):
        cmd.append("rtk")
    cmd.extend(["cargo", "run", "--release", "--locked", "-p", "dev", "--bin", "smart_build", "--", "--update"])
    if force:
        cmd.append("--force")
    if PROCESSING_MODE == "images_only":
        cmd.append("--img")
    elif PROCESSING_MODE == "videos_only":
        cmd.append("--vid")
    else:
        cmd.append("--all")

    logger.debug(f"Executing build script: {' '.join(cmd)}")
    res = run_delegated(cmd, parent_script="drag_and_drop_processor.py")
    if res.returncode != 0:
        print(f"{RED}ERROR: Build failed. Please check the logs.{RESET}")
        if sys.stdin.isatty():
            try:
                input("Press Enter to exit...")
            except EOFError:
                pass
        sys.exit(1)


def check_tools():
    """Backward-compatible alias; prefer ensure_tools_ready() at processing time."""
    ensure_tools_ready(force=False, quiet=False)


def rebuild_tools():
    """Attempt to rebuild tools automatically"""
    print(f"\n{YELLOW}Attempting automatic rebuild...{RESET}")
    cmd = ["cargo", "run", "--locked", "-p", "dev", "--bin", "smart_build", "--"]
    if PROCESSING_MODE == "images_only":
        cmd.append("--img")
    elif PROCESSING_MODE == "videos_only":
        cmd.append("--vid")

    res = run_delegated(cmd, parent_script="drag_and_drop_processor.py")
    if res.returncode != 0:
        print(f"{RED}ERROR: Automatic rebuild failed. Please check the logs.{RESET}")
        return False

    print(f"{GREEN}OK: Rebuild completed successfully.{RESET}\n")
    return True


def draw_separator(title):
    print(f"{DIM}# {BOLD}{WHITE}{title}{RESET} {DIM}{'#' * 50}{RESET}\n")


def unescape_path(path_str: str) -> str:
    """Handle shell-escaped paths common in terminal drag-and-drop."""
    if not path_str:
        return path_str

    # Remove surrounding quotes
    path_str = path_str.strip("\"'")

    # If the path doesn't exist but has backslashes, it might be escaped
    if "\\" in path_str and not os.path.exists(path_str):
        try:
            import shlex

            # shlex.split handles shell escaping
            parts = shlex.split(path_str)
            if parts:
                return parts[0]
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
            # Fallback: manual replacement for common terminal escapes if shlex fails
            return (
                path_str.replace("\\ ", " ")
                .replace("\\!", "!")
                .replace("\\&", "&")
                .replace("\\(", "(")
                .replace("\\)", ")")
                .replace("\\'", "'")
                .replace('\\"', '"')
            )

    return path_str


def get_target_directory():
    global TARGET_DIR
    if not TARGET_DIR and not os.environ.get("FROM_APP"):
        draw_header()
        print(f"{CYAN}Waiting for input...{RESET}")
        print(f"{DIM}   Please drag and drop a folder here, then press Enter.{RESET}")
        drain_stdin()
        TARGET_DIR = input(f"   {BOLD}> {RESET}").strip()
        TARGET_DIR = unescape_path(TARGET_DIR)

    if "\n" in TARGET_DIR or "\r" in TARGET_DIR:
        print(f"\n{RED}ERROR: Path contains unsupported control characters.{RESET}")
        sys.exit(1)

    p = Path(TARGET_DIR)
    if not p.is_dir():
        print(f"\n{RED}ERROR: Directory not found.{RESET}")
        print(f"{DIM}   Path: {TARGET_DIR}{RESET}")
        sys.exit(1)


def get_unique_output_path(base_path: Path) -> Path:
    """
    If base_path exists, append (1), (2), etc. until a unique path is found.
    """
    if not base_path.exists():
        return base_path

    parent = base_path.parent
    name = base_path.name
    counter = 1
    while True:
        new_path = parent / f"{name} ({counter})"
        if not new_path.exists():
            return new_path
        counter += 1


# Global lock file object to prevent garbage collection and early release
_GLOBAL_LOCK_FILE = None


def acquire_global_lock(dir_path: str):
    """
    Acquire a long-lived exclusive lock on the directory via the MFB central lock system.
    Returns the file handle that must be kept alive.
    """
    global _GLOBAL_LOCK_FILE
    try:
        # 1. Get the unified BLAKE3 hash from Rust
        result = subprocess.run(
            [str(IMGQUALITY_HEVC), "path-hash", dir_path],
            capture_output=True,
            text=True,
            check=True,
        )
        lock_hash = result.stdout.strip()

        # 2. Prepare lock directory
        lock_dir = MFB_STATE_ROOT / "locks"
        lock_dir.mkdir(parents=True, exist_ok=True)

        lock_file_path = lock_dir / f"{lock_hash}.lock"

        # 3. Open and flock
        _GLOBAL_LOCK_FILE = open(lock_file_path, "w")
        import fcntl

        try:
            # Non-blocking exclusive lock
            fcntl.flock(_GLOBAL_LOCK_FILE, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except OSError:
            # Lock is already held
            print(f"\n{RED}ERROR: Directory Already In Use!{RESET}")
            print(f"   Target: {DIM}{dir_path}{RESET}")
            print(
                f"   {YELLOW}Another instance of Modern Format Boost is currently processing this folder.{RESET}"
            )
            print(f"   {DIM}Please wait for the other task to complete.{RESET}")
            sys.exit(3)

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
        # If hash tool fails, we fall back to standard execution which will catch locks later
        logger.warning("Directory lock preflight degraded for %s: %s", dir_path, exc)
        audit_session(
            format_audit_event(
                "DIR_LOCK_PREFLIGHT_DEGRADED",
                target=dir_path,
                error=f"{type(exc).__name__}: {exc}",
            ),
            echo=True,
        )


def check_directory_exclusion(dir_path: str):
    # This is now replaced by the more robust acquire_global_lock
    acquire_global_lock(dir_path)


def safety_check():
    try:
        # Standardize path to avoid bypasses and ensure correct matching
        s = str(Path(TARGET_DIR).resolve())
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
        s = str(TARGET_DIR)

    # System roots: block the directory and all its subdirectories
    system_unsafe = ["/", "/System", "/usr", "/bin", "/sbin"]
    for p in system_unsafe:
        if s == p or s.startswith(p + "/"):
            print(f"\n{RED}{ICO_WARN}  SAFETY BLOCK{RESET}")
            print("   System or root directories cannot be processed directly.")
            sys.exit(1)

    # User roots: block only the directory itself, allow subdirectories
    user_unsafe = [
        str(Path.home()),
        str(Path.home() / "Desktop"),
        str(Path.home() / "Documents"),
    ]
    for p in user_unsafe:
        if s == p:
            print(f"\n{RED}{ICO_WARN}  SAFETY BLOCK{RESET}")
            print(
                "   Common user folders cannot be processed directly. Please process a subdirectory."
            )
            sys.exit(1)


def read_key():
    import termios
    import tty

    fd = sys.stdin.fileno()
    old_settings = termios.tcgetattr(fd)
    try:
        tty.setraw(fd)
        ch = sys.stdin.read(1)
        if ch == "\x1b":
            # Use non-blocking read to instantly capture arrow sequences
            import fcntl

            flags = fcntl.fcntl(fd, fcntl.F_GETFL)
            fcntl.fcntl(fd, fcntl.F_SETFL, flags | os.O_NONBLOCK)
            try:
                next_chars = sys.stdin.read(2)
                ch += next_chars
            except OSError:
                pass
            finally:
                fcntl.fcntl(fd, fcntl.F_SETFL, flags)
        return ch
    finally:
        termios.tcsetattr(fd, termios.TCSADRAIN, old_settings)


def choose_fast_img_action() -> str:
    print(f"\n{GREEN}FAST MODE SELECTED{RESET}")
    print(f"   {BOLD}1{RESET} - {GREEN}Shortest Path (Default){RESET}")
    print(
        f"       {DIM}JXL-only delivery, strict verification, automatic iCloud Photos import, then local JXL folder cleanup.{RESET}"
    )
    print(f"   {BOLD}2{RESET} - {CYAN}Normal Mode{RESET}")
    print(
        f"       {DIM}JXL-only adjacent output; user imports manually. Source JPEGs are still deleted after strict verification.{RESET}"
    )
    print(f"   {BOLD}3{RESET} - {CYAN}Restore to JPEG{RESET}")
    print(
        f"       {DIM}Decode JXL outputs back to adjacent JPEGs with metadata and folder structure preserved.{RESET}"
    )
    drain_stdin()
    try:
        answer = input(
            f"\n   {BOLD}Choose Fast Mode path [1/2/3] ({GREEN}Enter = Shortest Path{RESET}{BOLD}): {RESET}"
        ).strip()
    except EOFError:
        answer = ""
    if answer == "2":
        return "normal"
    if answer == "3":
        return "restore_jpeg"
    return "shortest_path"


def choose_fast_vid_shortest_path() -> bool:
    print(f"\n{GREEN}FAST VIDEO MODE SELECTED{RESET}")
    print(f"   {BOLD}1{RESET} - {GREEN}Shortest Path (Default){RESET}")
    print(
        f"       {DIM}Full LoopIntent video and animated-image delivery through Rust vid run.{RESET}"
    )
    print(f"   {BOLD}2{RESET} - {CYAN}Normal Mode{RESET}")
    print(
        f"       {DIM}Full vid pipeline adjacent output with archive-quality settings.{RESET}"
    )
    drain_stdin()
    try:
        answer = input(
            f"\n   {BOLD}Choose Fast Video path [1/2] ({GREEN}Enter = Shortest Path{RESET}{BOLD}): {RESET}"
        ).strip()
    except EOFError:
        answer = ""
    return answer != "2"


def select_mode():
    global FAST_IMG_ACTION, FAST_IMG_SHORTEST_PATH, OUTPUT_MODE, OUTPUT_DIR
    global FAST_VID_SHORTEST_PATH
    selected = 0
    hide_cursor()

    # We merge the first two modes into one dynamic display item
    # Internal state for the "Mode" item:
    # 0: adjacent, 1: inplace, 2: fast mode (path choice happens after select).
    # Note: when PROCESSING_MODE in ("images_only", "videos_only"), the order is swapped:
    # 0: fast mode, 1: inplace, 2: adjacent.
    fastmode_count = 3 if PROCESSING_MODE in ("images_only", "videos_only") else 2
    if PROCESSING_MODE in ("images_only", "videos_only"):
        if OUTPUT_MODE in ("fast_img", "fast_vid"):
            mode_sub_state = 0
        elif OUTPUT_MODE == "inplace":
            mode_sub_state = 1
        else:
            mode_sub_state = 2  # adjacent
    else:
        mode_sub_state = 0 if OUTPUT_MODE == "adjacent" else 1
    # Internal state for the "Workspace Tools" item (0: Collect, 1: Merge XMP, 2: iCloud Import)
    workspace_sub_state = 0
    # Internal state for the "Maintenance Tools" item (0: Diagnostic Analysis, 1: Cleanup Cache, 2: Database Manager)
    maintenance_sub_state = 0

    options = ["Optimization Mode", "Workspace Tools", "Maintenance Tools"]

    while True:
        clear_screen()
        draw_header()
        print(f"{BOLD}Select Operation Mode:{RESET}\n")

        for i, opt in enumerate(options):
            is_selected = i == selected

            if i == 0:  # Combined Mode Item
                if PROCESSING_MODE in ("images_only", "videos_only"):
                    if mode_sub_state == 0:
                        if PROCESSING_MODE == "videos_only":
                            display_text = "Mode: Fast Video Mode [Tab to Switch]"
                            description = (
                                "Full LoopIntent path for videos and animated images."
                            )
                        else:
                            display_text = "Mode: Fast Image Mode [Tab to Switch]"
                            description = (
                                "JPEG-to-JXL fast path or JXL-to-JPEG restore."
                            )
                    elif mode_sub_state == 1:
                        display_text = "Mode: In-Place Optimization [Tab to Switch]"
                        description = "Replaces original files. Saves disk space."
                    else:  # mode_sub_state == 2
                        display_text = "Mode: Output to Adjacent Folder [Tab to Switch]"
                        description = "Safe mode. Keeps originals untouched."
                else:
                    if mode_sub_state == 0:
                        display_text = "Mode: Output to Adjacent Folder [Tab to Switch]"
                        description = "Safe mode. Keeps originals untouched."
                    else:  # mode_sub_state == 1
                        display_text = "Mode: In-Place Optimization [Tab to Switch]"
                        description = "Replaces original files. Saves disk space."

                if is_selected:
                    if "Console" in globals():
                        console.print(
                            f"  [bold {BRAND_BLUE}]➜[/bold {BRAND_BLUE}] [reverse {BRAND_BLUE}] {display_text} [/reverse {BRAND_BLUE}]"
                        )
                        console.print(f"     [#00ccff]{description}[/#00ccff]\n")
                    else:
                        print(f"  {CYAN}➜ {BOLD}{display_text}{RESET}")
                        print(f"    {CYAN}{DIM}{description}{RESET}\n")
                else:
                    if "Console" in globals():
                        console.print(f"     [dim]○ {display_text}[/dim]")
                        console.print(f"     [dim]{description}[/dim]\n")
                    else:
                        print(f"    {DIM}- {display_text}{RESET}")
                        print(f"    {DIM}{description}{RESET}\n")
            elif i == 1:  # Workspace Tools item
                if workspace_sub_state == 0:
                    display_text = "Tool: Collect Optimized Media [Tab to Switch]"
                    desc = "Move optimized outputs into a mirrored directory tree."
                elif workspace_sub_state == 1:
                    display_text = "Tool: Merge XMP Attachments [Tab to Switch]"
                    desc = "Automatically embed XMP sidecars into source media files safely."
                else:
                    display_text = "Tool: iCloud Photo Import [Tab to Switch]"
                    desc = "Import processed assets into iCloud using osxphotos (Auto-Album)."

                if is_selected:
                    if "Console" in globals():
                        console.print(
                            f"  [bold {BRAND_BLUE}]➜[/bold {BRAND_BLUE}] [reverse {BRAND_BLUE}] {display_text} [/reverse {BRAND_BLUE}]"
                        )
                        console.print(f"     [#00ccff]{desc}[/#00ccff]\n")
                    else:
                        print(f"  {CYAN}➜ {BOLD}{display_text}{RESET}")
                        print(f"    {CYAN}{DIM}{desc}{RESET}\n")
                else:
                    if "Console" in globals():
                        console.print(f"     [dim]○ {display_text}[/dim]")
                        console.print(f"     [dim]{desc}[/dim]\n")
                    else:
                        print(f"    {DIM}○ {display_text}{RESET}")
                        print(f"    {DIM}{desc}{RESET}\n")
            else:  # Maintenance Tools item (i == 2)
                if maintenance_sub_state == 0:
                    display_text = "Tool: Diagnostic Analysis [Tab to Switch]"
                    desc = "Analyze logs for edge cases and verify output integrity."
                elif maintenance_sub_state == 1:
                    display_text = "Tool: Cleanup Cache & Logs [Tab to Switch]"
                    desc = "Clear analysis cache, session logs, and ALL task progress."
                else:
                    display_text = "Tool: Database Manager [Tab to Switch]"
                    desc = "Clean, train, backup and manage database with interactive menu."

                if is_selected:
                    if "Console" in globals():
                        console.print(
                            f"  [bold {BRAND_BLUE}]➜[/bold {BRAND_BLUE}] [reverse {BRAND_BLUE}] {display_text} [/reverse {BRAND_BLUE}]"
                        )
                        console.print(f"     [#00ccff]{desc}[/#00ccff]\n")
                    else:
                        print(f"  {CYAN}➜ {BOLD}{display_text}{RESET}")
                        print(f"    {CYAN}{DIM}{desc}{RESET}\n")
                else:
                    if "Console" in globals():
                        console.print(f"     [dim]○ {display_text}[/dim]")
                        console.print(f"     [dim]{desc}[/dim]\n")
                    else:
                        print(f"    {DIM}○ {display_text}{RESET}")
                        print(f"    {DIM}{desc}{RESET}\n")

        print(
            f"{DIM}(Use ↑/↓ to navigate, Tab to toggle mode, Enter to select, q to quit){RESET}"
        )

        if sys.stdin.isatty():
            key = read_key()
            if key in ("\x1b[A", "\x1b[D"):  # Up / Left
                selected = (selected - 1) % len(options)
            elif key in ("\x1b[B", "\x1b[C"):  # Down / Right
                selected = (selected + 1) % len(options)
            elif key == "\t":  # Tab
                if selected == 0:
                    mode_sub_state = (mode_sub_state + 1) % fastmode_count
                elif selected == 1:
                    workspace_sub_state = (workspace_sub_state + 1) % 3
                elif selected == 2:
                    maintenance_sub_state = (maintenance_sub_state + 1) % 3
            elif key in ("\r", "\n"):
                # Action based on selection
                if selected == 0:
                    actual_state = mode_sub_state
                    if PROCESSING_MODE in ("images_only", "videos_only"):
                        if mode_sub_state == 0:
                            actual_state = 2
                        elif mode_sub_state == 2:
                            actual_state = 0

                    if actual_state == 0:
                        OUTPUT_MODE = "adjacent"
                        tdir = Path(TARGET_DIR).resolve()
                        OUTPUT_DIR = str(
                            get_unique_output_path(
                                tdir.parent / (tdir.name + "_optimized")
                            )
                        )
                        print(f"\n{GREEN}ADJACENT MODE SELECTED{RESET}")
                        print(f"   Output: {DIM}{OUTPUT_DIR}{RESET}")
                        print(f"   {DIM}Creating directory structure...{RESET}")
                        create_directory_structure(TARGET_DIR, OUTPUT_DIR)
                        show_cursor()
                        break  # Exit select_mode and start processing
                    elif actual_state == 1:
                        OUTPUT_MODE = "inplace"
                        print(f"\n{RED}WARNING: IN-PLACE OPTIMIZATION SELECTED{RESET}")
                        print(
                            f"{BOLD}{WHITE}   Original files will be replaced after successful conversion.{RESET}"
                        )
                        print(
                            f"{YELLOW}   This action is irreversible if you don't have backups.{RESET}\n"
                        )
                        drain_stdin()
                        confirm = input(
                            f"   {BOLD}To proceed, type {RED}'yes'{RESET}{BOLD} (case-sensitive) and press Enter: {RESET}"
                        )
                        if confirm != "yes":
                            print(
                                f"\n{RED}ERROR: In-place optimization cancelled. Incorrect confirmation.{RESET}"
                            )
                            print(
                                f"{DIM}   Returning to main menu in 3 seconds...{RESET}"
                            )
                            time.sleep(3)
                            continue  # Redraw menu
                        else:
                            acquire_global_lock(TARGET_DIR)
                            show_cursor()
                            break  # Confirmed, start processing
                    else:
                        tdir = Path(TARGET_DIR).resolve()
                        if PROCESSING_MODE == "videos_only":
                            OUTPUT_MODE = "fast_vid"
                            FAST_VID_SHORTEST_PATH = choose_fast_vid_shortest_path()
                            OUTPUT_DIR = str(fast_vid_output_dir_for_target(tdir))
                            mode_label = (
                                "FAST VIDEO SHORTEST PATH SELECTED"
                                if FAST_VID_SHORTEST_PATH
                                else "FAST VIDEO MODE SELECTED"
                            )
                            print(f"\n{GREEN}{mode_label}{RESET}")
                            print(f"   Output: {DIM}{OUTPUT_DIR}{RESET}")
                            print(
                                f"   {YELLOW}Video and animated-image assets will use the full LoopIntent vid pipeline.{RESET}"
                            )
                            if FAST_VID_SHORTEST_PATH:
                                print(
                                    f"   {YELLOW}Shortest-path import is disabled until full vid-run import proof is shared-core verified.{RESET}"
                                )
                            print(
                                f"   {DIM}Delegating to Rust vid run pipeline...{RESET}"
                            )
                        else:
                            OUTPUT_MODE = "fast_img"
                            FAST_IMG_ACTION = choose_fast_img_action()
                            FAST_IMG_SHORTEST_PATH = FAST_IMG_ACTION == "shortest_path"
                            if FAST_IMG_ACTION == "restore_jpeg":
                                OUTPUT_DIR = str(
                                    fast_img_restore_output_dir_for_target(tdir)
                                )
                                print(f"\n{GREEN}RESTORE TO JPEG SELECTED{RESET}")
                                print(f"   Output: {DIM}{OUTPUT_DIR}{RESET}")
                                print(
                                    f"   {DIM}Delegating to Rust restore-jpeg pipeline...{RESET}"
                                )
                            else:
                                OUTPUT_DIR = str(
                                    fast_img_output_dir_for_target(
                                        tdir,
                                        has_resume_marker=(
                                            lambda candidate: (
                                                load_fast_img_marker_for_optimized(
                                                    candidate
                                                )[0]
                                                is not None
                                            )
                                        ),
                                    )
                                )
                                mode_label = (
                                    "FAST MODE SHORTEST PATH SELECTED"
                                    if FAST_IMG_SHORTEST_PATH
                                    else "FAST MODE SELECTED"
                                )
                                print(f"\n{GREEN}{mode_label}{RESET}")
                                print(f"   Output: {DIM}{OUTPUT_DIR}{RESET}")
                                print(
                                    f"   {YELLOW}Original JPEG files will be deleted after strict verification.{RESET}"
                                )
                                if FAST_IMG_SHORTEST_PATH:
                                    print(
                                        f"   {YELLOW}Verified JXL outputs will be imported to iCloud Photos automatically.{RESET}"
                                    )
                                print(
                                    f"   {DIM}Delegating to Rust fast-img pipeline...{RESET}"
                                )
                        show_cursor()
                        break  # Exit select_mode and let main intercept
                elif selected == 1:
                    if workspace_sub_state == 0:
                        OUTPUT_MODE = "collect"
                        tdir = Path(TARGET_DIR).resolve()
                        OUTPUT_DIR = str(
                            get_unique_output_path(
                                tdir.parent / (tdir.name + "_collected")
                            )
                        )
                        print(f"\n{GREEN}COLLECT OPTIMIZED MEDIA SELECTED{RESET}")
                        print(f"   Source: {DIM}{TARGET_DIR}{RESET}")
                        print(f"   Output: {DIM}{OUTPUT_DIR}{RESET}\n")
                        drain_stdin()
                        collect_env = os.environ.copy()
                        if VERBOSE_LOG_FILE:
                            collect_env["MFB_SESSION_AUDIT"] = str(
                                Path(VERBOSE_LOG_FILE).resolve()
                            )
                        subprocess.run(
                            [
                                "cargo",
                                "run",
                                "--locked",
                                "-p",
                                "dev",
                                "--bin",
                                "drag_and_drop_processor",
                                "--",
                                "--mode",
                                "collect",
                                "--output",
                                OUTPUT_DIR,
                                str(TARGET_DIR),
                            ],
                            cwd=PROJECT_ROOT,
                            env=collect_env,
                        )
                        drain_stdin()
                        try:
                            input(
                                f"\n   {CYAN}Press Enter to return to menu... {RESET}"
                            )
                        except EOFError:
                            pass
                        continue
                    elif workspace_sub_state == 1:
                        OUTPUT_MODE = "merge_xmp"
                        print(f"\n{GREEN}MERGE XMP ATTACHMENTS SELECTED{RESET}")
                        print(f"   Source: {DIM}{TARGET_DIR}{RESET}\n")
                        drain_stdin()
                        subprocess.run(
                            [
                                "cargo",
                                "run",
                                "--locked",
                                "-p",
                                "dev",
                                "--bin",
                                "drag_and_drop_processor",
                                "--",
                                "--mode",
                                "merge-xmp",
                                str(TARGET_DIR),
                            ],
                            cwd=PROJECT_ROOT,
                        )
                        drain_stdin()
                        try:
                            input(
                                f"\n   {CYAN}Press Enter to return to menu... {RESET}"
                            )
                        except EOFError:
                            pass
                        continue
                    else:  # workspace_sub_state == 2
                        OUTPUT_MODE = "icloud_import"
                        print(f"\n{GREEN}ICLOUD PHOTO IMPORT SELECTED{RESET}")
                        print(f"   Target: {DIM}{TARGET_DIR}{RESET}\n")
                        drain_stdin()
                        subprocess.run(
                            [
                                "cargo",
                                "run",
                                "--locked",
                                "-p",
                                "dev",
                                "--bin",
                                "drag_and_drop_processor",
                                "--",
                                "--mode",
                                "icloud-import",
                                str(TARGET_DIR),
                            ],
                            cwd=PROJECT_ROOT,
                            check=False,
                        )
                        drain_stdin()
                        try:
                            input(
                                f"\n   {CYAN}Press Enter to return to menu...{RESET} "
                            )
                        except EOFError:
                            pass
                        continue
                elif selected == 2:
                    if maintenance_sub_state == 0:
                        OUTPUT_MODE = "diagnostic_analysis"
                        print(f"\n{GREEN}DIAGNOSTIC ANALYSIS SELECTED{RESET}")
                        print(f"   Target: {DIM}{TARGET_DIR}{RESET}")
                        print(f"   Logs:   {DIM}{LOG_DIR}{RESET}\n")

                        # Manual diagnostic mode: run the unified verifier with logs.
                        drain_stdin()
                        subprocess.run(
                            [
                                "cargo",
                                "run",
                                "--locked",
                                "-p",
                                "dev",
                                "--bin",
                                "drag_and_drop_processor",
                                "--",
                                "--mode",
                                "diagnostic",
                                str(TARGET_DIR),
                            ],
                            cwd=PROJECT_ROOT,
                            check=False,
                        )

                        drain_stdin()
                        try:
                            input(
                                f"\n   {CYAN}Press Enter to return to menu... {RESET}"
                            )
                        except EOFError:
                            pass
                        continue
                    elif maintenance_sub_state == 1:
                        OUTPUT_MODE = "cache_clean"
                        # Single confirmation lives in cache_cleaner.perform_full_cleanup (not here).
                        drain_stdin()
                        clean_res = subprocess.run(
                            [
                                "cargo",
                                "run",
                                "--locked",
                                "-p",
                                "dev",
                                "--bin",
                                "drag_and_drop_processor",
                                "--",
                                "--mode",
                                "cache-clean",
                                str(TARGET_DIR),
                            ],
                            cwd=PROJECT_ROOT,
                            check=False,
                        )
                        if clean_res.returncode != 0:
                            print(
                                f"\n{RED}{ICO_ERR} Cleanup or rebuild failed (exit {clean_res.returncode}).{RESET}"
                            )
                            print(
                                f"{YELLOW}   Manual rebuild: cargo run --locked -p dev --bin smart_build -- --force{RESET}\n"
                            )
                        else:
                            print(f"\n{GREEN}{ICO_OK} Cleanup complete.{RESET}")
                            # Belt-and-suspenders: ensure binaries if rebuild was skipped.
                            if (
                                not IMGQUALITY_HEVC.is_file()
                                or not VIDQUALITY_HEVC.is_file()
                            ):
                                print(
                                    f"{YELLOW}   img/vid missing — running smart_build...{RESET}"
                                )
                                ensure_tools_ready(force=True, quiet=False)
                        reset_runtime_session_state(clear_processing_stats=True)
                        drain_stdin()
                        try:
                            input(
                                f"\n   {CYAN}Press Enter to return to menu... {RESET}"
                            )
                        except EOFError:
                            pass
                        continue
                    else:  # maintenance_sub_state == 2
                        OUTPUT_MODE = "database_manager"
                        print(f"\n{GREEN}DATABASE MANAGER SELECTED{RESET}\n")
                        drain_stdin()
                        run_delegated(
                            [
                                "cargo",
                                "run",
                                "--locked",
                                "-p",
                                "dev",
                                "--bin",
                                "drag_and_drop_processor",
                                "--",
                                "--mode",
                                "database-manager",
                                str(TARGET_DIR),
                            ],
                            parent_script="drag_and_drop_processor.py",
                            check=False,
                        )
                        drain_stdin()
                        try:
                            input(
                                f"\n   {CYAN}Press Enter to return to menu...{RESET} "
                            )
                        except EOFError:
                            pass
                        continue
            elif key.lower() == "q":
                show_cursor()
                sys.exit(0)
        else:
            # Non-interactive mode: Default to adjacent if not set
            if OUTPUT_MODE == "adjacent" and not OUTPUT_DIR:
                tdir = Path(TARGET_DIR).resolve()
                OUTPUT_DIR = str(
                    get_unique_output_path(tdir.parent / (tdir.name + "_optimized"))
                )
                print(f"\n{GREEN}NON-INTERACTIVE MODE: ADJACENT SELECTED{RESET}")
                print(f"   Output: {DIM}{OUTPUT_DIR}{RESET}")
                create_directory_structure(TARGET_DIR, OUTPUT_DIR)
            show_cursor()
            break


def create_directory_structure(src, dest):
    """Create directory structure and preserve timestamps"""
    src_path = Path(src)
    dest_path = Path(dest)
    dest_path.mkdir(parents=True, exist_ok=True)
    try:
        shutil.copystat(src_path, dest_path)
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
        logger.warning(
            "Failed to preserve directory metadata from %s to %s: %s",
            src_path,
            dest_path,
            exc,
        )
        append_session_audit(
            format_audit_event(
                "DIR_METADATA_CLONE_DEGRADED",
                source=str(src_path),
                dest=str(dest_path),
                error=f"{type(exc).__name__}: {exc}",
            )
        )

    for root, dirs, _ in os.walk(src):
        for d in dirs:
            src_dir = Path(root) / d
            rel = src_dir.relative_to(src_path)
            dest_dir = dest_path / rel
            dest_dir.mkdir(parents=True, exist_ok=True)
            try:
                shutil.copystat(src_dir, dest_dir)
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
                logger.warning(
                    "Failed to preserve directory metadata from %s to %s: %s",
                    src_dir,
                    dest_dir,
                    exc,
                )
                append_session_audit(
                    format_audit_event(
                        "DIR_METADATA_CLONE_DEGRADED",
                        source=str(src_dir),
                        dest=str(dest_dir),
                        error=f"{type(exc).__name__}: {exc}",
                    )
                )


IMG_COUNT = 0
VID_COUNT = 0
MEDIA_TOTAL_SIZE = 0


def count_files():
    global IMG_COUNT, VID_COUNT, MEDIA_TOTAL_SIZE
    global ROUTED_IMAGE_REL_PATHS, ROUTED_VIDEO_REL_PATHS
    draw_separator("Scanning Content")
    print(f"{DIM}   Analyzing directory structure...{RESET}")

    total, img, vid, xmp, other, media_size = 0, 0, 0, 0, 0, 0
    routed_image_rel_paths = set()
    routed_video_rel_paths = set()
    for root, _, files in os.walk(TARGET_DIR):
        for file in files:
            if file.startswith("."):
                continue
            total += 1
            p = Path(root) / file
            ext = p.suffix.lower()
            try:
                route = classify_media_owner(p)
            except (OSError, MediaProbeError) as exc:
                append_session_audit(
                    format_audit_event(
                        "ROUTING_PROBE_FAILED",
                        path=os.path.relpath(p, TARGET_DIR),
                        error=f"{type(exc).__name__}: {exc}",
                    )
                )
                route = None
            is_img = route == PIPELINE_IMAGE
            is_vid = route == PIPELINE_VIDEO

            if is_img:
                img += 1
                routed_image_rel_paths.add(os.path.relpath(p, TARGET_DIR))
            if is_vid:
                vid += 1
                routed_video_rel_paths.add(os.path.relpath(p, TARGET_DIR))
            if ext == ".xmp":
                xmp += 1
            elif not is_img and not is_vid:
                other += 1

            if ext in MEDIA_EXTS and (
                PROCESSING_MODE == "both"
                or (PROCESSING_MODE == "images_only" and is_img)
                or (PROCESSING_MODE == "videos_only" and is_vid)
            ):
                try:
                    media_size += p.stat().st_size
                except OSError:
                    pass

    # Apply filtering based on PROCESSING_MODE
    if PROCESSING_MODE == "images_only":
        vid = 0
    elif PROCESSING_MODE == "videos_only":
        img = 0

    # Lock ONLY for the final state update to avoid blocking file detection threads
    with stats_lock:
        IMG_COUNT, VID_COUNT, MEDIA_TOTAL_SIZE = img, vid, media_size
        ROUTED_IMAGE_REL_PATHS = routed_image_rel_paths
        ROUTED_VIDEO_REL_PATHS = routed_video_rel_paths

    print(f"   {ICO_DIR} Total Files: {BOLD}{total}{RESET}")
    print(f"   {ICO_IMG}  Images:      {BOLD}{CYAN}{img}{RESET}")
    print(f"   {ICO_VID} Videos:      {BOLD}{MAGENTA}{vid}{RESET}")
    print(f"   {ICO_CLIP} Metadata:    {BOLD}{DIM}{xmp}{RESET}")
    print(f"   {ICO_PKG} Others:      {BOLD}{DIM}{other}{RESET} (Copy only)\n")

    append_session_audit(
        f"ROUTING_SUMMARY images={len(ROUTED_IMAGE_REL_PATHS)} videos={len(ROUTED_VIDEO_REL_PATHS)} mode={PROCESSING_MODE}"
    )
    for rel_path in sorted(ROUTED_IMAGE_REL_PATHS):
        append_session_audit(format_session_audit_routed(PIPELINE_IMAGE, rel_path))
    for rel_path in sorted(ROUTED_VIDEO_REL_PATHS):
        append_session_audit(format_session_audit_routed(PIPELINE_VIDEO, rel_path))


def _signed_bytes_label(diff_bytes: int) -> str:
    if diff_bytes < 0:
        return f"-{format_bytes(abs(diff_bytes))}"
    if diff_bytes > 0:
        return f"+{format_bytes(diff_bytes)}"
    return format_bytes(0)


def _signed_percent_label(value: float | None) -> str:
    if value is None:
        return "N/A"
    if value >= 0:
        return f"+{value:.1f}%"
    return f"{value:.1f}%"


def _stat_size_or_audit(path: Path) -> int:
    try:
        return path.stat().st_size
    except OSError as exc:
        append_session_audit(
            format_audit_event(
                "SIZE_SUMMARY_STAT_FAILED",
                path=str(path),
                error=f"{type(exc).__name__}: {exc}",
            )
        )
        return 0


def processing_mode_accepts_route(route: str | None) -> bool:
    if route == PIPELINE_IMAGE:
        return PROCESSING_MODE in ("both", "images_only")
    if route == PIPELINE_VIDEO:
        return PROCESSING_MODE in ("both", "videos_only")
    return False


def selected_media_tree_size(root: Path) -> int:
    if root.is_file():
        try:
            route = classify_media_owner(root)
        except (OSError, MediaProbeError) as exc:
            append_session_audit(
                format_audit_event(
                    "SIZE_SUMMARY_CLASSIFY_FAILED",
                    path=str(root),
                    error=f"{type(exc).__name__}: {exc}",
                )
            )
            return 0
        return _stat_size_or_audit(root) if processing_mode_accepts_route(route) else 0

    if not root.is_dir():
        return 0

    total = 0
    for path in root.rglob("*"):
        if not path.is_file() or path.name.startswith("."):
            continue
        try:
            route = classify_media_owner(path)
        except (OSError, MediaProbeError) as exc:
            append_session_audit(
                format_audit_event(
                    "SIZE_SUMMARY_CLASSIFY_FAILED",
                    path=str(path),
                    error=f"{type(exc).__name__}: {exc}",
                )
            )
            continue
        if processing_mode_accepts_route(route):
            total += _stat_size_or_audit(path)
    return total


def size_change_percent(before_bytes: int, after_bytes: int) -> float | None:
    if before_bytes <= 0:
        return None
    return ((after_bytes / before_bytes) - 1.0) * 100.0


def build_size_comparison_summary(
    *,
    before_bytes: int,
    after_bytes: int,
    operation_mode: str,
    processing_type: str,
) -> str:
    diff_bytes = after_bytes - before_bytes
    change_pct = size_change_percent(before_bytes, after_bytes)
    return "\n".join(
        [
            f"{ICO_STATS} Before/After Size Comparison",
            f"   Operation Mode:  {operation_mode}",
            f"   Processing Type: {processing_type}",
            f"   Total Before:    {format_bytes(before_bytes)}",
            f"   Total After:     {format_bytes(after_bytes)}",
            f"   Difference:      {_signed_bytes_label(diff_bytes)}",
            f"   Change:          {_signed_percent_label(change_pct)}",
        ]
    )


def output_size_for_summary() -> int:
    if SIZE_SUMMARY_AFTER_OVERRIDE is not None:
        return SIZE_SUMMARY_AFTER_OVERRIDE
    if OUTPUT_MODE == "inplace":
        return selected_media_tree_size(Path(TARGET_DIR)) if TARGET_DIR else 0
    if OUTPUT_MODE == "fast_img" and FAST_IMG_OUTPUT_CLEANED:
        return 0
    if not OUTPUT_DIR:
        return 0
    return selected_media_tree_size(Path(OUTPUT_DIR))


def operation_mode_label() -> str:
    if OUTPUT_MODE in ("fast_img", "fast_vid"):
        return "fastmode"
    if OUTPUT_MODE == "inplace":
        return "every"
    return "normal"


def processing_type_label() -> str:
    if PROCESSING_MODE == "images_only":
        return "img"
    if PROCESSING_MODE == "videos_only":
        return "vid"
    return "both"


def final_size_comparison_summary() -> str:
    return build_size_comparison_summary(
        before_bytes=MEDIA_TOTAL_SIZE,
        after_bytes=output_size_for_summary(),
        operation_mode=operation_mode_label(),
        processing_type=processing_type_label(),
    )


def snapshot_selected_media_size() -> int:
    total = 0
    if not TARGET_DIR:
        return total
    target = Path(TARGET_DIR)
    rel_paths = set()
    if PROCESSING_MODE in ("both", "images_only"):
        rel_paths.update(ROUTED_IMAGE_REL_PATHS)
    if PROCESSING_MODE in ("both", "videos_only"):
        rel_paths.update(ROUTED_VIDEO_REL_PATHS)
    if rel_paths:
        for rel_path in rel_paths:
            path = target / rel_path
            if path.is_file():
                total += _stat_size_or_audit(path)
        return total
    return selected_media_tree_size(target)


def check_system_resources(check_dir):
    """Safety checks for disk space, memory, and CPU load"""
    try:
        if "psutil" in globals():
            # Detailed Disk Check
            usage = psutil.disk_usage(check_dir)
            free_gb = usage.free / (1024**3)
            required_gb = (MEDIA_TOTAL_SIZE / (1024**3)) + 1.0  # Buffer

            if free_gb < required_gb:
                console.print(
                    f"[bold red]{ICO_ERR} Error: Insufficient disk space on {check_dir}[/bold red]"
                )
                console.print(
                    f"   Available: {free_gb:.2f} GB, Required: {required_gb:.2f} GB"
                )
                print(f"\n{YELLOW}   Returning to home menu in 5 seconds...{RESET}")
                time.sleep(5)
                raise ReturnToHomeException()

            # Memory Check
            mem = psutil.virtual_memory()
            if mem.percent > 95:
                console.print(
                    f"[bold yellow]{ICO_WARN}  Caution: System memory is very low ({mem.percent}% used).[/bold yellow]"
                )
                print(f"\n{YELLOW}   Returning to home menu in 5 seconds...{RESET}")
                time.sleep(5)
                raise ReturnToHomeException()

            # CPU Check
            cpu = psutil.cpu_percent(interval=0.1)
            if cpu > 90:
                console.print(
                    f"[bold yellow]{ICO_WARN}  Notice: System CPU usage is high ({cpu}%). Processing may take longer.[/bold yellow]"
                )
        else:
            # Fallback
            free = shutil.disk_usage(check_dir).free
            required = MEDIA_TOTAL_SIZE + 1024**3
            if free < required:
                print(f"{RED}{ICO_ERR} Insufficient disk space{RESET}")
                print(f"\n{YELLOW}   Returning to home menu in 5 seconds...{RESET}")
                time.sleep(5)
                raise ReturnToHomeException()

        os.environ["MFB_SKIP_DISK_PRECHECK"] = "1"
    except ReturnToHomeException:
        raise
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
        logger.warning("Disk/resource precheck degraded for %s: %s", check_dir, exc)
        audit_session(
            format_audit_event(
                "DISK_PRECHECK_DEGRADED",
                target=str(check_dir),
                error=f"{type(exc).__name__}: {exc}",
            ),
            echo=True,
        )


def parse_processor_stats(
    output: str,
    *,
    parse_type: str,
    restore_jpeg: bool = False,
) -> tuple[int, int, int, int]:
    if restore_jpeg:
        restore_matches = re.findall(
            r"\brestored\s+(\d+)\s+JPEGs?\s+to\s+.+?\((\d+)\s+existing outputs skipped\)",
            output,
            flags=re.IGNORECASE,
        )
        if restore_matches:
            restored, skipped = restore_matches[-1]
            return int(restored), int(skipped), 0, 0

    succ = re.findall(r"Succeeded:\s*(\d+)", output)
    skip = re.findall(r"Skipped:\s*(\d+)", output)
    ign = re.findall(r"Ignored:\s*(\d+)", output)
    fail = re.findall(r"Failed:\s*(\d+)", output)

    s_val = int(succ[-1]) if succ else 0
    sk_val = int(skip[-1]) if skip else 0
    ig_val = int(ign[-1]) if ign else 0
    f_val = int(fail[-1]) if fail else 0
    return s_val, sk_val, ig_val, f_val


def stream_and_log_process(
    cmd,
    parse_type,
    *,
    restore_jpeg: bool = False,
    fail_fast: bool | None = None,
):
    tmp_out = ""
    pipeline_tag = "IMG" if parse_type == "img" else "VID"
    exit_on_error = drag_drop_fail_fast_enabled() if fail_fast is None else fail_fast
    audit_session(
        format_audit_event(
            f"{pipeline_tag}_PIPELINE_SPAWN",
            cmd=" ".join(str(c) for c in cmd),
            error_mode=(
                DRAG_DROP_ERROR_MODE_FAIL_FAST
                if exit_on_error
                else DRAG_DROP_ERROR_MODE_LOG_AND_CONTINUE
            ),
        ),
        echo=True,
    )

    # Create a Pseudo-Terminal pair (Master/Slave)
    # This tricks the Rust binary into thinking it's on a real TTY,
    # forcing its most optimized, high-frequency "indicatif" rendering mode.
    # Resource Management: Ensure PTY descriptors are strictly contained
    master_fd, slave_fd = pty.openpty()

    # Sync PTY size with the actual terminal size so indicatif's progress bars wrap correctly
    # and do not clear lines above them (e.g. the summary reports).
    try:
        import fcntl
        import struct
        import termios

        cols, rows = os.get_terminal_size()
        fcntl.ioctl(slave_fd, termios.TIOCSWINSZ, struct.pack("HHHH", rows, cols, 0, 0))
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
        pass

    lf = None
    res = None

    try:
        # Launch process using the PTY slave as stdout/stderr
        # We use a try-finally block here specifically to ensure slave_fd is closed even if Popen fails
        try:
            res = subprocess.Popen(
                cmd, stdout=slave_fd, stderr=slave_fd, env=os.environ.copy()
            )
            _set_active_child_process(res)
        finally:
            # Close the slave in the master process now that it's passed to the child (or Popen failed)
            try:
                os.close(slave_fd)
            except OSError:
                pass

        if res is None:
            raise RuntimeError("Process creation failed without raising an exception.")

        # Ensure log directory exists at the moment of opening to prevent race conditions
        if LOG_DIR:
            try:
                LOG_DIR.mkdir(parents=True, exist_ok=True)
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
                if VERBOSE_MODE:
                    print(
                        f"   {YELLOW}{ICO_WARN}  Warning: Failed to create log directory: {e}{RESET}"
                    )

        try:
            # Use absolute path to ensure reliability across directory context shifts
            log_path = Path(LOG_FILE).absolute()
            lf = open(log_path, "ab")
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
            # Graceful fallback: if logging fails, process still continues but without persistent log
            lf = None
            if VERBOSE_MODE:
                print(
                    f"   {YELLOW}{ICO_WARN}  Warning: Logging suspended for this task (Reason: {e}){RESET}"
                )

        try:
            # Set master PTY to non-blocking for ultra-smooth capture
            os.set_blocking(master_fd, False)
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
            # Fallback to blocking if OS doesn't support non-blocking PTY
            pass

        log_buffer = ""
        last_heartbeat = time.monotonic()
        try:
            while True:
                if time.monotonic() - last_heartbeat >= 60.0:
                    append_session_audit(
                        format_audit_event(
                            "SESSION_HEARTBEAT",
                            pipeline=pipeline_tag,
                            pid=res.pid if res else 0,
                        )
                    )
                    last_heartbeat = time.monotonic()
                try:
                    # Kernel-level read from PTY Master
                    chunk = os.read(
                        master_fd, 16384
                    )  # Larger buffer for high-throughput TTY data
                except InterruptedError:
                    if res.poll() is not None:
                        break
                    continue
                except BlockingIOError:
                    if res.poll() is not None:
                        break
                    time.sleep(0.001)  # Sub-millisecond cycle
                    continue
                except OSError:
                    # Often happens when the child closes the slave side
                    break

                if not chunk:
                    if res.poll() is not None:
                        break
                    time.sleep(0.001)
                    continue

                # 1:1 Relay to screen (native speed, zero buffering)
                try:
                    os.write(sys.stdout.fileno(), chunk)
                    sys.stdout.flush()
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
                    pass

                # Record for stats parsing
                try:
                    s = chunk.decode("utf-8", errors="ignore")
                    tmp_out += s
                    # Prevent Memory Leak for long processes
                    if len(tmp_out) > 50000:
                        tmp_out = tmp_out[-50000:]

                    if lf:
                        log_buffer += s
                        if "\n" in log_buffer:
                            lines = log_buffer.split("\n")
                            log_buffer = lines.pop()
                            for line in lines:
                                if "\r" in line:
                                    line = line.rsplit("\r", 1)[-1]

                                # Remove ANSI escape sequences
                                clean_line = re.sub(
                                    r"\x1B(?:[@-Z\\-_]|\[[0-?]*[ -/]*[@-~])", "", line
                                )

                                if not clean_line.strip():
                                    continue

                                # Filter out common progress bar characters
                                progress_chars = [
                                    "█",
                                    "▓",
                                    "▒",
                                    "░",
                                    "▏",
                                    "▕",
                                    "⏱️",
                                    "ETA",
                                ]
                                if not _pty_session_log_includes_progress() and any(
                                    c in clean_line for c in progress_chars
                                ):
                                    continue

                                lf.write((clean_line + "\n").encode("utf-8"))
                                lf.flush()
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
                    pass
        finally:
            # Cleanup for the internal read loop
            pass

    finally:
        # Final cleanup: ensure process is untracked and all files/PTYs are closed
        if res:
            _clear_active_child_process(res)

        if lf:
            try:
                # Flush final buffer if any
                if log_buffer.strip():
                    if "\r" in log_buffer:
                        log_buffer = log_buffer.rsplit("\r", 1)[-1]
                    clean_line = re.sub(
                        r"\x1B(?:[@-Z\\-_]|\[[0-?]*[ -/]*[@-~])", "", log_buffer
                    )
                    if clean_line.strip():
                        progress_chars = ["█", "▓", "▒", "░", "▏", "▕", "⏱️", "ETA"]
                        if _pty_session_log_includes_progress() or not any(
                            c in clean_line for c in progress_chars
                        ):
                            lf.write((clean_line + "\n").encode("utf-8"))
                            lf.flush()
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
                pass
            finally:
                try:
                    lf.close()
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
                    pass

        try:
            os.close(master_fd)
        except OSError:
            pass

    # Post-process checks
    if res and res.returncode == 130:
        raise KeyboardInterrupt

    s_val, sk_val, ig_val, f_val = parse_processor_stats(
        tmp_out,
        parse_type=parse_type,
        restore_jpeg=restore_jpeg,
    )

    if res and res.returncode != 0:
        if (
            "console" in globals()
            and hasattr(console, "print")
            and "Panel" in globals()
        ):
            print()
            console.print(
                Panel(
                    f"[bold red]The '{parse_type}' processor exited unexpectedly with code {res.returncode}.[/bold red]\n"
                    f"[yellow]Please review the terminal output above to see the specific error message.[/yellow]",
                    title="[bold white on red] 🚨 CRITICAL ERROR 🚨 [/bold white on red]",
                    border_style="red",
                    expand=False,
                )
            )
        else:
            print(
                f"\n{RED}🚨 CRITICAL ERROR: The '{parse_type}' processor exited unexpectedly with code {res.returncode}.{RESET}"
            )
            print(
                f"{YELLOW}Please review the terminal output above to see the specific error message.{RESET}\n"
            )

        if exit_on_error:
            if sys.stdin.isatty():
                try:
                    input("\nPress Enter to exit and close this window...")
                except EOFError:
                    pass
            sys.exit(res.returncode)
        if f_val == 0:
            f_val = 1

    exit_code = res.returncode if res else -1
    audit_session(
        format_audit_event(
            f"{pipeline_tag}_PIPELINE_EXIT",
            code=exit_code,
            succeeded=s_val,
            skipped=sk_val,
            ignored=ig_val,
            failed=f_val,
        ),
        echo=True,
    )

    if parse_type == "img":
        with stats_lock:
            global IMG_SUCCEEDED, IMG_SKIPPED, IMG_IGNORED, IMG_FAILED
            IMG_SUCCEEDED, IMG_SKIPPED, IMG_IGNORED, IMG_FAILED = (
                s_val,
                sk_val,
                ig_val,
                f_val,
            )
    else:
        with stats_lock:
            global VID_SUCCEEDED, VID_SKIPPED, VID_IGNORED, VID_FAILED
            VID_SUCCEEDED, VID_SKIPPED, VID_IGNORED, VID_FAILED = (
                s_val,
                sk_val,
                ig_val,
                f_val,
            )

    return ProcessorRunResult(
        returncode=exit_code,
        succeeded=s_val,
        skipped=sk_val,
        ignored=ig_val,
        failed=f_val,
    )


def _image_run_command(input_path: Path, *, fail_fast_batch: bool) -> list[str]:
    cmd = [
        "cargo",
        "run",
        "--locked",
        "-p",
        "dev",
        "--bin",
        "drag_and_drop_processor",
        "--",
        "--mode",
        "images",
    ]
    if RESUME_MODE:
        cmd.append("--resume")
    if ULTIMATE_MODE:
        cmd.append("--ultimate")
    cmd.append("--verbose")

    if OUTPUT_MODE == "inplace":
        cmd.append("--in-place")
    else:
        if not fail_fast_batch:
            cmd.extend(["--base-dir", str(TARGET_DIR)])
        cmd.extend(["--output", str(OUTPUT_DIR)])

    cmd.append(str(input_path))
    return cmd


def _video_run_command(input_path: Path, *, fail_fast_batch: bool) -> list[str]:
    cmd = [
        "cargo",
        "run",
        "--locked",
        "-p",
        "dev",
        "--bin",
        "drag_and_drop_processor",
        "--",
        "--mode",
        "videos",
    ]
    if RESUME_MODE:
        cmd.append("--resume")
    if ULTIMATE_MODE:
        cmd.append("--ultimate")
    cmd.append("--verbose")

    if OUTPUT_MODE == "inplace":
        cmd.append("--in-place")
    else:
        if not fail_fast_batch:
            cmd.extend(["--base-dir", str(TARGET_DIR)])
        cmd.extend(["--output", str(OUTPUT_DIR)])

    cmd.append(str(input_path))
    return cmd


def count_true_jpeg_magic_files(root: Path) -> int:
    count = 0
    for walk_root, _, files in os.walk(root):
        for file in files:
            if file.startswith("."):
                continue
            path = Path(walk_root) / file
            try:
                with path.open("rb") as handle:
                    if handle.read(3) == b"\xff\xd8\xff":
                        count += 1
            except OSError:
                continue
    return count


def fast_img_launch_failure_count(cmd: list[str]) -> int | None:
    if OUTPUT_MODE != "fast_img" or "fast-img" not in [str(part) for part in cmd]:
        return None
    count = count_true_jpeg_magic_files(Path(TARGET_DIR))
    if count > 0:
        return count
    return 1


def record_processor_launch_failure(
    parse_type: str, cmd: list[str], exc: BaseException
) -> None:
    failure_count = fast_img_launch_failure_count(cmd) if parse_type == "img" else None
    if failure_count is None:
        failure_count = 1
    if parse_type == "img" and failure_count == 1 and IMG_COUNT > 0:
        failure_count = IMG_COUNT
    elif parse_type == "vid" and VID_COUNT > 0:
        failure_count = VID_COUNT

    audit_session(
        format_audit_event(
            "PROCESSOR_LAUNCH_FAILED",
            processor=parse_type,
            cmd=" ".join(str(c) for c in cmd),
            error=f"{type(exc).__name__}: {exc}",
            failed=failure_count,
        ),
        echo=True,
    )

    with stats_lock:
        global IMG_FAILED, VID_FAILED
        if parse_type == "img":
            IMG_FAILED = max(IMG_FAILED, failure_count)
        else:
            VID_FAILED = max(VID_FAILED, failure_count)


def process_images():
    if IMG_COUNT == 0:
        return

    # Existence check with auto-rebuild
    if not IMGQUALITY_HEVC.exists():
        print(f"\n{RED}{ICO_ERR} Critical Error: img binary not found{RESET}")
        print(f"{DIM}   Expected path: {IMGQUALITY_HEVC}{RESET}")
        print(f"{DIM}   The build may have failed or been cleaned.{RESET}")

        if not rebuild_tools():
            print(
                f"{YELLOW}   Manual rebuild required: cargo run --locked -p dev --bin smart_build{RESET}"
            )
            print(f"{DIM}   Or drag/drop again after build completes.{RESET}\n")
            sys.exit(1)

        # Verify rebuild succeeded
        if not IMGQUALITY_HEVC.exists():
            print(
                f"{RED}{ICO_ERR} Rebuild verification failed: binary still missing.{RESET}\n"
            )
            sys.exit(1)

    draw_separator(f"Processing Images ({IMG_COUNT})")
    if drag_drop_fail_fast_enabled():
        if RESUME_MODE:
            print(
                f"{DIM}   ✓ Progress Resume: ENABLED (skipping already completed files){RESET}"
            )
        cmd = _image_run_command(Path(TARGET_DIR), fail_fast_batch=True)
        stream_and_log_process(cmd, "img", fail_fast=True)
        print()
        return

    routed_image_paths = [
        Path(TARGET_DIR) / rel for rel in sorted(ROUTED_IMAGE_REL_PATHS)
    ]
    if RESUME_MODE:
        print(
            f"{DIM}   ✓ Progress Resume: ENABLED (skipping already completed files){RESET}"
        )

    succeeded = skipped = ignored = failed = 0
    for source_path in routed_image_paths:
        cmd = _image_run_command(source_path, fail_fast_batch=False)
        try:
            result = stream_and_log_process(cmd, "img", fail_fast=False)
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
            failed += 1
            audit_session(
                format_audit_event(
                    "IMG_PIPELINE_FILE_FAILED",
                    target=str(source_path),
                    error=f"{type(exc).__name__}: {exc}",
                ),
                echo=True,
            )
            continue
        succeeded += result.succeeded
        skipped += result.skipped
        ignored += result.ignored
        failed += result.failed
    with stats_lock:
        global IMG_SUCCEEDED, IMG_SKIPPED, IMG_IGNORED, IMG_FAILED
        IMG_SUCCEEDED = succeeded
        IMG_SKIPPED = skipped
        IMG_IGNORED = ignored
        IMG_FAILED = failed
    print()


def process_videos():
    if VID_COUNT == 0:
        return

    # Existence check with auto-rebuild
    if not VIDQUALITY_HEVC.exists():
        print(f"\n{RED}{ICO_ERR} Critical Error: vid binary not found{RESET}")
        print(f"{DIM}   Expected path: {VIDQUALITY_HEVC}{RESET}")
        print(f"{DIM}   The build may have failed or been cleaned.{RESET}")

        if not rebuild_tools():
            print(
                f"{YELLOW}   Manual rebuild required: cargo run --locked -p dev --bin smart_build{RESET}"
            )
            print(f"{DIM}   Or drag/drop again after build completes.{RESET}\n")
            sys.exit(1)

        # Verify rebuild succeeded
        if not VIDQUALITY_HEVC.exists():
            print(
                f"{RED}{ICO_ERR} Rebuild verification failed: binary still missing.{RESET}\n"
            )
            sys.exit(1)

    draw_separator(f"Processing Videos ({VID_COUNT})")
    if drag_drop_fail_fast_enabled():
        if RESUME_MODE:
            print(
                f"{DIM}   ✓ Progress Resume: ENABLED (skipping already completed files){RESET}"
            )
        cmd = _video_run_command(Path(TARGET_DIR), fail_fast_batch=True)
        stream_and_log_process(cmd, "vid", fail_fast=True)
        print()
        return

    routed_video_paths = [
        Path(TARGET_DIR) / rel for rel in sorted(ROUTED_VIDEO_REL_PATHS)
    ]
    if RESUME_MODE:
        print(
            f"{DIM}   ✓ Progress Resume: ENABLED (skipping already completed files){RESET}"
        )

    succeeded = skipped = ignored = failed = 0
    for source_path in routed_video_paths:
        cmd = _video_run_command(source_path, fail_fast_batch=False)
        try:
            result = stream_and_log_process(cmd, "vid", fail_fast=False)
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
            failed += 1
            audit_session(
                format_audit_event(
                    "VID_PIPELINE_FILE_FAILED",
                    target=str(source_path),
                    error=f"{type(exc).__name__}: {exc}",
                ),
                echo=True,
            )
            continue
        succeeded += result.succeeded
        skipped += result.skipped
        ignored += result.ignored
        failed += result.failed
    with stats_lock:
        global VID_SUCCEEDED, VID_SKIPPED, VID_IGNORED, VID_FAILED
        VID_SUCCEEDED = succeeded
        VID_SKIPPED = skipped
        VID_IGNORED = ignored
        VID_FAILED = failed
    print()


def run_img_vid_pipeline() -> None:
    """
    Run img and/or vid per PROCESSING_MODE. Sets phase to ``post_img_vid`` on success.

    Handoff preserve is allowed only after this function completes.
    """
    global _PIPELINE_PHASE, _IMG_VID_PIPELINE_RAN

    if _PIPELINE_PHASE not in ("idle",):
        audit_session(
            format_audit_event(
                "PIPELINE_BLOCKED",
                attempt="img_vid_run",
                phase=_PIPELINE_PHASE,
            ),
            echo=True,
        )
        return

    _PIPELINE_PHASE = "img_vid_running"
    _IMG_VID_PIPELINE_RAN = False
    audit_session(
        format_audit_event(
            "IMG_VID_RUN_START",
            mode=PROCESSING_MODE,
            output_mode=OUTPUT_MODE,
        ),
        echo=True,
    )

    ensure_tools_ready(quiet=True)
    verify_database_mandatory()

    ran_any = False
    if PROCESSING_MODE in ("both", "images_only"):
        audit_session("IMG_PIPELINE_START", echo=True)
        process_images()
        audit_session("IMG_PIPELINE_END", echo=True)
        ran_any = True
    if PROCESSING_MODE in ("both", "videos_only"):
        audit_session("VID_PIPELINE_START", echo=True)
        process_videos()
        audit_session("VID_PIPELINE_END", echo=True)
        ran_any = True

    if not ran_any:
        audit_session("IMG_VID_RUN_ABORT reason=no_pipeline_for_mode", echo=True)
        _PIPELINE_PHASE = "idle"
        return

    _IMG_VID_PIPELINE_RAN = True
    _PIPELINE_PHASE = HANDOFF_PRESERVE_PHASE_POST_IMG_VID
    audit_session("IMG_VID_RUN_COMPLETE", echo=True)


def finalize_handoff_preservation() -> int:
    """
    Optional, explicit step after img/vid: offer to copy video-route sources that still
    have no optimized stem match. Never runs without terminal ``y``/``yes`` confirmation.

    MUST only be called from ``run_post_img_vid_adjacent_steps`` (phase gate).
    """
    global _PIPELINE_PHASE

    if (
        _PIPELINE_PHASE != HANDOFF_PRESERVE_PHASE_POST_IMG_VID
        or not _IMG_VID_PIPELINE_RAN
    ):
        msg = audit_handoff_blocked(
            "wrong_pipeline_phase",
            phase=_PIPELINE_PHASE,
            img_vid_ran=str(_IMG_VID_PIPELINE_RAN).lower(),
        )
        audit_session(msg, echo=True)
        print(
            f"\n{RED}{ICO_BLOCK} Handoff preserve blocked:{RESET} not in post-img/vid phase "
            f"(phase={_PIPELINE_PHASE}).\n"
        )
        return 0

    if OUTPUT_MODE != "adjacent" or not OUTPUT_DIR:
        audit_session("HANDOFF_PRESERVE_SKIP reason=not_adjacent_mode", echo=True)
        return 0
    if not ROUTED_VIDEO_REL_PATHS:
        audit_session("HANDOFF_PRESERVE_SKIP reason=no_video_routes", echo=True)
        return 0

    audit_session(
        format_audit_event(
            "HANDOFF_PRESERVE_ENTER",
            phase=_PIPELINE_PHASE,
            trigger="post_img_vid_only",
        ),
        echo=True,
    )

    source_root = Path(TARGET_DIR).resolve()
    optimized_root = Path(OUTPUT_DIR).resolve()
    candidates = list_handoff_preserve_candidates(
        source_root, optimized_root, ROUTED_VIDEO_REL_PATHS
    )

    draw_separator("Handoff Preserve (optional — post img/vid only)")
    audit_session(f"HANDOFF_PRESERVE_SCAN candidates={len(candidates)}", echo=True)

    print(
        f"\n{YELLOW}Handoff preserve{RESET} — copies {BOLD}video-route{RESET} sources into the "
        f"optimized folder when img ignored them and vid did not produce a same-stem output.\n"
        f"   Source:    {DIM}{source_root}{RESET}\n"
        f"   Optimized: {DIM}{optimized_root}{RESET}\n"
    )

    if not candidates:
        print(
            f"   {GREEN}✓ No handoff gaps detected{RESET} "
            f"(every video-route file already has optimized output).\n"
        )
        audit_session("HANDOFF_PRESERVE_NONE_NEEDED", echo=True)
        return 0

    total_bytes = sum(c.size_bytes for c in candidates)
    print(
        f"   {YELLOW}{ICO_WARN_ALT} {len(candidates)} file(s){RESET} would be copied "
        f"({format_bytes(total_bytes)} total):\n"
    )
    for c in candidates:
        print(f"      • {c.rel_path}  ({format_bytes(c.size_bytes)})")
        audit_session(
            f"HANDOFF_PRESERVE_CANDIDATE path={c.rel_path} bytes={c.size_bytes}"
        )

    audit_session("HANDOFF_PRESERVE_PROMPT offered=1", echo=True)
    print(
        f"\n{YELLOW}These are source-file copies, not re-encodes.{RESET} "
        f"Integrity may still report handoff gaps if you skip.\n"
    )
    try:
        choice = (
            input(
                f"   {CYAN}Copy these files into optimized? "
                f"[{GREEN}y{CYAN}/{RED}n{CYAN}]: {RESET}"
            )
            .strip()
            .lower()
        )
    except EOFError:
        choice = "n"
        print(
            f"\n{RED}{ICO_BLOCK} No TTY — handoff preserve skipped (treated as no).{RESET}"
        )

    if choice not in ("y", "yes"):
        print(
            f"\n{RED}{ICO_BLOCK} Handoff preserve declined.{RESET} No files were copied.\n"
        )
        audit_session("HANDOFF_PRESERVE_DECLINED user_choice=no", echo=True)
        return 0

    audit_session("HANDOFF_PRESERVE_CONFIRMED user_choice=yes", echo=True)
    print(f"\n{CYAN}Copying {len(candidates)} file(s)...{RESET}\n")

    preserved = preserve_handoff_gaps(
        source_root,
        optimized_root,
        ROUTED_VIDEO_REL_PATHS,
        only_candidates=candidates,
        phase=HANDOFF_PRESERVE_PHASE_POST_IMG_VID,
        audit_log=append_session_audit,
    )
    for rel in preserved:
        print(f"   {GREEN}✓{RESET} {rel}")

    if len(preserved) < len(candidates):
        failed = len(candidates) - len(preserved)
        print(
            f"\n   {YELLOW}{ICO_WARN_ALT} {failed} file(s) could not be copied (see session log).{RESET}"
        )

    print(
        f"\n   {GREEN}{ICO_OK} Handoff preserve complete:{RESET} "
        f"{len(preserved)}/{len(candidates)} copied.\n"
    )
    audit_session(f"PRESERVE_HANDOFF_SUMMARY count={len(preserved)}", echo=True)
    return len(preserved)


def run_post_img_vid_adjacent_steps() -> None:
    """
    Single entry for post-pipeline adjacent work (handoff → rsync → verify).

    Handoff preserve is invoked only here, and only when phase is ``post_img_vid``.
    """
    global _PIPELINE_PHASE

    if _PIPELINE_PHASE != HANDOFF_PRESERVE_PHASE_POST_IMG_VID:
        audit_session(
            format_audit_event(
                "ADJACENT_STEPS_BLOCKED",
                reason="img_vid_not_complete",
                phase=_PIPELINE_PHASE,
            ),
            echo=True,
        )
        print(
            f"\n{RED}{ICO_BLOCK} Post-pipeline steps skipped:{RESET} img/vid batch did not complete.\n"
        )
        return

    audit_session("ADJACENT_STEPS_START", echo=True)
    finalize_handoff_preservation()
    sync_non_media_files()
    draw_separator("Auto Verification")
    print(f"   {DIM}Running unified integrity verification via Rust verify...{RESET}")
    # Pass unified log dir so verify can load Bundle_* img_run/vid_run MFB_AUDIT lines.
    run_unified_verification(include_logs=True, auto_mode=True)
    _PIPELINE_PHASE = "adjacent_finalized"
    audit_session("ADJACENT_STEPS_COMPLETE", echo=True)


def sync_non_media_files():
    draw_separator("Syncing Non-Media Files")
    # Exclude only files actually owned by an enabled pipeline.
    # This avoids dropping static assets in videos-only mode for overlap formats
    # such as PNG / WebP / AVIF / HEIC / JXL.
    excludes = [
        "--exclude=*.[xX][mM][pP]"
    ]  # always exclude sidecars (handled separately)
    if IMG_COUNT > 0:
        excludes.extend(
            f"--exclude=/{Path(rel_path).as_posix()}"
            for rel_path in sorted(ROUTED_IMAGE_REL_PATHS)
        )
    if VID_COUNT > 0:
        excludes.extend(
            f"--exclude=/{Path(rel_path).as_posix()}"
            for rel_path in sorted(ROUTED_VIDEO_REL_PATHS)
        )
    rsync = (
        "/opt/homebrew/opt/rsync/bin/rsync"
        if os.path.exists("/opt/homebrew/opt/rsync/bin/rsync")
        else "rsync"
    )
    audit_session(f"RSYNC_START excludes={len(excludes)}", echo=True)
    for exclude in excludes:
        audit_session(f"RSYNC_EXCLUDE {exclude}")
    cmd = (
        [rsync, "-av", "--ignore-existing"]
        + excludes
        + [f"{TARGET_DIR}/", f"{OUTPUT_DIR}/"]
    )
    audit_session(f"RSYNC_CMD {' '.join(cmd)}")
    proc = subprocess.run(cmd, capture_output=True, text=True)
    if proc.returncode != 0:
        audit_session(
            f"RSYNC_FAIL code={proc.returncode} stderr={proc.stderr[:500]}", echo=True
        )
        print(
            f"   {RED}{ICO_WARN_ALT} rsync exited {proc.returncode} (see session log).{RESET}"
        )
    else:
        audit_session("RSYNC_OK", echo=True)
        print(f"   {GREEN}{ICO_OK} Non-media files synced.{RESET}")
    ts_cmd = [
        str(IMGQUALITY_HEVC),
        "restore-timestamps",
        str(TARGET_DIR),
        str(OUTPUT_DIR),
    ]
    audit_session(f"TIMESTAMP_RESTORE_CMD {' '.join(ts_cmd)}")
    ts_proc = subprocess.run(ts_cmd, capture_output=True, text=True)
    if ts_proc.returncode != 0:
        audit_session(
            f"TIMESTAMP_RESTORE_FAIL code={ts_proc.returncode} stderr={ts_proc.stderr[:300]}",
            echo=True,
        )
        print(
            f"   {YELLOW}{ICO_WARN_ALT} Timestamp restore exited {ts_proc.returncode} (see session log).{RESET}"
        )
    else:
        audit_session("TIMESTAMP_RESTORE_OK", echo=True)
        print(f"   {GREEN}{ICO_OK} Timestamps restored.{RESET}")


def run_unified_verification(
    include_logs: bool = False,
    auto_mode: bool = False,
    fast_img_delivery: bool = False,
    fast_img_restore: bool = False,
) -> bool:
    """
    Unified verification entrypoint.
    Delegates integrity verification to the Rust verify bin as the single source of truth.
    """
    global LAST_VERIFY_WARNINGS, LAST_VERIFY_SUMMARY, LAST_VERIFY_ISSUE_COUNT
    reset_runtime_session_state(clear_processing_stats=False)
    if fast_img_delivery and fast_img_restore:
        print(
            f"   {RED}{ICO_ERR} Verification mode conflict: delivery and restore cannot both be enabled.{RESET}"
        )
        LAST_VERIFY_WARNINGS = True
        LAST_VERIFY_ISSUE_COUNT = max(1, LAST_VERIFY_ISSUE_COUNT)
        return False
    src_dir = Path(TARGET_DIR).resolve()
    opt_dir = None

    if OUTPUT_MODE in ("adjacent", "fast_img") and OUTPUT_DIR:
        opt_dir = Path(OUTPUT_DIR).resolve()
    else:
        # Manual diagnostic mode fallback: infer adjacent optimized dir.
        candidate = src_dir.parent / (src_dir.name + "_optimized")
        if candidate.is_dir():
            opt_dir = candidate
        else:
            candidate2 = src_dir.parent / (src_dir.name + "__optimized")
            if candidate2.is_dir():
                opt_dir = candidate2

    cmd = ["cargo", "run", "--locked", "-p", "dev", "--bin", "verify", "--"]
    if opt_dir is not None:
        cmd.extend(["--verify", str(src_dir), str(opt_dir)])
        cmd.extend(["--mode", PROCESSING_MODE])
        if fast_img_delivery:
            cmd.append("--fast-img-delivery")
        if fast_img_restore:
            cmd.append("--fast-img-restore")
    else:
        print(
            f"   {YELLOW}{ICO_WARN} Optimized pair not found; running log-only analysis.{RESET}"
        )

    if include_logs:
        # Instead of scanning the entire log directory recursively, target the exact logs for this session.
        img_log = LOG_DIR / f"img_run_{SESSION_START_TIME}.log"
        vid_log = LOG_DIR / f"vid_run_{SESSION_START_TIME}.log"
        added_any = False
        if img_log.is_file():
            cmd.append(str(img_log))
            added_any = True
        if vid_log.is_file():
            cmd.append(str(vid_log))
            added_any = True
        if not added_any:
            cmd.append(str(LOG_DIR))
    if VERBOSE_LOG_FILE and Path(VERBOSE_LOG_FILE).is_file():
        cmd.extend(["--session-audit", str(Path(VERBOSE_LOG_FILE).resolve())])
    if auto_mode:
        cmd.append("--print-integrity-summary")
        proc = subprocess.run(cmd, capture_output=True, text=True)
        if proc.stdout:
            print(proc.stdout, end="" if proc.stdout.endswith("\n") else "\n")
            LAST_VERIFY_SUMMARY = proc.stdout
            status_match = re.search(
                r"^\s*Integrity:\s+(WARNINGS|CLEAN)\s*$", proc.stdout, re.MULTILINE
            )
            issue_match = re.search(
                r"^\s*Integrity Issues:\s*(\d+)\s*$", proc.stdout, re.MULTILINE
            )
            LAST_VERIFY_WARNINGS = (
                status_match.group(1) == "WARNINGS" if status_match else None
            )
            LAST_VERIFY_ISSUE_COUNT = int(issue_match.group(1)) if issue_match else 0
            if LOG_FILE:
                try:
                    with open(LOG_FILE, "a", encoding="utf-8") as f:
                        f.write("\n========================================\n")
                        f.write("🔍 Auto Verification Summary\n")
                        f.write("========================================\n")
                        f.write(proc.stdout)
                        if not proc.stdout.endswith("\n"):
                            f.write("\n")
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
                    pass
        if proc.stderr:
            print(proc.stderr, end="" if proc.stderr.endswith("\n") else "\n")
        if proc.returncode != 0:
            LAST_VERIFY_WARNINGS = True
            LAST_VERIFY_ISSUE_COUNT = max(1, LAST_VERIFY_ISSUE_COUNT)
            return False
        if LAST_VERIFY_WARNINGS is None:
            LAST_VERIFY_WARNINGS = True
            LAST_VERIFY_ISSUE_COUNT = max(1, LAST_VERIFY_ISSUE_COUNT)
            return False
        return not LAST_VERIFY_WARNINGS

    return subprocess.run(cmd).returncode == 0


def count_fast_img_jxl_outputs(output_dir: Path) -> tuple[int, int]:
    count = 0
    total_size = 0
    if not output_dir.is_dir():
        return count, total_size
    for path in output_dir.rglob("*"):
        if not path.is_file():
            continue
        try:
            true_format = detect_true_format(path)
        except (OSError, MediaProbeError) as exc:
            raise RuntimeError(
                f"fast-img output probe failed for {path}: {exc}"
            ) from exc
        if true_format == "jxl":
            count += 1
            try:
                total_size += path.stat().st_size
            except OSError as exc:
                raise RuntimeError(
                    f"fast-img output stat failed for true JXL {path}: {exc}"
                ) from exc
    return count, total_size


def _integrity_summary_int(summary: str, label: str) -> int | None:
    match = re.search(rf"^\s*{re.escape(label)}:\s*(\d+)\s*$", summary, re.MULTILINE)
    if not match:
        return None
    return int(match.group(1))


def fast_img_integrity_counts(summary: str) -> tuple[int, int, int, int] | None:
    recorded_sources = _integrity_summary_int(summary, "Recorded source JPEGs")
    optimized_jxls = _integrity_summary_int(summary, "Optimized JXL files")
    recorded_skips = _integrity_summary_int(summary, "Recorded skipped JPEGs")
    recorded_failures = _integrity_summary_int(summary, "Recorded failed JPEGs")
    if recorded_sources is None or optimized_jxls is None or recorded_skips is None:
        return None
    if recorded_failures is None:
        recorded_failures = 0
    return recorded_sources, optimized_jxls, recorded_skips, recorded_failures


def fast_img_restore_integrity_counts(summary: str) -> tuple[int, int, int, int] | None:
    source_jxls = _integrity_summary_int(summary, "Source JXL files")
    source_remaining_jxls = _integrity_summary_int(
        summary, "Source remaining JXL files"
    )
    verified_deleted_jxls = _integrity_summary_int(
        summary, "Manifest verified deleted source JXLs"
    )
    restored_jpegs = _integrity_summary_int(summary, "Restored JPEG files")
    if source_jxls is None or restored_jpegs is None:
        return None
    if source_remaining_jxls is None:
        source_remaining_jxls = source_jxls
    if verified_deleted_jxls is None:
        verified_deleted_jxls = 0
    return source_jxls, restored_jpegs, source_remaining_jxls, verified_deleted_jxls


def _fast_img_marker_entry_out_rel(source_rel: str, entry: dict) -> str:
    out_rel = entry.get("out_rel")
    if isinstance(out_rel, str) and out_rel.strip():
        return out_rel
    return Path(source_rel).with_suffix(".JXL").as_posix()


def load_fast_img_marker_for_optimized(
    optimized_dir: Path,
) -> tuple[dict | None, Path | None, str | None]:
    verify_src = PROJECT_ROOT / "crates" / "dev" / "src" / "bin" / "verify.rs"
    verify_bin = PROJECT_ROOT / "target" / "release" / "verify"
    if not verify_bin.is_file():
        verify_bin = PROJECT_ROOT / "target" / "debug" / "verify"
    if (
        verify_bin.is_file()
        and verify_src.is_file()
        and verify_bin.stat().st_mtime >= verify_src.stat().st_mtime
    ):
        cmd = [str(verify_bin), "--fast-img-marker-json", str(optimized_dir)]
    else:
        cmd = [
            "cargo",
            "run",
            "--locked",
            "-p",
            "dev",
            "--bin",
            "verify",
            "--",
            "--fast-img-marker-json",
            str(optimized_dir),
        ]
    probe_env = os.environ.copy()
    try:
        import pwd

        real_home = pwd.getpwuid(os.getuid()).pw_dir
    except (ImportError, KeyError, OSError):
        real_home = ""
    if real_home:
        probe_env["HOME"] = real_home
        probe_env.setdefault("CARGO_HOME", str(Path(real_home) / ".cargo"))
        probe_env.setdefault("RUSTUP_HOME", str(Path(real_home) / ".rustup"))
    proc = _FAST_IMG_MARKER_PROBE_RUN(
        cmd,
        cwd=PROJECT_ROOT,
        capture_output=True,
        text=True,
        env=probe_env,
    )
    if proc.returncode != 0:
        stderr = (proc.stderr or proc.stdout or "").strip()
        return None, None, f"fast-img marker probe failed: {stderr}"
    try:
        payload = json.loads(proc.stdout)
    except json.JSONDecodeError as exc:
        return None, None, f"fast-img marker probe returned invalid JSON: {exc}"
    marker = payload.get("marker")
    marker_path = payload.get("marker_path")
    marker_error = payload.get("marker_error")
    return (
        marker if isinstance(marker, dict) else None,
        Path(marker_path) if isinstance(marker_path, str) else None,
        marker_error if isinstance(marker_error, str) else None,
    )


FAST_IMG_RETRY_STAGES = {"gate1_failed", "gate2_failed", "gate3_failed"}


def fast_img_marker_requires_retry(output_dir: Path) -> bool:
    marker, _marker_path, marker_error = load_fast_img_marker_for_optimized(output_dir)
    if marker_error or marker is None:
        return False
    stage = str(marker.get("stage") or "").strip().lower()
    if stage in FAST_IMG_RETRY_STAGES:
        return True
    if stage == "cleanup_complete":
        failed_sources = marker.get("failed_sources")
        return isinstance(failed_sources, dict) and bool(failed_sources)
    return False


def build_fast_img_delivery_command(*, retry: bool | None = None) -> list[str]:
    if retry is None:
        retry = fast_img_marker_requires_retry(Path(OUTPUT_DIR))
    cmd = launch_drag_drop_rs_cli(
        "--mode",
        "fast-img",
        str(TARGET_DIR),
        "--output",
        str(OUTPUT_DIR),
        "--archive",
    )
    if FAST_IMG_SHORTEST_PATH:
        cmd.append("--shortest-path")
    if retry:
        cmd.append("--retry")
    return cmd


def run_fast_img_delivery_with_auto_retry():
    cmd = build_fast_img_delivery_command()
    result = subprocess.run(cmd)
    if (
        result.returncode != 0
        and "--retry" not in cmd
        and fast_img_marker_requires_retry(Path(OUTPUT_DIR))
    ):
        print(
            f"{YELLOW}{ICO_RETRY} Recoverable failure detected, retrying automatically...{RESET}"
        )
        cmd = build_fast_img_delivery_command(retry=True)
        result = subprocess.run(cmd)
    return cmd, result


def _fast_img_safe_output_path(output_root: Path, rel: str) -> Path:
    rel_path = Path(rel)
    if rel_path.is_absolute() or any(part == ".." for part in rel_path.parts):
        raise ValueError(f"fast-img marker contains unsafe output path: {rel}")
    root = output_root.resolve(strict=True)
    target = (root / rel_path).resolve(strict=False)
    try:
        target.relative_to(root)
    except ValueError as exc:
        raise ValueError(
            f"fast-img marker output escapes optimized directory: {rel}"
        ) from exc
    return target


def _fast_img_marker_cleanup_targets(output_dir: Path) -> list[Path]:
    marker, marker_path, marker_error = load_fast_img_marker_for_optimized(output_dir)
    if marker_error:
        raise RuntimeError(marker_error)
    if marker is None:
        raise RuntimeError("fast-img marker missing for optimized directory")
    blake3_log = marker.get("blake3_log")
    if not isinstance(blake3_log, dict):
        raise RuntimeError(f"fast-img marker has invalid blake3_log: {marker_path}")
    targets = []
    for source_rel, entry in blake3_log.items():
        if not isinstance(source_rel, str) or not isinstance(entry, dict):
            raise RuntimeError(
                f"fast-img marker has invalid blake3 entry: {marker_path}"
            )
        targets.append(
            _fast_img_safe_output_path(
                output_dir, _fast_img_marker_entry_out_rel(source_rel, entry)
            )
        )
    targets.sort()
    return targets


def _fast_img_prune_empty_dirs(output_root: Path, candidate_dirs: set[Path]) -> int:
    if not output_root.exists():
        return 0
    root = output_root.resolve(strict=True)
    dirs = set()
    for candidate in candidate_dirs:
        current = candidate.resolve(strict=False)
        while True:
            try:
                current.relative_to(root)
            except ValueError:
                break
            dirs.add(current)
            if current == root:
                break
            current = current.parent
    pruned = 0
    for directory in sorted(dirs, key=lambda path: len(path.parts), reverse=True):
        if not directory.exists():
            continue
        if not directory.is_dir():
            continue
        try:
            next(directory.iterdir())
            continue
        except StopIteration:
            directory.rmdir()
            pruned += 1
    return pruned


FAST_IMG_CLEANUP_IGNORABLE_FILES = {".DS_Store"}


def _fast_img_remove_ignorable_cleanup_files(output_root: Path) -> int:
    if not output_root.exists():
        return 0
    removed = 0
    for path in output_root.rglob("*"):
        if not path.is_file() or path.name not in FAST_IMG_CLEANUP_IGNORABLE_FILES:
            continue
        path.unlink()
        removed += 1
    return removed


def delete_fast_img_shortest_path_output_dir(output_dir: Path) -> None:
    global FAST_IMG_OUTPUT_CLEANED
    if not output_dir.exists():
        FAST_IMG_OUTPUT_CLEANED = True
        return
    if not output_dir.is_dir():
        raise NotADirectoryError(str(output_dir))
    deleted = 0
    already_absent = 0
    prune_candidates = {output_dir}
    for target in _fast_img_marker_cleanup_targets(output_dir):
        if not target.exists():
            already_absent += 1
            prune_candidates.add(target.parent)
            continue
        if not target.is_file():
            raise RuntimeError(f"fast-img cleanup target is not a file: {target}")
        try:
            true_format = detect_true_format(target)
        except (OSError, MediaProbeError) as exc:
            raise RuntimeError(
                f"fast-img cleanup probe failed for {target}: {exc}"
            ) from exc
        if true_format != "jxl":
            raise RuntimeError(
                f"fast-img cleanup refused non-JXL marker output {target} "
                f"(true_format={true_format})"
            )
        target.unlink()
        deleted += 1
        prune_candidates.add(target.parent)

    ignored_removed = _fast_img_remove_ignorable_cleanup_files(output_dir)
    prune_candidates.add(output_dir)
    for directory in output_dir.rglob("*"):
        if directory.is_dir():
            prune_candidates.add(directory)
    pruned = _fast_img_prune_empty_dirs(output_dir, prune_candidates)
    FAST_IMG_OUTPUT_CLEANED = not output_dir.exists()
    if FAST_IMG_OUTPUT_CLEANED:
        print(
            f"   {GREEN}{ICO_OK} Shortest Path cleanup: removed {deleted} imported JXL file(s) and empty output folder after verified iCloud import: {output_dir}{RESET}"
        )
    else:
        print(
            f"   {GREEN}{ICO_OK} Shortest Path cleanup: removed {deleted} imported JXL file(s), already absent={already_absent}, ignored files removed={ignored_removed}, empty dirs pruned={pruned}; preserved residual files in {output_dir}{RESET}"
        )


def run_fast_img_post_success():
    global IMG_COUNT, IMG_SUCCEEDED, IMG_SKIPPED, IMG_IGNORED, IMG_FAILED
    global VID_COUNT, VID_SUCCEEDED, VID_SKIPPED, VID_IGNORED, VID_FAILED
    global LAST_VERIFY_WARNINGS, LAST_VERIFY_ISSUE_COUNT
    global SIZE_SUMMARY_AFTER_OVERRIDE

    output_dir = (
        Path(OUTPUT_DIR).resolve()
        if OUTPUT_DIR
        else fast_img_output_dir_for_target(Path(TARGET_DIR).resolve())
    )
    delivered_count, delivered_size = count_fast_img_jxl_outputs(output_dir)
    SIZE_SUMMARY_AFTER_OVERRIDE = delivered_size
    with stats_lock:
        IMG_COUNT = delivered_count
        IMG_SUCCEEDED = delivered_count
        IMG_SKIPPED = 0
        IMG_IGNORED = 0
        IMG_FAILED = 0
        VID_COUNT = 0
        VID_SUCCEEDED = 0
        VID_SKIPPED = 0
        VID_IGNORED = 0
        VID_FAILED = 0

    draw_separator("Auto Verification")
    print(f"   {DIM}Running fast-img delivery verification via Rust verify...{RESET}")
    verified = run_unified_verification(
        include_logs=False, auto_mode=True, fast_img_delivery=True
    )
    integrity_counts = fast_img_integrity_counts(LAST_VERIFY_SUMMARY)
    if integrity_counts is not None:
        source_count, optimized_count, skipped_count, failed_count = integrity_counts
        with stats_lock:
            IMG_COUNT = source_count
            IMG_SUCCEEDED = optimized_count
            IMG_SKIPPED = skipped_count
            IMG_FAILED = failed_count
        SIZE_SUMMARY_AFTER_OVERRIDE = delivered_size
    if FAST_IMG_SHORTEST_PATH and verified:
        try:
            delete_fast_img_shortest_path_output_dir(output_dir)
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
            LAST_VERIFY_WARNINGS = True
            LAST_VERIFY_ISSUE_COUNT = max(1, LAST_VERIFY_ISSUE_COUNT)
            print(
                f"   {RED}{ICO_ERR} Shortest Path cleanup failed for {output_dir}: {exc}{RESET}"
            )


def run_fast_img_restore_post_success():
    global IMG_COUNT, IMG_SUCCEEDED, IMG_SKIPPED, IMG_IGNORED, IMG_FAILED
    global VID_COUNT, VID_SUCCEEDED, VID_SKIPPED, VID_IGNORED, VID_FAILED
    global MEDIA_TOTAL_SIZE
    global SIZE_SUMMARY_AFTER_OVERRIDE

    with stats_lock:
        IMG_COUNT = IMG_SUCCEEDED + IMG_SKIPPED + IMG_IGNORED + IMG_FAILED
        VID_COUNT = 0
        VID_SUCCEEDED = 0
        VID_SKIPPED = 0
        VID_IGNORED = 0
        VID_FAILED = 0

    draw_separator("Auto Verification")
    print(f"   {DIM}Running fast-img restore verification via Rust verify...{RESET}")
    run_unified_verification(include_logs=False, auto_mode=True, fast_img_restore=True)
    restore_counts = fast_img_restore_integrity_counts(LAST_VERIFY_SUMMARY)
    if restore_counts is not None:
        (
            source_jxl_count,
            restored_jpeg_count,
            _source_remaining_jxls,
            _verified_deleted_jxls,
        ) = restore_counts
        with stats_lock:
            IMG_COUNT = source_jxl_count
            if (
                IMG_SUCCEEDED == 0
                and IMG_SKIPPED == 0
                and IMG_IGNORED == 0
                and IMG_FAILED == 0
            ):
                IMG_SUCCEEDED = restored_jpeg_count
    SIZE_SUMMARY_AFTER_OVERRIDE = (
        selected_media_tree_size(Path(OUTPUT_DIR)) if OUTPUT_DIR else 0
    )


def should_run_img_vid_pipeline(output_mode: str) -> bool:
    return output_mode != "fast_img"


def effective_success_failure_counts(
    *,
    total_success: int,
    total_failed: int,
    verify_warnings: bool | None,
    verify_issue_count: int,
) -> tuple[int, int, int]:
    integrity_penalty = 0
    if verify_warnings:
        integrity_penalty = max(1, verify_issue_count)
    effective_success = max(0, total_success - integrity_penalty)
    effective_failed = total_failed + integrity_penalty
    return effective_success, effective_failed, integrity_penalty


def finish_log():
    if not LOG_FILE:
        return
    try:
        with open(LOG_FILE, "a") as f:
            f.write("\n========================================\n")
            f.write("📊 Final Statistics\n")
            f.write("========================================\n")
            f.write(f"End Time: {format_session_stamp()}\n\n")
            f.write(
                f"Images:  {IMG_SUCCEEDED} succeeded, {IMG_SKIPPED} skipped, {IMG_FAILED} failed\n"
            )
            f.write(
                f"Videos:  {VID_SUCCEEDED} succeeded, {VID_SKIPPED} skipped, {VID_FAILED} failed\n\n"
            )

            tot_s = IMG_SUCCEEDED + VID_SUCCEEDED
            tot_sk = IMG_SKIPPED + VID_SKIPPED
            tot_f = IMG_FAILED + VID_FAILED
            tot_proc = tot_s + tot_sk + tot_f
            effective_s, effective_f, integrity_penalty = (
                effective_success_failure_counts(
                    total_success=tot_s,
                    total_failed=tot_f,
                    verify_warnings=LAST_VERIFY_WARNINGS,
                    verify_issue_count=LAST_VERIFY_ISSUE_COUNT,
                )
            )

            f.write(
                f"Total:   {effective_s} succeeded, {tot_sk} skipped, {effective_f} failed\n"
            )
            if integrity_penalty > 0:
                f.write(
                    f"Adjusted: raw failures={tot_f}, integrity penalty={integrity_penalty}\n"
                )
            if tot_proc > 0:
                f.write(f"Success Rate: {(effective_s * 100) // tot_proc}%\n")
            if LAST_VERIFY_WARNINGS is not None:
                integrity_state = "WARNINGS" if LAST_VERIFY_WARNINGS else "CLEAN"
                f.write(f"Integrity: {integrity_state}\n")
            f.write("\n")
            f.write(final_size_comparison_summary())
            f.write(
                "\n========================================\nSession completed.\n========================================\n"
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
    ):
        pass

    append_session_audit(
        format_audit_event(
            "SESSION_COMPLETED",
            images_ok=IMG_SUCCEEDED,
            images_skip=IMG_SKIPPED,
            images_fail=IMG_FAILED,
            videos_ok=VID_SUCCEEDED,
            videos_skip=VID_SKIPPED,
            videos_fail=VID_FAILED,
        )
    )

    # UX Refinement: Only show the log path if the session had anomalies or failures.
    # For clean runs, we keep the terminal clutter-free.
    tot_f = IMG_FAILED + VID_FAILED
    has_integrity_issues = LAST_VERIFY_WARNINGS and LAST_VERIFY_ISSUE_COUNT > 0
    if tot_f > 0 or has_integrity_issues:
        print(f"   {DIM}{ICO_LOG} Session log:  {LOG_FILE}{RESET}")


def organize_session_logs():
    """Move all session artifacts into ``Bundle_{stamp}/`` with ``manifest.json`` (no merge)."""
    if not LOG_DIR or not SESSION_START_TIME:
        return

    try:
        session_dt = parse_session_stamp(SESSION_START_TIME)
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
        session_dt = None
        if LOG_FILE and Path(LOG_FILE).is_file():
            session_dt = datetime.datetime.fromtimestamp(os.path.getmtime(LOG_FILE))

    append_session_audit(
        format_audit_event(
            "SESSION_ARCHIVE_BEGIN",
            session_id=SESSION_START_TIME,
        )
    )

    try:
        bundle = archive_drag_drop_session_bundle(
            LOG_DIR,
            SESSION_START_TIME,
            session_log=Path(LOG_FILE) if LOG_FILE else None,
            verbose_log=Path(VERBOSE_LOG_FILE) if VERBOSE_LOG_FILE else None,
            session_audit=Path(SESSION_AUDIT_FILE) if SESSION_AUDIT_FILE else None,
            session_started_at=session_dt,
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
    ) as exc:
        logger.warning("Failed to archive session bundle: %s", exc)
        append_session_audit(
            format_audit_event(
                "SESSION_ARCHIVE_FAILED",
                error=f"{type(exc).__name__}: {exc}",
            )
        )
        print(f"   {RED}{ICO_WARN}  Failed to organize logs: {exc}{RESET}")
        return

    if bundle is None:
        append_session_audit(
            format_audit_event("SESSION_ARCHIVE_SKIP", reason="no_session_artifacts")
        )
        return

    audit_dest = bundle / f"session_audit_{SESSION_START_TIME}.jsonl"
    if audit_dest.is_file():
        append_jsonl_audit_record(
            audit_dest,
            format_audit_event("SESSION_ARCHIVE_DONE", bundle=str(bundle.resolve())),
        )
    try:
        rel = bundle.relative_to(PROJECT_ROOT)
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
        rel = bundle
    print(f"   {DIM}Session logs archived in: {rel}{RESET}")


def main():
    logger.info("=" * 60)
    logger.info(f"DRAG AND DROP PROCESSOR STARTED - PID {os.getpid()}")
    logger.debug(f"sys.argv: {sys.argv}")
    logger.debug(f"os.environ['PATH']: {os.environ.get('PATH')}")
    logger.debug(f"Current working directory: {os.getcwd()}")

    guard_main("drag_and_drop_processor.py")
    # Optimization: Tighten GIL switch interval for smoother high-load terminal relaying
    sys.setswitchinterval(0.0005)
    global \
        ULTIMATE_MODE, \
        VERBOSE_MODE, \
        WATCH_MODE, \
        RESUME_MODE, \
        TARGET_DIR, \
        PROCESSING_MODE, \
        MEDIA_TOTAL_SIZE, \
        SIZE_SUMMARY_AFTER_OVERRIDE, \
        FAST_IMG_ACTION, \
        FAST_IMG_SHORTEST_PATH, \
        FAST_VID_SHORTEST_PATH
    os.environ["MFB_GUI_LAUNCH"] = "1"
    os.environ["FORCE_COLOR"] = "1"
    os.environ["CLICOLOR_FORCE"] = "1"
    # Strict delivery is on by default; do not override a launcher that disabled it.
    os.environ.setdefault("MODERN_FORMAT_DISABLE_STRICT_MEDIA_CONVERSION", "0")
    init_log()

    args = sys.argv[1:]
    non_flag_args = []

    for arg in args:
        if arg == "--ultimate":
            ULTIMATE_MODE = True
        elif arg in ("--verbose", "-v"):
            VERBOSE_MODE = True
        elif arg == "--watch":
            WATCH_MODE = True
        elif arg == "--resume":
            RESUME_MODE = True
        elif arg == "--images-only":
            PROCESSING_MODE = "images_only"
        elif arg == "--videos-only":
            PROCESSING_MODE = "videos_only"
        elif arg in ("--help", "-h"):
            print("Usage: drag_and_drop_processor.py [options] [target_directory]")
            print("\nOptions:")
            print("  --ultimate    Enable ultimate optimization mode")
            print("  --verbose, -v Enable verbose output")
            print("  --resume      Resume from last completed session")
            print("  --watch       Watch directory for new files")
            print("  --images-only Only process static images")
            print("  --videos-only Only process videos and animated media")
            print("  --help, -h    Show this help message")
            sys.exit(0)
        else:
            non_flag_args.append(arg)

    if non_flag_args:
        TARGET_DIR = unescape_path(non_flag_args[0])

    # Set terminal window size to wide format (wide aspect ratio)
    resize_terminal(35, 110)

    get_target_directory()
    rename_log_to_project()  # Rename log once project name is known

    if not os.environ.get("FROM_APP"):
        if "Table" in globals():
            # Dashboard Config
            table = Table(box=None, padding=(0, 2))
            table.add_column("Setting", style="dim", justify="right")
            table.add_column("Value", style=f"bold {BRAND_BLUE}")

            table.add_row(f"{ICO_FOLDER} Target Path", str(TARGET_DIR))
            table.add_row(
                f"{ICO_LAUNCH} Mode", "Ultimate" if ULTIMATE_MODE else "Standard"
            )

            target_type = "Everything"
            if PROCESSING_MODE == "images_only":
                target_type = "Images Only"
            elif PROCESSING_MODE == "videos_only":
                target_type = "Videos/Animated Media Only"
            table.add_row(f"{ICO_TARGET} Target Type", target_type)

            # System Snapshot
            if "psutil" in globals():
                cpu = psutil.cpu_percent()
                mem = psutil.virtual_memory().percent
                table.add_row("{ICO_TEMP}  CPU Load", f"{cpu}%")
                table.add_row("{ICO_STATS} RAM Usage", f"{mem}%")

            console.print(
                Panel(
                    table,
                    title="[#888888]Runtime Configuration[/#888888]",
                    border_style="#333333",
                    expand=False,
                )
            )
            print()
        else:
            print()
            print(f"{CYAN}{ICO_CLIP} Configuration:{RESET}")
            print(f"   {DIM}Target: {RESET}{BOLD}{TARGET_DIR}{RESET}")
            if ULTIMATE_MODE:
                print(
                    f"   {MAGENTA}[ULTIMATE] Ultimate Mode: {RESET}{GREEN}ENABLED{RESET}"
                )
            if VERBOSE_MODE:
                print(f"   {CYAN}{ICO_MSG} Verbose: {RESET}{GREEN}ENABLED{RESET}")
            print()

    safety_check()

    while True:
        try:
            select_mode()

            # Mutex logic: Only enforce exclusive locking if we are modifying original files (In-Place)
            if OUTPUT_MODE == "in_place":
                acquire_global_lock(str(TARGET_DIR))

            if OUTPUT_MODE in ("fast_img", "fast_vid"):
                count_files()
                break

            count_files()

            if IMG_COUNT > 0 or VID_COUNT > 0:
                check_path = OUTPUT_DIR if OUTPUT_MODE == "adjacent" else TARGET_DIR
                check_system_resources(check_path)

            # If we reach here, we proceed to process
            break
        except ReturnToHomeException:
            # Clear any partial state if necessary and loop back to menu
            continue

    if WATCH_MODE:
        draw_separator("Watch Mode Enabled")
        console.print(f"[bold yellow]Monitoring:[/bold yellow] {TARGET_DIR}")
        console.print("[dim]Press Ctrl+C to stop. Debouncing active.[/dim]\n")

        def trigger_watch_processing():
            global is_processing, watch_timer
            with stats_lock:
                if is_processing:
                    return
                is_processing = True

            try:
                count_files()
                ensure_tools_ready(quiet=True)
                verify_database_mandatory()
                process_images()
                process_videos()
            finally:
                with stats_lock:
                    is_processing = False
                    watch_timer = None

        class Handler(FileSystemEventHandler):
            def on_closed(self, event):
                global watch_timer
                if not event.is_directory:
                    p = Path(event.src_path)
                    # Support multiple extensions for triggering
                    if p.suffix.lower() in {
                        ".jpg",
                        ".jpeg",
                        ".png",
                        ".gif",
                        ".heic",
                        ".mp4",
                        ".mov",
                        ".mkv",
                    }:
                        console.print(
                            f"  [bold cyan]File Activity Detected:[/bold cyan] {p.name}"
                        )
                        with stats_lock:
                            if watch_timer:
                                watch_timer.cancel()
                            watch_timer = threading.Timer(
                                watch_debounce_seconds, trigger_watch_processing
                            )
                            watch_timer.start()

            def on_moved(self, event):
                global watch_timer
                if not event.is_directory:
                    p = Path(event.dest_path)
                    if p.suffix.lower() in {
                        ".jpg",
                        ".jpeg",
                        ".png",
                        ".gif",
                        ".heic",
                        ".mp4",
                        ".mov",
                        ".mkv",
                    }:
                        console.print(
                            f"  [bold cyan]File Activity Detected (Moved):[/bold cyan] {p.name}"
                        )
                        with stats_lock:
                            if watch_timer:
                                watch_timer.cancel()
                            watch_timer = threading.Timer(
                                watch_debounce_seconds, trigger_watch_processing
                            )
                            watch_timer.start()

        observer = Observer()
        observer.schedule(Handler(), str(TARGET_DIR), recursive=True)
        observer.start()
        try:
            while True:
                time.sleep(1)
        except KeyboardInterrupt:
            observer.stop()
        observer.join()
        sys.exit(0)

    start_elapsed_spinner()

    # Intentional delay before starting actual work
    print(f"\n{CYAN}⏳ Pacing start to ensure system stability...{RESET}")
    time.sleep(1.5)

    if OUTPUT_MODE == "fast_img":
        with stats_lock:
            MEDIA_TOTAL_SIZE = snapshot_selected_media_size()
        stop_elapsed_spinner()
        print(
            f"\n{BLUE}{pick_symbol('🚀', '[LAUNCH]')} Launching Rust Fast Mode Pipeline...{RESET}"
        )
        ensure_tools_ready(force=FAST_IMG_FORCE_SMART_BUILD, quiet=True)
        if FAST_IMG_ACTION == "restore_jpeg":
            cmd = launch_drag_drop_rs_cli(
                "--mode",
                "restore-jpeg",
                str(TARGET_DIR),
                "--output",
                str(OUTPUT_DIR),
            )
        else:
            cmd = build_fast_img_delivery_command()
        try:
            if FAST_IMG_ACTION == "restore_jpeg":
                result = stream_and_log_process(cmd, "img", restore_jpeg=True)
                if result.returncode == 0:
                    run_fast_img_restore_post_success()
            else:
                cmd, result = run_fast_img_delivery_with_auto_retry()
                if result.returncode == 0:
                    run_fast_img_post_success()
                elif drag_drop_fail_fast_enabled():
                    sys.exit(result.returncode)
                else:
                    record_processor_launch_failure(
                        "img",
                        cmd,
                        RuntimeError(f"fast-img exited with code {result.returncode}"),
                    )
        except OSError as e:
            print(f"{RED}Error launching fast-img command {cmd!r}: {e}{RESET}")
            if drag_drop_fail_fast_enabled():
                sys.exit(1)
            record_processor_launch_failure("img", cmd, e)

    if OUTPUT_MODE == "fast_vid":
        with stats_lock:
            MEDIA_TOTAL_SIZE = snapshot_selected_media_size()
        stop_elapsed_spinner()
        print(
            f"\n{BLUE}{pick_symbol('🚀', '[LAUNCH]')} Launching Rust Fast Video Pipeline...{RESET}"
        )
        ensure_tools_ready(force=FAST_IMG_FORCE_SMART_BUILD, quiet=True)
        cmd = launch_drag_drop_rs_cli(
            "--mode",
            "fast-vid",
            str(TARGET_DIR),
            "--output",
            str(OUTPUT_DIR),
        )
        if FAST_VID_SHORTEST_PATH:
            cmd.append("--shortest-path")
        try:
            result = subprocess.run(cmd)
            if result.returncode == 0:
                SIZE_SUMMARY_AFTER_OVERRIDE = output_size_for_summary()
            elif drag_drop_fail_fast_enabled():
                sys.exit(result.returncode)
            else:
                record_processor_launch_failure(
                    "vid",
                    cmd,
                    RuntimeError(f"fast-vid exited with code {result.returncode}"),
                )
        except OSError as e:
            print(f"{RED}Error launching fast-vid command {cmd!r}: {e}{RESET}")
            if drag_drop_fail_fast_enabled():
                sys.exit(1)
            record_processor_launch_failure("vid", cmd, e)

    if should_run_img_vid_pipeline(OUTPUT_MODE):
        run_img_vid_pipeline()
        if IMG_COUNT > 0 or VID_COUNT > 0:
            stop_elapsed_spinner()

        if OUTPUT_MODE == "adjacent":
            run_post_img_vid_adjacent_steps()

    draw_separator("Task Completed")

    tot_s = IMG_SUCCEEDED + VID_SUCCEEDED
    tot_sk = IMG_SKIPPED + VID_SKIPPED
    tot_ig = IMG_IGNORED + VID_IGNORED
    tot_f = IMG_FAILED + VID_FAILED
    effective_s, effective_f, integrity_penalty = effective_success_failure_counts(
        total_success=tot_s,
        total_failed=tot_f,
        verify_warnings=LAST_VERIFY_WARNINGS,
        verify_issue_count=LAST_VERIFY_ISSUE_COUNT,
    )
    integrity_state = None
    if LAST_VERIFY_WARNINGS is not None:
        integrity_state = "WARNINGS" if LAST_VERIFY_WARNINGS else "CLEAN"
    size_summary = final_size_comparison_summary()

    if "Table" in globals():
        # Premium Rich Stats
        # Success rate only penalizes on failures, skipped/ignored are neutral.
        rate_denominator = effective_s + effective_f
        success_rate = (
            (effective_s * 100) // rate_denominator if rate_denominator > 0 else 100
        )
        rate_color = (
            "green" if success_rate >= 90 else "yellow" if success_rate >= 50 else "red"
        )

        table = Table(title="Optimization Summary Report", border_style="dim")
        table.add_column("Type", justify="left", style="bold #cccccc")
        table.add_column("Succeeded", justify="center", style="green")
        table.add_column("Skipped", justify="center", style="yellow")
        table.add_column("Ignored", justify="center", style="dim")
        table.add_column("Failed", justify="center", style="red")

        if IMG_COUNT > 0:
            table.add_row(
                "🖼️  Images",
                str(IMG_SUCCEEDED),
                str(IMG_SKIPPED),
                str(IMG_IGNORED),
                str(IMG_FAILED),
            )
        if VID_COUNT > 0:
            table.add_row(
                "🎬 Videos",
                str(VID_SUCCEEDED),
                str(VID_SKIPPED),
                str(VID_IGNORED),
                str(VID_FAILED),
            )

        table.add_section()
        table.add_row(
            "📦 Total",
            f"[bold]{effective_s}[/bold]",
            str(tot_sk),
            str(tot_ig),
            str(effective_f),
        )

        print()
        console.print(table)

        # Success Bar
        if rate_denominator > 0:
            bar_len = 20
            filled = int((success_rate / 100) * bar_len)
            bar = "█" * filled + "░" * (bar_len - filled)
            console.print(
                f"   [bold #cccccc]Success Rate:[/bold #cccccc] [{rate_color}]{bar}[/{rate_color}] {success_rate}%"
            )
        if integrity_state is not None:
            integrity_color = "yellow" if integrity_state == "WARNINGS" else "green"
            console.print(
                f"   [bold #cccccc]Integrity:[/bold #cccccc] [{integrity_color}]{integrity_state}[/{integrity_color}]"
            )
            if integrity_penalty > 0:
                console.print(
                    f"   [bold #cccccc]Adjusted Failures:[/bold #cccccc] [yellow]+{integrity_penalty} integrity-derived failures[/yellow]"
                )
        print()
        print(size_summary)
        print()
    else:
        # Success rate only penalizes on failures, skipped/ignored are neutral.
        rate_denominator = effective_s + effective_f
        success_rate = (
            (effective_s * 100) // rate_denominator if rate_denominator > 0 else 100
        )

        print(f"   {GREEN}{ICO_OK} Optimization Finished Successfully{RESET}\n")
        print(f"   {BOLD}{ICO_STATS} Merged Statistics Report{RESET}")
        print(f"   {DIM}───────────────────────────────────{RESET}")
        if IMG_COUNT > 0:
            print(
                f"   {CYAN}{ICO_IMG}  Images:{RESET} {GREEN}{IMG_SUCCEEDED}{RESET} succeeded, {YELLOW}{IMG_SKIPPED}{RESET} skipped, {DIM}{IMG_IGNORED}{RESET} ignored, {RED}{IMG_FAILED}{RESET} failed"
            )
        if VID_COUNT > 0:
            print(
                f"   {MAGENTA}{ICO_VID} Videos:{RESET} {GREEN}{VID_SUCCEEDED}{RESET} succeeded, {YELLOW}{VID_SKIPPED}{RESET} skipped, {DIM}{VID_IGNORED}{RESET} ignored, {RED}{VID_FAILED}{RESET} failed"
            )
        print(f"   {DIM}───────────────────────────────────{RESET}")
        print(
            f"   {WHITE}{ICO_PKG} Total:{RESET}  {GREEN}{effective_s}{RESET} succeeded, {YELLOW}{tot_sk}{RESET} skipped, {DIM}{tot_ig}{RESET} ignored, {RED}{effective_f}{RESET} failed"
        )

        if rate_denominator > 0:
            print(
                f"   {WHITE}{ICO_CHART} Success Rate:{RESET} {GREEN}{success_rate}%{RESET}\n"
            )
        if integrity_state is not None:
            integrity_color = YELLOW if integrity_state == "WARNINGS" else GREEN
            print(
                f"   {WHITE}{ICO_CHECK} Integrity:{RESET} {integrity_color}{integrity_state}{RESET}\n"
            )
            if integrity_penalty > 0:
                print(
                    f"   {WHITE}{ICO_PLUS} Adjusted Failures:{RESET} {YELLOW}+{integrity_penalty} integrity-derived failures{RESET}\n"
                )
        print(size_summary)
        print()

    if OUTPUT_MODE in ("adjacent", "fast_img"):
        if OUTPUT_MODE == "fast_img" and FAST_IMG_OUTPUT_CLEANED:
            print(
                f"   {BLUE}{ICO_FOLDER} Output: {OUTPUT_DIR} {DIM}(cleaned after verified iCloud import){RESET}"
            )
        else:
            print(f"   {BLUE}{ICO_FOLDER} Output: {OUTPUT_DIR}{RESET}")
        # Automatic opening disabled as per user feedback
        # try:
        #     subprocess.run(
        #         ["open", str(OUTPUT_DIR)],
        #         stdout=subprocess.DEVNULL,
        #         stderr=subprocess.DEVNULL,
        #     )
        # except (OSError, ValueError, RuntimeError, TypeError, KeyError, IndexError, AttributeError, UnicodeError):
        #     pass

    try:
        drain_stdin()
        # Auto-exit (removed manual wait)
        print(f"\n   {GREEN}{ICO_OK} Task finished. Auto-exiting...{RESET}")
    except (EOFError, KeyboardInterrupt):
        pass

    finish_log()
    organize_session_logs()

    if effective_f > 0:
        print(
            f"\n   {RED}{ICO_ERR} Exiting with failures: {effective_f} file(s) did not complete successfully.{RESET}"
        )
        sys.exit(1)


if __name__ == "__main__":
    ensure_runtime_dependencies()
    install_runtime_signal_handlers()
    try:
        main()
    except KeyboardInterrupt:
        stop_elapsed_spinner()
        sys.exit(130)
