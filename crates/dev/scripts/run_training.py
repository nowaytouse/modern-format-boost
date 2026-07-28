#!/usr/bin/env python3
"""Training pipeline driver — Strictly aligned with the Multi-Scenario architecture.
Version 6.4: RUST TIER PROBE (collect=ingest entropy) + TIER AUDIT JSONL/JSONB
- Safety: MANDATORY PHYSICAL REPLICAS (shutil.copy2). New Inode for every sample.
- Integrity: ZERO SILENT FORGERY. Only real entropy/physics admitted to DB.
- Source trees: never write sidecars into `training_rules` local_dirs; replicas live only
  under `tempfile` + optional `MFB_TRAINING_SOURCE_MAP` pointing at a JSON file in that
  temp dir (restored after each batch; caller-supplied env is never cleared unless we set it).
- Helper subprocesses (``training_pipeline``, ``cargo``, ``pip`` probes) use a sanitized env
  without ``MFB_TRAINING_SOURCE_MAP`` so they never read a deleted batch-local map path.
- Rust ingest CLIs (``train_quality`` / ``train_knn``) use ``ingest_rust_cli_env``: canonical
  ``MFB_PG_CONNSTR`` and a replica map only when the map file still exists (stale shell env
  is dropped). Remote downloads use ``.mfb_part`` staging + replace to avoid truncated files.
- Storage Guard: 10GB Safety Threshold. Auto-suspend on disk pressure.
- Performance: C-API Batch Ingestion (Single DB session per 400 files).
- Runtime fill (opt-in after successful ingest): loop_intent KNN stats + directory scores,
  image_quality LightGBM when mature, then reports. Default is ingest-only; pass
  --fill-runtime-assets to run post-ingest finalize (may take multiple passes while pending).
- Default: ingest + DB tier audit. Use --dry-run for plan/JSONL only (no PostgreSQL writes).
- Training scope: `--training-mode all|static|loop` (default all) plus `--label` and
  `--loop-intent-label` for isolated append runs (e.g. static low only, loop high/low).
- Entry guard: refuse shell wrappers (``*.sh`` / ``nohup`` chains) and non-canonical
  ``argv[0]``; allow only direct ``python3 …/run_training.py``, ``--background`` self-reexec,
  or ``training_pipeline.py`` subprocess (``MFB_TRAINING_INVOKER``).
"""

from __future__ import annotations

import argparse
import json
import mimetypes
import os
import re
import shutil
import subprocess
import sys
import tempfile
import time
import urllib.request
from collections import Counter
from collections.abc import Callable, Iterator, Mapping, Sequence
from dataclasses import dataclass
from datetime import datetime, timezone
from math import ceil
from pathlib import Path
from typing import Any, Final, NamedTuple, cast

from mfb_ui_tokens import pick_symbol

_SCRIPTS_DIR = Path(__file__).resolve().parent
if str(_SCRIPTS_DIR) not in sys.path:
    sys.path.insert(0, str(_SCRIPTS_DIR))

from fabrication_policy import (
    fail_closed_training_enabled,
    run_training_except_policy,
    training_quality_exit,
)
from media_scope import (
    ANIMATION_CAPABLE_IMAGE_EXTS as MEDIA_SCOPE_ANIMATION_CAPABLE_IMAGE_EXTS,
)
from media_scope import (
    IMG_EXTS as MEDIA_SCOPE_IMAGE_EXTS,
)
from media_scope import (
    PURE_VIDEO_EXTS as MEDIA_SCOPE_PURE_VIDEO_EXTS,
)
from media_scope import (
    MediaProbeError,
    detect_true_format,
    is_animated_gif,
    is_animated_jxl,
    is_animated_png,
    is_animated_webp,
    is_probably_animated_isobmff,
)
from mfb_config_load import ensure_allowed_keys
from mfb_dylib import apply_foundation_lib_env, ensure_foundation_dylib
from mfb_entry_guard import (
    QUALITY_MODEL_PYTHON_ENV,
    TRAINING_SOURCE_MAP_ENV,
    detach_to_background,
    guard_main,
    helper_env,
    run_delegated,
)
from mfb_entry_guard import (
    run_rust_ingest as _guarded_run_rust_ingest,
)
from mfb_performance import ScanGovernor
from mfb_training_scan import (
    ScanPlanningError,
    ScanSegment,
    format_scan_plan_summary,
    plan_scan_segments,
)

# Cross-layer fail-closed contract:
# - Rust ingest CLIs enforce
#   `foundation::training_entry_guard::assert_train_quality_entry` and
#   `foundation::training_entry_guard::assert_train_knn_entry`.
# - Helper Rust bins enforce
#   `foundation::entry_guard::assert_pipeline_tool_entry` and
#   `foundation::entry_guard::assert_dev_tool_entry`.
# - Python entry points remain `guard_main` / `run_delegated` / `invoke_script`
#   controlled, propagate `MFB_TRAINING_INVOKER`, and route DB-bound Rust calls
#   through `rust_ingest_env`.
# - Shell-wrapper policy is fail-closed: `guard_main` enforces "refusing shell-wrapped"
#   invocations before execute/ingest paths can mutate state.


def discover_root(script_path: Path) -> Path:
    expected_relative = Path("crates") / "dev" / "scripts" / script_path.name
    for candidate in script_path.parents:
        if (candidate / "Cargo.toml").exists() and (
            candidate / expected_relative
        ).exists():
            return candidate
    raise SystemExit(
        f"Could not locate repository root from {script_path}; "
        f"expected Cargo.toml and {expected_relative.as_posix()}"
    )


# Paths
ROOT = discover_root(Path(__file__).resolve())
SCRIPTS_DIR = Path(__file__).resolve().parent
RULES_FILE = ROOT / "crates" / "dev" / "src" / "config" / "training_rules.json"
RULES_LOCAL_FILE = RULES_FILE.parent / "training_rules.local.json"

from mfb_log_paths import (
    TRAINING_LOG_LANES,
    archive_training_session_bundle,
    coerce_log_dir,
    ensure_training_session_stamp,
    ensure_unified_log_dir,
    persistent_log_dir,
    training_lane_pid_is_active,
    training_lane_slug,
    unified_log_dir,
)
from mfb_training_session_audit import (
    TrainingSessionRecorder,
    format_exception,
    summarize_argv,
)

TRAINING_LOG_DIR = unified_log_dir()
BACKGROUND_PID_FILE = TRAINING_LOG_DIR / "run_training.pid"
_TRAINING_SESSION: TrainingSessionRecorder | None = None


def training_session_heartbeat(**fields: Any) -> None:
    rec = _TRAINING_SESSION
    if rec is not None:
        rec.maybe_heartbeat(**fields)


def pin_training_log_dir() -> Path:
    """Pin training logs to the unified home log root (never ``target/training_*``)."""
    global TRAINING_LOG_DIR
    TRAINING_LOG_DIR = ensure_unified_log_dir()
    return TRAINING_LOG_DIR


INGEST_PROFILE_ALLOWED_KEYS: Final = frozenset(
    {
        "_comment",
        "training_mode",
        "balance",
        "max_high",
        "max_low",
        "max_loop",
        "max_non_loop",
        "fill_runtime_assets",
        "no_balance_complexity",
    }
)
RULES_SCHEMA_VERSION: Final = 1
TRAINING_PIPELINE_SCRIPT = ROOT / "crates" / "dev" / "scripts" / "training_pipeline.py"
TRAINING_REQUIREMENTS_FILE = ROOT / "crates" / "dev" / "scripts" / "requirements.txt"
TRAIN_BIN_QUALITY = ROOT / "target" / "debug" / "train_quality"
TRAIN_BIN_KNN = ROOT / "target" / "debug" / "train_knn"
WORKSPACE_VENV_PYTHON = ROOT / ".venv" / "bin" / "python"
RUN_TRAINING_SCRIPT = SCRIPTS_DIR / "run_training.py"
DEFAULT_CONNSTR = "postgresql://localhost/modern_format_boost"
# Hard SSOT ingest ceilings — enforced in enforce_training_db_caps() after profile merge.
STATIC_QUALITY_DB_CAP_PER_CLASS: Final = 4000
LOOP_INTENT_DB_CAP_PER_CLASS: Final = 2000
FOUR_LANE_STATIC_QUALITY_DB_CAP: Final = "1450"
FOUR_LANE_LOOP_INTENT_DB_CAP: Final = "450"
FOUR_LANE_SPECS: Final[tuple[tuple[str, list[str]], ...]] = (
    (
        "static_high",
        [
            "--training-mode",
            "static",
            "--label",
            "high",
            "--no-loop",
            "--no-fill-runtime-assets",
            "--max-high",
            FOUR_LANE_STATIC_QUALITY_DB_CAP,
        ],
    ),
    (
        "static_low",
        [
            "--training-mode",
            "static",
            "--label",
            "low",
            "--no-loop",
            "--no-fill-runtime-assets",
            "--max-low",
            FOUR_LANE_STATIC_QUALITY_DB_CAP,
        ],
    ),
    (
        "loop_high",
        [
            "--training-mode",
            "loop",
            "--loop-intent-label",
            "high",
            "--max-loop",
            FOUR_LANE_LOOP_INTENT_DB_CAP,
        ],
    ),
    (
        "loop_low",
        [
            "--training-mode",
            "loop",
            "--loop-intent-label",
            "video",
            "--max-non-loop",
            FOUR_LANE_LOOP_INTENT_DB_CAP,
        ],
    ),
)
FOUR_LANE_KNOWN_LANES: Final[frozenset[str]] = frozenset(
    lane for lane, _ in FOUR_LANE_SPECS
)
DB_TRAIN_CLOSURE_DOC_MARKERS: Final[tuple[tuple[tuple[str, ...], str], ...]] = (
    (
        ("AUDIT_SINGLE_SOURCE_OF_TRUTH_WITH_VERIFY.md", "SSOT.md"),
        "DB_TRAIN_BOUNDED_AUDIT=17/17",
    ),
    (
        ("SINGLE_SOURCE_OF_TRUTH_WITH_CLOSURE.md", "SSOT.md"),
        "DB_TRAIN_FOUR_LANE_RESET_GATE=4/4",
    ),
    (
        ("SINGLE_SOURCE_OF_TRUTH_WITH_CLOSURE.md", "SSOT.md"),
        "DB_TRAIN_LAUNCHER_CLOSURE_DOC_GATE=4/4",
    ),
    (
        ("SINGLE_SOURCE_OF_TRUTH_WITH_CLOSURE.md", "SSOT.md"),
        "DB_TRAIN_TRAINING_LAUNCH_ALLOWED=yes",
    ),
)
TRAINING_VERBOSE_ENV = "MFB_TRAINING_VERBOSE"
TRAINING_INGEST_PROGRESS_ENV = "MFB_TRAINING_INGEST_PROGRESS"
TRAINING_STATIC_TIER_PROGRESS_ENV = "MFB_STATIC_TIER_SCAN_INTERVAL"
TRAINING_SCAN_HEARTBEAT_SECS_ENV = "MFB_TRAINING_SCAN_HEARTBEAT_SECS"
TRAINING_TIER_AUDIT_ENV = "MFB_TRAINING_TIER_AUDIT"
TRAINING_TIER_AUDIT_STREAM_ENV = "MFB_TRAINING_TIER_AUDIT_STREAM"
TRAINING_REPLICA_AUDIT_ENV = "MFB_TRAINING_REPLICA_AUDIT"
TRAINING_ERROR_MODE_ENV = "MFB_TRAINING_ERROR_MODE"
TRAINING_ERROR_MODE_FAIL_FAST = "fail-fast"
TRAINING_ERROR_MODE_LOG_AND_CONTINUE = "log-and-continue"
STATIC_TIER_SLOW_PROBE_SECS = 8.0
DEFAULT_STATIC_TIER_SCAN_INTERVAL = 200
DEFAULT_TRAINING_SCAN_HEARTBEAT_SECS = 15.0
REPLICA_PROGRESS_INTERVAL = 25
TIER_AUDIT_FLUSH_EVERY = 500
TIER_AUDIT_RECORDS: list[JsonObject] = []
_tier_audit_stream: Any = None
_tier_audit_stream_path: Path | None = None
LAST_BALANCE_SUMMARY: JsonObject | None = None

BATCH_SIZE = 400
MAX_RUNTIME_FILL_PASSES: Final = 5
DISK_SAFETY_THRESHOLD_GB = 10
USER_AGENT = "ModernFormatBoost-Training/1.0"
IMAGE_EXTS = {ext.lstrip(".") for ext in MEDIA_SCOPE_IMAGE_EXTS}
ANIMATED_IMAGE_EXTS = {
    ext.lstrip(".") for ext in MEDIA_SCOPE_ANIMATION_CAPABLE_IMAGE_EXTS
}
VIDEO_EXTS = {ext.lstrip(".") for ext in MEDIA_SCOPE_PURE_VIDEO_EXTS}
JUNK_EXTS = {
    ".ds_store",
    ".xmp",
    ".txt",
    ".md",
    ".json",
    ".ini",
    ".db",
    ".lnk",
    ".bak",
    ".tmp",
}


def training_verbose_enabled(explicit_flag: bool) -> bool:
    """Per-file ingest lines; default off to keep logs reconcilable at a glance."""
    if explicit_flag:
        return True
    env_raw = (os.environ.get(TRAINING_VERBOSE_ENV) or "").strip().lower()
    return env_raw in ("1", "true", "yes", "on")


def training_env_truthy(name: str, *, default: bool) -> bool:
    raw = (os.environ.get(name) or "").strip().lower()
    if not raw:
        return default
    return raw not in ("0", "false", "no", "off")


def training_error_mode() -> str:
    raw = (os.environ.get(TRAINING_ERROR_MODE_ENV) or "").strip().lower()
    if not raw:
        return TRAINING_ERROR_MODE_FAIL_FAST
    if raw in {"fail-fast", "failfast", "abort", "strict"}:
        return TRAINING_ERROR_MODE_FAIL_FAST
    if raw in {"log-and-continue", "log_and_continue", "continue", "report"}:
        return TRAINING_ERROR_MODE_LOG_AND_CONTINUE
    raise ValueError(
        f"invalid {TRAINING_ERROR_MODE_ENV}={raw!r}; expected "
        f"{TRAINING_ERROR_MODE_FAIL_FAST} or {TRAINING_ERROR_MODE_LOG_AND_CONTINUE}"
    )


def training_ingest_fail_fast() -> bool:
    return training_error_mode() == TRAINING_ERROR_MODE_FAIL_FAST


def abort_training_sample_failure_if_fail_fast(context: str, message: str) -> None:
    if training_ingest_fail_fast():
        training_quality_exit(1, f"     [FAIL-FAST] {context}: {message}")


def training_ingest_failure_exit_code(
    *, total_success: int, total_fail_other: int, total_fail_label_conflict: int
) -> int:
    total_fail = total_fail_other + total_fail_label_conflict
    if total_fail == 0:
        return 0
    if total_success == 0:
        return 2
    if training_ingest_fail_fast():
        return 1
    return 0


def training_ingest_progress_enabled() -> bool:
    """Rust C-API `[INGEST-RUST]` stderr progress; default on."""
    return training_env_truthy(TRAINING_INGEST_PROGRESS_ENV, default=True)


def parse_positive_int_env(name: str, default: int) -> int:
    raw = (os.environ.get(name) or "").strip()
    if not raw:
        return default
    try:
        value = int(raw)
    except ValueError:
        msg = f"invalid {name}={raw!r}; expected integer >= 1"
        if fail_closed_training_enabled():
            raise ValueError(msg)
        print(
            f"  [WARN] Ignoring {msg}. Using {default}.",
            file=sys.stderr,
            flush=True,
        )
        return default
    if value < 1:
        msg = f"invalid {name}={raw!r}; expected integer >= 1"
        if fail_closed_training_enabled():
            raise ValueError(msg)
        print(
            f"  [WARN] Ignoring {msg}. Using {default}.",
            file=sys.stderr,
            flush=True,
        )
        return default
    return value


def parse_positive_float_env(name: str, default: float) -> float:
    raw = (os.environ.get(name) or "").strip()
    if not raw:
        return default
    try:
        value = float(raw)
    except ValueError:
        msg = f"invalid {name}={raw!r}; expected float > 0"
        if fail_closed_training_enabled():
            raise ValueError(msg)
        print(
            f"  [WARN] Ignoring {msg}. Using {default}.",
            file=sys.stderr,
            flush=True,
        )
        return default
    if value <= 0:
        msg = f"invalid {name}={raw!r}; expected float > 0"
        if fail_closed_training_enabled():
            raise ValueError(msg)
        print(
            f"  [WARN] Ignoring {msg}. Using {default}.",
            file=sys.stderr,
            flush=True,
        )
        return default
    return value


def four_lane_python_exe() -> str:
    if WORKSPACE_VENV_PYTHON.is_file():
        return str(WORKSPACE_VENV_PYTHON)
    return sys.executable


def stop_four_lane(lane_dir: Path) -> None:
    pid_file = lane_dir / "run_training.pid"
    if not pid_file.is_file():
        return
    try:
        pid = int(pid_file.read_text(encoding="utf-8").strip())
    except ValueError:
        pid_file.unlink(missing_ok=True)
        return
    if pid > 0:
        try:
            os.kill(pid, 15)
        except OSError:
            pid_file.unlink(missing_ok=True)
            return
        deadline = time.monotonic() + 8.0
        while time.monotonic() < deadline:
            try:
                os.kill(pid, 0)
            except OSError:
                pid_file.unlink(missing_ok=True)
                return
            time.sleep(0.25)
        try:
            os.kill(pid, 9)
        except OSError:
            pass
    pid_file.unlink(missing_ok=True)


def latest_lane_log_stamp(lane_dir: Path) -> str | None:
    logs = sorted(
        lane_dir.glob("run_training_*.log"),
        key=lambda path: path.stat().st_mtime,
        reverse=True,
    )
    for log_path in logs:
        stem = log_path.stem
        if stem.startswith("run_training_"):
            stamp = stem.removeprefix("run_training_").strip()
            if stamp:
                return stamp
    return None


def record_stale_lane_death(lane_dir: Path, *, lane: str) -> None:
    pid_file = lane_dir / "run_training.pid"
    root_exit_path = lane_dir / "training_session_exit.json"
    if not pid_file.is_file() or root_exit_path.is_file():
        return
    try:
        pid = int(pid_file.read_text(encoding="utf-8").strip())
    except ValueError:
        pid = 0
    stale_stamp = latest_lane_log_stamp(lane_dir)
    finished_at = datetime.now(timezone.utc)
    bundle_stamp = stale_stamp or f"stale_pid_{finished_at:%Y%m%d_%H%M%S}"
    exit_dir = lane_dir / f"TrainingBundle_{bundle_stamp}"
    exit_dir.mkdir(parents=True, exist_ok=True)
    exit_path = exit_dir / "training_session_exit.json"
    payload: dict[str, object] = {
        "session_stamp": stale_stamp,
        "lane": lane,
        "pid": pid,
        "exit_code": 137,
        "reason": "stale-pid-dead-process",
        "phase": "unknown",
        "interrupted": True,
        "finished_at": finished_at.isoformat(),
        "diagnostic": (
            "run_training.pid pointed at a dead process and no Python exit "
            "snapshot existed; previous worker likely died via SIGKILL/OOM or "
            "host shutdown before atexit/signal handlers could run"
        ),
        "root_exit_path_reserved_for_active_lane": str(root_exit_path),
    }
    exit_path.write_text(
        json.dumps(payload, ensure_ascii=False, indent=2) + "\n",
        encoding="utf-8",
    )
    with open(
        lane_dir / "training_session_audit.jsonl", "a", encoding="utf-8"
    ) as audit:
        audit.write(
            json.dumps(
                {"event": "stale_pid_death_detected", **payload},
                ensure_ascii=False,
            )
            + "\n"
        )


def audit_four_lane_training_state(log_root: Path, lanes: Sequence[str]) -> None:
    for lane in lanes:
        lane_dir = log_root / lane
        exit_path = lane_dir / "training_session_exit.json"
        pid_path = lane_dir / "run_training.pid"
        if training_lane_pid_is_active(lane_dir):
            status = "running"
            detail = f"pid={pid_path.read_text(encoding='utf-8').strip()}"
        elif exit_path.is_file():
            try:
                snapshot = json.loads(exit_path.read_text(encoding="utf-8"))
            except (OSError, json.JSONDecodeError) as exc:
                status = "audit-unreadable"
                detail = str(exc)
            else:
                exit_code = snapshot.get("exit_code")
                reason = snapshot.get("reason")
                phase = snapshot.get("phase")
                status = "completed" if exit_code == 0 else "failed"
                detail = f"exit_code={exit_code} phase={phase} reason={reason}"
        elif pid_path.is_file():
            status = "failed"
            detail = "stale pid without exit snapshot"
        else:
            status = "not-started"
            detail = "no pid or exit snapshot"
        print(f"  [AUDIT] lane={lane} status={status} {detail}")


def start_four_lane(
    *,
    lane: str,
    argv_tail: list[str],
    log_root: Path,
    stamp: str,
    connstr: str,
    dry_run: bool,
) -> tuple[str, int, Path]:
    lane_dir = log_root / lane
    lane_dir.mkdir(parents=True, exist_ok=True)
    if training_lane_pid_is_active(lane_dir):
        stop_four_lane(lane_dir)
    else:
        record_stale_lane_death(lane_dir, lane=lane)
    log_path = lane_dir / f"run_training_{stamp}.log"
    if dry_run:
        return lane, 0, log_path

    env = {
        **os.environ,
        "PYTHONUNBUFFERED": "1",
        "MFB_LOG_DIR": str(lane_dir.resolve()),
        "MFB_TRAINING_ALLOW_PARALLEL": "1",
        "MFB_TRAINING_SESSION_STAMP": stamp,
        "MFB_PG_CONNSTR": connstr,
        "MFB_TRAINING_LANE": lane,
        TRAINING_ERROR_MODE_ENV: TRAINING_ERROR_MODE_LOG_AND_CONTINUE,
        "SHARED_UTILS_LIB_PATH": ensure_foundation_dylib(),
    }

    launcher = TrainingSessionRecorder(lane_dir, session_stamp=stamp)
    launcher.emit(
        "launcher_spawn",
        child_argv=argv_tail,
        log_path=str(log_path),
        allow_parallel=True,
    )
    with open(log_path, "ab", buffering=0) as log_f:
        header = (
            f"\n=== run_training lane={lane} stamp={stamp} "
            f"launcher_pid={os.getpid()} ===\n"
        ).encode()
        log_f.write(header)
        proc = subprocess.Popen(
            [four_lane_python_exe(), "-u", str(RUN_TRAINING_SCRIPT), *argv_tail],
            cwd=str(ROOT),
            stdin=subprocess.DEVNULL,
            stdout=log_f,
            stderr=subprocess.STDOUT,
            start_new_session=True,
            env=env,
        )
    try:
        (lane_dir / "run_training.pid").write_text(f"{proc.pid}\n", encoding="utf-8")
    except OSError as exc:
        terminated = False
        terminate_error: str | None = None
        try:
            os.kill(proc.pid, 15)
            terminated = True
        except OSError as stop_exc:
            terminate_error = str(stop_exc)
            print(
                f"  [ERROR] lane={lane} pid file write failed and spawned pid "
                f"{proc.pid} could not be stopped: {stop_exc}",
                file=sys.stderr,
            )
        try:
            launcher.emit(
                "launcher_pid_write_failed",
                child_pid=proc.pid,
                error=str(exc),
                terminated=terminated,
                terminate_error=terminate_error,
            )
        except OSError as audit_exc:
            print(
                f"  [WARN] lane={lane} failed to audit pid-file write failure: {audit_exc}",
                file=sys.stderr,
            )
        raise RuntimeError(
            f"lane={lane} pid file write failed after spawn; child pid={proc.pid}; "
            f"terminated={terminated}; see {log_path}: {exc}"
        ) from exc
    launcher.emit("launcher_spawned", child_pid=proc.pid)
    time.sleep(2.5)
    early_code = proc.poll()
    if early_code is not None:
        (lane_dir / "run_training.pid").unlink(missing_ok=True)
        launcher.emit("launcher_failed_early", child_pid=proc.pid, exit_code=early_code)
        raise RuntimeError(
            f"lane={lane} exited during bootstrap (code={early_code}); see {log_path}"
        )
    return lane, proc.pid, log_path


def four_lane_slug_from_tail(tail: list[str]) -> str:
    mode = (
        tail[tail.index("--training-mode") + 1]
        if "--training-mode" in tail
        else "static"
    )
    label = tail[tail.index("--label") + 1] if "--label" in tail else None
    loop_li = (
        tail[tail.index("--loop-intent-label") + 1]
        if "--loop-intent-label" in tail
        else "auto"
    )
    return training_lane_slug(
        training_mode=mode, label=label, loop_intent_label=loop_li
    )


def ensure_reset_db_before_training(*, reset_db: bool, dry_run: bool) -> None:
    if not reset_db and not dry_run:
        raise SystemExit(
            "  [ERROR] --reset-db is required before four-lane training; "
            "refusing to start with potentially polluted cross-run DB state"
        )


def training_cache_base_dir() -> Path:
    base = (
        os.environ.get("MFB_HOME_ROOT")
        or os.environ.get("HOME")
        or os.environ.get("USERPROFILE")
        or str(ROOT)
    )
    path = Path(base).expanduser()
    if path.name != ".modern_format_boost":
        path = path / ".modern_format_boost"
    return path / "cache"


def image_quality_model_dir() -> Path:
    return training_cache_base_dir() / "models" / "image_quality"


def image_quality_model_artifact_paths() -> list[Path]:
    model_dir = image_quality_model_dir()
    paths = {
        model_dir / "lightgbm_model.txt",
        model_dir / "lightgbm_model.metadata.json",
    }
    for env_name in (
        "MFB_IMAGE_QUALITY_MODEL_PATH",
        "MFB_IMAGE_QUALITY_MODEL_METADATA_PATH",
    ):
        explicit = os.environ.get(env_name)
        if explicit and explicit.strip():
            paths.add(Path(explicit).expanduser())
    if model_dir.is_dir():
        paths.update(model_dir.glob("lightgbm_model*"))
        paths.update(model_dir.glob("*.metadata.json"))
    return sorted(paths)


def purge_image_quality_model_artifacts(*, dry_run: bool) -> list[Path]:
    purged: list[Path] = []
    for path in image_quality_model_artifact_paths():
        if not path.exists() and not path.is_symlink():
            continue
        if dry_run:
            purged.append(path)
            continue
        try:
            if path.is_dir() and not path.is_symlink():
                shutil.rmtree(path)
            else:
                path.unlink()
        except OSError as exc:
            raise SystemExit(
                f"  [ERROR] failed to purge LightGBM artifact {path}: {exc}"
            ) from exc
        purged.append(path)
    if purged:
        for path in purged:
            print(f"  [PURGE] removed stale LightGBM artifact: {path}")
    else:
        print("  [PURGE] no stale LightGBM artifacts found")
    return purged


def default_hardening_dir() -> Path:
    for candidate in (ROOT / "docs" / "hardening", ROOT / ".agents" / "harding"):
        if candidate.is_dir():
            return candidate
    return ROOT / "docs" / "hardening"


def ensure_db_training_closure_before_training(
    hardening_dir: Path | None = None,
) -> None:
    hardening_root = hardening_dir or default_hardening_dir()
    missing: list[str] = []
    for filenames, marker in DB_TRAIN_CLOSURE_DOC_MARKERS:
        errors: list[str] = []
        marker_found = False
        for filename in filenames:
            path = hardening_root / filename
            try:
                text = path.read_text(encoding="utf-8")
            except OSError as exc:
                errors.append(f"{filename}: unreadable: {exc}")
                continue
            if marker in text:
                marker_found = True
                break
            errors.append(f"{filename}: missing {marker!r}")
        if not marker_found:
            missing.append(" or ".join(errors))

    if missing:
        details = "; ".join(missing)
        raise SystemExit(
            "  [ERROR] DB/train closure gate is not closed; refusing to start "
            f"four-lane training. {details}"
        )


def resolve_launch_log_root(explicit_log_root: Path | None) -> Path:
    return coerce_log_dir((explicit_log_root or persistent_log_dir()).expanduser())


def add_four_lane_launcher_args(parser: argparse.ArgumentParser) -> None:
    parser.add_argument(
        "--four-lane",
        action="store_true",
        help=(
            "Launch the four detached training lanes from run_training.py itself. "
            "Requires --reset-db unless --dry-run is used."
        ),
    )
    parser.add_argument(
        "--rebuild-dylib",
        action="store_true",
        help="Force cargo rustc -p foundation --lib --crate-type cdylib and refresh .modern_format_boost/artifacts dylib before four-lane launch.",
    )
    parser.add_argument("--stop", action="store_true", help="Stop four-lane workers.")
    parser.add_argument("--log-root", type=Path, default=None)
    parser.add_argument(
        "--lane",
        type=str,
        nargs="+",
        metavar="LANE",
        help=(
            "Restrict four-lane start/stop to lane(s). "
            f"Known lanes: {', '.join(sorted(FOUR_LANE_KNOWN_LANES))}."
        ),
    )


def run_four_lane_launcher(args: argparse.Namespace) -> None:
    log_root = resolve_launch_log_root(args.log_root)
    log_root.mkdir(parents=True, exist_ok=True)

    if args.lane:
        unknown = sorted(set(args.lane) - FOUR_LANE_KNOWN_LANES)
        if unknown:
            print(
                f"  [ERROR] unknown lane(s): {', '.join(unknown)}\n"
                f"  Known lanes: {', '.join(sorted(FOUR_LANE_KNOWN_LANES))}",
                file=sys.stderr,
            )
            raise SystemExit(1)

    if args.stop:
        lanes_to_stop = args.lane or list(TRAINING_LOG_LANES)
        for lane in lanes_to_stop:
            stop_four_lane(log_root / lane)
        stopped = ", ".join(lanes_to_stop)
        print(f"  [OK] training lanes stopped: {stopped}")
        return

    audit_lanes = args.lane or list(TRAINING_LOG_LANES)
    audit_four_lane_training_state(log_root, audit_lanes)

    stamp = ensure_training_session_stamp()
    connstr = (
        os.environ.get("MFB_PG_CONNSTR") or DEFAULT_CONNSTR
    ).strip() or DEFAULT_CONNSTR
    ensure_db_training_closure_before_training()
    ensure_reset_db_before_training(reset_db=args.reset_db, dry_run=args.dry_run)

    if args.reset_db and not args.dry_run:
        reset_training_db(connstr)
        purge_image_quality_model_artifacts(dry_run=False)

    dylib_path = apply_foundation_lib_env(force_rebuild=args.rebuild_dylib)
    print(f"  [DYLIB] SHARED_UTILS_LIB_PATH={dylib_path}")
    print(f"  [LAUNCH] stamp={stamp} log_root={log_root}")

    started_lanes: list[str] = []
    for lane, tail in FOUR_LANE_SPECS:
        if args.lane and lane not in args.lane:
            continue
        slug = four_lane_slug_from_tail(tail)
        if slug != lane:
            print(
                f"  [WARN] lane slug mismatch: spec={lane} computed={slug}",
                file=sys.stderr,
            )
        try:
            name, pid_or_code, log_path = start_four_lane(
                lane=lane,
                argv_tail=list(tail),
                log_root=log_root,
                stamp=stamp,
                connstr=connstr,
                dry_run=args.dry_run,
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
            for started in started_lanes:
                stop_four_lane(log_root / started)
            raise SystemExit(f"  [ERROR] lane bootstrap failed: {lane}: {exc}") from exc
        if args.dry_run:
            print(f"  [DRY] {name} exit={pid_or_code}")
        else:
            print(f"  [OK] {name} pid={pid_or_code} log={log_path}")
            started_lanes.append(name)

    if not args.dry_run and not args.lane:
        print(
            "  [POST] four-lane ingest launched; run post_training_closure.py for verify/finalize"
        )


def four_lane_main(argv: Sequence[str] | None = None) -> None:
    parser = argparse.ArgumentParser(
        description="Start four parallel training lanes (detached, new session)."
    )
    parser.add_argument("--dry-run", action="store_true")
    parser.add_argument(
        "--reset-db",
        action="store_true",
        help="TRUNCATE training tables before launch; required for non-dry-run starts.",
    )
    parser.add_argument(
        "--rebuild-dylib",
        action="store_true",
        help="Force cargo rustc -p foundation --lib --crate-type cdylib and refresh .modern_format_boost/artifacts dylib before launch.",
    )
    parser.add_argument("--stop", action="store_true")
    parser.add_argument("--log-root", type=Path, default=None)
    parser.add_argument(
        "--lane",
        type=str,
        nargs="+",
        metavar="LANE",
        help=(
            "Restrict to specific lane(s); applies to both start and --stop. "
            f"Known lanes: {', '.join(sorted(FOUR_LANE_KNOWN_LANES))}."
        ),
    )
    run_four_lane_launcher(parser.parse_args(argv))


_scan_governor: ScanGovernor | None = None


def reset_training_scan_governor(*, sister_load: bool = False) -> ScanGovernor:
    global _scan_governor
    _scan_governor = ScanGovernor(sister_training_load=sister_load)
    return _scan_governor


def training_scan_governor() -> ScanGovernor:
    global _scan_governor
    if _scan_governor is None:
        _scan_governor = ScanGovernor()
    return _scan_governor


def static_tier_scan_interval() -> int:
    return training_scan_governor().scan_interval()


def training_scan_heartbeat_secs() -> float:
    return training_scan_governor().heartbeat_secs()


def format_elapsed_secs(seconds: float) -> str:
    if seconds >= 10.0:
        return f"{seconds:.1f}s"
    return f"{seconds:.2f}s"


def format_counter_top(counts: Counter[str], *, max_keys: int = 6) -> str:
    if not counts:
        return ""
    parts: list[str] = []
    for key, n in counts.most_common(max_keys):
        parts.append(f"{key}={n}")
    tail = sum(counts.values()) - sum(n for _, n in counts.most_common(max_keys))
    if tail > 0:
        parts.append(f"other={tail}")
    return ", ".join(parts)


def should_emit_scan_progress(
    scanned: int,
    *,
    last_progress_scanned: int,
    now: float,
    last_progress_at: float,
    count_interval: int,
    heartbeat_secs: float,
) -> bool:
    if scanned <= 0:
        return False
    if scanned == 1:
        return True
    if scanned - last_progress_scanned >= count_interval:
        return True
    return now - last_progress_at >= heartbeat_secs


JsonObject = dict[str, Any]
IngestBatchFn = Callable[[str, list[str], str | None, str], int]
ResolveLibPathFn = Callable[[], Path]
GLOBAL_LABEL_OWNERS: dict[str, str] = {}


def clear_ephemeral_training_state() -> None:
    """Reset per-process collectors so a second plan/ingest in one interpreter cannot cross-contaminate."""
    import mfb_entry_guard as _meg

    GLOBAL_LABEL_OWNERS.clear()
    TIER_AUDIT_RECORDS.clear()
    close_tier_audit_stream()
    _meg._LOGGED_STALE_SOURCE_MAP = False


# Training tables that accumulate rows across runs; wiped by --reset-db.
# Order is insertion-safe (no FK cascade needed — all are leaf or sibling tables).
_TRAINING_TABLES_TO_RESET: tuple[str, ...] = (
    "inference_log",
    "loop_intent_inference_log",
    "image_quality_inference_log",
    "animated_image_quality_inference_log",
    "video_quality_inference_log",
    "loop_samples",
    "image_quality_samples",
    "animated_image_quality_samples",
    "video_quality_samples",
    "multi_scenario_metadata",
    "path_tree_snapshots",
    "live_audit",
    "decision_snapshots",
    "media_entries",
)


def reset_training_db(conn_str: str) -> None:
    """TRUNCATE all training tables in a single transaction.

    Tables that do not yet exist are silently skipped so the function is
    safe to call on a freshly-created or partially-migrated schema.
    Row counts cleared are printed for an audit trail.
    """
    try:
        import psycopg2
    except ImportError:
        training_quality_exit(
            1,
            "  [RESET-DB] ERROR: psycopg2 not available; refusing to start training without a verified clean DB",
        )

    print("  [RESET-DB] Clearing training tables before run…")
    try:
        conn = psycopg2.connect(conn_str)
        conn.autocommit = False
        cur = conn.cursor()
        total_deleted = 0
        for table in _TRAINING_TABLES_TO_RESET:
            cur.execute(
                "SELECT EXISTS (SELECT 1 FROM information_schema.tables "
                "WHERE table_schema = 'public' AND table_name = %s)",
                (table,),
            )
            row = cur.fetchone()
            if not (row and row[0]):
                continue
            cur.execute(f"SELECT COUNT(*) FROM {table}")
            count_row = cur.fetchone()
            n = count_row[0] if count_row else 0
            cur.execute(f"TRUNCATE TABLE {table}")
            total_deleted += n
            if n:
                print(f"      cleared {table}: {n} rows")
        conn.commit()
        cur.close()
        conn.close()
        print(f"  [RESET-DB] Done — {total_deleted} rows removed across all tables.")
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
        training_quality_exit(1, f"  [RESET-DB] ERROR: {exc}")


def sanitized_subprocess_env(
    *,
    conn_str: str | None = None,
    quality_model_python: str | None = None,
) -> dict[str, str]:
    """Delegate to ``mfb_entry_guard.helper_env`` (drops stale replica source map)."""
    return helper_env(
        "run_training.py",
        conn_str=conn_str,
        quality_model_python=quality_model_python,
        strip_training_source_map=True,
    )


def ingest_rust_cli_env(conn_str: str) -> dict[str, str]:
    """Delegate to ``mfb_entry_guard.rust_ingest_env``."""
    from mfb_entry_guard import rust_ingest_env

    env = rust_ingest_env(conn_str)
    if training_ingest_progress_enabled():
        env.setdefault(TRAINING_INGEST_PROGRESS_ENV, "1")
    return env


@dataclass(frozen=True)
class ApiInfo:
    direct_links: tuple[str, ...] = ()
    url_template: str = ""
    media_field: str = ""


@dataclass(frozen=True)
class SampleSources:
    local_dirs: tuple[str, ...] = ()
    remote_apis: tuple[str, ...] = ()
    selection_strategy: str = ""
    file_quality_filter: JsonObject | None = None


@dataclass(frozen=True)
class QualityGroup:
    """Committed tier: file_quality_filter + logic/rules (entropy, pixels)."""

    sources: SampleSources
    tier_logic: str = "ANY"
    tier_rules: tuple[JsonObject, ...] = ()


TIER_AMBIGUOUS_POLICIES: Final = frozenset({"exclude", "prefer_high", "prefer_low"})

# Keep in sync with `crates/foundation/src/training_tier_audit.rs` (tier combiner + thresholds).
RUST_STATIC_TIER_CONTRACT: Final = {
    "high_quality": {
        "logic": "ANY",
        "rules": {"entropy_ge": 7.7, "pixel_min_dim_ge": 1080},
    },
    "low_quality": {
        "logic": "ANY",
        "rules": {"entropy_le": 2.8, "pixel_max_dim_le": 512},
    },
}


@dataclass(frozen=True)
class RulesConfig:
    strict_unknown_rules: bool
    strict_no_silent_fallbacks: bool
    tier_ambiguous_policy: str
    remote_apis: dict[str, ApiInfo]
    static_image: dict[str, QualityGroup]
    animated_image: dict[str, QualityGroup]
    # Parsed from training_rules.local.json → ingest (run_training.py only; not tier rules).
    ingest: JsonObject | None = None


EMPTY_SAMPLE_SOURCES: Final = SampleSources()
EMPTY_QUALITY_GROUP: Final = QualityGroup(sources=EMPTY_SAMPLE_SOURCES)
QUALITY_MODEL_MODULES: Final[tuple[str, ...]] = (
    "lightgbm",
    "numpy",
    "psycopg2",
    "sklearn",
)


def as_object(value: object) -> JsonObject:
    if not isinstance(value, Mapping):
        return {}
    parsed: JsonObject = {}
    for key, item in value.items():
        if isinstance(key, str):
            parsed[key] = item
    return parsed


def as_object_list(value: object) -> list[object]:
    if not isinstance(value, list):
        return []
    parsed: list[object] = []
    for item in value:
        parsed.append(item)
    return parsed


def as_string(value: object) -> str:
    return value if isinstance(value, str) else ""


def as_string_list(value: object) -> list[str]:
    return [item for item in as_object_list(value) if isinstance(item, str)]


def validate_video_section(value: object) -> None:
    video_obj = as_object(value)
    if not video_obj:
        return
    section_names = (
        "contrast_fast_silent_loop",
        "prefer_grey_zone_loop_low",
        "contrast_with_audio",
        "contrast_silent_anim",
        "deprioritize_grey_zone",
        # Legacy (optional, still validated when present):
        "keep_with_audio",
        "keep_silent",
        "reject",
    )
    allowed_top = {name for name in section_names} | {
        k for k in video_obj if k.startswith("_comment")
    }
    ensure_allowed_keys(video_obj, allowed_top, "video")
    allowed_video_rules = {
        "has_audio AND duration_in_grey_zone",
        "no_audio AND duration_in_grey_zone",
        "has_audio AND duration_lt",
        "has_audio AND duration_gt",
        "no_audio AND duration_lt",
        "no_audio AND duration_gt",
        "duration_lt",
        "duration_gt",
    }
    for section in section_names:
        if section not in video_obj:
            continue
        section_obj = as_object(video_obj.get(section))
        if not section_obj:
            raise ValueError(f"video.{section} is required")
        ensure_allowed_keys(section_obj, {"logic", "rules"}, f"video.{section}")
        logic = as_string(section_obj.get("logic")).upper()
        if logic not in {"ANY", "ALL"}:
            raise ValueError(f"video.{section}.logic must be ALL or ANY")
        rules = as_object_list(section_obj.get("rules"))
        if not rules:
            raise ValueError(f"video.{section}.rules must be non-empty")
        for idx, raw_rule in enumerate(rules):
            rule_obj = as_object(raw_rule)
            ensure_allowed_keys(
                rule_obj,
                {"rule", "value", "grey_zone_secs", "desc"},
                f"video.{section}.rules[{idx}]",
            )
            rule_name = as_string(rule_obj.get("rule")).strip()
            if not rule_name:
                raise ValueError(f"video.{section}.rules[{idx}].rule must be non-empty")
            if rule_name not in allowed_video_rules:
                raise ValueError(
                    f"video.{section}.rules[{idx}].rule is unsupported: {rule_name}"
                )
            desc = as_string(rule_obj.get("desc")).strip()
            if not desc:
                raise ValueError(f"video.{section}.rules[{idx}].desc must be non-empty")
            if rule_name in {
                "has_audio AND duration_in_grey_zone",
                "no_audio AND duration_in_grey_zone",
            }:
                grey = as_object(rule_obj.get("grey_zone_secs"))
                if not grey:
                    raise ValueError(
                        f"video.{section}.rules[{idx}] requires grey_zone_secs"
                    )
                ensure_allowed_keys(
                    grey, {"min", "max"}, f"video.{section}.rules[{idx}].grey_zone_secs"
                )
                try:
                    min_v = float(cast(Any, grey.get("min")))
                    max_v = float(cast(Any, grey.get("max")))
                except (TypeError, ValueError) as exc:
                    raise ValueError(
                        f"video.{section}.rules[{idx}].grey_zone_secs must be numeric"
                    ) from exc
                if min_v >= max_v:
                    raise ValueError(
                        f"video.{section}.rules[{idx}].grey_zone_secs requires min < max"
                    )
                if "value" in rule_obj:
                    raise ValueError(
                        f"video.{section}.rules[{idx}] must not define value when using grey_zone_secs"
                    )
            elif rule_name in {
                "duration_lt",
                "duration_gt",
                "has_audio AND duration_lt",
                "has_audio AND duration_gt",
                "no_audio AND duration_lt",
                "no_audio AND duration_gt",
            }:
                if "grey_zone_secs" in rule_obj:
                    raise ValueError(
                        f"video.{section}.rules[{idx}] must not define grey_zone_secs for {rule_name}"
                    )
                if "value" not in rule_obj:
                    raise ValueError(
                        f"video.{section}.rules[{idx}] requires numeric value for {rule_name}"
                    )
                try:
                    float(cast(Any, rule_obj.get("value")))
                except (TypeError, ValueError) as exc:
                    raise ValueError(
                        f"video.{section}.rules[{idx}].value must be numeric"
                    ) from exc


def parse_api_info(value: object) -> ApiInfo:
    obj = as_object(value)
    ensure_allowed_keys(
        obj, {"direct_links", "url_template", "media_field"}, "remote_apis item"
    )
    return ApiInfo(
        direct_links=tuple(as_string_list(obj.get("direct_links"))),
        url_template=as_string(obj.get("url_template")),
        media_field=as_string(obj.get("media_field")),
    )


def parse_sample_sources(value: object) -> SampleSources:
    obj = as_object(value)
    ensure_allowed_keys(
        obj,
        {"local_dirs", "remote_apis", "selection_strategy", "file_quality_filter"},
        "sample_sources",
    )
    return SampleSources(
        local_dirs=tuple(as_string_list(obj.get("local_dirs"))),
        remote_apis=tuple(as_string_list(obj.get("remote_apis"))),
        selection_strategy=as_string(obj.get("selection_strategy")),
        file_quality_filter=as_object(obj.get("file_quality_filter")) or None,
    )


def parse_quality_group(item_obj: JsonObject, context: str) -> QualityGroup:
    ensure_allowed_keys(
        item_obj,
        {
            "sample_sources",
            "selection_strategy",
            "file_quality_filter",
            "logic",
            "rules",
        }
        | {k for k in item_obj if k.startswith("_comment")},
        context,
    )
    sample_sources_obj = as_object(item_obj.get("sample_sources"))
    merged_obj: JsonObject = dict(sample_sources_obj)
    if "selection_strategy" in item_obj and "selection_strategy" not in merged_obj:
        merged_obj["selection_strategy"] = item_obj["selection_strategy"]
    if "file_quality_filter" in item_obj and "file_quality_filter" not in merged_obj:
        merged_obj["file_quality_filter"] = item_obj["file_quality_filter"]
    tier_logic = as_string(item_obj.get("logic")).upper() or "ANY"
    tier_rules = tuple(as_object(r) for r in as_object_list(item_obj.get("rules")))
    return QualityGroup(
        sources=parse_sample_sources(merged_obj),
        tier_logic=tier_logic,
        tier_rules=tier_rules,
    )


def parse_sample_source_section(
    value: object, section_name: str
) -> dict[str, QualityGroup]:
    section = as_object(value)
    parsed: dict[str, QualityGroup] = {}
    for key, item in section.items():
        if key.startswith("_comment"):
            continue
        parsed[key] = parse_quality_group(as_object(item), f"{section_name}.{key}")
    return parsed


def default_resolved_library_path() -> Path:
    if sys.platform == "darwin":
        lib_name = "libfoundation.dylib"
    elif sys.platform == "win32":
        lib_name = "foundation.dll"
    else:
        lib_name = "libfoundation.so"
    return ROOT / "target" / "debug" / lib_name


def missing_ingest_media_samples_batch(
    conn_str: str, file_paths: list[str], label: str | None, scenario: str
) -> int:
    raise RuntimeError(
        "python_api bridge is unavailable; build foundation or rerun without --use-api"
    )


ingest_media_samples_batch: IngestBatchFn = missing_ingest_media_samples_batch
resolved_library_path: ResolveLibPathFn = default_resolved_library_path
has_c_api = False


def _empty_last_ingest_error() -> str:
    return ""


get_last_ingest_error: Callable[[], str] = _empty_last_ingest_error


def _missing_probe_static_still_image(_path: str) -> dict[str, Any]:
    return {
        "ok": False,
        "error": "python_api bridge unavailable; build foundation for Rust tier probe",
    }


def _missing_probe_loop_intent(_path: str) -> dict[str, Any]:
    return {
        "ok": False,
        "error": "python_api bridge unavailable; build foundation for loop probe",
    }


probe_static_still_image: Callable[[str], dict[str, Any]] = (
    _missing_probe_static_still_image
)
probe_loop_intent: Callable[[str], dict[str, Any]] = _missing_probe_loop_intent

try:
    scripts_dir = str(SCRIPTS_DIR)
    if scripts_dir not in sys.path:
        sys.path.append(scripts_dir)
    from python_api import (
        get_last_ingest_error as imported_get_last_ingest_error,
    )
    from python_api import (
        ingest_media_samples_batch as imported_ingest_media_samples_batch,
    )
    from python_api import (
        probe_loop_intent as imported_probe_loop_intent,
    )
    from python_api import (
        probe_static_still_image as imported_probe_static_still_image,
    )
    from python_api import resolved_library_path as imported_resolved_library_path
except ImportError:
    pass
else:
    ingest_media_samples_batch = imported_ingest_media_samples_batch
    get_last_ingest_error = imported_get_last_ingest_error
    probe_static_still_image = imported_probe_static_still_image
    probe_loop_intent = imported_probe_loop_intent
    resolved_library_path = imported_resolved_library_path
    has_c_api = True
    from mfb_dylib import apply_foundation_lib_env
    from python_api import reset_rust_lib_cache

    apply_foundation_lib_env()
    reset_rust_lib_cache()

HAS_C_API: Final = has_c_api


RUNTIME_CONTRACT_SOURCES = [
    ROOT / "crates" / "foundation" / "Cargo.toml",
    ROOT / "crates" / "foundation" / "src" / "c_api.rs",
    ROOT / "crates" / "foundation" / "src" / "multi_scenario_db.rs",
    ROOT / "crates" / "foundation" / "src" / "scenario.rs",
]
QUALITY_CLI_SOURCES = RUNTIME_CONTRACT_SOURCES + [
    ROOT / "crates" / "foundation" / "src" / "bin" / "train_quality.rs",
]
LOOP_CLI_SOURCES = RUNTIME_CONTRACT_SOURCES + [
    ROOT / "crates" / "foundation" / "src" / "bin" / "train_knn.rs",
]


class Sample(NamedTuple):
    path_or_url: str
    base_label: str
    is_remote: bool
    source: SampleSources
    tier_audit: JsonObject | None = None


@dataclass(frozen=True)
class StaticImageProbe:
    width: int
    height: int
    entropy: float
    format: str
    high_tier: bool
    low_tier: bool
    high_rule_hits: tuple[str, ...]
    low_rule_hits: tuple[str, ...]
    resolved_tier: str | None
    ambiguous_both_tiers: bool
    raw: JsonObject


def merge_local_sample_dirs(
    base: dict[str, QualityGroup], overlay_section: object
) -> dict[str, QualityGroup]:
    """Overlay gitignored local section: local_dirs only (no strategy overrides)."""
    if not overlay_section:
        return base
    merged = dict(base)
    for key, item in as_object(overlay_section).items():
        if key not in merged:
            raise ValueError(f"local override references unknown quality group: {key}")
        item_obj = as_object(item)
        ensure_allowed_keys(
            item_obj,
            {"sample_sources"},
            f"local override group {key}",
        )
        sample_sources_obj = as_object(item_obj.get("sample_sources"))
        ensure_allowed_keys(
            sample_sources_obj,
            {"local_dirs"},
            f"local override group {key}.sample_sources",
        )
        local_obj: JsonObject = dict(sample_sources_obj)
        local = parse_sample_sources(local_obj)
        if not local.local_dirs:
            continue
        prev = merged[key]
        prev_sources = prev.sources
        merged[key] = QualityGroup(
            sources=SampleSources(
                local_dirs=local.local_dirs or prev_sources.local_dirs,
                remote_apis=prev_sources.remote_apis,
                selection_strategy=prev_sources.selection_strategy,
                file_quality_filter=prev_sources.file_quality_filter,
            ),
            tier_logic=prev.tier_logic,
            tier_rules=prev.tier_rules,
        )
    return merged


def load_rules() -> RulesConfig:
    if not RULES_FILE.exists():
        raise FileNotFoundError(
            f"{RULES_FILE} is required; refusing empty training rule fallback"
        )
    with open(RULES_FILE, encoding="utf-8") as f:
        root = as_object(json.load(f))
    ensure_allowed_keys(
        root,
        {
            "_comment",
            "_consumer",
            "rule_engine",
            "remote_apis",
            "static_image",
            "animated_image",
            "video",
        },
        "training_rules.json",
    )
    rules_consumer = as_string(root.get("_consumer"))
    if rules_consumer != "run_training.py":
        raise ValueError(
            "training_rules.json._consumer must be exactly 'run_training.py'; "
            f"got {rules_consumer!r}"
        )
    rule_engine = as_object(root.get("rule_engine"))
    ensure_allowed_keys(
        rule_engine,
        {
            "schema_version",
            "strict_no_silent_fallbacks",
            "strict_unknown_rules",
            "tier_ambiguous_policy",
        },
        "rule_engine",
    )
    try:
        schema_version = int(cast(Any, rule_engine.get("schema_version")))
    except (TypeError, ValueError) as exc:
        raise ValueError("rule_engine.schema_version must be an integer") from exc
    if schema_version != RULES_SCHEMA_VERSION:
        raise ValueError(
            "rule_engine.schema_version mismatch: "
            f"expected {RULES_SCHEMA_VERSION}, got {schema_version}"
        )
    strict_no_silent_fallbacks = bool(rule_engine.get("strict_no_silent_fallbacks"))
    strict_unknown_rules = bool(rule_engine.get("strict_unknown_rules", True))
    tier_ambiguous_policy = (
        (as_string(rule_engine.get("tier_ambiguous_policy")) or "exclude")
        .strip()
        .lower()
    )
    if tier_ambiguous_policy not in TIER_AMBIGUOUS_POLICIES:
        raise ValueError(
            "rule_engine.tier_ambiguous_policy must be one of "
            f"{sorted(TIER_AMBIGUOUS_POLICIES)}, got {tier_ambiguous_policy!r}"
        )
    validate_video_section(root.get("video"))

    remote_apis: dict[str, ApiInfo] = {}
    for key, value in as_object(root.get("remote_apis")).items():
        remote_apis[key] = parse_api_info(value)

    static_image = parse_sample_source_section(root.get("static_image"), "static_image")
    animated_image = parse_sample_source_section(
        root.get("animated_image"), "animated_image"
    )
    ingest_profile: JsonObject | None = None
    if RULES_LOCAL_FILE.is_file():
        with open(RULES_LOCAL_FILE, encoding="utf-8") as local_f:
            local_root = as_object(json.load(local_f))
        ensure_allowed_keys(
            local_root,
            {
                "_comment",
                "_consumer",
                "rule_engine",
                "ingest",
                "static_image",
                "animated_image",
            },
            "training_rules.local.json",
        )
        consumer = as_string(local_root.get("_consumer"))
        if consumer != "run_training.py":
            raise ValueError(
                "training_rules.local.json._consumer is required and must be "
                f"'run_training.py'; got {consumer!r}"
            )
        raw_ingest = as_object(local_root.get("ingest"))
        if raw_ingest:
            ensure_allowed_keys(
                raw_ingest,
                set(INGEST_PROFILE_ALLOWED_KEYS),
                "training_rules.local.json.ingest",
            )
            ingest_profile = raw_ingest
        local_engine = as_object(local_root.get("rule_engine"))
        if not local_engine:
            raise ValueError(
                "training_rules.local.json.rule_engine.schema_version is required"
            )
        ensure_allowed_keys(
            local_engine,
            {"schema_version"},
            "training_rules.local.json.rule_engine",
        )
        try:
            local_schema_version = int(cast(Any, local_engine.get("schema_version")))
        except (TypeError, ValueError) as exc:
            raise ValueError(
                "training_rules.local.json.rule_engine.schema_version must be an integer"
            ) from exc
        if local_schema_version != schema_version:
            raise ValueError(
                "training_rules.local.json.rule_engine.schema_version mismatch: "
                f"expected {schema_version}, got {local_schema_version}"
            )
        static_image = merge_local_sample_dirs(
            static_image, local_root.get("static_image")
        )
        animated_image = merge_local_sample_dirs(
            animated_image, local_root.get("animated_image")
        )

    return RulesConfig(
        strict_unknown_rules=strict_unknown_rules,
        strict_no_silent_fallbacks=strict_no_silent_fallbacks,
        tier_ambiguous_policy=tier_ambiguous_policy,
        remote_apis=remote_apis,
        static_image=static_image,
        animated_image=animated_image,
        ingest=ingest_profile,
    )


def extract_field(data: object, field_path: str) -> list[str]:
    parts = field_path.split(".")
    current: list[object] = [data]
    for part in parts:
        next_nodes: list[object] = []
        for node in current:
            if part.endswith("[*]"):
                key = part[:-3]
                if key == "":
                    next_nodes.extend(as_object_list(node))
                    continue
                node_dict = as_object(node)
                next_nodes.extend(as_object_list(node_dict.get(key)))
            else:
                node_dict = as_object(node)
                if part in node_dict:
                    next_nodes.append(node_dict[part])
        current = next_nodes
    return [node for node in current if isinstance(node, str)]


def resolve_api_urls(api_name: str, rules: RulesConfig) -> list[str]:
    api_info = rules.remote_apis.get(api_name)
    if not api_info:
        msg = f"Unknown API definition: {api_name}"
        print(f"  [WARN] {msg}", file=sys.stderr)
        if fail_closed_training_enabled():
            raise RuntimeError(msg)
        return []

    direct_links = list(api_info.direct_links)
    if direct_links:
        print(f"  [API] Using {len(direct_links)} direct links from {api_name}")
        return direct_links

    url_template = api_info.url_template
    media_field = api_info.media_field
    if not url_template or not media_field:
        msg = f"API {api_name} missing url_template or media_field"
        if fail_closed_training_enabled():
            raise RuntimeError(msg)
        return []

    resolved_url = url_template
    for key in ["GIPHY_KEY", "TENOR_KEY", "UNSPLASH_KEY"]:
        if f"{{{key}}}" in resolved_url:
            value = os.environ.get(key)
            if not value:
                msg = f"API key missing for {api_name}: set {key}"
                print(f"  [SKIP] {msg}", file=sys.stderr)
                if fail_closed_training_enabled():
                    raise RuntimeError(msg)
                return []
            resolved_url = resolved_url.replace(f"{{{key}}}", value)

    try:
        print(f"  [API] Resolving {api_name} using: {resolved_url.split('?', 1)[0]}...")
        req = urllib.request.Request(resolved_url, headers={"User-Agent": USER_AGENT})
        with urllib.request.urlopen(req, timeout=10) as response:
            urls = extract_field(
                json.loads(response.read().decode("utf-8")), media_field
            )
            print(f"  [API] Found {len(urls)} media links from {api_name}")
            return urls
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
        msg = f"API fetch failed for {api_name}: {e}"
        print(f"  [FAIL] {msg}", file=sys.stderr)
        raise RuntimeError(msg) from e


def is_junk_path(p: Path) -> bool:
    if p.name.startswith(".") or p.name.lower() in {".ds_store", "thumbs.db"}:
        return True
    if p.suffix.lower() in JUNK_EXTS:
        return True
    parts = [part.lower() for part in p.parts]
    if any(k in parts for k in ["backup", "tmp", "old", "redundant"]):
        return True
    return False


def iter_media_files(root: Path) -> Iterator[Path]:
    """Walk media files without ``sorted(rglob('*'))`` (large trees stay responsive)."""
    stack: list[Path] = [root]
    while stack:
        current = stack.pop()
        try:
            with os.scandir(current) as entries:
                subdirs: list[Path] = []
                for entry in entries:
                    try:
                        if entry.is_dir(follow_symlinks=False):
                            subdirs.append(Path(entry.path))
                            continue
                        if not entry.is_file(follow_symlinks=False):
                            continue
                        item = Path(entry.path)
                    except OSError as exc:
                        raise ScanPlanningError(
                            f"training media entry probe failed under {current}: {exc}"
                        ) from exc
                    if is_junk_path(item):
                        continue
                    yield item
                stack.extend(reversed(subdirs))
        except OSError as exc:
            raise ScanPlanningError(
                f"training media scan failed for {current}: {exc}"
            ) from exc


def _media_scan_candidate(path: Path) -> bool:
    return not is_junk_path(path)


def iter_segmented_media_files(
    root: Path,
    *,
    tag: str,
) -> Iterator[tuple[ScanSegment, Path]]:
    """Yield ``(segment, file)`` using one-shot or top-level segmented scan plan."""
    segments = plan_scan_segments(root, is_media_file=_media_scan_candidate)
    print(
        f"  [{tag}] scan_plan {format_scan_plan_summary(root, segments)}",
        flush=True,
    )
    for segment in segments:
        print(
            f"  [{tag}] segment_start {segment.index}/{segment.total} "
            f"mode={segment.mode.value} label={segment.label} "
            f"roots={len(segment.roots)}",
            flush=True,
        )
        for seg_root in segment.roots:
            for item in iter_media_files(seg_root):
                yield segment, item


def scan_progress_path_suffix(item: Path, *, verbose: bool) -> str:
    """Default progress lines use basename only (full paths every 50 files flood I/O)."""
    if verbose:
        return f"path={item}"
    return f"file={item.name}"


def require_local_training_dirs(local_dirs: Sequence[str], context: str) -> bool:
    if local_dirs:
        return True
    msg = f"{context}: no local_dirs configured; refusing empty training corpus"
    if fail_closed_training_enabled():
        raise RuntimeError(msg)
    print(f"  [SKIP] {msg} (debug empty fallback enabled)", file=sys.stderr)
    return False


def _other_run_training_pids() -> list[tuple[int, str]]:
    rows: list[tuple[int, str]] = []
    try:
        out = subprocess.run(
            ["ps", "-ax", "-o", "pid=,comm=,command="],
            capture_output=True,
            text=True,
            check=False,
        )
        self_pid = os.getpid()
        for line in out.stdout.splitlines():
            if "run_training.py" not in line:
                continue
            parts = line.strip().split(None, 2)
            if len(parts) != 3:
                continue
            try:
                pid = int(parts[0])
            except ValueError:
                continue
            if pid == self_pid:
                continue
            comm = parts[1].lower()
            cmd = parts[2]
            # Only terminate real Python interpreters; IDE shell wrappers embed
            # `run_training.py` in `zsh -c` but have comm=zsh.
            if not (comm.startswith("python") or comm == "python"):
                continue
            rows.append((pid, cmd))
    except OSError as exc:
        print(
            f"  [GUARD] ps probe failed: {exc}; sibling-PID check skipped",
            file=sys.stderr,
            flush=True,
        )
        return []
    return rows


def stop_other_training_processes() -> int:
    """Terminate sibling ``run_training.py`` jobs (only one scan should own the machine)."""
    # Four-lane launcher (`start_training_four.py`) pins each child to
    # `~/.modern_format_boost/logs/<lane>/`; lane workers must never terminate
    # siblings even if an env var gets dropped by wrappers.
    lane_log_dir = TRAINING_LOG_DIR.name.strip().lower()
    if lane_log_dir in {
        "static_high",
        "static_low",
        "loop_high",
        "loop_low",
        "loop_video",
    }:
        return 0
    if (os.environ.get("MFB_TRAINING_ALLOW_PARALLEL") or "").strip().lower() in {
        "1",
        "true",
        "yes",
    }:
        return 0
    others = _other_run_training_pids()
    if not others:
        return 0
    print(
        "  [PERF] Stopping other run_training.py process(es) to restore UI responsiveness:",
        file=sys.stderr,
        flush=True,
    )
    for pid, cmd in others[:6]:
        print(f"         SIGTERM pid={pid} {cmd[:120]}", file=sys.stderr, flush=True)
        try:
            os.kill(pid, 15)
        except OSError as exc:
            print(f"         [WARN] kill {pid}: {exc}", file=sys.stderr, flush=True)
    time.sleep(2.0)
    survivors = _other_run_training_pids()
    for pid, cmd in survivors:
        print(f"         SIGKILL pid={pid} {cmd[:120]}", file=sys.stderr, flush=True)
        try:
            os.kill(pid, 9)
        except OSError:
            pass
    if _TRAINING_SESSION is not None:
        _TRAINING_SESSION.emit(
            "siblings_terminated",
            count=len(others),
            survivors_after_sigkill=len(survivors),
            pids=[pid for pid, _ in others[:12]],
        )
    return len(others)


def apply_training_scan_defaults() -> None:
    """Desktop-friendly defaults for long directory scans (user env still wins if preset)."""
    if "MFB_PERF_TIER" not in os.environ:
        os.environ["MFB_PERF_TIER"] = "tight"
        print(
            "  [PERF] scan governor default: tight (override with MFB_PERF_TIER=balanced|relaxed)",
            flush=True,
        )


def warn_concurrent_training_processes() -> bool:
    """Another ``run_training`` still running after ``stop_other_training_processes``."""
    others = _other_run_training_pids()
    if not others:
        return False
    print(
        "  [WARN] Other run_training.py still running — expect disk/CPU contention:",
        file=sys.stderr,
        flush=True,
    )
    for pid, cmd in others[:4]:
        print(f"         pid={pid} {cmd[:120]}", file=sys.stderr, flush=True)
    if len(others) > 4:
        print(f"         … and {len(others) - 4} more", file=sys.stderr, flush=True)
    return True


def rule_is_supported_image_file(path: Path) -> bool:
    ext = detect_media_extension(path)
    return ext in IMAGE_EXTS


def rule_is_supported_animated_image_file(path: Path) -> bool:
    ext = detect_media_extension(path)
    return ext in ANIMATED_IMAGE_EXTS


def rule_is_supported_non_loop_media_file(path: Path) -> bool:
    """Animated raster + container video (loop / non-loop training corpora)."""
    ext = detect_media_extension(path)
    return ext in ANIMATED_IMAGE_EXTS or ext in VIDEO_EXTS


def rule_is_supported_loop_intent_media_file(path: Path) -> bool:
    """Strong-loop corpus: GIF/WebP/APNG plus video containers (Rust loop probe)."""
    return rule_is_supported_non_loop_media_file(path)


def rule_file_size_kb_ge(path: Path, threshold_kb: object) -> bool:
    try:
        min_kb = float(cast(Any, threshold_kb))
    except (TypeError, ValueError):
        raise ValueError(f"file_size_kb_ge expects numeric value, got {threshold_kb!r}")
    try:
        size_kb = path.stat().st_size / 1024.0
    except OSError as exc:
        raise ScanPlanningError(
            f"file_size_kb_ge stat failed for {path}: {exc}"
        ) from exc
    return size_kb >= min_kb


def rule_file_size_kb_le(path: Path, threshold_kb: object) -> bool:
    try:
        max_kb = float(cast(Any, threshold_kb))
    except (TypeError, ValueError):
        raise ValueError(f"file_size_kb_le expects numeric value, got {threshold_kb!r}")
    try:
        size_kb = path.stat().st_size / 1024.0
    except OSError as exc:
        raise ScanPlanningError(
            f"file_size_kb_le stat failed for {path}: {exc}"
        ) from exc
    return size_kb <= max_kb


def rule_extension_not_in(path: Path, blocked_exts: object) -> bool:
    blocked = {
        str(item).strip().lower().lstrip(".")
        for item in as_object_list(blocked_exts)
        if str(item).strip()
    }
    if not blocked:
        return True
    return path.suffix.lower().lstrip(".") not in blocked


def rule_path_not_contains_any(path: Path, blocked_tokens: object) -> bool:
    blocked = [
        str(item).strip().lower()
        for item in as_object_list(blocked_tokens)
        if str(item).strip()
    ]
    if not blocked:
        return True
    path_s = str(path).lower()
    return not any(token in path_s for token in blocked)


def rule_filename_not_matches_regex(path: Path, pattern: object) -> bool:
    regex = as_string(pattern).strip()
    if not regex:
        raise ValueError("filename_not_matches_regex expects non-empty regex")
    try:
        return re.search(regex, path.name, flags=re.IGNORECASE) is None
    except re.error:
        raise ValueError(f"invalid regex: {regex}")


KNOWN_FILE_QUALITY_RULES: Final[set[str]] = {
    "is_supported_image_file",
    "is_supported_animated_image_file",
    "is_supported_non_loop_media_file",
    "is_supported_loop_intent_media_file",
    "file_size_kb_ge",
    "file_size_kb_le",
    "extension_not_in",
    "path_not_contains_any",
    "filename_not_matches_regex",
}

KNOWN_TIER_RULES: Final[set[str]] = {
    "entropy_ge",
    "entropy_le",
    "pixel_min_dim_ge",
    "pixel_max_dim_le",
}


def filter_target_name(path: Path, original_ref: str | None = None) -> str:
    if original_ref:
        return (
            os.path.basename(original_ref.split("?", 1)[0].split("#", 1)[0])
            or path.name
        )
    return path.name


def evaluate_file_quality_rule(
    path: Path,
    rule_obj: JsonObject,
    *,
    is_remote_source: bool = False,
    original_ref: str | None = None,
) -> bool | None:
    rule_name = as_string(rule_obj.get("rule"))
    if not rule_name:
        return None
    if rule_name == "is_supported_image_file":
        return rule_is_supported_image_file(path)
    if rule_name == "is_supported_animated_image_file":
        return rule_is_supported_animated_image_file(path)
    if rule_name == "is_supported_non_loop_media_file":
        return rule_is_supported_non_loop_media_file(path)
    if rule_name == "is_supported_loop_intent_media_file":
        return rule_is_supported_loop_intent_media_file(path)
    if rule_name == "file_size_kb_ge":
        return rule_file_size_kb_ge(path, rule_obj.get("value"))
    if rule_name == "file_size_kb_le":
        return rule_file_size_kb_le(path, rule_obj.get("value"))
    if rule_name == "extension_not_in":
        return rule_extension_not_in(path, rule_obj.get("value"))
    if rule_name == "path_not_contains_any":
        if is_remote_source:
            # Remote files are always staged under temp replica roots; path semantics
            # should not be decided by temporary filesystem layout.
            return True
        return rule_path_not_contains_any(path, rule_obj.get("value"))
    if rule_name == "filename_not_matches_regex":
        target_name = filter_target_name(path, original_ref)
        return rule_filename_not_matches_regex(Path(target_name), rule_obj.get("value"))
    raise ValueError(f"Unknown file_quality_filter rule: {rule_name}")


def validate_rule_value(rule_name: str, rule_obj: JsonObject) -> None:
    value = rule_obj.get("value")
    if rule_name in {"file_size_kb_ge", "file_size_kb_le"}:
        try:
            float(cast(Any, value))
        except (TypeError, ValueError) as exc:
            raise ValueError(f"{rule_name} requires numeric value") from exc
    elif rule_name in {"extension_not_in", "path_not_contains_any"}:
        if not isinstance(value, list) or not all(
            isinstance(v, str) and v.strip() for v in value
        ):
            raise ValueError(f"{rule_name} requires non-empty string list value")
    elif rule_name == "filename_not_matches_regex":
        regex = as_string(value).strip()
        if not regex:
            raise ValueError("filename_not_matches_regex requires regex string value")
        try:
            re.compile(regex, flags=re.IGNORECASE)
        except re.error as exc:
            raise ValueError(f"invalid regex for {rule_name}: {regex}") from exc


def validate_tier_rule_value(rule_name: str, rule_obj: JsonObject) -> None:
    value = rule_obj.get("value")
    if rule_name in {
        "entropy_ge",
        "entropy_le",
        "pixel_min_dim_ge",
        "pixel_max_dim_le",
    }:
        try:
            float(cast(Any, value))
        except (TypeError, ValueError) as exc:
            raise ValueError(f"{rule_name} requires numeric value") from exc


def validate_quality_group_rules(group_name: str, group: QualityGroup) -> None:
    validate_source_rules(group_name, group.sources)
    if not group.tier_rules:
        return
    logic = (group.tier_logic or "ANY").upper()
    if logic not in {"ALL", "ANY"}:
        raise ValueError(f"{group_name}: logic must be ALL or ANY")
    for raw_rule in group.tier_rules:
        rule_obj = as_object(raw_rule)
        rule_name = as_string(rule_obj.get("rule"))
        if not rule_name:
            raise ValueError(
                f"{group_name}: each tier rule must contain non-empty 'rule'"
            )
        if rule_name not in KNOWN_TIER_RULES:
            raise ValueError(f"{group_name}: unknown tier rule '{rule_name}'")
        validate_tier_rule_value(rule_name, rule_obj)


def validate_rust_tier_contract(rules: RulesConfig) -> None:
    """JSON tier thresholds/logic must match committed Rust `training_tier_audit` constants."""
    for group_key, spec in RUST_STATIC_TIER_CONTRACT.items():
        group = rules.static_image.get(group_key)
        if group is None:
            continue
        expected_logic = str(spec["logic"]).upper()
        actual_logic = (group.tier_logic or "ANY").upper()
        if actual_logic != expected_logic:
            raise ValueError(
                f"static_image.{group_key}.logic must be {expected_logic} "
                f"(Rust tier combiner), got {actual_logic}"
            )
        expected_rules: dict[str, float] = dict(spec["rules"])
        seen: dict[str, float] = {}
        for raw_rule in group.tier_rules:
            rule_obj = as_object(raw_rule)
            rule_name = as_string(rule_obj.get("rule"))
            if not rule_name or rule_name not in expected_rules:
                continue
            try:
                seen[rule_name] = float(cast(Any, rule_obj.get("value")))
            except (TypeError, ValueError) as exc:
                raise ValueError(
                    f"static_image.{group_key}.{rule_name} requires numeric value"
                ) from exc
        for rule_name, expected_value in expected_rules.items():
            actual_value = seen.get(rule_name)
            if actual_value is None:
                raise ValueError(
                    f"static_image.{group_key} missing tier rule {rule_name}"
                )
            if actual_value != expected_value:
                raise ValueError(
                    f"static_image.{group_key}.{rule_name} must be {expected_value} "
                    f"(Rust constant), got {actual_value}"
                )


def validate_source_rules(source_name: str, source: SampleSources) -> None:
    if not source.selection_strategy:
        return
    if source.selection_strategy != "file_quality_filtered":
        raise ValueError(
            f"{source_name}: unknown selection_strategy '{source.selection_strategy}'"
        )
    filter_obj = as_object(source.file_quality_filter)
    if not filter_obj:
        raise ValueError(
            f"{source_name}: file_quality_filtered requires file_quality_filter"
        )
    logic = as_string(filter_obj.get("logic")).upper()
    if logic not in {"ALL", "ANY"}:
        raise ValueError(f"{source_name}: file_quality_filter.logic must be ALL or ANY")
    raw_rules = as_object_list(filter_obj.get("rules"))
    if not raw_rules:
        raise ValueError(f"{source_name}: file_quality_filter.rules must be non-empty")
    for raw_rule in raw_rules:
        rule_obj = as_object(raw_rule)
        rule_name = as_string(rule_obj.get("rule"))
        if not rule_name:
            raise ValueError(
                f"{source_name}: each rule must contain non-empty 'rule' name"
            )
        if rule_name not in KNOWN_FILE_QUALITY_RULES:
            raise ValueError(
                f"{source_name}: unknown file_quality_filter rule '{rule_name}'"
            )
        validate_rule_value(rule_name, rule_obj)


def passes_file_quality_filter(
    path: Path,
    source: SampleSources,
    *,
    is_remote_source: bool = False,
    original_ref: str | None = None,
) -> bool:
    if source.selection_strategy != "file_quality_filtered":
        return True
    filter_obj = as_object(source.file_quality_filter)
    rules = as_object_list(filter_obj.get("rules"))
    if not rules:
        return True
    logic = as_string(filter_obj.get("logic")).upper() or "ALL"
    verdicts: list[bool] = []
    for raw_rule in rules:
        rule_obj = as_object(raw_rule)
        rule_name = as_string(rule_obj.get("rule"))
        if not rule_name:
            continue
        verdict = evaluate_file_quality_rule(
            path,
            rule_obj,
            is_remote_source=is_remote_source,
            original_ref=original_ref,
        )
        if verdict is None:
            raise ValueError(f"Unknown file_quality_filter rule: {rule_name}")
        verdicts.append(verdict)
    if not verdicts:
        return True
    if logic == "ANY":
        return any(verdicts)
    return all(verdicts)


def failed_file_quality_rules(
    path: Path,
    source: SampleSources,
    *,
    is_remote_source: bool = False,
    original_ref: str | None = None,
) -> list[str]:
    if source.selection_strategy != "file_quality_filtered":
        return []
    filter_obj = as_object(source.file_quality_filter)
    rules = as_object_list(filter_obj.get("rules"))
    failed: list[str] = []
    for raw_rule in rules:
        rule_obj = as_object(raw_rule)
        rule_name = as_string(rule_obj.get("rule"))
        if not rule_name:
            continue
        verdict = evaluate_file_quality_rule(
            path,
            rule_obj,
            is_remote_source=is_remote_source,
            original_ref=original_ref,
        )
        if verdict is None:
            continue
        ok = verdict
        if not ok:
            failed.append(rule_name)
    return failed


def is_animated_for_static_quality_skip(path: Path) -> bool:
    ext = detect_media_extension(path)
    if not ext:
        return False
    return should_route_to_animated_image_quality(path, ext)


def passes_loop_raster_animation_gate(path: Path) -> bool:
    """True when a raster may enter loop collect (animated PNG/WebP/GIF/APNG, or video)."""
    ext = detect_media_extension(path)
    if not ext:
        return False
    if ext in VIDEO_EXTS:
        return True
    if ext in ANIMATED_IMAGE_EXTS:
        return is_animated_for_static_quality_skip(path)
    return False


def loop_collect_tier_audit_from_probe(probe: dict[str, Any]) -> JsonObject:
    return {
        "loop_intent": probe.get("loop_intent"),
        "complexity": probe.get("complexity"),
        "loss_tolerance": probe.get("loss_tolerance"),
    }


def try_probe_loop_intent_for_collect(
    path_key: str,
) -> tuple[dict[str, Any], JsonObject | None]:
    """Return ``(probe_payload, tier_audit)``; ``tier_audit`` is None when collect must reject."""
    if not HAS_C_API:
        return (
            {
                "ok": False,
                "error": "python_api bridge unavailable; build foundation for loop probe",
            },
            None,
        )
    try:
        probe = probe_loop_intent(path_key)
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
        return ({"ok": False, "error": f"{type(exc).__name__}: {exc}"}, None)
    if not probe.get("ok"):
        return (probe, None)
    return (probe, loop_collect_tier_audit_from_probe(probe))


def emit_loop_collect_rejection(
    path_key: str,
    path: Path,
    *,
    reason: str,
    probe: dict[str, Any] | None = None,
) -> None:
    ext = detect_media_extension(path) or "unknown"
    detail = (
        describe_probe_failure(probe, fallback="loop probe returned not-ok payload")
        if probe
        else ""
    )
    msg = (
        "  [LOOP-COLLECT] loop_probe_rejected "
        f"path={path_key} ext={ext} reason={reason}"
    )
    if detail:
        msg += f" error={detail}"
    print(msg, file=sys.stderr, flush=True)


def read_image_dimensions_from_header(path: Path) -> tuple[int, int] | None:
    try:
        with open(path, "rb") as handle:
            header = handle.read(64 * 1024)
    except OSError:
        return None
    if header.startswith(b"\x89PNG\r\n\x1a\n") and len(header) >= 24:
        w = int.from_bytes(header[16:20], "big")
        h = int.from_bytes(header[20:24], "big")
        if w > 0 and h > 0:
            return w, h
    if header.startswith(b"\xff\xd8\xff"):
        idx = 2
        while idx + 9 < len(header):
            if header[idx] != 0xFF:
                break
            marker = header[idx + 1]
            if marker in (0xD8, 0xD9):
                idx += 2
                continue
            if idx + 4 > len(header):
                break
            seg_len = int.from_bytes(header[idx + 2 : idx + 4], "big")
            if seg_len < 2:
                break
            if marker in (0xC0, 0xC1, 0xC2, 0xC3, 0xC5, 0xC6, 0xC7, 0xC9, 0xCA, 0xCB):
                if idx + 7 < len(header):
                    h = int.from_bytes(header[idx + 5 : idx + 7], "big")
                    w = int.from_bytes(header[idx + 7 : idx + 9], "big")
                    if w > 0 and h > 0:
                        return w, h
                return None
            idx += 2 + seg_len
    if header.startswith(b"RIFF") and len(header) >= 30 and header[8:12] == b"WEBP":
        if header[12:16] == b"VP8 " and len(header) >= 30:
            w = int.from_bytes(header[26:28], "little") & 0x3FFF
            h = int.from_bytes(header[28:30], "little") & 0x3FFF
            if w > 0 and h > 0:
                return w, h
        if header[12:16] == b"VP8L" and len(header) >= 25:
            bits = int.from_bytes(header[21:25], "little")
            w = (bits & 0x3FFF) + 1
            h = ((bits >> 14) & 0x3FFF) + 1
            if w > 0 and h > 0:
                return w, h
    return None


def require_rust_tier_probe() -> None:
    if not HAS_C_API:
        raise RuntimeError(
            "Static training tier probe requires the Rust C-API (foundation). "
            "Run: cargo rustc -p foundation --lib --crate-type cdylib"
        )
    lib = resolved_library_path()
    if not lib.exists():
        raise FileNotFoundError(
            f"Rust library not found at {lib}. Run: cargo rustc -p foundation --lib --crate-type cdylib"
        )


def describe_probe_failure(
    result: Mapping[str, object] | None, *, fallback: str
) -> str:
    if result is not None:
        error_text = as_string(result.get("error"))
        if error_text:
            return " ".join(error_text.split())
    return fallback


def probe_static_image(path: Path) -> tuple[StaticImageProbe | None, str | None]:
    """Rust ``analyze_image`` entropy/geometry — same engine as DB ingest."""
    try:
        if is_animated_for_static_quality_skip(path):
            return None, None
    except (OSError, RuntimeError, MediaProbeError) as exc:
        return (
            None,
            f"static/animated routing probe failed: {type(exc).__name__}: {exc}",
        )
    try:
        result = probe_static_still_image(str(path))
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
        return None, f"tier probe raised {type(exc).__name__}: {exc}"
    if not result.get("ok"):
        return None, describe_probe_failure(
            result, fallback="tier probe returned not-ok payload"
        )
    try:
        width = int(result["width"])
        height = int(result["height"])
        entropy = float(result["entropy"])
    except (KeyError, TypeError, ValueError):
        return None, "tier probe returned malformed width/height/entropy payload"
    if width <= 0 or height <= 0 or not (entropy == entropy):
        return (
            None,
            f"tier probe returned invalid geometry/entropy width={width} height={height} entropy={entropy!r}",
        )
    high_hits = tuple(str(x) for x in as_object_list(result.get("high_rule_hits")))
    low_hits = tuple(str(x) for x in as_object_list(result.get("low_rule_hits")))
    resolved = as_string(result.get("resolved_tier")) or None
    return (
        StaticImageProbe(
            width=width,
            height=height,
            entropy=entropy,
            format=as_string(result.get("format")),
            high_tier=bool(result.get("high_tier")),
            low_tier=bool(result.get("low_tier")),
            high_rule_hits=high_hits,
            low_rule_hits=low_hits,
            resolved_tier=resolved,
            ambiguous_both_tiers=bool(result.get("ambiguous_both_tiers")),
            raw=dict(result),
        ),
        None,
    )


def tier_audit_stream_enabled() -> bool:
    return training_env_truthy(TRAINING_TIER_AUDIT_STREAM_ENV, default=True)


def default_tier_audit_path() -> Path:
    override = (os.environ.get(TRAINING_TIER_AUDIT_ENV) or "").strip()
    if override:
        return Path(override)
    return TRAINING_LOG_DIR / "training_tier_audit.jsonl"


def open_tier_audit_stream() -> Path | None:
    """Append tier decisions during long STATIC-TIER scans (crash-safe, auditable)."""
    global _tier_audit_stream, _tier_audit_stream_path
    if not tier_audit_stream_enabled():
        return None
    if _tier_audit_stream is not None:
        return _tier_audit_stream_path
    out = default_tier_audit_path()
    out.parent.mkdir(parents=True, exist_ok=True)
    _tier_audit_stream = open(out, "a", encoding="utf-8")
    _tier_audit_stream_path = out
    print(
        f"  [STATIC-TIER] audit_stream={out} "
        f"(incremental append; set {TRAINING_TIER_AUDIT_STREAM_ENV}=0 to disable)",
        flush=True,
    )
    return out


def close_tier_audit_stream() -> None:
    global _tier_audit_stream, _tier_audit_stream_path
    if _tier_audit_stream is not None:
        _tier_audit_stream.close()
    _tier_audit_stream = None
    _tier_audit_stream_path = None


def append_tier_audit_record(
    path_key: str,
    label: str,
    probe: StaticImageProbe,
    *,
    prefilter_high: bool,
    prefilter_low: bool,
) -> None:
    record: JsonObject = {
        "ts": datetime.now(timezone.utc).isoformat(),
        "path": path_key,
        "assigned_label": label,
        "prefilter_high": prefilter_high,
        "prefilter_low": prefilter_low,
        "entropy_engine": "rust_analyze_image",
        "probe": probe.raw,
        "tier_consistent": (
            probe.resolved_tier is not None and label == probe.resolved_tier
        ),
    }
    TIER_AUDIT_RECORDS.append(record)
    if _tier_audit_stream is not None:
        _tier_audit_stream.write(json.dumps(record, ensure_ascii=False) + "\n")
        if len(TIER_AUDIT_RECORDS) % TIER_AUDIT_FLUSH_EVERY == 0:
            _tier_audit_stream.flush()


def default_replica_audit_path() -> Path:
    override = (os.environ.get(TRAINING_REPLICA_AUDIT_ENV) or "").strip()
    if override:
        return Path(override)
    stamp = (
        os.environ.get("MFB_TRAINING_SESSION_STAMP") or ""
    ).strip() or datetime.now(timezone.utc).strftime("%Y%m%d_%H%M%S")
    return TRAINING_LOG_DIR / f"replica_audit_{stamp}.jsonl"


def append_replica_audit(record: JsonObject, handle: Any) -> None:
    record.setdefault("ts", datetime.now(timezone.utc).isoformat())
    handle.write(json.dumps(record, ensure_ascii=False) + "\n")


def write_tier_audit_jsonl(path: Path | None = None) -> Path | None:
    if not TIER_AUDIT_RECORDS:
        return None
    if _tier_audit_stream_path is not None:
        if _tier_audit_stream is not None:
            _tier_audit_stream.flush()
        return _tier_audit_stream_path
    out = path or default_tier_audit_path()
    out.parent.mkdir(parents=True, exist_ok=True)
    with open(out, "w", encoding="utf-8") as handle:
        handle.writelines(
            json.dumps(record, ensure_ascii=False) + "\n"
            for record in TIER_AUDIT_RECORDS
        )
    return out


def _append_loop_sample_from_path(
    path_key: str,
    item: Path,
    *,
    loop_src: SampleSources,
    samples: list[Sample],
    duplicate_skipped: int,
) -> tuple[int, bool]:
    if GLOBAL_LABEL_OWNERS.get(path_key):
        return duplicate_skipped + 1, False
    GLOBAL_LABEL_OWNERS[path_key] = "animated_loop"
    try:
        passes_animation_gate = passes_loop_raster_animation_gate(item)
    except (OSError, RuntimeError, MediaProbeError) as exc:
        emit_loop_collect_rejection(
            path_key,
            item,
            reason=f"animation_probe_failed:{type(exc).__name__}:{exc}",
        )
        return duplicate_skipped, False
    if not passes_animation_gate:
        emit_loop_collect_rejection(path_key, item, reason="static_raster")
        return duplicate_skipped, False
    probe, audit = try_probe_loop_intent_for_collect(path_key)
    if audit is None:
        emit_loop_collect_rejection(path_key, item, reason="probe_failed", probe=probe)
        return duplicate_skipped, False
    samples.append(Sample(path_key, "animated_loop", False, loop_src, tier_audit=audit))
    return duplicate_skipped, True


def collect_static_local_unified(
    high_group: QualityGroup,
    low_group: QualityGroup,
    *,
    label_filter: str | None,
    also_collect_loop: bool = False,
) -> list[Sample]:
    """Walk shared local_dirs (segmented when huge); Rust tier rules assign high vs low."""
    local_dirs = sorted(
        {
            d
            for d in (*high_group.sources.local_dirs, *low_group.sources.local_dirs)
            if d
        }
    )
    if not require_local_training_dirs(local_dirs, "static local collector"):
        return []
    require_rust_tier_probe()

    samples: list[Sample] = []
    scanned = 0
    passed_filter = 0
    tier_high = 0
    tier_low = 0
    tier_ambiguous_excluded = 0
    tier_unclassified = 0
    skipped_animated = 0
    skipped_unprobeable = 0
    loop_collected = 0
    loop_duplicate_skipped = 0
    loop_src = EMPTY_SAMPLE_SOURCES
    filter_failures: Counter[str] = Counter()
    gov = training_scan_governor()
    scan_interval = gov.scan_interval()
    heartbeat_secs = gov.heartbeat_secs()
    scan_started = time.monotonic()
    last_progress_scanned = 0
    last_progress_at = scan_started
    open_tier_audit_stream()
    print(
        "  [STATIC-TIER] rescan_start "
        f"dirs={len(local_dirs)} engine=mfb_probe_static_still_image "
        f"{gov.schedule_hint()} "
        f"progress_paths=basename_only "
        f"({TRAINING_VERBOSE_ENV}=1 for full paths)"
        + (" loop_collect=unified" if also_collect_loop else ""),
        flush=True,
    )

    for dir_index, d in enumerate(local_dirs, start=1):
        root = Path(d)
        if not root.is_dir():
            print(f"  [SKIP] Local dir not found: {d}", file=sys.stderr)
            continue
        print(
            f"  [STATIC-TIER] dir_start {dir_index}/{len(local_dirs)} path={root}",
            flush=True,
        )
        for _segment, item in iter_segmented_media_files(root, tag="STATIC-TIER"):
            scanned += 1
            if scanned % 32 == 0:
                gov.maybe_reprobe()
                scan_interval = gov.scan_interval()
                heartbeat_secs = gov.heartbeat_secs()
            gov.yield_scan_slot(scanned)
            now = time.monotonic()
            if should_emit_scan_progress(
                scanned,
                last_progress_scanned=last_progress_scanned,
                now=now,
                last_progress_at=last_progress_at,
                count_interval=scan_interval,
                heartbeat_secs=heartbeat_secs,
            ):
                elapsed = now - scan_started
                scan_rate = scanned / elapsed if elapsed > 0 else 0.0
                path_suffix = scan_progress_path_suffix(item, verbose=False)
                print(
                    "  [STATIC-TIER] scanning… "
                    f"scanned={scanned}, prefilter_pass={passed_filter}, "
                    f"high={tier_high}, low={tier_low}, "
                    f"unclassified={tier_unclassified}, animated_skip={skipped_animated}, "
                    f"dir={dir_index}/{len(local_dirs)}, {path_suffix}, "
                    f"rate={scan_rate:.1f}/s, elapsed={format_elapsed_secs(elapsed)}",
                    flush=True,
                )
                last_progress_scanned = scanned
                last_progress_at = now
                training_session_heartbeat(
                    pipeline="static_tier",
                    scanned=scanned,
                    high=tier_high,
                    low=tier_low,
                )
            try:
                routes_to_animation = is_animated_for_static_quality_skip(item)
            except (OSError, RuntimeError, MediaProbeError) as exc:
                skipped_unprobeable += 1
                print(
                    "  [STATIC-TIER] animation_probe_failed "
                    f"path={item} error={type(exc).__name__}: {exc} scanned={scanned}",
                    file=sys.stderr,
                    flush=True,
                )
                continue
            if routes_to_animation:
                skipped_animated += 1
                if also_collect_loop:
                    try:
                        path_key = str(item.resolve())
                    except OSError as exc:
                        print(
                            "  [STATIC-TIER] resolve_failed "
                            f"path={item} error={type(exc).__name__}: {exc}; "
                            "using absolute() key",
                            file=sys.stderr,
                            flush=True,
                        )
                        path_key = str(item.absolute())
                    loop_duplicate_skipped, added = _append_loop_sample_from_path(
                        path_key,
                        item,
                        loop_src=loop_src,
                        samples=samples,
                        duplicate_skipped=loop_duplicate_skipped,
                    )
                    if added:
                        loop_collected += 1
                continue

            high_pre = passes_file_quality_filter(item, high_group.sources)
            low_pre = passes_file_quality_filter(item, low_group.sources)
            if not high_pre and not low_pre:
                for rule_name in failed_file_quality_rules(item, high_group.sources):
                    filter_failures[rule_name] += 1
                continue
            passed_filter += 1

            probe_started = time.monotonic()
            try:
                probe, probe_error = probe_static_image(item)
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
                skipped_unprobeable += 1
                print(
                    "  [STATIC-TIER] probe_exception "
                    f"path={item} error={type(exc).__name__}: {exc} scanned={scanned}",
                    file=sys.stderr,
                    flush=True,
                )
                continue
            probe_elapsed = time.monotonic() - probe_started
            if probe_elapsed >= STATIC_TIER_SLOW_PROBE_SECS:
                print(
                    "  [STATIC-TIER] slow_probe "
                    f"path={item} elapsed={format_elapsed_secs(probe_elapsed)} "
                    f"scanned={scanned}",
                    flush=True,
                )
            if probe is None:
                skipped_unprobeable += 1
                if probe_error:
                    print(
                        "  [STATIC-TIER] probe_failed "
                        f"path={item} error={probe_error} scanned={scanned}",
                        file=sys.stderr,
                        flush=True,
                    )
                continue

            label = probe.resolved_tier
            if label is None:
                if probe.ambiguous_both_tiers:
                    tier_ambiguous_excluded += 1
                else:
                    tier_unclassified += 1
                continue

            if label_filter and label_filter != label:
                continue

            try:
                path_key = str(item.resolve())
            except OSError as exc:
                print(
                    "  [STATIC-TIER] resolve_failed "
                    f"path={item} error={type(exc).__name__}: {exc}; "
                    "using absolute() key",
                    file=sys.stderr,
                    flush=True,
                )
                path_key = str(item.absolute())
            owner = GLOBAL_LABEL_OWNERS.get(path_key)
            if owner and owner != label:
                raise ValueError(
                    f"cross-label contamination: {path_key} seen in both {owner} and {label}"
                )
            GLOBAL_LABEL_OWNERS[path_key] = label
            source = high_group.sources if label == "high" else low_group.sources
            append_tier_audit_record(
                path_key, label, probe, prefilter_high=high_pre, prefilter_low=low_pre
            )
            audit_snapshot: JsonObject = {
                "assigned_label": label,
                "entropy": probe.entropy,
                "width": probe.width,
                "height": probe.height,
                "high_rule_hits": list(probe.high_rule_hits),
                "low_rule_hits": list(probe.low_rule_hits),
                "ambiguous_both_tiers": probe.ambiguous_both_tiers,
            }
            samples.append(
                Sample(path_key, label, False, source, tier_audit=audit_snapshot)
            )
            if label == "high":
                tier_high += 1
            else:
                tier_low += 1

    print(
        "  [STATIC-TIER] "
        f"dirs={len(local_dirs)}, scanned={scanned}, prefilter_pass={passed_filter}, "
        f"high={tier_high}, low={tier_low}, ambiguous_excluded={tier_ambiguous_excluded}, "
        f"unclassified={tier_unclassified}, animated_skip={skipped_animated}, "
        f"unprobeable={skipped_unprobeable}, "
        f"loop_collected={loop_collected}, loop_dup_skip={loop_duplicate_skipped}, "
        f"entropy_engine=rust_analyze_image, "
        f"failed_by_rule={format_counter_top(filter_failures) or 'none'}, "
        f"elapsed={format_elapsed_secs(time.monotonic() - scan_started)}"
    )
    warn_corpus_tier_coverage(
        prefilter_pass=passed_filter,
        tier_high=tier_high,
        tier_low=tier_low,
        tier_unclassified=tier_unclassified,
        tier_ambiguous_excluded=tier_ambiguous_excluded,
    )
    if _tier_audit_stream_path is not None:
        print(
            f"  [STATIC-TIER] audit_stream_closed rows={len(TIER_AUDIT_RECORDS)} "
            f"path={_tier_audit_stream_path}",
            flush=True,
        )
    close_tier_audit_stream()
    return samples


def collect_loop_local_from_media_dirs(
    high_group: QualityGroup,
    low_group: QualityGroup,
) -> list[Sample]:
    """Collect animated assets from the same local_dirs as static (for loop_intent balance)."""
    local_dirs = sorted(
        {
            d
            for d in (*high_group.sources.local_dirs, *low_group.sources.local_dirs)
            if d
        }
    )
    if not require_local_training_dirs(local_dirs, "loop local collector"):
        return []
    loop_src = EMPTY_SAMPLE_SOURCES
    samples: list[Sample] = []
    scanned = 0
    skipped_static = 0
    duplicate_skipped = 0
    probe_failed = 0
    gov = training_scan_governor()
    scan_interval = gov.scan_interval()
    heartbeat_secs = gov.heartbeat_secs()
    scan_started = time.monotonic()
    last_progress_scanned = 0
    last_progress_at = scan_started
    print(
        f"  [LOOP-COLLECT] start dirs={len(local_dirs)} {gov.schedule_hint()}",
        flush=True,
    )
    for dir_index, d in enumerate(local_dirs, start=1):
        root = Path(d)
        if not root.is_dir():
            print(f"  [SKIP] Local dir not found: {d}", file=sys.stderr)
            continue
        print(
            f"  [LOOP-COLLECT] dir_start {dir_index}/{len(local_dirs)} path={root}",
            flush=True,
        )
        for _segment, item in iter_segmented_media_files(root, tag="LOOP-COLLECT"):
            scanned += 1
            if scanned % 32 == 0:
                gov.maybe_reprobe()
                scan_interval = gov.scan_interval()
                heartbeat_secs = gov.heartbeat_secs()
            gov.yield_scan_slot(scanned)
            now = time.monotonic()
            if should_emit_scan_progress(
                scanned,
                last_progress_scanned=last_progress_scanned,
                now=now,
                last_progress_at=last_progress_at,
                count_interval=scan_interval,
                heartbeat_secs=heartbeat_secs,
            ):
                elapsed = now - scan_started
                scan_rate = scanned / elapsed if elapsed > 0 else 0.0
                path_suffix = scan_progress_path_suffix(item, verbose=False)
                print(
                    "  [LOOP-COLLECT] scanning… "
                    f"scanned={scanned}, animated={len(samples)}, "
                    f"still_skipped={skipped_static}, duplicate_skipped={duplicate_skipped}, "
                    f"dir={dir_index}/{len(local_dirs)}, {path_suffix}, "
                    f"rate={scan_rate:.1f}/s, elapsed={format_elapsed_secs(elapsed)}",
                    flush=True,
                )
                last_progress_scanned = scanned
                last_progress_at = now
            try:
                routes_to_animation = is_animated_for_static_quality_skip(item)
            except (OSError, RuntimeError, MediaProbeError) as exc:
                probe_failed += 1
                print(
                    "  [LOOP-COLLECT] animation_probe_failed "
                    f"path={item} error={type(exc).__name__}: {exc} scanned={scanned}",
                    file=sys.stderr,
                    flush=True,
                )
                continue
            if not routes_to_animation:
                skipped_static += 1
                continue
            try:
                path_key = str(item.resolve())
            except OSError as exc:
                print(
                    "  [LOOP-COLLECT] resolve_failed "
                    f"path={item} error={type(exc).__name__}: {exc}; "
                    "using absolute() key",
                    file=sys.stderr,
                    flush=True,
                )
                path_key = str(item.absolute())
            duplicate_skipped, _added = _append_loop_sample_from_path(
                path_key,
                item,
                loop_src=loop_src,
                samples=samples,
                duplicate_skipped=duplicate_skipped,
            )
    print(
        f"  [LOOP-COLLECT] dirs={len(local_dirs)}, scanned={scanned}, "
        f"animated={len(samples)}, still_skipped={skipped_static}, "
        f"duplicate_skipped={duplicate_skipped}, probe_failed={probe_failed}, "
        f"elapsed={format_elapsed_secs(time.monotonic() - scan_started)}"
    )
    return samples


def check_disk_space() -> int:
    _, _, free = shutil.disk_usage("/")
    free_gb = free // (2**30)
    if free_gb < DISK_SAFETY_THRESHOLD_GB:
        print(
            f"{pick_symbol('🚨', ('[CRIT]'))} DISK SPACE CRISIS: Only {free_gb}GB free. "
            f"Safety threshold is {DISK_SAFETY_THRESHOLD_GB}GB. ABORTING."
        )
        sys.exit(28)  # ENOSPC
    return free_gb


def collect_samples(
    source: SampleSources,
    urls: Sequence[str],
    label: str,
) -> list[Sample]:
    samples: list[Sample] = []
    seen_paths: set[str] = set()
    local_scanned = 0
    local_passed = 0
    local_filtered = 0
    filter_failures: Counter[str] = Counter()
    gov = training_scan_governor()
    scan_interval = gov.scan_interval()
    heartbeat_secs = gov.heartbeat_secs()
    collect_started = time.monotonic()
    last_progress_scanned = 0
    last_progress_at = collect_started
    if source.local_dirs:
        print(
            "  [COLLECT] start "
            f"label={label} dirs={len(source.local_dirs)} {gov.schedule_hint()}",
            flush=True,
        )
    for dir_index, d in enumerate(source.local_dirs, start=1):
        p = Path(d)
        if not p.is_dir():
            print(f"  [SKIP] Local dir not found: {d}", file=sys.stderr)
            continue
        print(
            f"  [COLLECT] dir_start label={label} {dir_index}/{len(source.local_dirs)} path={p}",
            flush=True,
        )
        for _segment, item in iter_segmented_media_files(p, tag="COLLECT"):
            local_scanned += 1
            if local_scanned % 32 == 0:
                gov.maybe_reprobe()
                scan_interval = gov.scan_interval()
                heartbeat_secs = gov.heartbeat_secs()
            gov.yield_scan_slot(local_scanned)
            now = time.monotonic()
            if should_emit_scan_progress(
                local_scanned,
                last_progress_scanned=last_progress_scanned,
                now=now,
                last_progress_at=last_progress_at,
                count_interval=scan_interval,
                heartbeat_secs=heartbeat_secs,
            ):
                elapsed = now - collect_started
                scan_rate = local_scanned / elapsed if elapsed > 0 else 0.0
                path_suffix = scan_progress_path_suffix(item, verbose=False)
                print(
                    "  [COLLECT] scanning… "
                    f"label={label} scanned={local_scanned}, passed={local_passed}, "
                    f"filtered={local_filtered}, dir={dir_index}/{len(source.local_dirs)}, "
                    f"{path_suffix}, rate={scan_rate:.1f}/s, "
                    f"elapsed={format_elapsed_secs(elapsed)}",
                    flush=True,
                )
                last_progress_scanned = local_scanned
                last_progress_at = now
                training_session_heartbeat(
                    pipeline="collect",
                    label=label,
                    scanned=local_scanned,
                    passed=local_passed,
                )
            if not passes_file_quality_filter(item, source):
                local_filtered += 1
                for rule_name in failed_file_quality_rules(item, source):
                    filter_failures[rule_name] += 1
                continue
            try:
                path_key = str(item.resolve())
            except OSError as exc:
                print(
                    "  [COLLECT] resolve_failed "
                    f"path={item} error={type(exc).__name__}: {exc}; "
                    "using absolute() key",
                    file=sys.stderr,
                    flush=True,
                )
                path_key = str(item.absolute())
            if path_key in seen_paths:
                continue
            owner = GLOBAL_LABEL_OWNERS.get(path_key)
            if owner and owner != label:
                raise ValueError(
                    f"cross-label contamination: {path_key} seen in both {owner} and {label}"
                )
            GLOBAL_LABEL_OWNERS[path_key] = label
            seen_paths.add(path_key)
            if label == "animated_loop":
                try:
                    passes_animation_gate = passes_loop_raster_animation_gate(item)
                except (OSError, RuntimeError, MediaProbeError) as exc:
                    local_filtered += 1
                    filter_failures["loop_animation_probe_failed"] += 1
                    emit_loop_collect_rejection(
                        path_key,
                        item,
                        reason=f"animation_probe_failed:{type(exc).__name__}:{exc}",
                    )
                    continue
                if not passes_animation_gate:
                    local_filtered += 1
                    filter_failures["loop_static_raster"] += 1
                    emit_loop_collect_rejection(path_key, item, reason="static_raster")
                    continue
                probe, loop_audit = try_probe_loop_intent_for_collect(path_key)
                if loop_audit is None:
                    local_filtered += 1
                    filter_failures["loop_probe_rejected"] += 1
                    emit_loop_collect_rejection(
                        path_key, item, reason="probe_failed", probe=probe
                    )
                    continue
                samples.append(
                    Sample(path_key, label, False, source, tier_audit=loop_audit)
                )
            else:
                samples.append(Sample(path_key, label, False, source))
            local_passed += 1
    for url in urls:
        url_key = f"remote::{url}"
        owner = GLOBAL_LABEL_OWNERS.get(url_key)
        if owner and owner != label:
            raise ValueError(
                f"cross-label contamination: {url} seen in both {owner} and {label}"
            )
        GLOBAL_LABEL_OWNERS[url_key] = label
        samples.append(Sample(url, label, True, source))
    if source.selection_strategy == "file_quality_filtered":
        failures_top = format_counter_top(filter_failures) or "none"
        print(
            "  [FILTER] "
            f"{label}: scanned={local_scanned}, passed={local_passed}, "
            f"filtered={local_filtered}, remote_urls={len(urls)}, "
            f"failed_by_rule={failures_top}, "
            f"elapsed={format_elapsed_secs(time.monotonic() - collect_started)}"
        )
    return samples


def resolve_local_source_path(path_str: str) -> str:
    try:
        return str(Path(path_str).resolve())
    except OSError as exc:
        print(
            f"  [SCAN] resolve() failed for {path_str}: {exc}; using absolute() key",
            file=sys.stderr,
            flush=True,
        )
        return str(Path(path_str).absolute())


def print_plan(samples: list[Sample], *, scope_line: str | None = None) -> None:
    counts = Counter(s.base_label for s in samples)
    remote_count = sum(1 for s in samples if s.is_remote)
    print("=== DRY-RUN PLAN ===")
    if scope_line:
        print(f"  {scope_line}")
    for label, count in sorted(counts.items()):
        print(f"  {label:20s}: {count} samples")
    print(f"  WEB SOURCES       : {remote_count} links resolved")
    print(f"  TOTAL             : {len(samples)}")
    if LAST_BALANCE_SUMMARY:
        after = LAST_BALANCE_SUMMARY.get("after", {})
        mc = LAST_BALANCE_SUMMARY.get("mean_complexity", {})
        print(
            f"  BALANCE           : high={after.get('high', 0)} low={after.get('low', 0)} "
            f"loop={after.get('loop', 0)} non_loop={after.get('non_loop', 0)} "
            f"(mean_cx high={mc.get('high')} low={mc.get('low')})"
        )
    if TIER_AUDIT_RECORDS:
        audit_path = write_tier_audit_jsonl()
        if audit_path:
            consistent = sum(1 for r in TIER_AUDIT_RECORDS if r.get("tier_consistent"))
            print(
                f"  TIER AUDIT        : {len(TIER_AUDIT_RECORDS)} rows "
                f"({consistent} label/rule-consistent), rust entropy"
            )
            print(f"  AUDIT FILE        : {audit_path}")
    print("\nDefault run ingests to PostgreSQL; pass --dry-run to skip DB writes.")


def loop_intent_label_for_api(loop_intent_label: str) -> str | None:
    """Map CLI --loop-intent-label to C-API / ingest label; None means heuristic auto."""
    v = (loop_intent_label or "auto").strip().lower()
    if v in ("auto", ""):
        return None
    if v == "high":
        return "high"
    if v == "low":
        return "low"
    if v == "video":
        return "video"
    raise ValueError(f"unsupported loop_intent_label: {loop_intent_label!r}")


def enforce_training_db_caps(args: argparse.Namespace) -> None:
    """Clamp ingest caps to committed SSOT ceilings (CLI/local/json cannot exceed)."""
    mode = (args.training_mode or "static").strip().lower()
    label = (args.label or "").strip().lower()
    loop_bucket = explicit_loop_balance_bucket(
        getattr(args, "loop_intent_label", "auto")
    )

    targets: dict[str, int] = {}
    if mode in ("static", "all"):
        if label in ("", "high"):
            targets["max_high"] = STATIC_QUALITY_DB_CAP_PER_CLASS
        if label in ("", "low"):
            targets["max_low"] = STATIC_QUALITY_DB_CAP_PER_CLASS
    if mode in ("loop", "all"):
        if loop_bucket == "loop":
            targets["max_loop"] = LOOP_INTENT_DB_CAP_PER_CLASS
        elif loop_bucket == "non_loop":
            targets["max_non_loop"] = LOOP_INTENT_DB_CAP_PER_CLASS
        elif loop_bucket == "uncertain":
            targets["max_loop"] = LOOP_INTENT_DB_CAP_PER_CLASS
        else:
            targets["max_loop"] = LOOP_INTENT_DB_CAP_PER_CLASS
            targets["max_non_loop"] = LOOP_INTENT_DB_CAP_PER_CLASS

    applied: list[str] = []
    for cap_name, ceiling in targets.items():
        cur = int(getattr(args, cap_name, 0) or 0)
        if cur != ceiling:
            if cur > ceiling:
                applied.append(f"{cap_name}={cur}→{ceiling}")
                setattr(args, cap_name, ceiling)
            elif cur == 0:
                applied.append(f"{cap_name}={ceiling}")
                setattr(args, cap_name, ceiling)
    for cap_name, cap_ceil in (
        ("max_high", STATIC_QUALITY_DB_CAP_PER_CLASS),
        ("max_low", STATIC_QUALITY_DB_CAP_PER_CLASS),
        ("max_loop", LOOP_INTENT_DB_CAP_PER_CLASS),
        ("max_non_loop", LOOP_INTENT_DB_CAP_PER_CLASS),
    ):
        if cap_name in targets:
            continue
        cur = int(getattr(args, cap_name, 0) or 0)
        if cur > cap_ceil:
            setattr(args, cap_name, cap_ceil)
            applied.append(f"{cap_name}={cur}→{cap_ceil}")
    if mode == "static" and label == "high":
        for cap_name in ("max_low", "max_loop", "max_non_loop"):
            if int(getattr(args, cap_name, 0) or 0) != 0:
                setattr(args, cap_name, 0)
                applied.append(f"{cap_name}=0")
    elif mode == "static" and label == "low":
        for cap_name in ("max_high", "max_loop", "max_non_loop"):
            if int(getattr(args, cap_name, 0) or 0) != 0:
                setattr(args, cap_name, 0)
                applied.append(f"{cap_name}=0")
    elif mode == "loop" and loop_bucket == "loop":
        for cap_name in ("max_high", "max_low", "max_non_loop"):
            if int(getattr(args, cap_name, 0) or 0) != 0:
                setattr(args, cap_name, 0)
                applied.append(f"{cap_name}=0")
    elif mode == "loop" and loop_bucket == "non_loop":
        for cap_name in ("max_high", "max_low", "max_loop"):
            if int(getattr(args, cap_name, 0) or 0) != 0:
                setattr(args, cap_name, 0)
                applied.append(f"{cap_name}=0")
    elif mode == "loop" and loop_bucket == "uncertain":
        for cap_name in ("max_high", "max_low", "max_non_loop"):
            if int(getattr(args, cap_name, 0) or 0) != 0:
                setattr(args, cap_name, 0)
                applied.append(f"{cap_name}=0")
    if applied:
        print(
            "  [INGEST] training_db_caps_ssot: " + ", ".join(applied),
            flush=True,
        )


def apply_ingest_profile(args: argparse.Namespace, profile: JsonObject | None) -> None:
    """Apply training_rules.local.json → ingest (py-only knobs) when CLI left defaults."""
    if not profile:
        return
    applied: list[str] = []
    mode = as_string(profile.get("training_mode"))
    if mode in ("all", "static", "loop") and args.training_mode is None:
        args.training_mode = mode
        applied.append(f"training_mode={mode}")
    if bool(profile.get("balance")) and not args.balance:
        args.balance = True
        applied.append("balance=on")
    for cap_name in ("max_high", "max_low", "max_loop", "max_non_loop"):
        raw = profile.get(cap_name)
        if raw is None:
            continue
        try:
            cap_val = int(raw)
        except (TypeError, ValueError) as exc:
            raise ValueError(f"ingest.{cap_name} must be an integer") from exc
        if cap_val < 0:
            raise ValueError(f"ingest.{cap_name} must be >= 0")
        if cap_val > 0 and int(getattr(args, cap_name, 0) or 0) == 0:
            setattr(args, cap_name, cap_val)
            applied.append(f"{cap_name}={cap_val}")
    if bool(profile.get("fill_runtime_assets")) and args.fill_runtime_assets is None:
        args.fill_runtime_assets = True
        applied.append("fill_runtime_assets=on")
    if bool(profile.get("no_balance_complexity")) and not args.no_balance_complexity:
        args.no_balance_complexity = True
        applied.append("no_balance_complexity=on")
    if applied:
        print(
            "  [INGEST] training_rules.local.json ingest: " + ", ".join(applied),
            flush=True,
        )


def balancing_enabled(args: argparse.Namespace) -> bool:
    if getattr(args, "balance", False):
        return True
    return any(
        int(getattr(args, name, 0) or 0) > 0
        for name in ("max_high", "max_low", "max_loop", "max_non_loop")
    )


def sample_complexity_score(sample: Sample) -> float | None:
    audit = sample.tier_audit or {}
    entropy = audit.get("entropy")
    if isinstance(entropy, (int, float)) and float(entropy) == float(entropy):
        return float(entropy)
    if sample.base_label == "animated_loop" and not sample.is_remote:
        probe = probe_loop_intent(sample.path_or_url)
        if probe.get("ok"):
            if "complexity" not in probe:
                return None
            value = probe.get("complexity")
            try:
                parsed = float(value)
                return parsed if parsed == parsed else None
            except (TypeError, ValueError):
                return None
    return None


def _complexity_sort_key(sample: Sample) -> float:
    value = sample_complexity_score(sample)
    # Unknown complexity must not be forged as 0.0 (which biases balancing).
    return value if value is not None else float("inf")


def loop_collect_quality_group(
    rules: RulesConfig, loop_intent_label: str
) -> QualityGroup:
    """Pick animated corpus group for loop-mode collection (loop vs non-loop contrast)."""
    if explicit_loop_balance_bucket(loop_intent_label) == "non_loop":
        return rules.animated_image.get(
            "non_loop_intent",
            rules.animated_image.get("loop_intent", EMPTY_QUALITY_GROUP),
        )
    return rules.animated_image.get("loop_intent", EMPTY_QUALITY_GROUP)


def explicit_loop_balance_bucket(loop_intent_label: str) -> str | None:
    v = (loop_intent_label or "auto").strip().lower()
    if v == "high":
        return "loop"
    if v == "low":
        return "uncertain"
    if v == "video":
        return "non_loop"
    return None


def sample_loop_intent_bucket(
    sample: Sample, *, explicit_remote_bucket: str | None = None
) -> str:
    audit = sample.tier_audit or {}
    cached = audit.get("loop_intent")
    if isinstance(cached, str) and cached:
        return cached
    if sample.base_label != "animated_loop":
        return ""
    if sample.is_remote:
        return explicit_remote_bucket or "uncertain"
    probe = probe_loop_intent(sample.path_or_url)
    if probe.get("ok"):
        return str(probe.get("loop_intent") or "uncertain")
    error = describe_probe_failure(probe, fallback="loop probe returned not-ok payload")
    if fail_closed_training_enabled():
        raise RuntimeError(
            "loop intent balance probe failed; refusing uncertain fallback "
            f"path={sample.path_or_url} error={error}"
        )
    print(
        "  [BALANCE] loop_probe_failed "
        f"path={sample.path_or_url} error={error} debug_uncertain_fallback=enabled",
        file=sys.stderr,
        flush=True,
    )
    return "uncertain"


def _pick_quantile_matched(
    group_a: list[Sample],
    group_b: list[Sample],
    target: int,
    *,
    match_complexity: bool,
) -> tuple[list[Sample], list[Sample]]:
    if target <= 0:
        return [], []
    if not match_complexity:
        return group_a[:target], group_b[:target]
    a_sorted = sorted(group_a, key=_complexity_sort_key)
    b_sorted = sorted(group_b, key=_complexity_sort_key)
    if target == 1:
        mid_a = a_sorted[len(a_sorted) // 2]
        mid_b = b_sorted[len(b_sorted) // 2]
        return [mid_a], [mid_b]
    picks_a: list[Sample] = []
    picks_b: list[Sample] = []
    for i in range(target):
        ia = round(i * (len(a_sorted) - 1) / max(target - 1, 1))
        ib = round(i * (len(b_sorted) - 1) / max(target - 1, 1))
        picks_a.append(a_sorted[ia])
        picks_b.append(b_sorted[ib])
    return picks_a, picks_b


def _mean_complexity(samples: Sequence[Sample]) -> float:
    if not samples:
        return float("nan")
    values = [v for v in (sample_complexity_score(s) for s in samples) if v is not None]
    if not values:
        return float("nan")
    return sum(values) / float(len(values))


def _pick_capped_group(
    group: list[Sample], target: int, *, match_complexity: bool
) -> list[Sample]:
    if target <= 0:
        return []
    if not match_complexity:
        return group[:target]
    return sorted(group, key=_complexity_sort_key)[:target]


# Minimum share of prefilter-pass files that must resolve to high/low (corpus discipline).
CORPUS_MIN_CLASSIFIED_RATIO: Final = 0.02


def warn_corpus_tier_coverage(
    *,
    prefilter_pass: int,
    tier_high: int,
    tier_low: int,
    tier_unclassified: int,
    tier_ambiguous_excluded: int,
) -> None:
    """Fail-visible when too few probe-pass files earn a tier label (dead-band leakage)."""
    classified = tier_high + tier_low
    if prefilter_pass == 0:
        print(
            "  [WARN] training_corpus_tier_coverage: prefilter_pass=0 (no files to classify)",
            file=sys.stderr,
        )
        return
    ratio = classified / prefilter_pass
    if classified == 0:
        print(
            "  [WARN] training_corpus_tier_coverage: classified=0 "
            f"(prefilter_pass={prefilter_pass}, unclassified={tier_unclassified}, "
            f"ambiguous_excluded={tier_ambiguous_excluded}); "
            "check training_tier_audit thresholds",
            file=sys.stderr,
        )
        return
    if ratio < CORPUS_MIN_CLASSIFIED_RATIO:
        print(
            "  [WARN] training_corpus_tier_coverage: "
            f"classified_ratio={ratio:.4f}<{CORPUS_MIN_CLASSIFIED_RATIO} "
            f"(high={tier_high}, low={tier_low}, unclassified={tier_unclassified}, "
            f"prefilter_pass={prefilter_pass}); tier rules may be too tight for this library",
            file=sys.stderr,
        )
    else:
        print(
            "  [INFO] training_corpus_tier_coverage: "
            f"classified={classified}/{prefilter_pass} ({ratio:.1%}), "
            f"high={tier_high}, low={tier_low}, unclassified={tier_unclassified}, "
            f"ambiguous_excluded={tier_ambiguous_excluded}",
            flush=True,
        )


def warn_static_balance_skew(
    *,
    picked_high: list[Sample],
    picked_low: list[Sample],
    pair_target: int,
    max_high: int,
    max_low: int,
) -> None:
    """Surface tier/label skew when bilateral caps cannot be filled (fail-visible, not silent)."""
    high_n = len(picked_high)
    low_n = len(picked_low)
    if high_n == 0 and low_n == 0:
        return
    cap = max(max_high, max_low)
    if cap <= 0:
        return
    min_side = min(high_n, low_n)
    max_side = max(high_n, low_n)
    skewed = min_side == 0 or (max_side > 0 and min_side * 10 < max_side)
    under_cap = pair_target < min(cap, max_side) if cap > 0 else False
    if not skewed and not under_cap:
        return
    print(
        "  [WARN] training_ingest_balance_skew: "
        f"high={high_n} low={low_n} pair_target={pair_target} "
        f"caps={max_high}/{max_low}; "
        "tier rules may under-classify low-quality stills or corpus lacks lows — "
        "see training_tier_audit / static_image.low_quality rules",
        file=sys.stderr,
    )


def balance_training_samples(
    samples: list[Sample], args: argparse.Namespace
) -> list[Sample]:
    """Cap bilateral groups and align complexity (entropy / loop_frequency) between sides."""
    global LAST_BALANCE_SUMMARY
    require_rust_tier_probe()

    static_high = [s for s in samples if s.base_label == "high"]
    static_low = [s for s in samples if s.base_label == "low"]
    loop_samples = [s for s in samples if s.base_label == "animated_loop"]
    other = [s for s in samples if s.base_label not in ("high", "low", "animated_loop")]

    max_high = int(args.max_high or 0)
    max_low = int(args.max_low or 0)
    max_loop = int(args.max_loop or 0)
    max_non_loop = int(args.max_non_loop or 0)
    match_cx = not getattr(args, "no_balance_complexity", False)
    explicit_loop_bucket = explicit_loop_balance_bucket(
        getattr(args, "loop_intent_label", "auto")
    )

    loop_bucket: list[Sample] = []
    non_loop_bucket: list[Sample] = []
    loop_uncertain: list[Sample] = []
    for s in loop_samples:
        bucket = sample_loop_intent_bucket(
            s, explicit_remote_bucket=explicit_loop_bucket
        )
        if bucket == "loop":
            loop_bucket.append(s)
        elif bucket == "non_loop":
            non_loop_bucket.append(s)
        else:
            loop_uncertain.append(s)

    # Single-sided static corpus (e.g. --training-mode static --label high/low):
    # avoid bilateral matching from collapsing the selected side to zero.
    if max_high > 0 and max_low <= 0:
        picked_high = _pick_capped_group(
            static_high, max_high, match_complexity=match_cx
        )
        picked_low = []
        pair_target = 0
    elif max_low > 0 and max_high <= 0:
        picked_high = []
        picked_low = _pick_capped_group(static_low, max_low, match_complexity=match_cx)
        pair_target = 0
    else:
        pair_target = min(
            len(static_high),
            len(static_low),
            max_high if max_high > 0 else len(static_high),
            max_low if max_low > 0 else len(static_low),
        )
        picked_high, picked_low = _pick_quantile_matched(
            static_high, static_low, pair_target, match_complexity=match_cx
        )

    picked_loop_uncertain: list[Sample] = []
    if explicit_loop_bucket == "loop":
        loop_target = max_loop if max_loop > 0 else len(loop_bucket)
        picked_loop = _pick_capped_group(
            loop_bucket, loop_target, match_complexity=match_cx
        )
        picked_non_loop = []
    elif explicit_loop_bucket == "non_loop":
        non_loop_target = max_non_loop if max_non_loop > 0 else len(non_loop_bucket)
        picked_loop = []
        picked_non_loop = _pick_capped_group(
            non_loop_bucket, non_loop_target, match_complexity=match_cx
        )
    elif explicit_loop_bucket == "uncertain":
        uncertain_target = max_loop if max_loop > 0 else len(loop_uncertain)
        picked_loop = []
        picked_non_loop = []
        picked_loop_uncertain = _pick_capped_group(
            loop_uncertain, uncertain_target, match_complexity=match_cx
        )
    else:
        # Single-sided loop corpus (max_non_loop unset/0): cap loop positives only.
        # Bilateral loop↔video pairing applies only when max_non_loop > 0.
        if max_non_loop <= 0:
            loop_target = max_loop if max_loop > 0 else len(loop_bucket)
            picked_loop = _pick_capped_group(
                loop_bucket, loop_target, match_complexity=match_cx
            )
            picked_non_loop = []
        else:
            loop_pair_target = min(
                len(loop_bucket),
                len(non_loop_bucket),
                max_loop if max_loop > 0 else len(loop_bucket),
                max_non_loop,
            )
            picked_loop, picked_non_loop = _pick_quantile_matched(
                loop_bucket,
                non_loop_bucket,
                loop_pair_target,
                match_complexity=match_cx,
            )

    if max_high > 0 and len(picked_high) > max_high:
        picked_high = picked_high[:max_high]
    if max_low > 0 and len(picked_low) > max_low:
        picked_low = picked_low[:max_low]
    if max_loop > 0 and len(picked_loop) > max_loop:
        picked_loop = picked_loop[:max_loop]
    if max_non_loop > 0 and len(picked_non_loop) > max_non_loop:
        picked_non_loop = picked_non_loop[:max_non_loop]

    balanced = (
        other
        + picked_high
        + picked_low
        + picked_loop
        + picked_non_loop
        + picked_loop_uncertain
    )
    if (
        getattr(args, "balance_include_loop_uncertain", False)
        and explicit_loop_bucket != "uncertain"
    ):
        balanced.extend(loop_uncertain)

    LAST_BALANCE_SUMMARY = {
        "enabled": True,
        "match_complexity": match_cx,
        "before": {
            "high": len(static_high),
            "low": len(static_low),
            "loop": len(loop_bucket),
            "non_loop": len(non_loop_bucket),
            "loop_uncertain": len(loop_uncertain),
            "total": len(samples),
        },
        "after": {
            "high": len(picked_high),
            "low": len(picked_low),
            "loop": len(picked_loop),
            "non_loop": len(picked_non_loop),
            "loop_uncertain": len(picked_loop_uncertain),
            "total": len(balanced),
        },
        "mean_complexity": {
            "high": round(_mean_complexity(picked_high), 4),
            "low": round(_mean_complexity(picked_low), 4),
            "loop": round(_mean_complexity(picked_loop), 4),
            "non_loop": round(_mean_complexity(picked_non_loop), 4),
            "loop_uncertain": round(_mean_complexity(picked_loop_uncertain), 4),
        },
        "caps": {
            "max_high": max_high,
            "max_low": max_low,
            "max_loop": max_loop,
            "max_non_loop": max_non_loop,
        },
        "explicit_loop_bucket": explicit_loop_bucket,
        "static_pair_target": pair_target,
        "loop_balance_mode": (
            "explicit_loop"
            if explicit_loop_bucket is not None
            else "single_sided"
            if max_non_loop <= 0
            else "bilateral"
        ),
        "loop_low_scarcity_fallback": 0,
    }

    allowed_paths = {s.path_or_url for s in balanced}
    TIER_AUDIT_RECORDS[:] = [
        r for r in TIER_AUDIT_RECORDS if r.get("path") in allowed_paths
    ]

    print(
        "  [BALANCE] "
        f"high {len(static_high)}→{len(picked_high)}, "
        f"low {len(static_low)}→{len(picked_low)}, "
        f"loop {len(loop_bucket)}→{len(picked_loop)}, "
        f"non_loop {len(non_loop_bucket)}→{len(picked_non_loop)}"
        + (
            f", loop_uncertain {len(loop_uncertain)}→{len(picked_loop_uncertain)}"
            if explicit_loop_bucket == "uncertain"
            else ""
        )
        + (
            f", mean_entropy high={LAST_BALANCE_SUMMARY['mean_complexity']['high']:.3f} "
            f"low={LAST_BALANCE_SUMMARY['mean_complexity']['low']:.3f}"
            if picked_high and picked_low
            else ""
        )
        + (
            f", mean_loop_cx loop={LAST_BALANCE_SUMMARY['mean_complexity']['loop']:.3f} "
            f"non_loop={LAST_BALANCE_SUMMARY['mean_complexity']['non_loop']:.3f}"
            if picked_loop and picked_non_loop
            else ""
        )
        + (
            f", explicit_loop_label={getattr(args, 'loop_intent_label', 'auto')}"
            if explicit_loop_bucket is not None
            else ""
        )
        + (
            f", loop_uncertain_excluded={len(loop_uncertain)}"
            if loop_uncertain and explicit_loop_bucket != "uncertain"
            else ""
        )
    )
    warn_static_balance_skew(
        picked_high=picked_high,
        picked_low=picked_low,
        pair_target=pair_target,
        max_high=max_high,
        max_low=max_low,
    )
    return balanced


def training_scope_summary_line(args: argparse.Namespace) -> str:
    parts = [f"training_mode={args.training_mode}"]
    if args.training_mode in ("all", "static"):
        if args.training_mode == "static":
            parts.append("ingest=image_quality_only (animated→skip)")
        if args.label in ("high", "low"):
            parts.append(f"static_filter={args.label}_only")
        elif args.training_mode == "all" and args.label == "animated_loop":
            parts.append("static_filter=skipped (--label animated_loop)")
        else:
            parts.append("static_filter=high+low (rust tier rules)")
        if balancing_enabled(args):
            caps = []
            if int(args.max_high or 0) > 0:
                caps.append(f"high≤{args.max_high}")
            if int(args.max_low or 0) > 0:
                caps.append(f"low≤{args.max_low}")
            parts.append(
                "balance=on"
                + (f" ({', '.join(caps)})" if caps else " (paired high/low)")
            )
    if args.training_mode in ("all", "loop") and balancing_enabled(args):
        loop_caps = []
        if int(args.max_loop or 0) > 0:
            loop_caps.append(f"loop≤{args.max_loop}")
        if int(args.max_non_loop or 0) > 0:
            loop_caps.append(f"non_loop≤{args.max_non_loop}")
        elif int(args.max_loop or 0) > 0:
            loop_caps.append("loop_single_sided")
        if loop_caps:
            parts.append("loop_balance=" + ", ".join(loop_caps))
    if args.training_mode in ("all", "loop"):
        li = getattr(args, "loop_intent_label", "auto") or "auto"
        parts.append(f"loop_intent_label={li}")
    return "SCOPE: " + ", ".join(parts)


def collect_plan_samples(args: argparse.Namespace, rules: RulesConfig) -> list[Sample]:
    """Build sample list from rules + CLI scope (training-mode, --label, loop-intent)."""
    clear_ephemeral_training_state()
    TIER_AUDIT_RECORDS.clear()
    mode = args.training_mode
    if mode == "static" and args.label == "animated_loop":
        raise ValueError(
            "--training-mode static is still-image only; remove --label animated_loop "
            "or use --training-mode loop"
        )
    if mode == "loop" and args.label in ("high", "low"):
        raise ValueError(
            "--training-mode loop ingests loop_intent only; do not pass --label high/low. "
            "Use --loop-intent-label for loop overrides, or omit --label."
        )
    if mode == "loop" and args.no_loop:
        raise ValueError("--training-mode loop conflicts with --no-loop")

    include_static = mode in ("all", "static")
    include_loop = mode in ("all", "loop") and not args.no_loop

    all_samples: list[Sample] = []
    unified_loop_walk = False

    if include_static:
        high_group = rules.static_image.get("high_quality", EMPTY_QUALITY_GROUP)
        low_group = rules.static_image.get("low_quality", EMPTY_QUALITY_GROUP)
        static_dirs = {
            d
            for d in (*high_group.sources.local_dirs, *low_group.sources.local_dirs)
            if d
        }
        loop_dirs: set[str] = set()
        if include_loop:
            loop_group = rules.animated_image.get("loop_intent", EMPTY_QUALITY_GROUP)
            loop_dirs = {d for d in loop_group.sources.local_dirs if d}
        unified_loop_walk = bool(include_loop and loop_dirs <= static_dirs)
        all_samples.extend(
            collect_static_local_unified(
                high_group,
                low_group,
                label_filter=args.label if args.label in ("high", "low") else None,
                also_collect_loop=unified_loop_walk,
            )
        )
        for q, label in (("high_quality", "high"), ("low_quality", "low")):
            if args.label and args.label != label:
                continue
            group = rules.static_image.get(q, EMPTY_QUALITY_GROUP)
            src = group.sources
            urls: list[str] = []
            if args.allow_remote:
                for api in src.remote_apis:
                    urls.extend(resolve_api_urls(api, rules))
            if urls:
                all_samples.extend(collect_samples(src, urls, label))

    if include_loop:
        if mode == "all" and args.label in ("high", "low"):
            pass
        else:
            loop_group = loop_collect_quality_group(
                rules, getattr(args, "loop_intent_label", "auto") or "auto"
            )
            src = loop_group.sources
            if rules.strict_no_silent_fallbacks and loop_group == EMPTY_QUALITY_GROUP:
                raise ValueError(
                    "animated_image.loop_intent (or non_loop_intent for video lane) "
                    "is required when strict_no_silent_fallbacks=true"
                )
            if include_static and not unified_loop_walk:
                high_group = rules.static_image.get("high_quality", EMPTY_QUALITY_GROUP)
                low_group = rules.static_image.get("low_quality", EMPTY_QUALITY_GROUP)
                all_samples.extend(
                    collect_loop_local_from_media_dirs(high_group, low_group)
                )
            urls_loop: list[str] = []
            if args.allow_remote:
                for api in src.remote_apis:
                    urls_loop.extend(resolve_api_urls(api, rules))
            if src.local_dirs or urls_loop:
                all_samples.extend(collect_samples(src, urls_loop, "animated_loop"))

    if balancing_enabled(args):
        all_samples = balance_training_samples(all_samples, args)

    return all_samples


def clean_display_name(path_or_url: str, fallback: str) -> str:
    basename = os.path.basename(path_or_url.split("?", 1)[0].split("#", 1)[0])
    return basename or fallback


def detect_media_extension(path: Path) -> str:
    try:
        true_format = detect_true_format(path)
    except OSError as exc:
        raise RuntimeError(
            f"media true-format detection failed for {path}: {exc}"
        ) from exc

    if true_format == "unknown":
        return ""
    if true_format == "jpeg":
        return "jpg"
    if true_format == "png":
        return "apng" if is_animated_png(path) else "png"
    if true_format == "tiff":
        return "tiff"
    return true_format


def should_route_to_animated_image_quality(path: Path, ext: str) -> bool:
    if ext == "gif":
        return is_animated_gif(path)
    if ext == "apng":
        return True
    if ext == "png":
        return is_animated_png(path)
    if ext == "webp":
        return is_animated_webp(path)
    if ext in {"avif", "heic", "heif", "hif"}:
        return is_probably_animated_isobmff(path)
    if ext == "jxl":
        return is_animated_jxl(path)
    return False


def artifact_is_stale(artifact: Path, sources: list[Path]) -> bool:
    if not artifact.exists():
        return True
    try:
        artifact_mtime = artifact.stat().st_mtime_ns
    except OSError:
        return True

    for source in sources:
        if not source.exists():
            continue
        try:
            if source.stat().st_mtime_ns > artifact_mtime:
                return True
        except OSError:
            return True
    return False


def rebuild_rust_artifacts(bin_names: list[str]) -> None:
    cmd = ["cargo", "build", "-p", "foundation"]
    for bin_name in bin_names:
        cmd.extend(["--bin", bin_name])
    print(f"  [BUILD] Refreshing Rust artifacts: {' '.join(bin_names) or 'foundation'}")
    result = subprocess.run(
        cmd, cwd=ROOT, text=True, env=sanitized_subprocess_env(), check=False
    )
    if result.returncode != 0:
        print("[ERROR] Failed to rebuild Rust artifacts.", file=sys.stderr)
        sys.exit(result.returncode or 1)


def ensure_runtime_artifacts(
    has_quality_samples: bool, has_loop_samples: bool, use_api: bool
) -> None:
    if use_api and HAS_C_API:
        lib_path = resolved_library_path()
        if artifact_is_stale(lib_path, RUNTIME_CONTRACT_SOURCES):
            rebuild_rust_artifacts([])
        if not lib_path.exists():
            print(
                f"[ERROR] foundation dynamic library missing after rebuild: {lib_path}",
                file=sys.stderr,
            )
            sys.exit(1)

    cli_bins: list[str] = []
    if has_quality_samples and artifact_is_stale(
        TRAIN_BIN_QUALITY, QUALITY_CLI_SOURCES
    ):
        cli_bins.append("train_quality")
    if has_loop_samples and artifact_is_stale(TRAIN_BIN_KNN, LOOP_CLI_SOURCES):
        cli_bins.append("train_knn")
    if cli_bins:
        rebuild_rust_artifacts(cli_bins)


def ingest_artifact_for_cmd(cmd: Sequence[str]) -> tuple[Path, str, list[Path]] | None:
    if not cmd:
        raise ValueError("empty Rust ingest command")
    cmd_path = Path(os.fspath(cmd[0]))
    if cmd_path.name == TRAIN_BIN_QUALITY.name:
        return TRAIN_BIN_QUALITY, "train_quality", QUALITY_CLI_SOURCES
    if cmd_path.name == TRAIN_BIN_KNN.name:
        return TRAIN_BIN_KNN, "train_knn", LOOP_CLI_SOURCES
    return None


def ensure_rust_ingest_cli_available(cmd: Sequence[str]) -> None:
    artifact = ingest_artifact_for_cmd(cmd)
    if artifact is None:
        return
    bin_path, bin_name, sources = artifact
    if artifact_is_stale(bin_path, sources):
        rebuild_rust_artifacts([bin_name])
    if not bin_path.exists():
        raise FileNotFoundError(
            f"Rust ingest binary unavailable after rebuild: {bin_path}"
        )


def run_rust_ingest(
    cmd: Sequence[str],
    *,
    conn_str: str,
    cwd: str | Path | None = None,
    capture_output: bool = True,
    text: bool = True,
    **kwargs: Any,
) -> Any:
    ensure_rust_ingest_cli_available(cmd)
    return _guarded_run_rust_ingest(
        cmd,
        conn_str=conn_str,
        cwd=cwd,
        capture_output=capture_output,
        text=text,
        **kwargs,
    )


def preferred_training_python() -> str:
    explicit = os.environ.get(QUALITY_MODEL_PYTHON_ENV)
    if explicit and explicit.strip():
        return explicit.strip()
    if WORKSPACE_VENV_PYTHON.exists():
        return str(WORKSPACE_VENV_PYTHON)
    return sys.executable


def python_has_modules(python_cmd: str, modules: Sequence[str]) -> bool:
    probe = (
        "import importlib.util, sys; "
        "missing=[name for name in sys.argv[1:] if importlib.util.find_spec(name) is None]; "
        "sys.exit(0 if not missing else 1)"
    )
    try:
        result = subprocess.run(
            [python_cmd, "-c", probe, *modules],
            capture_output=True,
            text=True,
            check=False,
            env=sanitized_subprocess_env(),
        )
    except OSError:
        return False
    return result.returncode == 0


def install_python_training_requirements(python_cmd: str) -> int:
    if not TRAINING_REQUIREMENTS_FILE.exists():
        print(
            f"  [ERROR] Missing Python requirements file: {TRAINING_REQUIREMENTS_FILE}",
            file=sys.stderr,
        )
        return 1
    print(
        f"  {pick_symbol('🧰', ('[TOOL]'))} Installing missing Python training deps via {python_cmd}..."
    )
    result = subprocess.run(
        [
            python_cmd,
            "-m",
            "pip",
            "install",
            "-r",
            str(TRAINING_REQUIREMENTS_FILE),
        ],
        cwd=ROOT,
        text=True,
        check=False,
        env=sanitized_subprocess_env(),
    )
    return result.returncode


def ensure_python_training_requirements(python_cmd: str, install_missing: bool) -> bool:
    if python_has_modules(python_cmd, QUALITY_MODEL_MODULES):
        return True
    if not install_missing:
        print(
            "  [ERROR] Missing Python deps for image_quality model runtime "
            f"({', '.join(QUALITY_MODEL_MODULES)}). "
            "Re-run with --install-missing-python-deps or install "
            f"`-r {TRAINING_REQUIREMENTS_FILE}` into the workspace venv.",
            file=sys.stderr,
        )
        return False
    if install_python_training_requirements(python_cmd) != 0:
        return False
    if python_has_modules(python_cmd, QUALITY_MODEL_MODULES):
        return True
    print(
        "  [ERROR] Python training deps are still missing after installation attempt.",
        file=sys.stderr,
    )
    return False


def run_training_pipeline_command(
    conn_str: str,
    subcommand: str,
    *extra_args: str,
    python_cmd: str | None = None,
) -> int:
    python = python_cmd or preferred_training_python()
    env = sanitized_subprocess_env(conn_str=conn_str, quality_model_python=python)
    cmd = [
        python,
        str(TRAINING_PIPELINE_SCRIPT),
        "--connstr",
        conn_str,
        subcommand,
        *extra_args,
    ]
    result = run_delegated(
        cmd,
        parent_script="run_training.py",
        cwd=ROOT,
        env=env,
        strip_training_source_map=True,
        check=False,
    )
    return result.returncode


def repair_multi_scenario_schema(
    conn_str: str, *, python_cmd: str | None = None
) -> int:
    print(
        f"  {pick_symbol('🧱', ('[FIX]'))} Repairing strict multi-scenario schema before ingestion..."
    )
    return run_training_pipeline_command(
        conn_str,
        "repair-multi-scenario-schema",
        "--drop-legacy-gif-schema",
        python_cmd=python_cmd,
    )


def finalize_loop_intent_assets(conn_str: str, *, python_cmd: str | None = None) -> int:
    return run_training_pipeline_command(
        conn_str, "finalize-loop-intent", python_cmd=python_cmd
    )


def finalize_runtime_assets(
    conn_str: str,
    *,
    python_cmd: str | None = None,
    install_missing_python_deps: bool = False,
) -> int:
    extra: list[str] = []
    if install_missing_python_deps:
        extra.append("--install-missing-python-deps")
    return run_training_pipeline_command(
        conn_str, "finalize-runtime-assets", *extra, python_cmd=python_cmd
    )


def finalize_image_quality_model(
    conn_str: str,
    *,
    python_cmd: str | None = None,
    install_missing_python_deps: bool = False,
) -> int:
    python = python_cmd or preferred_training_python()
    if not ensure_python_training_requirements(python, install_missing_python_deps):
        return 1
    cmd = [
        python,
        str(TRAINING_PIPELINE_SCRIPT),
        "--connstr",
        conn_str,
        "finalize-image-quality-model",
    ]
    print(
        f"  {pick_symbol('🧠', ('[AI]'))} Finalizing image_quality model artifacts..."
    )
    result = run_delegated(
        cmd,
        parent_script="run_training.py",
        cwd=ROOT,
        env=helper_env(
            "run_training.py",
            conn_str=conn_str,
            quality_model_python=python,
            strip_training_source_map=True,
        ),
        check=False,
    )
    return result.returncode


def report_loop_clustering(conn_str: str, *, python_cmd: str | None = None) -> int:
    print(f"  {pick_symbol('📊', ('[STATS]'))} Reporting loop-clustering state...")
    return run_training_pipeline_command(
        conn_str, "report-loop-clustering", python_cmd=python_cmd
    )


def report_quality_regression(conn_str: str, *, python_cmd: str | None = None) -> int:
    print(f"  {pick_symbol('📊', ('[STATS]'))} Reporting quality-regression state...")
    return run_training_pipeline_command(
        conn_str, "report-quality-regression", python_cmd=python_cmd
    )


def verify_stack_readiness(conn_str: str, *, python_cmd: str | None = None) -> int:
    print(f"  {pick_symbol('🔍', ('[SEARCH]'))} Verifying stack readiness...")
    return run_training_pipeline_command(
        conn_str, "verify-stack-readiness", python_cmd=python_cmd
    )


def combine_finalize_exit_codes(*exit_codes: int) -> int:
    """Prefer hard failures (1) over pending maturity (2) over success (0)."""
    if any(code == 1 for code in exit_codes):
        return 1
    if any(code == 2 for code in exit_codes):
        return 2
    return 0


def fill_runtime_assets(
    conn_str: str,
    *,
    saw_image_quality_samples: bool,
    saw_loop_samples: bool,
    install_missing_python_deps: bool,
    verify_after: bool,
    training_mode: str = "all",
) -> int:
    python = preferred_training_python()
    include_image_quality = training_mode in ("all", "static")
    include_loop_intent = training_mode in ("all", "loop")

    if include_image_quality and not saw_image_quality_samples:
        print(
            "  [INFO] No new image_quality samples were routed in this run; "
            "still checking/filling image_quality LightGBM when mature."
        )
    if include_loop_intent and not saw_loop_samples:
        print(
            "  [INFO] No new loop_intent samples were routed in this run; "
            "still refreshing loop KNN stats / directory scores when rows exist."
        )
    if training_mode == "static":
        print(
            "  [INFO] training_mode=static: runtime fill skips loop_intent "
            "(image_quality LightGBM only)."
        )
    elif training_mode == "loop":
        print(
            "  [INFO] training_mode=loop: runtime fill skips image_quality LightGBM "
            "(loop_intent only)."
        )

    multi_exit = 2
    if include_image_quality or include_loop_intent:
        for pass_idx in range(1, MAX_RUNTIME_FILL_PASSES + 1):
            families = []
            if include_loop_intent:
                families.append("loop_intent")
            if include_image_quality:
                families.append("image_quality")
            print(
                f"  [INFO] Runtime finalize pass {pass_idx}/{MAX_RUNTIME_FILL_PASSES} "
                f"({'+'.join(families)})..."
            )
            pass_exits: list[int] = []
            if include_loop_intent:
                pass_exits.append(finalize_loop_intent_assets(conn_str))
            if include_image_quality:
                pass_exits.append(
                    finalize_image_quality_model(
                        conn_str,
                        install_missing_python_deps=install_missing_python_deps,
                    )
                )
            multi_exit = combine_finalize_exit_codes(*pass_exits)
            if multi_exit == 1:
                return multi_exit
            if multi_exit == 0:
                if pass_idx > 1:
                    print(
                        f"  [INFO] Runtime assets converged after {pass_idx} finalize pass(es)."
                    )
                break
        if multi_exit == 2:
            print(
                f"  [INFO] After {MAX_RUNTIME_FILL_PASSES} finalize pass(es), one or more "
                "runtime families are still pending maturity (expected when the corpus is small)."
            )

    report_loop_exit = 0
    if include_loop_intent:
        report_loop_exit = report_loop_clustering(conn_str, python_cmd=python)
        if report_loop_exit != 0:
            return report_loop_exit

    report_quality_exit = 0
    if include_image_quality:
        report_quality_exit = report_quality_regression(conn_str, python_cmd=python)
        if report_quality_exit != 0:
            return report_quality_exit

    if verify_after:
        return verify_stack_readiness(conn_str, python_cmd=python)
    if multi_exit == 2:
        pending = []
        if include_loop_intent:
            pending.append("loop_intent KNN")
        if include_image_quality:
            pending.append("image_quality LightGBM")
        print(
            "  [PENDING] One or more runtime families are not fully mature yet "
            f"({' and/or '.join(pending)}). "
            "Re-run with --verify-after for the full readiness table.",
            file=sys.stderr,
        )
        return 2
    return 0


def guess_extension_from_content_type(content_type: str | None) -> str:
    if not content_type or content_type == "application/octet-stream":
        return ""
    known_extensions = {
        "image/gif": ".gif",
        "image/jpeg": ".jpg",
        "image/png": ".png",
        "image/webp": ".webp",
        "image/avif": ".avif",
        "image/heic": ".heic",
        "image/heif": ".heif",
        "image/tiff": ".tiff",
        "image/bmp": ".bmp",
        "video/mp4": ".mp4",
        "video/quicktime": ".mov",
        "video/webm": ".webm",
        "video/x-matroska": ".mkv",
        "video/x-msvideo": ".avi",
    }
    return known_extensions.get(
        content_type, mimetypes.guess_extension(content_type) or ""
    )


def download_remote_asset(url: str, dest: Path) -> Path:
    req = urllib.request.Request(url, headers={"User-Agent": USER_AGENT})
    with urllib.request.urlopen(req, timeout=20) as response:
        final_dest = dest
        if not dest.suffix:
            guessed_ext = guess_extension_from_content_type(
                response.headers.get_content_type()
            )
            if guessed_ext:
                final_dest = dest.with_suffix(guessed_ext)
        staging = final_dest.with_name(final_dest.name + ".mfb_part")
        try:
            with open(staging, "wb") as out_file:
                shutil.copyfileobj(response, out_file)
            os.replace(staging, final_dest)
        except BaseException:
            try:
                if staging.exists():
                    staging.unlink()
            except OSError:
                pass
            raise
    return final_dest


def get_scenarios(path: Path, no_loop: bool) -> list[str]:
    ext = detect_media_extension(path)
    scenarios: list[str] = []
    routes_to_animated_image_quality = ext in ANIMATED_IMAGE_EXTS and (
        not no_loop and should_route_to_animated_image_quality(path, ext)
    )
    if ext in IMAGE_EXTS and not routes_to_animated_image_quality:
        scenarios.append("image_quality")
    if not no_loop:
        if routes_to_animated_image_quality:
            scenarios.append("animated_image_quality")
        if ext in VIDEO_EXTS:
            scenarios.append("video_quality")
    return scenarios


def filter_scenarios_for_training_mode(
    scenarios: list[str], training_mode: str
) -> list[str]:
    """
    Align routing with training scope.

    - ``static``: only ``image_quality`` (static stills; static-frame GIF allowed).
      Animated GIF/APNG/WebP/etc. must not enter quality tables — use ``loop`` mode.
    - ``loop``: handled via ``animated_loop`` samples and ``loop_paths`` (not here).
    - ``all``: keep full ``get_scenarios`` result.
    """
    if training_mode != "static":
        return scenarios
    return [s for s in scenarios if s == "image_quality"]


def build_quality_cmd(
    path: Path, label: str, scenario: str, conn_str: str
) -> list[str]:
    cmd = [
        str(TRAIN_BIN_QUALITY),
        str(path),
        "--label",
        label,
        "--scenario",
        scenario,
    ]
    cmd.extend(["--conn", conn_str])
    return cmd


def build_knn_cmd(
    input_path: Path, conn_str: str, loop_api_label: str | None
) -> list[str]:
    """CLI fallback for loop_intent. None = automatic heuristic (no --label)."""
    cmd = [str(TRAIN_BIN_KNN), str(input_path)]
    if loop_api_label is None:
        pass
    elif loop_api_label == "high":
        cmd.extend(["--label", "loop"])
    elif loop_api_label == "low":
        cmd.extend(["--label", "low"])
    elif loop_api_label == "video":
        cmd.extend(["--label", "non-loop"])
    else:
        raise ValueError(f"unsupported loop label for train_knn: {loop_api_label!r}")
    cmd.extend(["--conn", conn_str])
    return cmd


def log_ingest_status(
    status: str, label: str, path: Path, scenario: str | None = None
) -> None:
    display_name = clean_display_name(path.name, path.name)
    if scenario:
        print(f"     [{status}] {scenario}/{label:12s} {display_name}")
        return
    print(f"     [{status}] {label:12s} {display_name}")


def is_label_or_score_conflict(message: str) -> bool:
    """Detect DB/application guardrails for immutable labels or quality scores."""
    if not message:
        return False
    m = message.lower()
    return (
        "label_conflict:" in message
        or "immutable once set" in m
        or "quality_label is immutable" in m
        or "loop_samples.label is immutable" in m
        or "quality_score is immutable once set" in m
    )


def ingest_quality_via_cli(
    paths: list[Path], label: str, scenario: str, conn_str: str
) -> tuple[int, int, int]:
    """Returns (success_count, fail_other, fail_label_conflict)."""
    success_count = 0
    fail_other = 0
    fail_lc = 0
    for path in paths:
        result = run_rust_ingest(
            build_quality_cmd(path, label, scenario, conn_str),
            conn_str=conn_str,
            capture_output=True,
            text=True,
        )
        if result.returncode == 0:
            log_ingest_status("OK", label, path, scenario)
            success_count += 1
            continue
        stderr = result.stderr.strip() or result.stdout.strip() or "unknown error"
        if is_label_or_score_conflict(stderr):
            log_ingest_status("FAIL:label_conflict", label, path, scenario)
            print(
                f"     [FAIL:label_conflict] {scenario}/{label} {path.name}: {stderr}"
            )
            fail_lc += 1
            abort_training_sample_failure_if_fail_fast(
                f"{scenario}/{label} {path.name}", stderr
            )
        else:
            log_ingest_status("FAIL", label, path, scenario)
            print(f"     [FAIL] {scenario}/{label} {path.name}: {stderr}")
            fail_other += 1
            abort_training_sample_failure_if_fail_fast(
                f"{scenario}/{label} {path.name}", stderr
            )
    return success_count, fail_other, fail_lc


def _c_api_batch_fatal_message(return_code: int) -> str:
    detail = get_last_ingest_error().strip()
    code_names = {
        -2: "invalid scenario",
        -3: "database connect failed",
        -4: "schema init or loop feature-map bootstrap failed",
        -5: "invalid label or path encoding",
    }
    label = code_names.get(return_code, f"batch error code {return_code}")
    if detail:
        return f"{label}: {detail}"
    return label


def _abort_on_c_api_batch_fatal(
    return_code: int,
    *,
    context: str,
    path_count: int,
) -> tuple[int, int, int] | None:
    """Fail-closed when Rust batch ingest aborts before any path (negative return code)."""
    if return_code >= 0:
        return None
    msg = _c_api_batch_fatal_message(return_code)
    print(f"     [FAIL] {context}: {msg}", file=sys.stderr)
    if fail_closed_training_enabled():
        training_quality_exit(1, f"     [FAIL] {context}: {msg}")
    return 0, path_count, 0


def ingest_quality_group(
    paths: list[Path],
    label: str,
    scenario: str,
    conn_str: str,
    use_api: bool,
    *,
    verbose: bool,
) -> tuple[int, int, int]:
    """Returns (success_count, fail_other, fail_label_conflict)."""
    if use_api and HAS_C_API:
        print(
            f"     [INGEST-START] {scenario}/{label} n={len(paths)} "
            "engine=rust_c_api analyze_image+ffprobe "
            f"(stderr [INGEST-RUST] when {TRAINING_INGEST_PROGRESS_ENV} enabled)",
            flush=True,
        )
        try:
            successes = ingest_media_samples_batch(
                conn_str,
                [str(path) for path in paths],
                label,
                scenario,
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
            retry_error = f"{type(exc).__name__}: {exc}"

            def _mark_batch_retry() -> None:
                nonlocal successes
                print(
                    f"     [WARN] C-API batch failed for {scenario}/{label}: {retry_error}; "
                    "retrying per file"
                )
                successes = -1

            run_training_except_policy(
                f"C-API batch ingest {scenario}/{label}",
                exc,
                on_retry=_mark_batch_retry,
            )
        aborted = _abort_on_c_api_batch_fatal(
            successes,
            context=f"{scenario}/{label} C-API batch",
            path_count=len(paths),
        )
        if aborted is not None:
            return aborted
        if successes == len(paths):
            if verbose:
                for path in paths:
                    log_ingest_status("OK", label, path, scenario)
            else:
                print(f"     [OK] C-API batch {scenario}/{label} n={successes}")
            return successes, 0, 0

        if not paths:
            return 0, 0, 0

        if 0 < successes < len(paths):
            print(
                f"     [INFO] C-API batch ingested {successes}/{len(paths)} "
                f"for {scenario}/{label}; classifying per file"
            )
        elif successes == 0:
            print(
                f"     [WARN] C-API batch ingested 0/{len(paths)} "
                f"for {scenario}/{label}; retrying per file"
            )
        else:
            print(
                f"     [INFO] C-API batch unavailable for {scenario}/{label}; "
                "classifying per file"
            )

        success_count = 0
        fail_other = 0
        fail_lc = 0
        for path in paths:
            try:
                result = ingest_media_samples_batch(
                    conn_str, [str(path)], label, scenario
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
                msg = str(exc)
                if is_label_or_score_conflict(msg):
                    log_ingest_status("FAIL:label_conflict", label, path, scenario)
                    print(
                        f"     [FAIL:label_conflict] {scenario}/{label} "
                        f"{path.name}: {msg}"
                    )
                    fail_lc += 1
                    abort_training_sample_failure_if_fail_fast(
                        f"{scenario}/{label} {path.name}", msg
                    )
                else:
                    log_ingest_status("FAIL", label, path, scenario)
                    print(f"     [FAIL] {scenario}/{label} {path.name}: {msg}")
                    fail_other += 1
                    abort_training_sample_failure_if_fail_fast(
                        f"{scenario}/{label} {path.name}", msg
                    )
                continue
            if result == 1:
                if verbose:
                    log_ingest_status("OK", label, path, scenario)
                success_count += 1
                continue
            msg = get_last_ingest_error().strip()
            if not msg:
                msg = f"C-API return code {result}"
            if is_label_or_score_conflict(msg):
                log_ingest_status("FAIL:label_conflict", label, path, scenario)
                print(
                    f"     [FAIL:label_conflict] {scenario}/{label} {path.name}: {msg}"
                )
                fail_lc += 1
                abort_training_sample_failure_if_fail_fast(
                    f"{scenario}/{label} {path.name}", msg
                )
            else:
                log_ingest_status("FAIL", label, path, scenario)
                print(f"     [FAIL] {scenario}/{label} {path.name}: {msg}")
                fail_other += 1
                abort_training_sample_failure_if_fail_fast(
                    f"{scenario}/{label} {path.name}", msg
                )

        if success_count > 0:
            if not verbose:
                print(
                    f"     [OK] C-API per-file {scenario}/{label} "
                    f"n_ok={success_count} fail={fail_other} label_conflict={fail_lc}"
                )
            return success_count, fail_other, fail_lc

        if not TRAIN_BIN_QUALITY.exists():
            print(
                f"     [FAIL] CLI fallback unavailable: missing {TRAIN_BIN_QUALITY}",
                file=sys.stderr,
            )
            return success_count, fail_other, fail_lc

        print(
            f"     [WARN] C-API per-file ingestion had 0 successes for "
            f"{scenario}/{label}; falling back to per-file CLI"
        )
        return ingest_quality_via_cli(paths, label, scenario, conn_str)

    return ingest_quality_via_cli(paths, label, scenario, conn_str)


def ingest_loop_via_api(
    loop_paths: list[Path],
    conn_str: str,
    *,
    loop_api_label: str | None = None,
    verbose: bool,
) -> tuple[int, int, int]:
    """Returns (success_count, fail_other, fail_label_conflict)."""
    if not conn_str.strip():
        print(
            "     [FAIL] LoopIntent C-API requires a PostgreSQL connection string",
            file=sys.stderr,
        )
        return 0, len(loop_paths), 0

    try:
        successes = ingest_media_samples_batch(
            conn_str,
            [str(path) for path in loop_paths],
            loop_api_label,
            "loop_intent",
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
        retry_error = f"{type(exc).__name__}: {exc}"

        def _mark_loop_batch_retry() -> None:
            nonlocal successes
            print(
                f"     [WARN] LoopIntent C-API batch failed: {retry_error}; "
                "retrying per sample"
            )
            successes = -1

        run_training_except_policy(
            "LoopIntent C-API batch ingest",
            exc,
            on_retry=_mark_loop_batch_retry,
        )

    aborted = _abort_on_c_api_batch_fatal(
        successes,
        context="loop_intent C-API batch",
        path_count=len(loop_paths),
    )
    if aborted is not None:
        return aborted

    if successes == len(loop_paths):
        if verbose:
            for path in loop_paths:
                log_ingest_status("OK", "animated_loop", path)
        else:
            intent = loop_api_label or "auto"
            print(f"     [OK] C-API batch loop_intent n={successes} intent={intent}")
        return successes, 0, 0

    if successes >= 0:
        print(
            f"     [WARN] LoopIntent C-API partially ingested {successes}/{len(loop_paths)}; "
            "retrying per sample"
        )

    success_count = 0
    fail_other = 0
    fail_lc = 0
    for path in loop_paths:
        try:
            result = ingest_media_samples_batch(
                conn_str,
                [str(path)],
                loop_api_label,
                "loop_intent",
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
            msg = str(exc)
            if is_label_or_score_conflict(msg):
                log_ingest_status("FAIL:label_conflict", "animated_loop", path)
                print(f"     [FAIL:label_conflict] animated_loop {path.name}: {msg}")
                fail_lc += 1
                abort_training_sample_failure_if_fail_fast(
                    f"animated_loop {path.name}", msg
                )
            else:
                log_ingest_status("FAIL", "animated_loop", path)
                print(f"     [FAIL] animated_loop {path.name}: {msg}")
                fail_other += 1
                abort_training_sample_failure_if_fail_fast(
                    f"animated_loop {path.name}", msg
                )
            continue
        if result == 1:
            log_ingest_status("OK", "animated_loop", path)
            success_count += 1
            continue
        msg = get_last_ingest_error().strip()
        if not msg:
            msg = f"C-API return code {result}"
        if is_label_or_score_conflict(msg):
            log_ingest_status("FAIL:label_conflict", "animated_loop", path)
            print(f"     [FAIL:label_conflict] animated_loop {path.name}: {msg}")
            fail_lc += 1
            abort_training_sample_failure_if_fail_fast(
                f"animated_loop {path.name}", msg
            )
        else:
            log_ingest_status("FAIL", "animated_loop", path)
            print(f"     [FAIL] animated_loop {path.name}: {msg}")
            fail_other += 1
            abort_training_sample_failure_if_fail_fast(
                f"animated_loop {path.name}", msg
            )

    return success_count, fail_other, fail_lc


def ingest_loop_group(
    loop_paths: list[Path],
    conn_str: str,
    use_api: bool,
    *,
    loop_api_label: str | None = None,
    verbose: bool,
) -> tuple[int, int, int]:
    """Returns (success_count, fail_other, fail_label_conflict)."""
    if not loop_paths:
        return 0, 0, 0

    if use_api and HAS_C_API:
        return ingest_loop_via_api(
            loop_paths, conn_str, loop_api_label=loop_api_label, verbose=verbose
        )

    loop_root = loop_paths[0].parents[1]
    batch_result = run_rust_ingest(
        build_knn_cmd(loop_root, conn_str, loop_api_label),
        conn_str=conn_str,
        capture_output=True,
        text=True,
    )
    if batch_result.returncode == 0:
        if verbose:
            for path in loop_paths:
                log_ingest_status("OK", "animated_loop", path)
        else:
            print(f"     [OK] train_knn batch loop_intent n={len(loop_paths)}")
        return len(loop_paths), 0, 0

    stderr = (
        batch_result.stderr.strip() or batch_result.stdout.strip() or "unknown error"
    )
    print(
        f"     [WARN] train_knn batch failed for {loop_root.name}: {stderr}; "
        "retrying per sample"
    )

    success_count = 0
    fail_other = 0
    fail_lc = 0
    for path in loop_paths:
        result = run_rust_ingest(
            build_knn_cmd(path.parent, conn_str, loop_api_label),
            conn_str=conn_str,
            capture_output=True,
            text=True,
        )
        if result.returncode == 0:
            log_ingest_status("OK", "animated_loop", path)
            success_count += 1
            continue
        item_stderr = result.stderr.strip() or result.stdout.strip() or "unknown error"
        if is_label_or_score_conflict(item_stderr):
            log_ingest_status("FAIL:label_conflict", "animated_loop", path)
            print(
                f"     [FAIL:label_conflict] animated_loop {path.name}: {item_stderr}"
            )
            fail_lc += 1
            abort_training_sample_failure_if_fail_fast(
                f"animated_loop {path.name}", item_stderr
            )
        else:
            log_ingest_status("FAIL", "animated_loop", path)
            print(f"     [FAIL] animated_loop {path.name}: {item_stderr}")
            fail_other += 1
            abort_training_sample_failure_if_fail_fast(
                f"animated_loop {path.name}", item_stderr
            )
    return success_count, fail_other, fail_lc


def run_training_isolated(
    all_samples: list[Sample],
    no_loop: bool,
    use_api: bool,
    finalize_image_quality: bool,
    finalize_loop_intent: bool,
    fill_runtime_assets_after_ingest: bool,
    verify_after: bool,
    install_missing_python_deps: bool,
    *,
    training_mode: str,
    training_scope_summary: str,
    loop_intent_api_label: str | None,
    verbose: bool,
) -> int:
    print(
        f"\n{pick_symbol('🚀', ('[LAUNCH]'))} ZERO-POLLUTION ENGINE: Version 6.4 (Samples: {len(all_samples)})"
    )
    print(f"  {training_scope_summary}")
    # Planning already ran; drop label-ownership table so this phase cannot leak memory or
    # interact with any future in-process callers. Ingest uses only ``all_samples``.
    clear_ephemeral_training_state()
    if not verbose:
        print(
            "  [INFO] Progress: [STATIC-TIER] tier rescan (pre-ingest, "
            f"audit→{default_tier_audit_path().name}), "
            "[PHASE] 1/2 replica+jsonl audit, [PHASE] 2/2 ingest, "
            "[INGEST-RUST] on stderr during C-API; "
            f"per-file lines: --verbose or {TRAINING_VERBOSE_ENV}=1"
        )
    total_success = 0
    total_fail_other = 0
    total_fail_label_conflict = 0
    total_skip = 0
    saw_image_quality_samples = False
    saw_loop_samples = False
    conn_str = (
        os.environ.get("MFB_PG_CONNSTR") or DEFAULT_CONNSTR
    ).strip() or DEFAULT_CONNSTR

    has_quality_samples = any(
        sample.base_label != "animated_loop" for sample in all_samples
    )
    has_loop_samples = any(
        sample.base_label == "animated_loop" for sample in all_samples
    )

    if use_api and not HAS_C_API:
        print("  [WARN] Python C-API bridge unavailable; falling back to CLI.")

    needs_quality_cli = has_quality_samples and not (use_api and HAS_C_API)
    needs_loop_cli = has_loop_samples and not (use_api and HAS_C_API)
    ensure_runtime_artifacts(has_quality_samples, has_loop_samples, use_api)

    if needs_quality_cli and not TRAIN_BIN_QUALITY.exists():
        print(
            f"[ERROR] train_quality binary not found: {TRAIN_BIN_QUALITY}\n"
            "Build it first with `cargo build -p foundation --bin train_quality` "
            "or rerun with --use-api after building the foundation dynamic library.",
            file=sys.stderr,
        )
        sys.exit(1)

    if needs_loop_cli and not TRAIN_BIN_KNN.exists():
        print(
            f"[ERROR] train_knn binary not found: {TRAIN_BIN_KNN}\n"
            "Build it first with `cargo build -p foundation --bin train_knn`.",
            file=sys.stderr,
        )
        sys.exit(1)

    total_batches = ceil(len(all_samples) / BATCH_SIZE)
    for i in range(0, len(all_samples), BATCH_SIZE):
        free_gb = check_disk_space()
        batch = all_samples[i : i + BATCH_SIZE]
        print(
            f"  {pick_symbol('📦', ('[PKG]'))} Batch {i // BATCH_SIZE + 1}/{total_batches} (Free: {free_gb}GB)..."
        )

        with tempfile.TemporaryDirectory(prefix="mfb_training_replica_") as tmp_dir:
            tmp_path = Path(tmp_dir)
            quality_tasks: dict[tuple[str, str], list[Path]] = {}
            loop_paths: list[Path] = []
            replica_source_map: dict[str, str] = {}
            skip_no_scenario: Counter[str] = Counter()
            skip_static_mode_animated: Counter[str] = Counter()
            skip_remote_quality_filter: Counter[str] = Counter()

            # Phase 1: MANDATORY PHYSICAL REPLICATION (shutil.copy2)
            replica_audit_path = default_replica_audit_path()
            replica_audit_path.parent.mkdir(parents=True, exist_ok=True)
            phase1_started = time.monotonic()
            replica_ok = 0
            replica_fail = 0
            replica_bytes = 0
            print(
                f"     [PHASE] 1/2 replica+routing batch_size={len(batch)} "
                "(copy/download → scenario routing) "
                f"audit={replica_audit_path}",
                flush=True,
            )
            with open(replica_audit_path, "a", encoding="utf-8") as replica_audit:
                append_replica_audit(
                    {
                        "event": "phase1_start",
                        "batch_index": i // BATCH_SIZE + 1,
                        "batch_size": len(batch),
                    },
                    replica_audit,
                )
                for j, s in enumerate(batch):
                    if j == 0 or (j + 1) % REPLICA_PROGRESS_INTERVAL == 0:
                        elapsed = time.monotonic() - phase1_started
                        print(
                            f"     [REPLICA] {j + 1}/{len(batch)} "
                            f"label={s.base_label} remote={s.is_remote} "
                            f"ok={replica_ok} fail={replica_fail} "
                            f"bytes={replica_bytes} elapsed={format_elapsed_secs(elapsed)}",
                            flush=True,
                        )
                    display_name = clean_display_name(s.path_or_url, f"s_{j:05d}")
                    dest_dir = tmp_path / s.base_label / f"sample_{j:05d}"
                    dest_dir.mkdir(parents=True, exist_ok=True)
                    dest = dest_dir / display_name

                    try:
                        copy_started = time.monotonic()
                        if s.is_remote:
                            final_dest = download_remote_asset(s.path_or_url, dest)
                        else:
                            shutil.copy2(s.path_or_url, dest)
                            final_dest = dest
                            replica_source_map[str(final_dest.resolve())] = (
                                resolve_local_source_path(s.path_or_url)
                            )
                        size_bytes = final_dest.stat().st_size
                        replica_ok += 1
                        replica_bytes += size_bytes
                        if verbose or (j + 1) % REPLICA_PROGRESS_INTERVAL == 0:
                            append_replica_audit(
                                {
                                    "event": "replica_ok",
                                    "index": j + 1,
                                    "label": s.base_label,
                                    "remote": s.is_remote,
                                    "src": s.path_or_url,
                                    "dest": str(final_dest),
                                    "bytes": size_bytes,
                                    "elapsed_s": round(
                                        time.monotonic() - copy_started, 3
                                    ),
                                },
                                replica_audit,
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
                        replica_fail += 1
                        append_replica_audit(
                            {
                                "event": "replica_fail",
                                "index": j + 1,
                                "label": s.base_label,
                                "remote": s.is_remote,
                                "src": s.path_or_url,
                                "dest": str(dest),
                                "error": str(e),
                            },
                            replica_audit,
                        )
                        print(
                            f"     [FAIL] Replica creation failed for {display_name}: {e}"
                        )
                        total_fail_other += 1
                        continue

                    if s.is_remote and not passes_file_quality_filter(
                        final_dest,
                        s.source,
                        is_remote_source=True,
                        original_ref=s.path_or_url,
                    ):
                        total_skip += 1
                        for rule_name in failed_file_quality_rules(
                            final_dest,
                            s.source,
                            is_remote_source=True,
                            original_ref=s.path_or_url,
                        ):
                            skip_remote_quality_filter[rule_name] += 1
                        continue

                    if s.base_label == "animated_loop":
                        saw_loop_samples = True
                        loop_paths.append(final_dest)
                        continue

                    raw_scenarios = get_scenarios(final_dest, no_loop)
                    scenarios = filter_scenarios_for_training_mode(
                        raw_scenarios, training_mode
                    )
                    if not scenarios:
                        detected_ext = detect_media_extension(final_dest) or "unknown"
                        if training_mode == "static" and raw_scenarios:
                            if verbose:
                                print(
                                    f"     [SKIP] Animated/dynamic asset excluded from "
                                    f"static training: {final_dest.name} "
                                    f"(would_route={','.join(raw_scenarios)}; "
                                    "use --training-mode loop for loop_intent)"
                                )
                            skip_static_mode_animated[detected_ext] += 1
                        else:
                            if verbose:
                                print(
                                    f"     [SKIP] No scenario match for {final_dest.name} "
                                    f"(detected ext: {detected_ext})"
                                )
                            skip_no_scenario[detected_ext] += 1
                        total_skip += 1
                        continue

                    for scenario in scenarios:
                        if scenario == "image_quality":
                            saw_image_quality_samples = True
                        quality_tasks.setdefault((s.base_label, scenario), []).append(
                            final_dest
                        )

                append_replica_audit(
                    {
                        "event": "phase1_done",
                        "ok": replica_ok,
                        "fail": replica_fail,
                        "bytes": replica_bytes,
                        "quality_groups": len(quality_tasks),
                        "loop_paths": len(loop_paths),
                        "elapsed_s": round(time.monotonic() - phase1_started, 3),
                    },
                    replica_audit,
                )
            print(
                f"     [PHASE] 1/2 done replicas_ok={replica_ok} fail={replica_fail} "
                f"bytes={replica_bytes} audit={replica_audit_path} "
                f"elapsed={format_elapsed_secs(time.monotonic() - phase1_started)}",
                flush=True,
            )

            if skip_static_mode_animated and not verbose:
                total_sa = sum(skip_static_mode_animated.values())
                top_sa = format_counter_top(skip_static_mode_animated)
                print(
                    f"     [SKIP] static_mode_animated_excluded: {total_sa} sample(s); "
                    f"by_ext: {top_sa} (use --training-mode loop, not quality tables)"
                )
            if skip_no_scenario and not verbose:
                total_ns = sum(skip_no_scenario.values())
                top = format_counter_top(skip_no_scenario)
                print(
                    f"     [SKIP] no_scenario_match: {total_ns} sample(s); by_ext: {top}"
                )
            if skip_remote_quality_filter and not verbose:
                total_rf = sum(skip_remote_quality_filter.values())
                top_rf = format_counter_top(skip_remote_quality_filter)
                print(
                    f"     [SKIP] remote_file_quality_filter: {total_rf} hit(s); by_rule: {top_rf}"
                )

            map_file = tmp_path / "source_map.json"
            previous_source_map = os.environ.get(TRAINING_SOURCE_MAP_ENV)
            source_map_env_owned = False
            if replica_source_map:
                payload = json.dumps(replica_source_map, ensure_ascii=False, indent=2)
                staging = tmp_path / "source_map.json.partial"
                try:
                    staging.write_text(payload, encoding="utf-8")
                    os.replace(staging, map_file)
                finally:
                    try:
                        if staging.exists():
                            staging.unlink()
                    except OSError:
                        pass
                os.environ[TRAINING_SOURCE_MAP_ENV] = str(map_file)
                source_map_env_owned = True

            try:
                # Phase 2: Ingestion
                print(
                    "     [PHASE] 2/2 feature extraction + DB ingest "
                    f"(groups={len(quality_tasks)} quality, loop_paths={len(loop_paths)})",
                    flush=True,
                )
                for (label, scenario), paths in sorted(quality_tasks.items()):
                    success_count, fail_o, fail_lc = ingest_quality_group(
                        paths,
                        label,
                        scenario,
                        conn_str,
                        use_api,
                        verbose=verbose,
                    )
                    total_success += success_count
                    total_fail_other += fail_o
                    total_fail_label_conflict += fail_lc

                loop_success, loop_fail_o, loop_fail_lc = ingest_loop_group(
                    loop_paths,
                    conn_str,
                    use_api,
                    loop_api_label=loop_intent_api_label,
                    verbose=verbose,
                )
                total_success += loop_success
                total_fail_other += loop_fail_o
                total_fail_label_conflict += loop_fail_lc
            finally:
                if source_map_env_owned:
                    if previous_source_map is None:
                        os.environ.pop(TRAINING_SOURCE_MAP_ENV, None)
                    else:
                        os.environ[TRAINING_SOURCE_MAP_ENV] = previous_source_map

            sys.stdout.flush()

    total_fail = total_fail_other + total_fail_label_conflict
    print(
        f"\n{pick_symbol('🏁', ('[FINISH]'))} Finished: {total_success} OK, {total_fail} FAIL "
        f"({total_fail_label_conflict} label/score conflict), {total_skip} SKIP. "
        f"(Samples: {len(all_samples)})"
    )
    if total_fail > 0:
        failure_exit = training_ingest_failure_exit_code(
            total_success=total_success,
            total_fail_other=total_fail_other,
            total_fail_label_conflict=total_fail_label_conflict,
        )
        if failure_exit != 0:
            print(
                f"  [FAIL] Training ingest had {total_fail} failed sample(s) "
                f"({total_fail_label_conflict} label/score conflict, "
                f"{total_fail_other} other); "
                "refusing to report clean pipeline success.",
                file=sys.stderr,
            )
            return failure_exit
        print(
            f"  [WARN] Training ingest had {total_fail} failed sample(s) "
            f"({total_fail_label_conflict} label/score conflict, "
            f"{total_fail_other} other); continuing with failure report because "
            f"{TRAINING_ERROR_MODE_ENV}={TRAINING_ERROR_MODE_LOG_AND_CONTINUE}.",
            file=sys.stderr,
        )
    if total_success == 0:
        print(
            "  [FAIL] Training ingest produced zero successful samples; "
            f"skipped={total_skip}. Check routing rules and sample availability.",
            file=sys.stderr,
        )
        return 2

    audit_path = write_tier_audit_jsonl()
    if audit_path:
        consistent = sum(1 for r in TIER_AUDIT_RECORDS if r.get("tier_consistent"))
        print(
            f"  [AUDIT] tier rules trace: {audit_path} "
            f"({len(TIER_AUDIT_RECORDS)} collect rows, {consistent} tier_consistent)"
        )
        print(
            "  [AUDIT] DB metadata.training_tier_audit populated on image_quality ingest "
            "(verify: SELECT metadata->'training_tier_audit' FROM image_quality_samples LIMIT 1)"
        )

    if fill_runtime_assets_after_ingest:
        return fill_runtime_assets(
            conn_str,
            saw_image_quality_samples=saw_image_quality_samples,
            saw_loop_samples=saw_loop_samples,
            install_missing_python_deps=install_missing_python_deps,
            verify_after=verify_after,
            training_mode=training_mode,
        )
    if finalize_loop_intent and finalize_image_quality:
        return finalize_runtime_assets(
            conn_str,
            install_missing_python_deps=install_missing_python_deps,
        )
    if finalize_loop_intent:
        return finalize_loop_intent_assets(conn_str)
    if finalize_image_quality:
        if not saw_image_quality_samples:
            print(
                "  [INFO] No new image_quality samples were routed in this run; "
                "checking existing corpus state before finalize."
            )
        return finalize_image_quality_model(
            conn_str,
            install_missing_python_deps=install_missing_python_deps,
        )
    return 0


def _finalize_training_session_logs(scope_line: str | None) -> None:
    """Archive lane logs into ``TrainingBundle_{stamp}/`` (move, not merge)."""
    pid_file = TRAINING_LOG_DIR / "run_training.pid"
    if pid_file.is_file():
        try:
            pid = int(pid_file.read_text(encoding="utf-8").strip())
        except ValueError:
            pid = 0
        if pid == os.getpid():
            pid_file.unlink(missing_ok=True)
    if (os.environ.get("MFB_TRAINING_ARCHIVE_LOGS") or "1").strip().lower() in {
        "0",
        "false",
        "no",
    }:
        return
    close_tier_audit_stream()
    stamp = ensure_training_session_stamp()
    bundle = archive_training_session_bundle(TRAINING_LOG_DIR, stamp, scope=scope_line)
    if bundle is not None:
        print(f"  [ARCHIVE] training session → {bundle}", flush=True)
    elif _TRAINING_SESSION is not None:
        exit_snap = _TRAINING_SESSION.read_exit_snapshot()
        if exit_snap:
            print(
                "  [ARCHIVE] no bundle (no run log moved); "
                f"see {_TRAINING_SESSION.exit_path} reason={exit_snap.get('reason')}",
                flush=True,
            )


def main() -> None:
    guard_main("run_training.py")
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--dry-run",
        action="store_true",
        help=(
            "Plan only: scan, Rust tier probe, training_tier_audit.jsonl — no DB ingest. "
            "Default (no flag) runs full ingest + metadata.training_tier_audit."
        ),
    )
    add_four_lane_launcher_args(parser)
    parser.add_argument(
        "--balance",
        action="store_true",
        help=(
            "Balance high/low (and loop/non-loop when present) by complexity quantiles; "
            "also enabled when any --max-* cap is set."
        ),
    )
    parser.add_argument(
        "--max-high",
        type=int,
        default=0,
        metavar="N",
        help="Cap high-quality static samples after balance (0=unlimited).",
    )
    parser.add_argument(
        "--max-low",
        type=int,
        default=0,
        metavar="N",
        help="Cap low-quality static samples after balance (0=unlimited).",
    )
    parser.add_argument(
        "--max-loop",
        type=int,
        default=0,
        metavar="N",
        help="Cap loop-intent (strong loop) samples after balance (0=unlimited).",
    )
    parser.add_argument(
        "--max-non-loop",
        type=int,
        default=0,
        metavar="N",
        help="Cap non-loop (video-like) loop_intent samples after balance (0=unlimited).",
    )
    parser.add_argument(
        "--no-balance-complexity",
        action="store_true",
        help="Only apply bilateral caps; skip complexity quantile matching.",
    )
    parser.add_argument(
        "--balance-include-loop-uncertain",
        action="store_true",
        help="Keep loop samples with uncertain intent after balance (default: drop).",
    )
    parser.add_argument(
        "--execute",
        action="store_true",
        help=argparse.SUPPRESS,
    )
    parser.add_argument(
        "--training-mode",
        choices=["all", "static", "loop"],
        default=None,
        help=(
            "Scope: all=still images + loop (default); static=image_quality only "
            "(static stills / static-frame GIF; animated GIF/APNG/WebP are skipped — "
            "use loop mode for loop_intent, not animated_image_quality); "
            "loop=loop_intent only (use --loop-intent-label)."
        ),
    )
    parser.add_argument("--label", choices=["high", "low", "animated_loop"])
    parser.add_argument(
        "--loop-intent-label",
        choices=["auto", "high", "low", "video"],
        default="auto",
        help=(
            "Loop samples only: auto=per-file heuristic, high=strong loop, low=weak loop, "
            "video=non-loop dynamic. Used when --training-mode is all or loop."
        ),
    )
    parser.add_argument("--no-loop", action="store_true")
    parser.add_argument("--use-api", action="store_true")
    parser.add_argument(
        "--finalize-image-quality-model",
        action="store_true",
        help="After batch ingestion, check readiness and train the real image_quality LightGBM model",
    )
    parser.add_argument(
        "--finalize-loop-intent",
        action="store_true",
        help=(
            "After batch ingestion, refresh loop_intent KNN feature stats and backfill "
            "directory_loop_intent_score + HDBSCAN catalog (not LightGBM)"
        ),
    )
    parser.add_argument(
        "--finalize-all",
        action="store_true",
        help=(
            "Redundant alias: default ingest already runs full runtime fill "
            "(loop_intent + image_quality)"
        ),
    )
    parser.add_argument(
        "--allow-remote",
        action="store_true",
        help="Opt in to remote API samples from training_rules.json",
    )
    runtime_fill = parser.add_mutually_exclusive_group()
    runtime_fill.add_argument(
        "--fill-runtime-assets",
        dest="fill_runtime_assets",
        action="store_true",
        help=(
            "After successful batch ingestion, run runtime fill (DEFAULT). Repeats finalize "
            f"up to {MAX_RUNTIME_FILL_PASSES} times while loop/image readiness is still "
            "pending, then clustering + quality-regression reports."
        ),
    )
    runtime_fill.add_argument(
        "--no-fill-runtime-assets",
        dest="fill_runtime_assets",
        action="store_false",
        help="Skip post-ingest runtime fill (ingest only); incompatible with --verify-after",
    )
    parser.set_defaults(fill_runtime_assets=None)
    parser.add_argument(
        "--verify-after",
        action="store_true",
        help="After ingest and runtime-asset filling, run strict verify-stack-readiness",
    )
    parser.add_argument(
        "--repair-schema",
        action="store_true",
        help="Before executing ingestion, drop legacy gif_quality blockers and apply the strict multi-scenario schema migration",
    )
    parser.add_argument(
        "--reset-db",
        action="store_true",
        help=(
            "TRUNCATE all training tables before ingestion to avoid cross-run accumulation. "
            "Clears: image_quality_samples, animated_image_quality_samples, "
            "video_quality_samples, loop_samples, inference_log, "
            "loop_intent_inference_log, image_quality_inference_log, "
            "animated_image_quality_inference_log, video_quality_inference_log, "
            "multi_scenario_metadata, path_tree_snapshots, "
            "media_entries, decision_snapshots, live_audit."
        ),
    )
    parser.add_argument(
        "--install-missing-python-deps",
        action="store_true",
        help="Install missing image_quality Python training/runtime deps into the workspace venv when needed",
    )
    parser.add_argument(
        "--verbose",
        action="store_true",
        help=(
            "Log each ingested/skipped file (default: batch summaries only). "
            f"Or set {TRAINING_VERBOSE_ENV}=1."
        ),
    )
    parser.add_argument(
        "--background",
        action="store_true",
        help=(
            "Detach: re-exec this script in a new session (same argv except "
            "--background). Logs under the unified log directory (see docs/LOGGING_LAYOUT.md). "
            "No wrapper scripts."
        ),
    )
    args = parser.parse_args()
    if args.four_lane:
        run_four_lane_launcher(args)
        return
    if args.stop or args.log_root is not None or args.lane or args.rebuild_dylib:
        parser.error(
            "--stop, --log-root, --lane, and --rebuild-dylib require --four-lane"
        )
    pin_training_log_dir()
    if args.training_mode is None:
        args.training_mode = "all"
    lane = training_lane_slug(
        training_mode=args.training_mode,
        label=args.label,
        loop_intent_label=args.loop_intent_label,
    )
    os.environ.setdefault("MFB_TRAINING_LANE", lane)
    scope_line: str | None = None
    global _TRAINING_SESSION
    stamp = ensure_training_session_stamp()
    try:
        hb_secs = parse_positive_float_env("MFB_TRAINING_HEARTBEAT_SECS", 60.0)
    except ValueError as exc:
        parser.error(str(exc))
    _TRAINING_SESSION = TrainingSessionRecorder(
        TRAINING_LOG_DIR,
        session_stamp=stamp,
        heartbeat_secs=hb_secs,
    )
    _TRAINING_SESSION.install_handlers()
    _TRAINING_SESSION.emit(
        "session_start",
        lane=lane,
        training_mode=args.training_mode,
        label=args.label,
        loop_intent_label=args.loop_intent_label,
        dry_run=bool(args.dry_run),
        ingest_planned=not args.dry_run or bool(args.execute),
        argv=summarize_argv(),
        log_dir=str(TRAINING_LOG_DIR),
        pg_connstr_set=bool((os.environ.get("MFB_PG_CONNSTR") or "").strip()),
    )
    stop_other_training_processes()
    apply_training_scan_defaults()
    sister_training = warn_concurrent_training_processes()
    reset_training_scan_governor(sister_load=sister_training)
    if args.background:
        detach_to_background(
            repo_root=ROOT,
            log_dir=TRAINING_LOG_DIR,
            pid_file=BACKGROUND_PID_FILE,
        )
    if args.dry_run and args.execute:
        parser.error("--dry-run and --execute cannot be combined")
    args.ingest = not args.dry_run or args.execute
    for cap_name in ("max_high", "max_low", "max_loop", "max_non_loop"):
        cap_val = int(getattr(args, cap_name, 0) or 0)
        if cap_val < 0:
            parser.error(f"--{cap_name.replace('_', '-')} must be >= 0")
    if args.no_loop and args.label == "animated_loop":
        parser.error("--label animated_loop cannot be combined with --no-loop")
    if args.finalize_all:
        args.fill_runtime_assets = True
    if args.finalize_all and args.finalize_image_quality_model:
        parser.error("--finalize-all already includes image_quality finalize")
    if args.finalize_all and args.finalize_loop_intent:
        parser.error("--finalize-all already includes loop_intent finalize")

    try:
        rules = load_rules()
        apply_ingest_profile(args, rules.ingest)
        enforce_training_db_caps(args)
    except (OSError, ValueError) as exc:
        parser.error(str(exc))
    if args.fill_runtime_assets is None:
        args.fill_runtime_assets = True
    if args.no_loop and args.training_mode == "loop":
        parser.error("--training-mode loop cannot be combined with --no-loop")
    if args.verify_after and not args.fill_runtime_assets:
        parser.error(
            "--verify-after requires runtime fill; omit --no-fill-runtime-assets"
        )
    if args.fill_runtime_assets and (
        args.finalize_image_quality_model or args.finalize_loop_intent
    ):
        parser.error(
            "Default runtime fill already finalizes loop_intent and image_quality; "
            "do not combine with --finalize-image-quality-model or --finalize-loop-intent. "
            "For partial finalize only, pass --no-fill-runtime-assets with one of those flags."
        )
    # Rust collect/ingest always uses committed exclude; env is documentation-only.
    os.environ["MFB_TIER_AMBIGUOUS_POLICY"] = rules.tier_ambiguous_policy
    if not rules.strict_unknown_rules:
        parser.error(
            "rule_engine.strict_unknown_rules must be true (silent unknown rules are forbidden)"
        )
    # Strict mode: validate all sources up-front, no silent fallbacks.
    for name, group in rules.static_image.items():
        try:
            validate_quality_group_rules(f"static_image.{name}", group)
        except ValueError as exc:
            parser.error(str(exc))
    try:
        validate_rust_tier_contract(rules)
    except ValueError as exc:
        parser.error(str(exc))
    for name, group in rules.animated_image.items():
        try:
            validate_quality_group_rules(f"animated_image.{name}", group)
        except ValueError as exc:
            parser.error(str(exc))
    if rules.strict_no_silent_fallbacks:
        for required_key in ("high_quality", "low_quality"):
            if required_key not in rules.static_image:
                parser.error(
                    f"static_image.{required_key} is required when strict_no_silent_fallbacks=true"
                )

    if args.training_mode == "static" and args.loop_intent_label != "auto":
        print(
            "  [WARN] --loop-intent-label is ignored when --training-mode is static",
            file=sys.stderr,
        )

    scope_line = training_scope_summary_line(args)
    if _TRAINING_SESSION is not None:
        _TRAINING_SESSION.set_phase("plan", scope=scope_line)
    try:
        try:
            if _TRAINING_SESSION is not None:
                _TRAINING_SESSION.set_phase("collect")
            if getattr(args, "reset_db", False) and args.ingest:
                conn_str = (
                    os.environ.get("MFB_PG_CONNSTR") or DEFAULT_CONNSTR
                ).strip() or DEFAULT_CONNSTR
                reset_training_db(conn_str)
            all_samples = collect_plan_samples(args, rules)
        except ValueError as exc:
            parser.error(str(exc))

        if not all_samples:
            print("No samples found.")
            if args.ingest:
                conn_str = (
                    os.environ.get("MFB_PG_CONNSTR") or DEFAULT_CONNSTR
                ).strip() or DEFAULT_CONNSTR
                if args.repair_schema:
                    repair_exit = repair_multi_scenario_schema(conn_str)
                    if repair_exit != 0:
                        raise SystemExit(repair_exit)
                if args.fill_runtime_assets:
                    raise SystemExit(
                        fill_runtime_assets(
                            conn_str,
                            saw_image_quality_samples=False,
                            saw_loop_samples=False,
                            install_missing_python_deps=args.install_missing_python_deps,
                            verify_after=args.verify_after,
                            training_mode=args.training_mode,
                        )
                    )
                if args.finalize_loop_intent:
                    raise SystemExit(finalize_loop_intent_assets(conn_str))
                if args.finalize_image_quality_model:
                    raise SystemExit(
                        finalize_image_quality_model(
                            conn_str,
                            install_missing_python_deps=args.install_missing_python_deps,
                        )
                    )
                raise SystemExit(2)
            return

        if args.ingest:
            if args.execute:
                print(
                    "  [EXECUTE] training ingest (fail-closed: exit 1 on failures, 2 on zero success)",
                    flush=True,
                )
            if _TRAINING_SESSION is not None:
                _TRAINING_SESSION.set_phase("ingest", sample_count=len(all_samples))
            conn_str = (
                os.environ.get("MFB_PG_CONNSTR") or DEFAULT_CONNSTR
            ).strip() or DEFAULT_CONNSTR
            if args.repair_schema:
                repair_exit = repair_multi_scenario_schema(conn_str)
                if repair_exit != 0:
                    raise SystemExit(repair_exit)
            exit_code = run_training_isolated(
                all_samples,
                args.no_loop,
                args.use_api,
                args.finalize_image_quality_model,
                args.finalize_loop_intent,
                args.fill_runtime_assets,
                args.verify_after,
                args.install_missing_python_deps,
                training_mode=args.training_mode,
                training_scope_summary=scope_line,
                loop_intent_api_label=loop_intent_label_for_api(args.loop_intent_label),
                verbose=training_verbose_enabled(args.verbose),
            )
            raise SystemExit(exit_code)
        print_plan(all_samples, scope_line=scope_line)
        if args.fill_runtime_assets:
            print(
                "Default (no --dry-run): after ingest, runtime fill runs "
                f"(up to {MAX_RUNTIME_FILL_PASSES} finalize passes while pending), then reports; "
                "add --verify-after for strict stack readiness."
            )
        else:
            print(
                "Use without --dry-run: --no-fill-runtime-assets means ingest + audit only."
            )
        if args.finalize_image_quality_model:
            print(
                "Re-run without --dry-run to ingest and finalize the image_quality model."
            )
        if args.finalize_loop_intent:
            print(
                "Re-run without --dry-run to ingest and finalize loop_intent KNN runtime."
            )
    except SystemExit as exc:
        if isinstance(exc.code, int):
            code = exc.code
        elif exc.code is None:
            code = 0
        else:
            code = 1
        if _TRAINING_SESSION is not None and not _TRAINING_SESSION.finalized:
            _TRAINING_SESSION.finalize(
                code,
                reason="SystemExit",
                exit_code_raw=exc.code,
            )
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
        if _TRAINING_SESSION is not None and not _TRAINING_SESSION.finalized:
            _TRAINING_SESSION.finalize(
                1,
                reason=f"{type(exc).__name__}: {exc}",
                traceback=format_exception(exc),
            )
        raise
    finally:
        if _TRAINING_SESSION is not None and not _TRAINING_SESSION.finalized:
            _TRAINING_SESSION.finalize(0, reason="completed")
        _finalize_training_session_logs(scope_line)


if __name__ == "__main__":
    main()
