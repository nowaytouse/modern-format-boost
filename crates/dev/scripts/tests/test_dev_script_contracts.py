"""Bridge for Part 4 SC: execute Python contract tests from dev scripts."""

import sys
from pathlib import Path

DEV_SCRIPT_TESTS = Path(__file__).resolve().parent
if str(DEV_SCRIPT_TESTS) not in sys.path:
    sys.path.insert(0, str(DEV_SCRIPT_TESTS))

from test_fabrication_guards import *  # noqa: F401,F403,E402
