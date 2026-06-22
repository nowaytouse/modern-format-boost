from __future__ import annotations

import logging
from datetime import datetime
from logging.handlers import RotatingFileHandler

from mfb_log_paths import MFB_LOG_AUDIT_DAY_STAMP, ensure_unified_log_dir

LOG_FORMAT = (
    "%(asctime)s | PID:%(process)d | %(name)-20s | %(levelname)-7s | %(message)s"
)


def setup_logger(name: str) -> logging.Logger:
    """Configure a file-backed audit logger with warning-level stderr mirroring."""
    logger = logging.getLogger(name)

    # `hasHandlers()` also inspects ancestors and can make a child logger look
    # configured when it is not. Keep the check local to this logger.
    if logger.handlers:
        return logger

    logger.setLevel(logging.DEBUG)

    logs_dir = ensure_unified_log_dir()

    today_str = datetime.now().strftime(MFB_LOG_AUDIT_DAY_STAMP)
    log_file = logs_dir / f"mfb_audit_{today_str}.log"
    formatter = logging.Formatter(LOG_FORMAT)

    file_handler = RotatingFileHandler(
        filename=log_file,
        maxBytes=10 * 1024 * 1024,
        backupCount=5,
        encoding="utf-8",
    )
    file_handler.setLevel(logging.DEBUG)
    file_handler.setFormatter(formatter)
    logger.addHandler(file_handler)

    stderr_handler = logging.StreamHandler()
    stderr_handler.setLevel(logging.WARNING)
    stderr_handler.setFormatter(formatter)
    logger.addHandler(stderr_handler)

    logger.propagate = False
    return logger


logger = setup_logger("mfb.global")
