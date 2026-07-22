import importlib.util
import json
import sys
import unittest
from types import SimpleNamespace
from pathlib import Path
from unittest.mock import MagicMock, patch

import pytest


SCRIPT_DIR = Path(__file__).resolve().parents[1]
if str(SCRIPT_DIR) not in sys.path:
    sys.path.insert(0, str(SCRIPT_DIR))

if not (SCRIPT_DIR / "drag_and_drop_processor.py").is_file():
    raise unittest.SkipTest(
        "legacy Python drag processor was replaced by the Rust binary"
    )


def load_drag_processor(tmp_path, monkeypatch):
    monkeypatch.setenv("HOME", str(tmp_path / "home"))
    monkeypatch.setenv("FROM_APP", "1")
    module_name = "drag_and_drop_processor_under_test"
    spec = importlib.util.spec_from_file_location(
        module_name, SCRIPT_DIR / "drag_and_drop_processor.py"
    )
    module = importlib.util.module_from_spec(spec)
    sys.modules[module_name] = module
    assert spec.loader is not None
    spec.loader.exec_module(module)
    return module


def write_fast_img_marker(
    state_root: Path,
    source: Path,
    optimized: Path,
    blake3_log: dict[str, dict[str, str]],
    *,
    src_jpeg_count: int,
    stage: str = "cleanup_complete",
    skipped_sources: dict[str, dict[str, str]] | None = None,
    failed_sources: dict[str, dict[str, str]] | None = None,
) -> None:
    marker_dir = state_root / "fast_img" / "markers"
    marker_dir.mkdir(parents=True)
    (marker_dir / "test-marker.json").write_text(
        json.dumps(
            {
                "schema": 1,
                "src_dir": str(source),
                "working_copy": str(optimized),
                "stage": stage,
                "src_jpeg_count": src_jpeg_count,
                "transcoded_count": len(blake3_log),
                "blake3_log": blake3_log,
                "skipped_sources": skipped_sources or {},
                "failed_sources": failed_sources or {},
                "error": None,
            }
        ),
        encoding="utf-8",
    )


def test_fast_img_success_path_skips_normal_img_vid_pipeline(tmp_path, monkeypatch):
    drag = load_drag_processor(tmp_path, monkeypatch)

    assert drag.should_run_img_vid_pipeline("fast_img") is False
    assert drag.should_run_img_vid_pipeline("adjacent") is True


def test_drag_drop_error_mode_defaults_to_log_and_continue(tmp_path, monkeypatch):
    drag = load_drag_processor(tmp_path, monkeypatch)
    monkeypatch.delenv("MFB_DRAG_DROP_ERROR_MODE", raising=False)
    monkeypatch.delenv("MFB_DRAG_DROP_FAIL_FAST", raising=False)

    assert drag.drag_drop_error_mode() == "log-and-continue"
    assert drag.drag_drop_fail_fast_enabled() is False


def test_drag_drop_error_mode_accepts_fail_fast_env(tmp_path, monkeypatch):
    drag = load_drag_processor(tmp_path, monkeypatch)
    monkeypatch.setenv("MFB_DRAG_DROP_ERROR_MODE", "fail_fast")

    assert drag.drag_drop_error_mode() == "fail-fast"
    assert drag.drag_drop_fail_fast_enabled() is True


def test_stream_and_log_process_continue_mode_records_nonzero_without_exit(
    tmp_path, monkeypatch
):
    drag = load_drag_processor(tmp_path, monkeypatch)
    monkeypatch.setenv("MFB_DRAG_DROP_ERROR_MODE", "log-and-continue")
    drag.LOG_DIR = tmp_path
    drag.LOG_FILE = str(tmp_path / "processor.log")
    drag.VERBOSE_MODE = False

    with (
        patch("builtins.input", return_value="") as input_mock,
        patch.object(drag.sys.stdin, "isatty", return_value=True),
    ):
        result = drag.stream_and_log_process(
            [
                sys.executable,
                "-c",
                (
                    "import sys; "
                    "print('Succeeded: 1'); "
                    "print('Skipped: 0'); "
                    "print('Ignored: 0'); "
                    "print('Failed: 2'); "
                    "sys.exit(7)"
                ),
            ],
            "img",
        )

    assert result.returncode == 7
    input_mock.assert_not_called()
    assert drag.IMG_SUCCEEDED == 1
    assert drag.IMG_SKIPPED == 0
    assert drag.IMG_IGNORED == 0
    assert drag.IMG_FAILED == 2


def test_stream_and_log_process_fail_fast_env_exits_on_nonzero(tmp_path, monkeypatch):
    drag = load_drag_processor(tmp_path, monkeypatch)
    monkeypatch.setenv("MFB_DRAG_DROP_ERROR_MODE", "fail-fast")
    drag.LOG_DIR = tmp_path
    drag.LOG_FILE = str(tmp_path / "processor.log")
    drag.VERBOSE_MODE = False

    with pytest.raises(SystemExit) as raised:
        drag.stream_and_log_process(
            [
                sys.executable,
                "-c",
                "import sys; print('Failed: 1'); sys.exit(7)",
            ],
            "img",
        )

    assert raised.value.code == 7


def test_process_images_log_and_continue_runs_each_routed_file(tmp_path, monkeypatch):
    drag = load_drag_processor(tmp_path, monkeypatch)
    monkeypatch.setenv("MFB_DRAG_DROP_ERROR_MODE", "log-and-continue")
    source = tmp_path / "Album"
    output = tmp_path / "Album_optimized"
    nested = source / "nested"
    nested.mkdir(parents=True)
    output.mkdir()
    (source / "a.jpg").write_bytes(b"one")
    (nested / "b.jpg").write_bytes(b"two")
    drag.TARGET_DIR = str(source)
    drag.OUTPUT_DIR = str(output)
    drag.OUTPUT_MODE = "adjacent"
    drag.IMGQUALITY_HEVC = tmp_path / "img"
    drag.IMGQUALITY_HEVC.write_text("#!/bin/sh\n", encoding="utf-8")
    drag.IMG_COUNT = 2
    drag.ROUTED_IMAGE_REL_PATHS = {"a.jpg", "nested/b.jpg"}
    drag.VERBOSE_MODE = False
    calls = []

    def fake_stream(cmd, parse_type, **_kwargs):
        calls.append(cmd)
        assert parse_type == "img"
        if str(source / "a.jpg") in cmd:
            return drag.ProcessorRunResult(0, 1, 0, 0, 0)
        return drag.ProcessorRunResult(11, 0, 0, 0, 1)

    monkeypatch.setattr(drag, "stream_and_log_process", fake_stream)

    drag.process_images()

    assert len(calls) == 2
    assert all("--mode" in call for call in calls)
    assert all("images" in call for call in calls)
    assert all(str(source) in call for call in calls)
    assert drag.IMG_SUCCEEDED == 1
    assert drag.IMG_FAILED == 1


def test_process_images_fail_fast_uses_single_batch_command(tmp_path, monkeypatch):
    drag = load_drag_processor(tmp_path, monkeypatch)
    monkeypatch.setenv("MFB_DRAG_DROP_ERROR_MODE", "fail-fast")
    source = tmp_path / "Album"
    output = tmp_path / "Album_optimized"
    source.mkdir()
    output.mkdir()
    drag.TARGET_DIR = str(source)
    drag.OUTPUT_DIR = str(output)
    drag.OUTPUT_MODE = "adjacent"
    drag.IMGQUALITY_HEVC = tmp_path / "img"
    drag.IMGQUALITY_HEVC.write_text("#!/bin/sh\n", encoding="utf-8")
    drag.IMG_COUNT = 2
    drag.ROUTED_IMAGE_REL_PATHS = {"a.jpg", "nested/b.jpg"}
    calls = []

    def fake_stream(cmd, parse_type, **_kwargs):
        calls.append(cmd)
        assert parse_type == "img"
        return drag.ProcessorRunResult(0, 2, 0, 0, 0)

    monkeypatch.setattr(drag, "stream_and_log_process", fake_stream)

    drag.process_images()

    assert calls == [
        [
            "cargo",
            "run",
            "--locked",
            "-p",
            "dev",
            "--bin",
            "drag_and_drop_processor",
            "--",
            "--mode",
            "images",
            "--ultimate",
            "--verbose",
            "--output",
            str(output),
            str(source),
        ]
    ]


def test_record_processor_launch_failure_updates_failure_count(tmp_path, monkeypatch):
    drag = load_drag_processor(tmp_path, monkeypatch)
    drag.IMG_COUNT = 3
    drag.IMG_FAILED = 0

    drag.record_processor_launch_failure(
        "img",
        ["img", "fast-img"],
        RuntimeError("boom"),
    )

    assert drag.IMG_FAILED == 3


def test_fast_img_launch_failure_counts_true_jpegs_only(tmp_path, monkeypatch):
    drag = load_drag_processor(tmp_path, monkeypatch)
    source = tmp_path / "Album"
    source.mkdir()
    (source / "a.jpeg").write_bytes(b"\xff\xd8\xffjpeg-a")
    (source / "b.png").write_bytes(b"\xff\xd8\xffjpeg-disguised")
    (source / "not-fastmode.png").write_bytes(b"\x89PNG\r\n\x1a\n")
    drag.OUTPUT_MODE = "fast_img"
    drag.TARGET_DIR = str(source)
    drag.IMG_COUNT = 1917
    drag.IMG_FAILED = 0

    drag.record_processor_launch_failure(
        "img",
        ["img", "fast-img"],
        RuntimeError("boom"),
    )

    assert drag.IMG_FAILED == 2


def test_fast_img_requires_binary_check_before_rust_scan(tmp_path, monkeypatch):
    drag = load_drag_processor(tmp_path, monkeypatch)
    drag.OUTPUT_MODE = "fast_img"
    drag.PROCESSING_MODE = "images_only"
    drag.IMG_COUNT = 0
    drag.VID_COUNT = 0

    assert drag._pipeline_needs_binaries() is True


def test_fast_img_command_retries_when_marker_stage_requires_retry(
    tmp_path, monkeypatch
):
    drag = load_drag_processor(tmp_path, monkeypatch)
    state_root = tmp_path / "mfb_state"
    monkeypatch.setenv("MFB_HOME_ROOT", str(state_root))
    source = tmp_path / "Album"
    optimized = tmp_path / "Album_optimized"
    source.mkdir()
    optimized.mkdir()
    write_fast_img_marker(
        state_root,
        source,
        optimized,
        {},
        src_jpeg_count=1,
        stage="gate1_failed",
    )

    drag.IMGQUALITY_HEVC = Path("/opt/mfb/img")
    drag.TARGET_DIR = str(source)
    drag.OUTPUT_DIR = str(optimized)
    drag.FAST_IMG_SHORTEST_PATH = False

    command = drag.build_fast_img_delivery_command()

    assert "--retry" in command


def test_fast_img_command_does_not_retry_without_marker(tmp_path, monkeypatch):
    drag = load_drag_processor(tmp_path, monkeypatch)
    monkeypatch.setenv("MFB_HOME_ROOT", str(tmp_path / "mfb_state"))
    source = tmp_path / "Album"
    optimized = tmp_path / "Album_optimized"
    source.mkdir()
    optimized.mkdir()

    drag.IMGQUALITY_HEVC = Path("/opt/mfb/img")
    drag.TARGET_DIR = str(source)
    drag.OUTPUT_DIR = str(optimized)
    drag.FAST_IMG_SHORTEST_PATH = False

    command = drag.build_fast_img_delivery_command()

    assert "--retry" not in command


def test_fast_img_auto_retries_once_after_retryable_failure(
    tmp_path, monkeypatch, capsys
):
    drag = load_drag_processor(tmp_path, monkeypatch)
    state_root = tmp_path / "mfb_state"
    monkeypatch.setenv("MFB_HOME_ROOT", str(state_root))
    source = tmp_path / "Album"
    optimized = tmp_path / "Album_optimized"
    source.mkdir()
    optimized.mkdir()
    drag.IMGQUALITY_HEVC = Path("/opt/mfb/img")
    drag.TARGET_DIR = str(source)
    drag.OUTPUT_DIR = str(optimized)
    drag.FAST_IMG_SHORTEST_PATH = False
    calls = []

    def fake_run(command):
        calls.append(command)
        if len(calls) == 1:
            write_fast_img_marker(
                state_root,
                source,
                optimized,
                {},
                src_jpeg_count=1,
                stage="gate1_failed",
            )
            return SimpleNamespace(returncode=9)
        return SimpleNamespace(returncode=0)

    monkeypatch.setattr(drag.subprocess, "run", fake_run)

    command, result = drag.run_fast_img_delivery_with_auto_retry()

    assert result.returncode == 0
    assert command == calls[1]
    assert "--retry" not in calls[0]
    assert "--retry" in calls[1]
    assert len(calls) == 2
    assert "Recoverable failure detected" in capsys.readouterr().out


def test_fast_img_delivery_verify_warning_is_fail_closed(tmp_path, monkeypatch):
    drag = load_drag_processor(tmp_path, monkeypatch)
    source = tmp_path / "Album"
    optimized = tmp_path / "Album_optimized"
    source.mkdir()
    optimized.mkdir()
    drag.TARGET_DIR = str(source)
    drag.OUTPUT_DIR = str(optimized)
    drag.OUTPUT_MODE = "fast_img"
    drag.PROCESSING_MODE = "images_only"
    drag.LOG_FILE = ""
    drag.VERBOSE_LOG_FILE = ""

    verify_stdout = "\n".join(
        [
            "🔎 Integrity summary",
            "   Integrity Issues:2",
            "   Integrity:      WARNINGS",
            "",
        ]
    )
    completed = MagicMock(returncode=0, stdout=verify_stdout, stderr="")

    with patch.object(drag.subprocess, "run", return_value=completed):
        assert (
            drag.run_unified_verification(
                include_logs=False,
                auto_mode=True,
                fast_img_delivery=True,
            )
            is False
        )

    assert drag.LAST_VERIFY_WARNINGS is True
    assert drag.LAST_VERIFY_ISSUE_COUNT == 2


def test_fast_img_jxl_output_counter_uses_true_format(tmp_path, monkeypatch):
    drag = load_drag_processor(tmp_path, monkeypatch)
    output = tmp_path / "Album_optimized"
    output.mkdir()
    (output / "extensionless").write_bytes(b"\xff\x0atrue-jxl")
    (output / "spoofed.JXL").write_bytes(b"not-jxl")

    count, total_size = drag.count_fast_img_jxl_outputs(output)

    assert count == 1
    assert total_size == (output / "extensionless").stat().st_size


def test_size_comparison_summary_formats_stable_rows(tmp_path, monkeypatch):
    drag = load_drag_processor(tmp_path, monkeypatch)

    summary = drag.build_size_comparison_summary(
        before_bytes=1_000,
        after_bytes=750,
        operation_mode="normal",
        processing_type="both",
    )

    assert "Before/After Size Comparison" in summary
    assert "Operation Mode:  normal" in summary
    assert "Processing Type: both" in summary
    assert "Total Before:    1000 B" in summary
    assert "Total After:     750 B" in summary
    assert "Difference:      -250 B" in summary
    assert "Change:          -25.0%" in summary


def test_size_comparison_summary_covers_fastmode_images(tmp_path, monkeypatch):
    drag = load_drag_processor(tmp_path, monkeypatch)

    summary = drag.build_size_comparison_summary(
        before_bytes=2_048,
        after_bytes=1_024,
        operation_mode="fastmode",
        processing_type="img",
    )

    assert "Operation Mode:  fastmode" in summary
    assert "Processing Type: img" in summary
    assert "Difference:      -1.0 KB" in summary
    assert "Change:          -50.0%" in summary


def test_size_comparison_summary_covers_every_videos_growth(tmp_path, monkeypatch):
    drag = load_drag_processor(tmp_path, monkeypatch)

    summary = drag.build_size_comparison_summary(
        before_bytes=1_024,
        after_bytes=1_536,
        operation_mode="every",
        processing_type="vid",
    )

    assert "Operation Mode:  every" in summary
    assert "Processing Type: vid" in summary
    assert "Difference:      +512 B" in summary
    assert "Change:          +50.0%" in summary


def test_snapshot_selected_media_size_respects_processing_type(tmp_path, monkeypatch):
    drag = load_drag_processor(tmp_path, monkeypatch)
    source = tmp_path / "Album"
    source.mkdir()
    image = source / "photo.jpg"
    video = source / "clip.mp4"
    image.write_bytes(b"\xff\xd8\xff" + (b"i" * 7))
    video.write_bytes(b"\x00\x00\x00\x18ftypmp42" + (b"v" * 28))

    drag.TARGET_DIR = str(source)
    drag.ROUTED_IMAGE_REL_PATHS = {"photo.jpg"}
    drag.ROUTED_VIDEO_REL_PATHS = {"clip.mp4"}

    drag.PROCESSING_MODE = "images_only"
    assert drag.snapshot_selected_media_size() == image.stat().st_size

    drag.PROCESSING_MODE = "videos_only"
    assert drag.snapshot_selected_media_size() == video.stat().st_size

    drag.PROCESSING_MODE = "both"
    assert (
        drag.snapshot_selected_media_size()
        == image.stat().st_size + video.stat().st_size
    )


def test_count_files_logs_probe_failure_without_crashing(tmp_path, monkeypatch):
    drag = load_drag_processor(tmp_path, monkeypatch)
    source = tmp_path / "Album"
    source.mkdir()
    (source / "bad.GIF").write_bytes(b"GIF89a" + b"\x00" * 7)
    drag.TARGET_DIR = str(source)
    drag.PROCESSING_MODE = "images_only"
    drag.SESSION_AUDIT_FILE = str(tmp_path / "session_audit.jsonl")

    def fake_classify(path):
        raise drag.MediaProbeError(f"forced probe failure for {path}")

    monkeypatch.setattr(drag, "classify_media_owner", fake_classify)

    drag.count_files()

    assert drag.IMG_COUNT == 0
    assert drag.VID_COUNT == 0
    assert "ROUTING_PROBE_FAILED" in Path(drag.SESSION_AUDIT_FILE).read_text()


def test_output_size_for_summary_rescans_inplace_after_tree(tmp_path, monkeypatch):
    drag = load_drag_processor(tmp_path, monkeypatch)
    source = tmp_path / "Album"
    source.mkdir()
    image = source / "photo.jpg"
    image.write_bytes(b"\xff\xd8\xff" + (b"i" * 17))

    drag.TARGET_DIR = str(source)
    drag.OUTPUT_MODE = "inplace"
    drag.PROCESSING_MODE = "images_only"
    drag.MEDIA_TOTAL_SIZE = 999_999

    assert drag.output_size_for_summary() == image.stat().st_size


def test_fast_img_post_success_preserves_before_size_snapshot(tmp_path, monkeypatch):
    drag = load_drag_processor(tmp_path, monkeypatch)
    source = tmp_path / "Album"
    optimized = tmp_path / "Album_optimized"
    source.mkdir()
    optimized.mkdir()
    (optimized / "converted.JXL").write_bytes(b"\xff\x0a")
    drag.TARGET_DIR = str(source)
    drag.OUTPUT_DIR = str(optimized)
    drag.OUTPUT_MODE = "fast_img"
    drag.PROCESSING_MODE = "images_only"
    drag.FAST_IMG_SHORTEST_PATH = False
    drag.FAST_IMG_OUTPUT_CLEANED = False
    drag.MEDIA_TOTAL_SIZE = 10_000

    summary = "\n".join(
        [
            "🔎 Integrity summary",
            "   Recorded source JPEGs:          1",
            "   Optimized JXL files:            1",
            "   Recorded skipped JPEGs:         0",
            "   Integrity Issues:0",
            "   Integrity:      CLEAN",
            "",
        ]
    )

    def fake_verify(*_args, **_kwargs):
        drag.LAST_VERIFY_SUMMARY = summary
        drag.LAST_VERIFY_WARNINGS = False
        drag.LAST_VERIFY_ISSUE_COUNT = 0
        return True

    monkeypatch.setattr(drag, "run_unified_verification", fake_verify)

    drag.run_fast_img_post_success()

    assert drag.MEDIA_TOTAL_SIZE == 10_000
    assert drag.SIZE_SUMMARY_AFTER_OVERRIDE == 2


def test_shortest_path_cleanup_removes_only_marker_recorded_jxls_and_prunes_empty_dirs(
    tmp_path, monkeypatch
):
    drag = load_drag_processor(tmp_path, monkeypatch)
    monkeypatch.setenv("MFB_HOME_ROOT", str(tmp_path / "mfb_state"))
    source = tmp_path / "Album"
    output = tmp_path / "Album_optimized"
    source.mkdir()
    (output / "nested").mkdir(parents=True)
    (output / "nested" / "a.JXL").write_bytes(b"\xff\x0aoptimized")
    (output / "skipped").mkdir()
    (output / "skipped" / "truncated.jpeg").write_bytes(b"\xff\xd8\xff\xe0missing-eoi")
    (output / "untracked").mkdir()
    (output / "untracked" / "manual.JXL").write_bytes(b"\xff\x0auntracked")
    write_fast_img_marker(
        tmp_path / "mfb_state",
        source,
        output,
        {
            "nested/a.jpeg": {
                "src": "source-blake3",
                "out": "output-blake3",
                "out_rel": "nested/a.JXL",
            }
        },
        src_jpeg_count=2,
        skipped_sources={
            "skipped/truncated.jpeg": {
                "src": "skipped-source-blake3",
                "reason": "JPEG is truncated or missing EOI",
            }
        },
    )

    drag.delete_fast_img_shortest_path_output_dir(output)

    assert output.exists()
    assert not (output / "nested").exists()
    assert (output / "skipped" / "truncated.jpeg").is_file()
    assert (output / "untracked" / "manual.JXL").is_file()
    assert drag.FAST_IMG_OUTPUT_CLEANED is False


def test_shortest_path_cleanup_prunes_empty_output_dir_when_all_sources_skipped(
    tmp_path, monkeypatch
):
    drag = load_drag_processor(tmp_path, monkeypatch)
    monkeypatch.setenv("MFB_HOME_ROOT", str(tmp_path / "mfb_state"))
    source = tmp_path / "Album"
    output = tmp_path / "Album_optimized"
    source.mkdir()
    output.mkdir()
    write_fast_img_marker(
        tmp_path / "mfb_state",
        source,
        output,
        {},
        src_jpeg_count=1,
        skipped_sources={
            "skipped/truncated.jpeg": {
                "src": "skipped-source-blake3",
                "reason": "JPEG is truncated or missing EOI",
            }
        },
    )

    drag.delete_fast_img_shortest_path_output_dir(output)

    assert not output.exists()
    assert drag.FAST_IMG_OUTPUT_CLEANED is True


def test_shortest_path_cleanup_removes_ds_store_and_orphan_empty_dirs(
    tmp_path, monkeypatch
):
    drag = load_drag_processor(tmp_path, monkeypatch)
    monkeypatch.setenv("MFB_HOME_ROOT", str(tmp_path / "mfb_state"))
    source = tmp_path / "Album"
    output = tmp_path / "Album_optimized"
    source.mkdir()
    (output / "converted").mkdir(parents=True)
    (output / "converted" / "a.JXL").write_bytes(b"\xff\x0aoptimized")
    (output / "orphan").mkdir()
    (output / "nested" / "empty").mkdir(parents=True)
    (output / ".DS_Store").write_bytes(b"finder")
    write_fast_img_marker(
        tmp_path / "mfb_state",
        source,
        output,
        {
            "converted/a.jpeg": {
                "src": "source-blake3",
                "out": "output-blake3",
                "out_rel": "converted/a.JXL",
            }
        },
        src_jpeg_count=1,
    )

    drag.delete_fast_img_shortest_path_output_dir(output)

    assert not output.exists()
    assert drag.FAST_IMG_OUTPUT_CLEANED is True


def test_fast_img_post_success_counts_recorded_skipped_sources(tmp_path, monkeypatch):
    drag = load_drag_processor(tmp_path, monkeypatch)
    source = tmp_path / "Album"
    optimized = tmp_path / "Album_optimized"
    source.mkdir()
    optimized.mkdir()
    (optimized / "converted.JXL").write_bytes(b"\xff\x0a")
    drag.TARGET_DIR = str(source)
    drag.OUTPUT_DIR = str(optimized)
    drag.OUTPUT_MODE = "fast_img"
    drag.PROCESSING_MODE = "images_only"
    drag.FAST_IMG_SHORTEST_PATH = False
    drag.FAST_IMG_OUTPUT_CLEANED = False
    drag.MEDIA_TOTAL_SIZE = 0

    summary = "\n".join(
        [
            "🔎 Integrity summary",
            "   Recorded source JPEGs:          2",
            "   Optimized JXL files:            1",
            "   Recorded skipped JPEGs:         1",
            "   Recorded failed JPEGs:          0",
            "   Integrity Issues:0",
            "   Integrity:      CLEAN",
            "",
        ]
    )

    def fake_verify(*_args, **_kwargs):
        drag.LAST_VERIFY_SUMMARY = summary
        drag.LAST_VERIFY_WARNINGS = False
        drag.LAST_VERIFY_ISSUE_COUNT = 0
        return True

    monkeypatch.setattr(drag, "run_unified_verification", fake_verify)

    drag.run_fast_img_post_success()

    assert drag.IMG_COUNT == 2
    assert drag.IMG_SUCCEEDED == 1
    assert drag.IMG_SKIPPED == 1
    assert drag.IMG_FAILED == 0


def test_fast_img_post_success_counts_recorded_failed_sources(tmp_path, monkeypatch):
    drag = load_drag_processor(tmp_path, monkeypatch)
    source = tmp_path / "Album"
    optimized = tmp_path / "Album_optimized"
    source.mkdir()
    optimized.mkdir()
    (optimized / "converted.JXL").write_bytes(b"\xff\x0a")
    drag.TARGET_DIR = str(source)
    drag.OUTPUT_DIR = str(optimized)
    drag.OUTPUT_MODE = "fast_img"
    drag.PROCESSING_MODE = "images_only"
    drag.FAST_IMG_SHORTEST_PATH = False
    drag.FAST_IMG_OUTPUT_CLEANED = False
    drag.MEDIA_TOTAL_SIZE = 0

    summary = "\n".join(
        [
            "🔎 Integrity summary",
            "   Recorded source JPEGs:          2",
            "   Optimized JXL files:            1",
            "   Recorded skipped JPEGs:         0",
            "   Recorded failed JPEGs:          1",
            "   Integrity Issues:0",
            "   Integrity:      CLEAN",
            "",
        ]
    )

    def fake_verify(*_args, **_kwargs):
        drag.LAST_VERIFY_SUMMARY = summary
        drag.LAST_VERIFY_WARNINGS = False
        drag.LAST_VERIFY_ISSUE_COUNT = 0
        return True

    monkeypatch.setattr(drag, "run_unified_verification", fake_verify)

    drag.run_fast_img_post_success()

    assert drag.IMG_COUNT == 2
    assert drag.IMG_SUCCEEDED == 1
    assert drag.IMG_SKIPPED == 0
    assert drag.IMG_FAILED == 1


def test_restore_jpeg_stats_parser_reads_rust_djxl_completion_line(
    tmp_path, monkeypatch
):
    drag = load_drag_processor(tmp_path, monkeypatch)
    output = "\n".join(
        [
            "[SCAN    ] Found 169 true JXL files in /tmp/Album_optimized",
            "[DONE    ] restored 169 JPEGs to /tmp/Album_restored_jpeg (3 existing outputs skipped)",
            "",
        ]
    )

    stats = drag.parse_processor_stats(output, parse_type="img", restore_jpeg=True)

    assert stats == (169, 3, 0, 0)


def test_restore_jpeg_verification_uses_restore_mode_flag(tmp_path, monkeypatch):
    drag = load_drag_processor(tmp_path, monkeypatch)
    source = tmp_path / "Album_optimized"
    restored = tmp_path / "Album_restored_jpeg"
    source.mkdir()
    restored.mkdir()
    drag.TARGET_DIR = str(source)
    drag.OUTPUT_DIR = str(restored)
    drag.OUTPUT_MODE = "fast_img"
    drag.PROCESSING_MODE = "images_only"
    drag.LOG_FILE = ""
    drag.VERBOSE_LOG_FILE = ""
    completed = MagicMock(
        returncode=0,
        stdout="\n".join(
            [
                "🔎 Integrity summary",
                "   Integrity Issues:0",
                "   Integrity:      CLEAN",
                "",
            ]
        ),
        stderr="",
    )

    with patch.object(drag.subprocess, "run", return_value=completed) as run_mock:
        assert (
            drag.run_unified_verification(
                include_logs=False,
                auto_mode=True,
                fast_img_restore=True,
            )
            is True
        )

    command = run_mock.call_args[0][0]
    assert "--fast-img-restore" in command
    assert "--fast-img-delivery" not in command


def test_restore_jpeg_post_success_keeps_nonzero_final_image_counts(
    tmp_path, monkeypatch
):
    drag = load_drag_processor(tmp_path, monkeypatch)
    output = tmp_path / "Album_restored"
    output.mkdir()
    (output / "restored.jpg").write_bytes(b"\xff\xd8\xff" + (b"r" * 7))
    drag.OUTPUT_DIR = str(output)
    drag.OUTPUT_MODE = "fast_img"
    drag.PROCESSING_MODE = "images_only"
    drag.IMG_SUCCEEDED = 169
    drag.IMG_SKIPPED = 3
    drag.IMG_IGNORED = 0
    drag.IMG_FAILED = 0
    drag.MEDIA_TOTAL_SIZE = 123
    summary = "\n".join(
        [
            "🔎 Integrity summary",
            "   Source JXL files:                 172",
            "   Restored JPEG files:              169",
            "   Integrity Issues:0",
            "   Integrity:      CLEAN",
            "",
        ]
    )

    def fake_verify(*_args, **_kwargs):
        drag.LAST_VERIFY_SUMMARY = summary
        drag.LAST_VERIFY_WARNINGS = False
        drag.LAST_VERIFY_ISSUE_COUNT = 0
        return True

    monkeypatch.setattr(drag, "run_unified_verification", fake_verify)

    drag.run_fast_img_restore_post_success()

    assert drag.IMG_COUNT == 172
    assert drag.IMG_SUCCEEDED == 169
    assert drag.IMG_SKIPPED == 3
    assert drag.IMG_FAILED == 0
    assert drag.VID_COUNT == 0
    assert drag.MEDIA_TOTAL_SIZE == 123
    assert drag.SIZE_SUMMARY_AFTER_OVERRIDE == (output / "restored.jpg").stat().st_size


def test_restore_jpeg_integrity_counts_include_manifest_deleted_sources(
    tmp_path, monkeypatch
):
    drag = load_drag_processor(tmp_path, monkeypatch)
    summary = "\n".join(
        [
            "Integrity summary",
            "   Source JXL files:           172",
            "   Source remaining JXL files: 3",
            "   Manifest verified deleted source JXLs: 169",
            "   Restored JPEG files:        172",
            "",
        ]
    )

    assert drag.fast_img_restore_integrity_counts(summary) == (172, 172, 3, 169)


def test_integrity_warning_counts_as_failure_even_with_zero_successes(
    tmp_path, monkeypatch
):
    drag = load_drag_processor(tmp_path, monkeypatch)

    effective_s, effective_f, penalty = drag.effective_success_failure_counts(
        total_success=0,
        total_failed=0,
        verify_warnings=True,
        verify_issue_count=2,
    )

    assert effective_s == 0
    assert effective_f == 2
    assert penalty == 2
