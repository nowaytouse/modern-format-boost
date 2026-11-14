"""Append-only training session audit (lane-local JSONL + exit snapshot).

Survives abrupt termination better than stdout-only logs: heartbeats flush during
long scans; ``training_session_exit.json`` records the last known phase and reason
when Python gets SIGTERM/SIGINT/atexit (not SIGKILL/OOM).
"""

from __future__ import annotations

import atexit
import json
import os
import signal
import sys
import time
import traceback
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

TRAINING_SESSION_AUDIT_JSONL = "training_session_audit.jsonl"
TRAINING_SESSION_EXIT_JSON = "training_session_exit.json"
_DEFAULT_HEARTBEAT_SECS = 60.0


def _utc_now() -> str:
    return datetime.now(timezone.utc).isoformat()


class TrainingSessionRecorder:
    """Lane-scoped audit writer (one instance per ``run_training.py`` process)."""

    def __init__(
        self,
        log_dir: Path,
        *,
        session_stamp: str,
        heartbeat_secs: float = _DEFAULT_HEARTBEAT_SECS,
    ) -> None:
        self.log_dir = log_dir.expanduser()
        self.log_dir.mkdir(parents=True, exist_ok=True)
        self.session_stamp = session_stamp.strip()
        self.heartbeat_secs = max(15.0, float(heartbeat_secs))
        self.started_mono = time.monotonic()
        self.phase = "init"
        self.finalized = False
        self._last_heartbeat_mono = 0.0
        self._handlers_installed = False
        self.audit_path = self.log_dir / TRAINING_SESSION_AUDIT_JSONL
        self.exit_path = self.log_dir / TRAINING_SESSION_EXIT_JSON

    def emit(self, event: str, **fields: Any) -> None:
        record: dict[str, Any] = {
            "ts": _utc_now(),
            "event": event,
            "pid": os.getpid(),
            "session_stamp": self.session_stamp or None,
            "lane": (os.environ.get("MFB_TRAINING_LANE") or "").strip() or None,
            "phase": self.phase,
        }
        record.update(fields)
        line = json.dumps(record, ensure_ascii=False, default=str) + "\n"
        with open(self.audit_path, "a", encoding="utf-8") as handle:
            handle.write(line)
            handle.flush()
            os.fsync(handle.fileno())

    def set_phase(self, phase: str, **fields: Any) -> None:
        self.phase = phase
        self.emit("phase", phase=phase, **fields)

    def maybe_heartbeat(self, **fields: Any) -> None:
        now = time.monotonic()
        if now - self._last_heartbeat_mono < self.heartbeat_secs:
            return
        self._last_heartbeat_mono = now
        self.emit(
            "heartbeat",
            elapsed_secs=round(now - self.started_mono, 1),
            **fields,
        )

    def install_handlers(self) -> None:
        if self._handlers_installed:
            return
        self._handlers_installed = True
        atexit.register(self._atexit)
        for signum in (signal.SIGTERM, signal.SIGINT):
            try:
                signal.signal(signum, self._signal_handler)
            except (OSError, ValueError):
                pass
        if hasattr(signal, "SIGHUP"):
            try:
                signal.signal(signal.SIGHUP, self._signal_handler)
            except (OSError, ValueError):
                pass

    def _signal_handler(self, signum: int, _frame: object) -> None:
        try:
            name = signal.Signals(signum).name
        except (ValueError, AttributeError):
            name = str(signum)
        self.finalize(
            128 + signum if signum > 0 else 1,
            reason=f"signal:{name}",
            interrupted=True,
        )
        raise SystemExit(128 + signum if signum > 0 else 1)

    def _atexit(self) -> None:
        if self.finalized:
            return
        exc_type, exc_val, _ = sys.exc_info()
        if exc_type is SystemExit and isinstance(exc_val, SystemExit):
            code = int(exc_val.code) if exc_val.code is not None else 0
            self.finalize(code, reason="SystemExit")
            return
        if exc_type is not None and exc_val is not None:
            self.finalize(
                1,
                reason=f"{exc_type.__name__}: {exc_val}",
                traceback=format_exception(exc_val)
                if isinstance(exc_val, BaseException)
                else str(exc_val),
            )
            return
        self.finalize(0, reason="atexit")

    def finalize(
        self,
        exit_code: int,
        *,
        reason: str,
        interrupted: bool = False,
        **fields: Any,
    ) -> None:
        if self.finalized:
            return
        self.finalized = True
        elapsed = round(time.monotonic() - self.started_mono, 1)
        payload: dict[str, Any] = {
            "session_stamp": self.session_stamp or None,
            "lane": (os.environ.get("MFB_TRAINING_LANE") or "").strip() or None,
            "pid": os.getpid(),
            "exit_code": int(exit_code),
            "reason": reason,
            "phase": self.phase,
            "interrupted": interrupted,
            "elapsed_secs": elapsed,
            "finished_at": _utc_now(),
        }
        payload.update(fields)
        self.exit_path.write_text(
            json.dumps(payload, ensure_ascii=False, indent=2) + "\n",
            encoding="utf-8",
        )
        self.emit("session_exit", **payload)
        print(
            "  [TRAINING-EXIT] "
            f"code={exit_code} reason={reason} phase={self.phase} "
            f"elapsed={elapsed}s audit={self.audit_path}",
            file=sys.stderr,
            flush=True,
        )

    def read_exit_snapshot(self) -> dict[str, Any] | None:
        if not self.exit_path.is_file():
            return None
        try:
            data = json.loads(self.exit_path.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError):
            return None
        return data if isinstance(data, dict) else None


def summarize_argv(argv: list[str] | None = None) -> list[str]:
    """Redacted argv tail safe for audit logs."""
    tail = list(argv if argv is not None else sys.argv)
    out: list[str] = []
    skip_next = False
    for arg in tail:
        if skip_next:
            out.append("<redacted>")
            skip_next = False
            continue
        if arg in ("--password", "--connstr") or arg.startswith("--pg-"):
            out.append(arg)
            skip_next = True
            continue
        out.append(arg)
    return out[-24:]


def format_exception(exc: BaseException) -> str:
    return "".join(
        traceback.format_exception(type(exc), exc, exc.__traceback__)[-6:]
    ).strip()
