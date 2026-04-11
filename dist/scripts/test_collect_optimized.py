#!/usr/bin/env python3
import tempfile
import unittest
from pathlib import Path

import collect_optimized


class TestCollectOptimized(unittest.TestCase):
    def test_run_collection_invalid_source(self):
        with tempfile.TemporaryDirectory() as dest:
            result = collect_optimized.run_collection("/path/that/does/not/exist", dest)
        self.assertFalse(result)

    def test_run_collection_path_conflict(self):
        with tempfile.TemporaryDirectory() as src:
            result = collect_optimized.run_collection(src, str(Path(src) / "nested"))
        self.assertFalse(result)

    def test_prunes_empty_dirs_even_when_no_candidates_are_found(self):
        with tempfile.TemporaryDirectory() as src, tempfile.TemporaryDirectory() as dest:
            src_root = Path(src)

            (src_root / "empty" / "child").mkdir(parents=True)
            (src_root / "legacy").mkdir()
            (src_root / "legacy" / "keep.png").write_bytes(b"png")

            result = collect_optimized.run_collection(src, dest)

            self.assertTrue(result)
            self.assertFalse((src_root / "empty").exists())
            self.assertTrue((src_root / "legacy").is_dir())
            self.assertTrue((src_root / "legacy" / "keep.png").exists())

    def test_moves_only_optimized_and_mirrors_structure(self):
        with tempfile.TemporaryDirectory() as src, tempfile.TemporaryDirectory() as dest:
            src_root = Path(src)
            dest_root = Path(dest)

            (src_root / "images").mkdir()
            (src_root / "images" / "photo.jxl").write_bytes(b"jxl")
            (src_root / "images" / "legacy.png").write_bytes(b"png")
            (src_root / "empty" / "child").mkdir(parents=True)

            result = collect_optimized.run_collection(src, dest)

            self.assertTrue(result)
            self.assertFalse((src_root / "images" / "photo.jxl").exists())
            self.assertTrue((src_root / "images" / "legacy.png").exists())
            self.assertTrue((src_root / "images").is_dir())
            self.assertFalse((src_root / "empty").exists())
            self.assertTrue((dest_root / "images" / "photo.jxl").exists())
            self.assertFalse((dest_root / "images" / "legacy.png").exists())
            self.assertTrue((dest_root / "empty" / "child").is_dir())

    def test_removes_source_root_when_everything_was_relocated(self):
        with tempfile.TemporaryDirectory() as src_parent, tempfile.TemporaryDirectory() as dest:
            src_root = Path(src_parent) / "source"
            src_root.mkdir()
            dest_root = Path(dest)

            (src_root / "only.jxl").write_bytes(b"jxl")

            result = collect_optimized.run_collection(str(src_root), str(dest_root))

            self.assertTrue(result)
            self.assertFalse(src_root.exists())
            self.assertTrue((dest_root / "only.jxl").exists())

    def test_collects_hevc_mp4_and_skips_non_hevc_video(self):
        with tempfile.TemporaryDirectory() as src, tempfile.TemporaryDirectory() as dest:
            src_root = Path(src)
            dest_root = Path(dest)

            (src_root / "videos").mkdir()
            (src_root / "videos" / "clip.mp4").write_bytes(b"mp4")
            (src_root / "videos" / "old.mov").write_bytes(b"mov")

            def codec_probe(path):
                if path.endswith("clip.mp4"):
                    return "hevc", None
                return "h264", None

            result = collect_optimized.run_collection(src, dest, codec_probe=codec_probe)

            self.assertTrue(result)
            self.assertTrue((dest_root / "videos" / "clip.mp4").exists())
            self.assertFalse((src_root / "videos" / "clip.mp4").exists())
            self.assertFalse((dest_root / "videos" / "old.mov").exists())
            self.assertTrue((src_root / "videos" / "old.mov").exists())

    def test_dry_run_makes_no_changes(self):
        with tempfile.TemporaryDirectory() as src, tempfile.TemporaryDirectory() as parent:
            src_root = Path(src)
            dest_root = Path(parent) / "collected"

            (src_root / "gallery").mkdir()
            (src_root / "gallery" / "preview.jxl").write_bytes(b"jxl")

            result = collect_optimized.run_collection(src, str(dest_root), dry_run=True)

            self.assertTrue(result)
            self.assertTrue((src_root / "gallery" / "preview.jxl").exists())
            self.assertFalse(dest_root.exists())


if __name__ == "__main__":
    unittest.main()
