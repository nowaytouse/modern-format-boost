import os
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

SCRIPT_DIR = Path(__file__).resolve().parents[1]
if str(SCRIPT_DIR) not in sys.path:
    sys.path.insert(0, str(SCRIPT_DIR))

import mfb_dylib
import python_api


class TestProjectArtifactPaths(unittest.TestCase):
    def setUp(self):
        self.lib_path_env = patch.dict(
            os.environ, {"SHARED_UTILS_LIB_PATH": ""}, clear=False
        )
        self.lib_path_env.start()
        self.addCleanup(self.lib_path_env.stop)

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

    def test_python_api_prefers_app_bundle_foundation_dylib(self):
        with tempfile.TemporaryDirectory() as temp_dir:
            root = Path(temp_dir)
            app_dylib = (
                root
                / "Modern Format Boost.app"
                / "Contents"
                / "Frameworks"
                / mfb_dylib.rust_dylib_filename()
            )
            app_dylib.parent.mkdir(parents=True)
            app_dylib.touch()

            with (
                patch.object(mfb_dylib, "ROOT", root),
                patch.object(python_api, "ROOT", root),
                patch.object(python_api, "LIB_DIR", root / "target" / "debug"),
                patch.object(
                    python_api, "RELEASE_LIB_DIR", root / "target" / "release"
                ),
            ):
                candidates = python_api.candidate_library_paths()

        self.assertEqual(candidates[0], app_dylib)

    def test_python_api_rejects_stale_app_bundle_foundation_dylib(self):
        with tempfile.TemporaryDirectory() as temp_dir:
            root = Path(temp_dir)
            lib_name = mfb_dylib.rust_dylib_filename()
            app_dylib = (
                root / "Modern Format Boost.app" / "Contents" / "Frameworks" / lib_name
            )
            release_dylib = root / "target" / "release" / lib_name
            artifact_dir = root / "crates" / ".modern_format_boost" / "artifacts"
            app_dylib.parent.mkdir(parents=True)
            release_dylib.parent.mkdir(parents=True)
            app_dylib.touch()
            release_dylib.touch()
            os.utime(app_dylib, (1, 1))
            os.utime(release_dylib, (2, 2))

            with (
                patch.object(mfb_dylib, "ROOT", root),
                patch.object(mfb_dylib, "ARTIFACT_DIR", artifact_dir),
                patch.object(python_api, "ROOT", root),
                patch.object(python_api, "LIB_DIR", root / "target" / "debug"),
                patch.object(
                    python_api, "RELEASE_LIB_DIR", root / "target" / "release"
                ),
            ):
                candidates = python_api.candidate_library_paths()
                first_existing = next(path for path in candidates if path.is_file())
                ensured = Path(mfb_dylib.ensure_foundation_dylib())

            self.assertNotIn(app_dylib, candidates)
            self.assertEqual(first_existing, release_dylib)
            self.assertEqual(ensured, artifact_dir / lib_name)

    def test_mfb_dylib_uses_project_state_artifacts_dir(self):
        self.assertEqual(
            mfb_dylib.ARTIFACT_DIR,
            mfb_dylib.ROOT / "crates" / ".modern_format_boost" / "artifacts",
        )
