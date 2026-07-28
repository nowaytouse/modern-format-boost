#!/usr/bin/env python3
"""Compatibility entry for four-lane training.

Canonical implementation lives in ``run_training.py``. Keep this script so
existing app/menu/docs invocations continue to work without duplicating launcher
policy.
"""

from __future__ import annotations

import sys
from pathlib import Path

SCRIPT_DIR = Path(__file__).resolve().parent
if str(SCRIPT_DIR) not in sys.path:
    sys.path.insert(0, str(SCRIPT_DIR))

from mfb_entry_guard import guard_main
from run_training import (
    FOUR_LANE_KNOWN_LANES as _KNOWN_LANES,
)
from run_training import (
    FOUR_LANE_SPECS as LANE_SPECS,
)
from run_training import (
    ROOT,
    ensure_db_training_closure_before_training,
    ensure_reset_db_before_training,
    four_lane_main,
    resolve_launch_log_root,
)
from run_training import (
    RUN_TRAINING_SCRIPT as RUN_TRAINING,
)
from run_training import (
    WORKSPACE_VENV_PYTHON as VENV_PYTHON,
)
from run_training import (
    four_lane_python_exe as _python_exe,
)
from run_training import (
    four_lane_slug_from_tail as _lane_slug_from_tail,
)
from run_training import (
    start_four_lane as start_lane,
)
from run_training import (
    stop_four_lane as _stop_lane,
)

FOUR_LANE_CONTRACT_LANES = ("static_high", "static_low", "loop_high", "loop_low")
__all__ = (
    "FOUR_LANE_CONTRACT_LANES",
    "LANE_SPECS",
    "ROOT",
    "RUN_TRAINING",
    "VENV_PYTHON",
    "_KNOWN_LANES",
    "_lane_slug_from_tail",
    "_python_exe",
    "_stop_lane",
    "ensure_db_training_closure_before_training",
    "ensure_reset_db_before_training",
    "four_lane_main",
    "main",
    "resolve_launch_log_root",
    "start_lane",
)


def main() -> None:
    guard_main("start_training_four.py")
    four_lane_main()


if __name__ == "__main__":
    main()
