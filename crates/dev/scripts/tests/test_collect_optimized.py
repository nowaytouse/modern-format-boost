import sys
from pathlib import Path
from unittest.mock import patch

import pytest

SCRIPT_DIR = Path(__file__).resolve().parents[1]
if str(SCRIPT_DIR) not in sys.path:
    sys.path.insert(0, str(SCRIPT_DIR))

import collect_optimized


def test_snapshot_directories_fails_closed_on_stat_error(tmp_path):
    root = tmp_path / "src"
    root.mkdir()

    with (
        patch.object(
            collect_optimized.os,
            "stat",
            side_effect=OSError("metadata unavailable"),
        ),
        pytest.raises(collect_optimized.CollectMetadataError, match="snapshot failed"),
    ):
        collect_optimized.snapshot_directories(str(root))


def test_restore_directory_times_fails_closed_on_utime_error(tmp_path):
    src = tmp_path / "src"
    dest = tmp_path / "dest"
    src.mkdir()
    dest.mkdir()
    metadata = {str(src): (1.0, 2.0)}

    with (
        patch.object(
            collect_optimized.os,
            "utime",
            side_effect=OSError("metadata restore denied"),
        ),
        pytest.raises(collect_optimized.CollectMetadataError, match="restore failed"),
    ):
        collect_optimized.restore_directory_times(str(src), str(dest), metadata)
