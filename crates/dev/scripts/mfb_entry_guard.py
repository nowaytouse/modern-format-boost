#!/usr/bin/env python3
"""
Project-wide Python entry guards (single source of truth).

- ``guard_main(script_name)`` at the top of every production ``main()``.
- ``run_delegated(...)`` for all subprocess calls into other MFB scripts / Rust CLIs.
- Never read JSON configs here; use ``mfb_config_load.load_consumer_json``.
"""

from __future__ import annotations

import atexit
import datetime
import os
import re
import subprocess
import sys
from collections.abc import Sequence
from pathlib import Path
from typing import Any

MFB_INVOKER_ENV = "MFB_INVOKER"
MFB_TRAINING_INVOKER_ENV = "MFB_TRAINING_INVOKER"
TRAINING_SOURCE_MAP_ENV = "MFB_TRAINING_SOURCE_MAP"
QUALITY_MODEL_PYTHON_ENV = "MFB_QUALITY_MODEL_PYTHON"
BACKGROUND_PID_FILE_ENV = "MFB_BACKGROUND_PID_FILE"

INVOKER_DIRECT = "direct"
INVOKER_DRAG_AND_DROP = "drag_and_drop_processor.py"
INVOKER_INTERNAL_REEXEC = "internal-reexec"
INVOKER_TEST_HARNESS = "test-harness"
INVOKER_TRAINING_PIPELINE = "training_pipeline"
INVOKER_DATABASE_MANAGER = "database_manager"
INVOKER_RUN_TRAINING = "run_training"
INVOKER_CHECK_ALL = "check_all"

# Tokens allowed to invoke a given script (includes short names and ``*.py`` names).
_COMMON: frozenset[str] = frozenset(
    {
        INVOKER_DIRECT,
        INVOKER_INTERNAL_REEXEC,
        INVOKER_TEST_HARNESS,
        INVOKER_TRAINING_PIPELINE,
        INVOKER_DATABASE_MANAGER,
        INVOKER_RUN_TRAINING,
        INVOKER_CHECK_ALL,
        INVOKER_DRAG_AND_DROP,
        "run_training.py",
        "drag_and_drop_processor.py",
        "training_pipeline.py",
        "loop_intent_clustering.py",
        "quality_regression_model.py",
    }
)

SCRIPT_INVOKERS: dict[str, frozenset[str]] = {
    "run_training.py": _COMMON,
    "start_training_three.py": frozenset({INVOKER_DIRECT, INVOKER_TEST_HARNESS}),
    "start_training_four.py": frozenset({INVOKER_DIRECT, INVOKER_TEST_HARNESS}),
    "training_pipeline.py": _COMMON | frozenset({INVOKER_DATABASE_MANAGER}),
    "backfill_directory_scores.py": _COMMON,
    "loop_intent_clustering.py": _COMMON,
    "quality_regression_model.py": _COMMON,
    "media_conversion_delivery_heatmap.py": _COMMON,
    "drag_and_drop_processor.py": frozenset(
        {INVOKER_DIRECT, INVOKER_TEST_HARNESS, INVOKER_DATABASE_MANAGER}
    ),
    "python_api.py": _COMMON,
}

PRODUCTION_GUARDED_SCRIPTS: frozenset[str] = frozenset(
    {
        "run_training.py",
        "start_training_three.py",
        "start_training_four.py",
        "training_pipeline.py",
        "quality_regression_model.py",
        "drag_and_drop_processor.py",
        "backfill_directory_scores.py",
        "loop_intent_clustering.py",
        "media_conversion_delivery_heatmap.py",
        "python_api.py",
    }
)

_LOGGED_STALE_SOURCE_MAP = False

_SHELL_WRAPPER_RE = re.compile(
    r"(?:^|\s)(?:/usr/bin/env\s+)?(?:bash|sh|zsh|dash|fish|ksh)\s+[^\s]*\.sh(?:\s|$)",
    re.IGNORECASE,
)
_LEAN_CTX_WRAPPER_RE = re.compile(
    r"(?:^|\s)lean-ctx\b.*(?:\s-c\s|/shell_snapshots/)",
    re.IGNORECASE | re.DOTALL,
)
_MFB_SH_MARKER_RE = re.compile(
    r"(run_training|train_quality|train_knn|training_pipeline|database_manager|"
    r"backfill_directory|quality_regression|loop_intent|drag_and_drop|"
    r"crates/dev/scripts)[^\s]*\.sh",
    re.IGNORECASE,
)


def _process_args(pid: int) -> str:
    try:
        proc = subprocess.run(
            ["ps", "-p", str(pid), "-o", "args="],
            capture_output=True,
            text=True,
            check=False,
            timeout=2,
        )
    except (OSError, subprocess.TimeoutExpired) as exc:
        print(
            f"  [mfb-guard] ps probe failed ({exc}); wrapper-ancestry check degraded",
            file=sys.stderr,
        )
        return ""
    if proc.returncode != 0:
        return ""
    return (proc.stdout or "").strip()


def _parent_pid(pid: int) -> int:
    try:
        proc = subprocess.run(
            ["ps", "-p", str(pid), "-o", "ppid="],
            capture_output=True,
            text=True,
            check=False,
            timeout=2,
        )
    except (OSError, subprocess.TimeoutExpired) as exc:
        print(
            f"  [mfb-guard] ps probe failed ({exc}); wrapper-ancestry check degraded",
            file=sys.stderr,
        )
        return 0
    if proc.returncode != 0:
        return 0
    text = (proc.stdout or "").strip().split()
    if not text:
        return 0
    try:
        return int(text[0])
    except ValueError as exc:
        print(
            f"  [mfb-guard] ps probe failed ({exc}); wrapper-ancestry check degraded",
            file=sys.stderr,
        )
        return 0


def shell_wrapper_in_ancestry(max_depth: int = 6) -> str | None:
    pid = os.getppid()
    for _ in range(max_depth):
        if pid <= 1:
            break
        args = _process_args(pid)
        if args:
            lower = args.lower()
            if _LEAN_CTX_WRAPPER_RE.search(args):
                pid = _parent_pid(pid)
                continue
            if _SHELL_WRAPPER_RE.search(args) or _MFB_SH_MARKER_RE.search(args):
                return args
            if ".sh" in lower and "crates/dev/scripts" in lower:
                return args
        pid = _parent_pid(pid)
    return None


def resolved_invoker() -> str:
    for key in (MFB_INVOKER_ENV, MFB_TRAINING_INVOKER_ENV):
        raw = os.environ.get(key, "").strip()
        if raw:
            return raw
    return ""


def _cleanup_background_pid_file() -> None:
    raw_path = os.environ.get(BACKGROUND_PID_FILE_ENV, "").strip()
    if not raw_path:
        return
    pid_file = Path(raw_path)
    try:
        recorded_pid = int(pid_file.read_text(encoding="utf-8").strip())
    except (OSError, ValueError):
        return
    if recorded_pid != os.getpid():
        return
    try:
        pid_file.unlink(missing_ok=True)
    except OSError:
        return


def register_background_pid_cleanup() -> None:
    if os.environ.get(BACKGROUND_PID_FILE_ENV, "").strip():
        atexit.register(_cleanup_background_pid_file)


def _invoked_as_script_main() -> bool:
    """True when ``guard_main`` was reached from a top-level script ``main()``."""
    import inspect

    for frame_info in inspect.stack()[1:12]:
        module_name = frame_info.frame.f_globals.get("__name__")
        if module_name == "__main__":
            return True
        if module_name not in {__name__, "builtins", None}:
            break
    return False


def guard_main(
    script_name: str,
    *,
    require_invoker: bool = False,
    extra_invokers: frozenset[str] | None = None,
) -> None:
    """Call as the first line of ``main()`` (fail-closed)."""
    register_background_pid_cleanup()
    if not _invoked_as_script_main():
        raise SystemExit(
            f"{script_name} must be executed as __main__; "
            "do not import and call main() from other modules."
        )
    try:
        entry = Path(sys.argv[0]).resolve()
    except OSError:
        entry = Path(sys.argv[0])
    if entry.name != script_name:
        raise SystemExit(
            f"refusing non-canonical argv[0] for {script_name}: got {entry.name!r}"
        )

    wrapper = shell_wrapper_in_ancestry()
    if wrapper:
        raise SystemExit(
            f"refusing shell-wrapped invocation of {script_name} "
            f"(ancestor: {wrapper!r}). "
            f"Use: python3 crates/dev/scripts/{script_name} [flags]"
        )

    allowed = SCRIPT_INVOKERS.get(script_name, _COMMON)
    if extra_invokers:
        allowed = allowed | extra_invokers
    invoker = resolved_invoker()
    if os.environ.get("PYTEST_CURRENT_TEST"):
        os.environ[MFB_INVOKER_ENV] = INVOKER_TEST_HARNESS
        return
    if require_invoker and not invoker:
        raise SystemExit(
            f"refusing {script_name} without {MFB_INVOKER_ENV} "
            f"(allowed invokers: {', '.join(sorted(allowed))})"
        )
    if invoker and invoker not in allowed:
        raise SystemExit(
            f"refusing {script_name}: unknown invoker {invoker!r} "
            f"(allowed: {', '.join(sorted(allowed))})"
        )
    if not invoker:
        os.environ[MFB_INVOKER_ENV] = INVOKER_DIRECT
        os.environ.setdefault(MFB_TRAINING_INVOKER_ENV, INVOKER_DIRECT)

    if script_name in PRODUCTION_GUARDED_SCRIPTS:
        from mfb_log_paths import ensure_unified_log_dir

        ensure_unified_log_dir()


def assert_script_entry(
    script_name: str,
    *,
    allowed_invokers: frozenset[str] | None = None,
    require_invoker: bool = False,
) -> None:
    """Backward-compatible alias for ``guard_main``."""
    guard_main(
        script_name,
        require_invoker=require_invoker,
        extra_invokers=allowed_invokers,
    )


def rust_ingest_env(
    conn_str: str, extra: dict[str, str] | None = None
) -> dict[str, str]:
    """Environment for ``train_quality`` / ``train_knn`` (canonical PG + source-map hygiene)."""
    global _LOGGED_STALE_SOURCE_MAP
    env = child_env_for_script("run_training.py", extra)
    env[MFB_TRAINING_INVOKER_ENV] = INVOKER_RUN_TRAINING
    env["MFB_PG_CONNSTR"] = conn_str.strip() or conn_str
    raw = env.get(TRAINING_SOURCE_MAP_ENV)
    if raw:
        path_str = raw.strip()
        if not path_str or not Path(path_str).is_file():
            env.pop(TRAINING_SOURCE_MAP_ENV, None)
            if path_str and not _LOGGED_STALE_SOURCE_MAP:
                _LOGGED_STALE_SOURCE_MAP = True
                print(
                    f"  [WARN] Dropping invalid {TRAINING_SOURCE_MAP_ENV}={path_str!r} "
                    "(not a readable file).",
                    file=sys.stderr,
                )
    return env


def child_env_for_rust_ingest(extra: dict[str, str] | None = None) -> dict[str, str]:
    """Backward-compatible alias when conn_str is applied by caller."""
    env = child_env_for_script("run_training.py", extra)
    env[MFB_TRAINING_INVOKER_ENV] = INVOKER_RUN_TRAINING
    return env


def helper_env(
    parent_script: str,
    *,
    conn_str: str | None = None,
    quality_model_python: str | None = None,
    strip_training_source_map: bool = True,
    extra: dict[str, str] | None = None,
) -> dict[str, str]:
    """Env for cargo/pip/training_pipeline helpers (no stale replica map)."""
    merged: dict[str, str] = {}
    if extra:
        merged.update(extra)
    env = child_env_for_script(parent_script, merged)
    if strip_training_source_map:
        env.pop(TRAINING_SOURCE_MAP_ENV, None)
    if conn_str is not None:
        env["MFB_PG_CONNSTR"] = conn_str
    if quality_model_python:
        env.setdefault(QUALITY_MODEL_PYTHON_ENV, quality_model_python)
    return env


def run_rust_ingest(
    cmd: Sequence[str],
    *,
    conn_str: str,
    cwd: str | Path | None = None,
    capture_output: bool = True,
    text: bool = True,
    **kwargs: Any,
) -> subprocess.CompletedProcess[str]:
    """Run a Rust ingest CLI with stamped invoker + PG connstr."""
    return subprocess.run(
        list(cmd),
        cwd=cwd,
        env=rust_ingest_env(conn_str),
        capture_output=capture_output,
        text=text,
        check=kwargs.pop("check", False),
        **kwargs,
    )


def invoke_script(
    script_name: str,
    argv_tail: Sequence[str] | None = None,
    *,
    parent_script: str,
    cwd: str | Path | None = None,
    **kwargs: Any,
) -> subprocess.CompletedProcess[str]:
    """``python3 crates/dev/scripts/<script>`` with invoker stamp."""
    scripts_dir = Path(__file__).resolve().parent
    cmd = [sys.executable, str(scripts_dir / script_name)]
    if argv_tail:
        cmd.extend(argv_tail)
    return run_delegated(cmd, parent_script=parent_script, cwd=cwd, **kwargs)


def child_env_for_script(
    parent_script: str, extra: dict[str, str] | None = None
) -> dict[str, str]:
    from mfb_log_paths import unified_log_dir

    env = os.environ.copy()
    env[MFB_INVOKER_ENV] = parent_script
    log_root = unified_log_dir()
    env["MFB_LOG_DIR"] = str(log_root.resolve())
    if extra:
        env.update(extra)
    return env


def run_delegated(
    cmd: Sequence[str],
    *,
    parent_script: str,
    cwd: str | Path | None = None,
    check: bool = False,
    text: bool = True,
    strip_training_source_map: bool = False,
    **kwargs: Any,
) -> subprocess.CompletedProcess[str]:
    """Subprocess wrapper: always stamps ``MFB_INVOKER`` for the child."""
    env = kwargs.pop("env", None)
    if env is None:
        merged_env = child_env_for_script(parent_script)
    else:
        merged_env = child_env_for_script(parent_script, dict(env))
    if strip_training_source_map:
        merged_env.pop(TRAINING_SOURCE_MAP_ENV, None)
    return subprocess.run(
        list(cmd),
        cwd=cwd,
        env=merged_env,
        check=check,
        text=text,
        **kwargs,
    )


def detach_to_background(
    *,
    repo_root: Path,
    log_dir: Path,
    pid_file: Path,
    flag_to_strip: str = "--background",
) -> None:
    """Re-exec current script without ``flag_to_strip`` (same file only)."""
    from mfb_log_paths import coerce_log_dir

    log_dir = coerce_log_dir(log_dir)
    log_dir.mkdir(parents=True, exist_ok=True)
    if pid_file.is_file():
        try:
            old_pid = int(pid_file.read_text(encoding="utf-8").strip())
        except ValueError:
            old_pid = 0
        stale_pid_file = old_pid <= 0
        if old_pid > 0:
            try:
                os.kill(old_pid, 0)
            except OSError:
                stale_pid_file = True
            else:
                raise SystemExit(
                    f"已有后台进程 PID={old_pid}；停止: kill {old_pid} && rm -f {pid_file}"
                )
        if stale_pid_file:
            try:
                pid_file.unlink()
            except OSError:
                pass
            else:
                print(f"  [BACKGROUND] Cleared stale pid file: {pid_file}")
    stamp = datetime.datetime.now().strftime("%Y%m%d_%H%M%S")
    log_path = log_dir / f"{Path(sys.argv[0]).stem}_{stamp}.log"
    argv = [str(a) for a in sys.argv if a != flag_to_strip]
    with open(log_path, "ab", buffering=0) as log_f:
        proc = subprocess.Popen(
            [sys.executable, *argv],
            cwd=str(repo_root),
            stdin=subprocess.DEVNULL,
            stdout=log_f,
            stderr=subprocess.STDOUT,
            start_new_session=True,
            env={
                **os.environ,
                "PYTHONUNBUFFERED": "1",
                "MFB_LOG_DIR": str(log_dir),
                MFB_INVOKER_ENV: INVOKER_INTERNAL_REEXEC,
                MFB_TRAINING_INVOKER_ENV: INVOKER_INTERNAL_REEXEC,
                BACKGROUND_PID_FILE_ENV: str(pid_file),
            },
        )
    try:
        pid_file.write_text(f"{proc.pid}\n", encoding="utf-8")
    except OSError as exc:
        print(
            f"  [BACKGROUND] pid file write failed ({pid_file}): {exc}; "
            f"terminating detached child PID={proc.pid}",
            file=sys.stderr,
        )
        proc.terminate()
        try:
            proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            proc.kill()
        raise SystemExit(
            f"pid file write failed after spawn; child PID={proc.pid} "
            f"terminated; log={log_path}: {exc}"
        ) from exc
    print(f"  [BACKGROUND] PID={proc.pid} log={log_path}")
    raise SystemExit(0)
