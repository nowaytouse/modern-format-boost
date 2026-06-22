"""Unified log directory and filename conventions (Python SSOT).

Must stay aligned with ``foundation::logging::LogConfig::unified_log_dir()`` and
``progress_mode::set_default_run_log_file`` (Rust).

Resolution order (both languages):

1. ``MFB_LOG_DIR`` — explicit override (session, CI, manual) — **rejected** if under MFB workspace ``logs/`` or ``target/training*``
2. ``MFB_HOME_ROOT/logs`` — state root (tests, app cache layout)
3. ``~/.modern_format_boost/logs`` — default persistent user location
4. System temp — last resort (Rust only, via ``delivery_log_dir_from_env_or_temp``)

Logs are never written under the git workspace (no ``<repo>/logs`` or ``target/training_*``).

Layout (under resolved log root):

| Pattern | Owner | Purpose |
|---------|-------|---------|
| ``{binary}_run_{stamp}.log`` | Rust ``progress_mode`` | Human-readable run trace |
| ``{binary}_{stamp}.jsonl`` | Rust ``init_logging`` | Structured tracing (audit) |
| ``MFB_Session_{stamp}.log`` | drag-and-drop | Session summary |
| ``verbose_{stamp}.log`` | drag-and-drop | ROUTED / HANDOFF / RSYNC audit |
| ``Bundle_{stamp}/`` | drag-and-drop | Archives session artifacts (worker logs, jsonl, verbose, audit, manifest) |
| ``session_audit_{stamp}.jsonl`` | drag-and-drop | Append-only session lifecycle audit (ROUTED / HANDOFF / pipeline) |
| ``mfb_audit_{day}.log`` | ``mfb_logger`` | Python tooling audit |
| ``diagnostic_report_{stamp}.txt`` | Rust ``verify`` | Post-batch integrity report |
| ``run_training_{stamp}.log`` | ``run_training.py`` | Training session log |
| ``training_session_audit.jsonl`` | ``run_training.py`` | Append-only lifecycle audit (start/phase/heartbeat/exit/signals) |
| ``training_session_exit.json`` | ``run_training.py`` | Last exit snapshot (reason, phase, code) — survives incomplete stdout logs |
| ``training_tier_audit.jsonl`` | ``run_training.py`` | Static tier probe stream (per session, archived on exit) |
| ``replica_audit_{stamp}.jsonl`` | ``run_training.py`` | Phase 1/2 replica audit |
| ``TrainingBundle_{stamp}/`` | ``run_training.py`` | Archived training session artifacts (move, not merge) |

Parallel training lanes (separate ``MFB_LOG_DIR`` per job):

| Lane | CLI |
|------|-----|
| ``static_high`` | ``--training-mode static --label high --no-loop`` |
| ``static_low`` | ``--training-mode static --label low --no-loop`` |
| ``loop_high`` | ``--training-mode loop --loop-intent-label high`` |
| ``loop_low`` | ``--training-mode loop --loop-intent-label low`` (grey-zone / uncertain loop) |

Session stamp: ``YYYYMMDD_HHMMSS`` (legacy ``YYYY-MM-DD_HH-MM-SS`` still parsed).
"""

from __future__ import annotations

import json
import os
import shutil
import sys
import tempfile
from datetime import datetime, timedelta
from pathlib import Path

# Align with Rust ``chrono::Local::now().format("%Y%m%d_%H%M%S")``.
MFB_LOG_SESSION_STAMP = "%Y%m%d_%H%M%S"
MFB_LOG_AUDIT_DAY_STAMP = "%Y%m%d"
MFB_DEFAULT_HOME_DIRNAME = ".modern_format_boost"


def find_mfb_workspace_root(start: Path | None = None) -> Path | None:
    """Return repo root when ``start`` (or cwd) is inside the MFB workspace tree."""
    try:
        dir_path = (start or Path.cwd()).expanduser().resolve()
    except OSError:
        return None
    for _ in range(16):
        if (dir_path / "Cargo.toml").is_file() and (dir_path / "crates").is_dir():
            return dir_path
        parent = dir_path.parent
        if parent == dir_path:
            break
        dir_path = parent
    return None


def is_forbidden_log_path(path: Path) -> bool:
    """True when ``path`` is ``<workspace>/logs`` or ``<workspace>/target/training*``."""
    try:
        resolved = path.expanduser().resolve()
    except OSError:
        resolved = path.expanduser()
    workspace = find_mfb_workspace_root(resolved) or find_mfb_workspace_root()
    if workspace is None:
        return False
    try:
        rel = resolved.relative_to(workspace)
    except ValueError:
        return False
    if not rel.parts:
        return False
    if rel.parts[0] == "logs":
        return True
    return (
        len(rel.parts) >= 2
        and rel.parts[0] == "target"
        and rel.parts[1].startswith("training")
    )


def _default_user_log_dir() -> Path:
    if home := os.getenv("HOME"):
        return Path(home) / MFB_DEFAULT_HOME_DIRNAME / "logs"
    return Path(tempfile.gettempdir()) / "modern_format_boost_logs"


def persistent_log_dir() -> Path:
    """User/state log root without ``MFB_LOG_DIR`` override."""
    if home_root := os.getenv("MFB_HOME_ROOT"):
        candidate = Path(home_root).expanduser() / "logs"
        if not is_forbidden_log_path(candidate):
            return candidate
        fallback = _default_user_log_dir()
        if not is_forbidden_log_path(fallback):
            print(
                f"[MFB] Refusing workspace MFB_HOME_ROOT log dir {candidate} — using {fallback}",
                file=sys.stderr,
            )
            return fallback
    return _default_user_log_dir()


def coerce_log_dir(candidate: Path) -> Path:
    """Redirect forbidden workspace log paths to ``persistent_log_dir()``."""
    if not is_forbidden_log_path(candidate):
        return candidate
    fallback = persistent_log_dir()
    if is_forbidden_log_path(fallback):
        fallback = _default_user_log_dir()
    if is_forbidden_log_path(fallback):
        fallback = Path(tempfile.gettempdir()) / "modern_format_boost_logs"
    print(
        f"[MFB] Refusing workspace log dir {candidate} — using {fallback}",
        file=sys.stderr,
    )
    return fallback


def _log_dir_from_env() -> Path | None:
    raw = (os.getenv("MFB_LOG_DIR") or "").strip()
    if not raw:
        return None
    return coerce_log_dir(Path(raw))


def unified_log_dir() -> Path:
    """Resolve log root (same priority as ``LogConfig::unified_log_dir``)."""
    if explicit := _log_dir_from_env():
        return explicit
    return persistent_log_dir()


def ensure_unified_log_dir() -> Path:
    """Return unified log root, creating it and pinning ``MFB_LOG_DIR`` for child processes."""
    log_dir = unified_log_dir()
    log_dir.mkdir(parents=True, exist_ok=True)
    os.environ["MFB_LOG_DIR"] = str(log_dir.resolve())
    return log_dir


def session_timestamp() -> datetime:
    """Current local time for session/bundle/run log filenames."""
    return datetime.now()


def format_session_stamp(when: datetime | None = None) -> str:
    """Filename-safe session stamp (``YYYYMMDD_HHMMSS``)."""
    return (when or session_timestamp()).strftime(MFB_LOG_SESSION_STAMP)


def parse_session_stamp(stamp: str) -> datetime:
    """Parse session stamp; accepts legacy ``YYYY-MM-DD_HH-MM-SS`` bundles."""
    for fmt in (MFB_LOG_SESSION_STAMP, "%Y-%m-%d_%H-%M-%S"):
        try:
            return datetime.strptime(stamp, fmt)
        except ValueError:
            continue
    raise ValueError(f"unrecognized session log stamp: {stamp!r}")


# Parallel training lanes (each lane = its own ``MFB_LOG_DIR`` subdirectory).
TRAINING_LOG_LANES: tuple[str, ...] = (
    "static_high",
    "static_low",
    "loop_high",
    "loop_low",
)
LEGACY_TRAINING_LOG_LANES: tuple[str, ...] = (
    "static",
    "all_high",
    "loop",
    "loop_video",
)
TRAINING_BUNDLE_PREFIX = "TrainingBundle_"
SESSION_BUNDLE_PREFIX = "Bundle_"


def training_lane_slug(
    *,
    training_mode: str,
    label: str | None = None,
    loop_intent_label: str = "auto",
) -> str:
    """Stable lane name for log directory layout (SSOT for launch scripts)."""
    mode = (training_mode or "all").strip().lower()
    if mode == "static":
        if label == "low":
            return "static_low"
        if label == "high":
            return "static_high"
        return "static"
    if mode == "loop":
        li = (loop_intent_label or "auto").strip().lower()
        if li == "high":
            return "loop_high"
        if li == "low":
            return "loop_low"
        if li == "video":
            return "loop_video"
        return "loop"
    if mode == "all":
        li = (loop_intent_label or "auto").strip().lower()
        if li == "high":
            return "all_high"
        return "all"
    return mode


def ensure_training_session_stamp() -> str:
    """Pin ``MFB_TRAINING_SESSION_STAMP`` for replica/run log filenames (local time)."""
    stamp = (os.environ.get("MFB_TRAINING_SESSION_STAMP") or "").strip()
    if not stamp:
        stamp = format_session_stamp()
        os.environ["MFB_TRAINING_SESSION_STAMP"] = stamp
    return stamp


def training_lane_pid_is_active(lane_dir: Path) -> bool:
    """True when ``run_training.pid`` points at a live process."""
    pid_file = lane_dir / "run_training.pid"
    if not pid_file.is_file():
        return False
    try:
        pid = int(pid_file.read_text(encoding="utf-8").strip())
    except ValueError:
        return False
    if pid <= 0:
        return False
    try:
        os.kill(pid, 0)
    except OSError:
        return False
    return True


def archive_training_session_bundle(
    log_dir: Path,
    session_stamp: str,
    *,
    scope: str | None = None,
) -> Path | None:
    """Move session artifacts into ``TrainingBundle_{stamp}/`` (no content merge).

    Aligns with drag-and-drop ``Bundle_{stamp}/``: keeps files separate to avoid
    giant merged logs; writes a small manifest for auditability.
    """
    log_dir = log_dir.expanduser()
    if not log_dir.is_dir():
        return None
    stamp = (session_stamp or "").strip()
    if not stamp:
        return None

    bundle = log_dir / f"{TRAINING_BUNDLE_PREFIX}{stamp}"
    moved: list[str] = []

    def _move(name: str) -> None:
        src = log_dir / name
        if not src.is_file():
            return
        bundle.mkdir(parents=True, exist_ok=True)
        dest = bundle / name
        if dest.exists():
            dest.unlink()
        shutil.move(str(src), str(dest))
        moved.append(name)

    exit_snapshot: dict | None = None
    exit_src = log_dir / "training_session_exit.json"
    if exit_src.is_file():
        try:
            raw_exit = json.loads(exit_src.read_text(encoding="utf-8"))
            if isinstance(raw_exit, dict):
                exit_snapshot = raw_exit
        except (OSError, json.JSONDecodeError):
            exit_snapshot = None

    _move(f"run_training_{stamp}.log")
    _move("training_session_audit.jsonl")
    _move("training_session_exit.json")
    _move(f"replica_audit_{stamp}.jsonl")
    tier_live = log_dir / "training_tier_audit.jsonl"
    if tier_live.is_file():
        bundle.mkdir(parents=True, exist_ok=True)
        dest = bundle / f"training_tier_audit_{stamp}.jsonl"
        if dest.exists():
            dest.unlink()
        shutil.move(str(tier_live), str(dest))
        moved.append(dest.name)

    if not moved:
        return None

    manifest: dict = {
        "session_stamp": stamp,
        "training_lane": (os.environ.get("MFB_TRAINING_LANE") or "").strip() or None,
        "scope": scope,
        "files": sorted(moved),
        "archived_at": datetime.now().isoformat(timespec="seconds"),
    }
    if exit_snapshot is not None:
        manifest["exit"] = exit_snapshot
    manifest_path = bundle / "manifest.json"
    manifest_path.write_text(
        json.dumps(manifest, ensure_ascii=False, indent=2) + "\n",
        encoding="utf-8",
    )
    return bundle


def append_jsonl_audit_record(audit_path: Path, event: str) -> None:
    """Append one structured audit record to an existing ``.jsonl`` file."""
    stamp = datetime.now().isoformat(timespec="seconds")
    record = {"ts": stamp, "event": event}
    with open(audit_path.expanduser(), "a", encoding="utf-8") as audit_f:
        audit_f.write(json.dumps(record, ensure_ascii=False) + "\n")


def archive_drag_drop_session_bundle(
    log_dir: Path,
    session_stamp: str,
    *,
    session_log: Path | None = None,
    verbose_log: Path | None = None,
    session_audit: Path | None = None,
    session_started_at: datetime | None = None,
) -> Path | None:
    """Move drag-and-drop session artifacts into ``Bundle_{stamp}/`` (move, not merge).

    Archives worker ``img_*`` / ``vid_*`` logs and jsonl traces, session summary,
    verbose audit, structured session audit, and diagnostic reports from the session window.
    """
    log_dir = log_dir.expanduser()
    if not log_dir.is_dir():
        return None
    stamp = (session_stamp or "").strip()
    if not stamp:
        return None

    try:
        session_dt = session_started_at or parse_session_stamp(stamp)
    except ValueError:
        session_dt = datetime.now()

    def _in_session_window(path: Path) -> bool:
        try:
            mtime = datetime.fromtimestamp(path.stat().st_mtime)
        except OSError:
            return False
        return mtime >= session_dt - timedelta(seconds=10)

    bundle = log_dir / f"{SESSION_BUNDLE_PREFIX}{stamp}"
    moved: list[str] = []

    def _move_path(src: Path) -> None:
        if not src.is_file():
            return
        try:
            resolved = src.expanduser().resolve()
            log_resolved = log_dir.expanduser().resolve()
            if resolved.parent != log_resolved:
                return
        except OSError:
            if src.parent.expanduser() != log_dir.expanduser():
                return
        bundle.mkdir(parents=True, exist_ok=True)
        dest = bundle / src.name
        if dest.exists():
            dest.unlink()
        shutil.move(str(src), str(dest))
        moved.append(src.name)

    explicit = [session_log, verbose_log, session_audit]
    for path in explicit:
        if path is not None:
            _move_path(path)

    patterns = (
        "img_*.log",
        "vid_*.log",
        "img_*.jsonl",
        "vid_*.jsonl",
        f"MFB_Session_{stamp}.log",
        f"MFB_*_{stamp}.log",
        f"verbose_{stamp}.log",
        f"session_audit_{stamp}.jsonl",
        "diagnostic_report_*.txt",
    )
    for pattern in patterns:
        for candidate in sorted(log_dir.glob(pattern), key=os.path.getmtime):
            if not _in_session_window(candidate):
                continue
            _move_path(candidate)

    if not moved:
        return None

    manifest: dict = {
        "session_stamp": stamp,
        "session_id": (os.environ.get("MFB_SESSION_ID") or "").strip() or None,
        "files": sorted(set(moved)),
        "archived_at": datetime.now().isoformat(timespec="seconds"),
        "log_root": str(log_dir.resolve()),
    }
    manifest_path = bundle / "manifest.json"
    manifest_path.write_text(
        json.dumps(manifest, ensure_ascii=False, indent=2) + "\n",
        encoding="utf-8",
    )
    return bundle


def iter_training_log_dirs(log_root: Path) -> list[Path]:
    """Log root plus known parallel training lane directories."""
    log_root = log_root.expanduser()
    dirs = [log_root]
    for lane in (*TRAINING_LOG_LANES, *LEGACY_TRAINING_LOG_LANES):
        lane_dir = log_root / lane
        if lane_dir.is_dir():
            dirs.append(lane_dir)
    return dirs
