"""GAP-5: §PythonTests — icloud_import.py reliability tests.

Synthesized fixtures only — no project assets.
"""

import sys
from pathlib import Path
from unittest.mock import MagicMock, patch

sys.path.insert(0, str(Path(__file__).parent.parent))
import icloud_import

# ── strip_folder_suffix ──────────────────────────────────────────────────────


def test_strip_folder_suffix_optimized():
    assert icloud_import.strip_folder_suffix("trip_optimized") == "trip"


def test_strip_folder_suffix_collected():
    assert icloud_import.strip_folder_suffix("trip_collected") == "trip"


def test_strip_folder_suffix_both_orders():
    assert icloud_import.strip_folder_suffix("trip_optimized_collected") == "trip"
    assert icloud_import.strip_folder_suffix("trip_collected_optimized") == "trip"


def test_strip_folder_suffix_no_suffix():
    assert icloud_import.strip_folder_suffix("trip") == "trip"


def test_strip_folder_suffix_empty():
    assert icloud_import.strip_folder_suffix("") == ""


# ── rename_with_emoji ────────────────────────────────────────────────────────


def test_rename_with_emoji_already_has_prefix(tmp_path):
    d = tmp_path / "✨already"
    d.mkdir()
    result = icloud_import.rename_with_emoji(str(d))
    assert Path(result).name.startswith("✨")
    assert Path(result).exists()


def test_rename_with_emoji_adds_prefix(tmp_path):
    d = tmp_path / "myfolder"
    d.mkdir()
    result = icloud_import.rename_with_emoji(str(d))
    name = Path(result).name
    assert name.startswith("✨") or name.startswith("[*]")
    assert Path(result).exists()


def test_optimized_album_template_prefixes_root_and_child_album():
    template = icloud_import.optimized_album_template("/some/folder/my_album_optimized")
    assert template.startswith("✨/✨")
    assert template == "✨/✨my_album"


# ── find_osxphotos ────────────────────────────────────────────────────────────


def test_find_osxphotos_returns_none_when_absent():
    """When osxphotos is not installed, find_osxphotos returns None."""
    with patch("subprocess.run", side_effect=FileNotFoundError):
        with patch("os.path.exists", return_value=False):
            result = icloud_import.find_osxphotos()
    assert result is None


def test_find_osxphotos_returns_path_when_present():
    mock_result = MagicMock()
    mock_result.returncode = 0
    with patch("subprocess.run", return_value=mock_result):
        result = icloud_import.find_osxphotos()
    assert result == "osxphotos"


# ── check_osxphotos ───────────────────────────────────────────────────────────


def test_check_osxphotos_false_when_absent():
    with patch.object(icloud_import, "find_osxphotos", return_value=None):
        assert icloud_import.check_osxphotos() is False


def test_check_osxphotos_true_when_present():
    with patch.object(
        icloud_import, "find_osxphotos", return_value="/usr/local/bin/osxphotos"
    ):
        assert icloud_import.check_osxphotos() is True


# ── import lock ───────────────────────────────────────────────────────────────


def test_import_lock_uses_mfb_state_root(tmp_path, monkeypatch):
    monkeypatch.setenv("MFB_HOME_ROOT", str(tmp_path / "mfb"))

    lock_path = icloud_import.get_import_lock_path()

    assert lock_path == tmp_path / "mfb" / "locks" / "photos_import.lock"
    assert lock_path.parent.is_dir()


# ── run_optimized_import / run_simple_import ──────────────────────────────────


def test_run_optimized_import_missing_input(tmp_path):
    missing = str(tmp_path / "nonexistent_dir")
    try:
        result = icloud_import.run_optimized_import(missing)
        assert result is False
    except SystemExit as exc:
        assert exc.code not in (0, None)
    except FileNotFoundError:
        pass


def test_run_simple_import_empty_dir(tmp_path):
    empty = tmp_path / "empty"
    empty.mkdir()
    with patch.object(icloud_import, "find_osxphotos", return_value=None):
        with patch("subprocess.run") as run:
            result = icloud_import.run_simple_import(str(empty))
            assert result is False
            run.assert_not_called()
