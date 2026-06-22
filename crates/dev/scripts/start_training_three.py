#!/usr/bin/env python3
"""Compatibility entry for legacy three-lane training invocations.

Canonical launch policy lives in start_training_four.py/run_training.py.
"""

from __future__ import annotations

from start_training_four import main


if __name__ == "__main__":
    main()
