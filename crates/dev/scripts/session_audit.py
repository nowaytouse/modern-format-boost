"""Append machine-readable lines to the MFB session / verbose audit log."""

from __future__ import annotations

import datetime
import os
from pathlib import Path


def audit_log_path() -> Path | None:
    raw = os.environ.get("MFB_SESSION_AUDIT", "").strip()
    if not raw:
        return None
    return Path(raw)


def append_session_audit(line: str) -> None:
    path = audit_log_path()
    if path is None:
        return
    try:
        path.parent.mkdir(parents=True, exist_ok=True)
        stamp = datetime.datetime.now().isoformat(timespec="seconds")
        with path.open("a", encoding="utf-8") as audit_f:
            audit_f.write(f"{stamp} {line}\n")
    except OSError:
        pass
