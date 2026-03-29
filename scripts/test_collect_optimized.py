#!/usr/bin/env python3
import unittest
from unittest.mock import patch, MagicMock, call

# Import the code to test
import collect_optimized


class TestCollectOptimized(unittest.TestCase):
    @patch("subprocess.run")
    def test_get_finder_comment_mdls_success(self, mock_run):
        # Mock mdls finding the marker
        mock_run.return_value = MagicMock(
            returncode=0,
            stdout='kMDItemFinderComment = "[Optimized by Modern Format Boost]"',
        )
        self.assertTrue(collect_optimized.get_finder_comment("/path/to/file.jxl"))
        mock_run.assert_called_once()

    @patch("subprocess.run")
    @patch("subprocess.check_output")
    def test_get_finder_comment_xattr_fallback(self, mock_xattr, mock_mdls):
        # Mock mdls failure, but xattr success
        mock_mdls.return_value = MagicMock(returncode=1, stdout="")
        mock_xattr.return_value = b"[Optimized by Modern Format Boost]"

        self.assertTrue(collect_optimized.get_finder_comment("/path/to/file.jxl"))
        mock_mdls.assert_called_once()
        mock_xattr.assert_called_once()

    @patch("subprocess.run")
    @patch("subprocess.check_output")
    def test_get_finder_comment_fail(self, mock_xattr, mock_mdls):
        # Mock both failing
        mock_mdls.return_value = MagicMock(returncode=1, stdout="")
        mock_xattr.side_effect = Exception("No xattr")

        self.assertFalse(collect_optimized.get_finder_comment("/path/to/file.jxl"))

    @patch("os.path.isdir")
    @patch("os.path.abspath")
    def test_run_collection_invalid_source(self, mock_abs, mock_isdir):
        mock_abs.side_effect = lambda x: x
        mock_isdir.return_value = False

        # Should return False if source is not a directory
        result = collect_optimized.run_collection("/src", "/dest")
        self.assertFalse(result)

    @patch("os.path.isdir")
    @patch("os.path.abspath")
    @patch("os.path.exists")
    def test_run_collection_path_conflict(self, mock_exists, mock_abs, mock_isdir):
        mock_abs.side_effect = lambda x: x
        mock_isdir.return_value = True

        # Test destination inside source
        result = collect_optimized.run_collection("/src", "/src/nested")
        self.assertFalse(result)

    @patch("collect_optimized.get_finder_comment")
    @patch("os.walk")
    @patch("os.path.isdir")
    @patch("os.path.exists")
    @patch("os.makedirs")
    @patch("shutil.move")
    @patch("os.utime")
    @patch("os.stat")
    def test_full_collection_flow(
        self,
        mock_stat,
        mock_utime,
        mock_move,
        mock_mkdir,
        mock_exists,
        mock_isdir,
        mock_walk,
        mock_comment,
    ):
        # Setup mocks
        mock_isdir.return_value = True
        # Return True for source dir, but False for target files
        mock_exists.side_effect = lambda p: p == "/src" or p == "/dest"
        mock_walk.return_value = [
            ("/src", ["subdir"], ["file1.jxl", "file2.jpg"]),
            ("/src/subdir", [], ["file3.mov"]),
        ]
        mock_comment.side_effect = lambda p: "file1" in p or "file3" in p
        mock_stat.return_value = MagicMock(st_atime=123, st_mtime=456)

        # Run collection
        result = collect_optimized.run_collection("/src", "/dest")

        self.assertTrue(result)
        # Should move file1 and file3, but not file2
        self.assertEqual(mock_move.call_count, 2)

        # Check specific moves
        expected_calls = [
            call("/src/file1.jxl", "/dest/file1.jxl"),
            call("/src/subdir/file3.mov", "/dest/subdir/file3.mov"),
        ]
        mock_move.assert_has_calls(expected_calls, any_order=True)

    @patch("collect_optimized.get_finder_comment")
    @patch("os.walk")
    @patch("os.path.isdir")
    @patch("os.path.exists")
    @patch("shutil.move")
    def test_dry_run_no_moves(
        self, mock_move, mock_exists, mock_isdir, mock_walk, mock_comment
    ):
        mock_isdir.return_value = True
        mock_exists.return_value = True
        mock_walk.return_value = [("/src", [], ["optimized.jxl"])]
        mock_comment.return_value = True

        # Run with dry_run=True
        result = collect_optimized.run_collection("/src", "/dest", dry_run=True)

        self.assertTrue(result)
        mock_move.assert_not_called()


if __name__ == "__main__":
    unittest.main()
