"""Zero-fabrication policy helpers for dev/training scripts.

Training and quality ingestion must not swallow errors without a non-zero exit.
Set ``MFB_TRAINING_FAIL_CLOSED=0`` only for local debugging; default is fail-closed.
"""

from __future__ import annotations

import os
import sys
from collections.abc import Callable


def fail_closed_training_enabled() -> bool:
    value = os.environ.get("MFB_TRAINING_FAIL_CLOSED", "1").strip().lower()
    return value not in ("0", "false", "no", "off")


def training_quality_exit(code: int, message: str) -> None:
    print(message, file=sys.stderr)
    raise SystemExit(code)


def re_raise_training_exception(context: str, exc: BaseException) -> None:
    raise RuntimeError(f"{context}: {type(exc).__name__}: {exc}") from exc


def run_training_except_policy(
    context: str,
    exc: BaseException,
    *,
    on_retry: Callable[[], None] | None = None,
) -> None:
    """Fail-closed by default; optional retry hook when debugging."""
    if fail_closed_training_enabled():
        re_raise_training_exception(context, exc)
    if on_retry is not None:
        on_retry()
        return
    re_raise_training_exception(context, exc)
