import os
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch


SCRIPT_DIR = Path(__file__).resolve().parents[1]
RUST_BIN_DIR = SCRIPT_DIR.parent / "src" / "bin"
if str(SCRIPT_DIR) not in sys.path:
    sys.path.insert(0, str(SCRIPT_DIR))

import fastmode_paths  # noqa: E402


class TestFastModePaths(unittest.TestCase):
    def test_fastmode_output_dir_uses_adjacent_optimized_suffix(self):
        src = Path("/Users/example/Pictures/Album")

        self.assertEqual(
            fastmode_paths.fast_img_output_dir_for_target(src),
            Path("/Users/example/Pictures/Album_optimized"),
        )

    def test_fastmode_output_collision_uses_numbered_optimized_suffix(self):
        with tempfile.TemporaryDirectory() as tmp:
            src = Path(tmp) / "Album"
            src.mkdir()
            (Path(tmp) / "Album_optimized").mkdir()

            self.assertEqual(
                fastmode_paths.fast_img_output_dir_for_target(src),
                Path(tmp) / "Album_optimized_2",
            )

    def test_fastmode_output_dir_allows_legacy_mfb_wc_resume_for_cleanup(self):
        with tempfile.TemporaryDirectory() as tmp:
            src = Path(tmp) / "Album"
            src.mkdir()
            optimized = Path(tmp) / "Album_optimized"
            optimized.mkdir()
            (optimized / ".mfb_wc").write_text("legacy marker")

            self.assertEqual(
                fastmode_paths.fast_img_output_dir_for_target(src),
                optimized,
            )

    def test_fastmode_state_root_defaults_to_user_home_for_app_launch(self):
        with tempfile.TemporaryDirectory() as home:
            with patch.dict(os.environ, {"HOME": home, "FROM_APP": "1"}, clear=True):
                self.assertEqual(
                    fastmode_paths.default_mfb_state_root(),
                    Path(home) / ".modern_format_boost",
                )

    def test_fastmode_uses_smart_build_without_force(self):
        self.assertIs(fastmode_paths.FAST_IMG_FORCE_SMART_BUILD, False)

    def test_fastmode_normal_command_uses_local_jxl_delivery(self):
        command = fastmode_paths.build_fast_img_command(
            Path("/opt/mfb/img"),
            Path("/Users/example/Pictures/Album"),
            shortest_path=False,
            archive=False,
        )

        self.assertEqual(
            command,
            [
                "/opt/mfb/img",
                "fast-img",
                "/Users/example/Pictures/Album",
                "--recursive",
            ],
        )
        self.assertNotIn("--shortest-path", command)
        self.assertNotIn("--auto-import", command)
        self.assertNotIn("--archive", command)
        self.assertNotIn("--retry", command)

    def test_fastmode_archive_command_requests_archive_quality(self):
        command = fastmode_paths.build_fast_img_command(
            Path("/opt/mfb/img"),
            Path("/Users/example/Pictures/Album"),
            shortest_path=False,
            archive=True,
        )

        self.assertEqual(
            command,
            [
                "/opt/mfb/img",
                "fast-img",
                "/Users/example/Pictures/Album",
                "--recursive",
                "--archive",
            ],
        )

    def test_fastmode_shortest_path_command_auto_imports_after_shared_delivery(self):
        command = fastmode_paths.build_fast_img_command(
            Path("/opt/mfb/img"),
            Path("/Users/example/Pictures/Album"),
            shortest_path=True,
            archive=True,
        )

        self.assertEqual(
            command,
            [
                "/opt/mfb/img",
                "fast-img",
                "/Users/example/Pictures/Album",
                "--recursive",
                "--archive",
                "--shortest-path",
                "--auto-import",
            ],
        )

    def test_fastmode_retry_flag_is_explicit(self):
        command = fastmode_paths.build_fast_img_command(
            Path("/opt/mfb/img"),
            Path("/Users/example/Pictures/Album"),
            shortest_path=False,
            archive=False,
            retry=True,
        )

        self.assertIn("--retry", command)

    def test_fastmode_restore_jpeg_dir_uses_adjacent_suffix(self):
        src = Path("/Users/example/Pictures/Album")

        self.assertEqual(
            fastmode_paths.fast_img_restore_output_dir_for_target(src),
            Path("/Users/example/Pictures/Album_restored_jpeg"),
        )

    def test_fastmode_restore_jpeg_command_uses_rust_restore_subcommand(self):
        command = fastmode_paths.build_fast_img_restore_command(
            Path("/opt/mfb/img"),
            Path("/Users/example/Pictures/Album_optimized"),
            output_dir=Path("/Users/example/Pictures/Album_restored_jpeg"),
        )

        self.assertEqual(
            command,
            [
                "/opt/mfb/img",
                "restore-jpeg",
                "/Users/example/Pictures/Album_optimized",
                "--output",
                "/Users/example/Pictures/Album_restored_jpeg",
                "--recursive",
            ],
        )

    def test_fast_vid_dirs_use_adjacent_suffixes(self):
        src = Path("/Users/example/Movies/Clips")

        self.assertEqual(
            fastmode_paths.fast_vid_output_dir_for_target(src),
            Path("/Users/example/Movies/Clips_optimized"),
        )

    def test_fast_vid_command_uses_full_loop_intent_run_pipeline(self):
        command = fastmode_paths.build_fast_vid_command(
            Path("/opt/mfb/vid"),
            Path("/Users/example/Movies/Clips"),
            output_dir=Path("/Users/example/Movies/Clips_optimized"),
            shortest_path=False,
        )

        self.assertEqual(
            command,
            [
                "/opt/mfb/vid",
                "run",
                "/Users/example/Movies/Clips",
                "--output",
                "/Users/example/Movies/Clips_optimized",
                "--base-dir",
                "/Users/example/Movies/Clips",
                "--recursive",
                "--apple-compat",
                "--ultimate",
                "--archive",
            ],
        )

    def test_fast_vid_shortest_path_command_stays_on_full_run_pipeline(self):
        command = fastmode_paths.build_fast_vid_command(
            Path("/opt/mfb/vid"),
            Path("/Users/example/Movies/Clips"),
            output_dir=Path("/Users/example/Movies/Clips_optimized"),
            shortest_path=True,
        )

        self.assertIn("run", command)
        self.assertNotIn("fast-gif", command)
        self.assertNotIn("--shortest-path", command)
        self.assertNotIn("--auto-import", command)

    def test_drag_processor_wires_shortest_path_fastmode_to_rust_flags(self):
        source = (SCRIPT_DIR / "drag_and_drop_processor.py").read_text(encoding="utf-8")

        self.assertIn("mode_sub_state = (mode_sub_state + 1) % fastmode_count", source)
        self.assertIn("FAST_IMG_ACTION = choose_fast_img_action()", source)

        rust_source = (RUST_BIN_DIR / "drag_and_drop_processor.rs").read_text(
            encoding="utf-8"
        )
        self.assertIn("build_fast_img_command(", rust_source)
        self.assertIn("args.shortest_path", rust_source)
        self.assertIn("args.archive", rust_source)

    def test_drag_processor_defaults_to_cli_shell_not_vue(self):
        source = (RUST_BIN_DIR / "drag_and_drop_processor.rs").read_text(
            encoding="utf-8"
        )

        self.assertIn("default_value_t = LaunchMode::Auto", source)
        self.assertIn("Vue prototype is scaffolding only", source)
        self.assertIn("plan_cli_invocations", source)

    def test_drag_processor_python_ui_choice_defaults_to_shortest_path(self):
        source = (SCRIPT_DIR / "drag_and_drop_processor.py").read_text(encoding="utf-8")

        self.assertIn("def choose_fast_img_action", source)
        self.assertIn("Enter = Shortest Path", source)
        self.assertIn('return "restore_jpeg"', source)

    def test_drag_processor_archives_session_logs(self):
        source = (RUST_BIN_DIR / "drag_and_drop_processor.rs").read_text(
            encoding="utf-8"
        )

        self.assertIn("ensure_unified_log_dir", source)
        self.assertIn("archive_drag_drop_session_bundle", source)
        self.assertIn("SESSION_ARCHIVE_DONE", source)

    def test_drag_processor_python_ui_runs_fastmode_verify_summary_after_success(self):
        source = (SCRIPT_DIR / "drag_and_drop_processor.py").read_text(encoding="utf-8")

        self.assertIn("def run_fast_img_post_success", source)
        self.assertIn("run_fast_img_post_success()", source)
        self.assertIn("fast_img_delivery=True", source)

    def test_drag_processor_wires_videos_only_fastmode_to_full_vid_run(self):
        source = (SCRIPT_DIR / "drag_and_drop_processor.py").read_text(encoding="utf-8")

        self.assertIn('OUTPUT_MODE = "fast_vid"', source)
        self.assertIn(
            "FAST_VID_SHORTEST_PATH = choose_fast_vid_shortest_path()", source
        )

        rust_source = (RUST_BIN_DIR / "drag_and_drop_processor.rs").read_text(
            encoding="utf-8"
        )
        self.assertIn("LaunchMode::FastVid", rust_source)
        self.assertIn("build_fast_vid_command(", rust_source)
