"""Keep the Python development selector and test entry point aligned."""

import os
import shlex
import subprocess
import unittest
from pathlib import Path
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[4]


class TestPythonRuntimeContract(unittest.TestCase):
    def test_default_test_entry_matches_runtime_selector(self):
        version = (ROOT / ".python-version").read_text(encoding="utf-8").strip()
        self.assertEqual(version, "3.15")
        with patch.dict(os.environ, {}, clear=True):
            result = subprocess.run(
                ["make", "--no-print-directory", "-n", "test-py"],
                cwd=ROOT,
                check=True,
                capture_output=True,
                text=True,
            )
        self.assertEqual(
            shlex.split(result.stdout),
            [f"python{version}", "-m", "pytest", "crates/dev/scripts/tests/", "-v"],
        )

    def test_test_entry_preserves_explicit_interpreter(self):
        result = subprocess.run(
            [
                "make",
                "--no-print-directory",
                "-n",
                "test-py",
                "PYTHON=python-custom",
            ],
            cwd=ROOT,
            check=True,
            capture_output=True,
            text=True,
        )
        self.assertEqual(shlex.split(result.stdout)[0], "python-custom")
