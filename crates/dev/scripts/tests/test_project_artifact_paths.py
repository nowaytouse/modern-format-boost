import sys
import unittest
from pathlib import Path


SCRIPT_DIR = Path(__file__).resolve().parents[1]
if str(SCRIPT_DIR) not in sys.path:
    sys.path.insert(0, str(SCRIPT_DIR))

import mfb_dylib  # noqa: E402
import python_api  # noqa: E402


class TestProjectArtifactPaths(unittest.TestCase):
    def test_python_api_candidates_use_project_state_artifacts_dir(self):
        if sys.platform == "darwin":
            lib_name = "libfoundation.dylib"
        elif sys.platform == "win32":
            lib_name = "foundation.dll"
        else:
            lib_name = "libfoundation.so"

        expected = (
            python_api.ROOT / "crates" / ".modern_format_boost" / "artifacts" / lib_name
        )
        candidates = python_api.candidate_library_paths()

        self.assertIn(expected, candidates)
        self.assertFalse(any(".mfb_artifacts" in path.parts for path in candidates))

    def test_mfb_dylib_uses_project_state_artifacts_dir(self):
        self.assertEqual(
            mfb_dylib.ARTIFACT_DIR,
            mfb_dylib.ROOT / "crates" / ".modern_format_boost" / "artifacts",
        )
