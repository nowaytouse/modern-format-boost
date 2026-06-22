import sys
from pathlib import Path
from unittest.mock import patch

import pytest


SCRIPT_DIR = Path(__file__).resolve().parents[1]
if str(SCRIPT_DIR) not in sys.path:
    sys.path.insert(0, str(SCRIPT_DIR))

import mfb_training_scan  # noqa: E402


def _is_media_file(path: Path) -> bool:
    return path.suffix.lower() in {".jpg", ".jpeg", ".png", ".mp4"}


def test_estimate_media_file_count_fails_closed_on_unreadable_root(tmp_path):
    root = tmp_path / "corpus"
    root.mkdir()

    with patch.object(
        mfb_training_scan.os,
        "scandir",
        side_effect=OSError("permission denied"),
    ):
        with pytest.raises(
            mfb_training_scan.ScanPlanningError, match="media count scan failed"
        ):
            mfb_training_scan.estimate_media_file_count(
                root,
                is_media_file=_is_media_file,
                cap=10,
            )


def test_top_level_subdirs_fails_closed_on_unreadable_root(tmp_path):
    root = tmp_path / "corpus"
    root.mkdir()

    with patch.object(
        mfb_training_scan.os,
        "scandir",
        side_effect=OSError("permission denied"),
    ):
        with pytest.raises(
            mfb_training_scan.ScanPlanningError, match="top-level subdir scan failed"
        ):
            mfb_training_scan._top_level_subdirs(root)
