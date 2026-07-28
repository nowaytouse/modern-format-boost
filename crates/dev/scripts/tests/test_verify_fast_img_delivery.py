import io
import json
import sys
import unittest
from pathlib import Path

SCRIPT_DIR = Path(__file__).resolve().parents[1]
if str(SCRIPT_DIR) not in sys.path:
    sys.path.insert(0, str(SCRIPT_DIR))

if not (SCRIPT_DIR / "verify.py").is_file():
    raise unittest.SkipTest("legacy Python verifier was replaced by the Rust binary")

import media_scope
import verify


def _hex_text(text: str) -> str:
    return text.encode("utf-8").hex()


def write_fast_img_marker(
    state_root: Path,
    source: Path,
    optimized: Path,
    *,
    src_jpeg_count: int,
    transcoded_count: int | None = None,
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
                "stage": "cleanup_complete",
                "src_jpeg_count": src_jpeg_count,
                "transcoded_count": (
                    src_jpeg_count if transcoded_count is None else transcoded_count
                ),
                "blake3_log": {},
                "skipped_sources": skipped_sources or {},
                "failed_sources": failed_sources or {},
                "error": None,
            }
        ),
        encoding="utf-8",
    )


def test_fast_img_delivery_check_accepts_deleted_sources_and_jxl_only_output(
    tmp_path, monkeypatch
):
    source = tmp_path / "Album"
    optimized = tmp_path / "Album_optimized"
    state_root = tmp_path / "mfb_state"
    monkeypatch.setenv("MFB_HOME_ROOT", str(state_root))
    (source / "day1").mkdir(parents=True)
    (optimized / "day1").mkdir(parents=True)
    (optimized / "day1" / "photo.JXL").write_bytes(b"\xff\x0a")
    write_fast_img_marker(state_root, source, optimized, src_jpeg_count=1)

    stats = verify.run_fast_img_delivery_check(
        source,
        optimized,
        io.StringIO(),
        processing_mode="images_only",
    )

    assert stats["source_files_label"] == "Recorded source JPEGs"
    assert stats["source_count_source"] == "fast_img_marker"
    assert stats["optimized_files_label"] == "Optimized JXL files"
    assert stats["source_files"] == 1
    assert stats["source_remaining_files"] == 0
    assert stats["optimized_files"] == 1
    assert stats["count_delta"] == 0
    assert stats["count_status_label"] == "FAST_IMG_JXL_ONLY_DELIVERY"
    assert stats["integrity_failures"] == 0
    assert stats["has_warnings"] is False


def test_fast_img_delivery_accepts_recorded_skipped_sources_remaining(
    tmp_path, monkeypatch
):
    source = tmp_path / "Album"
    optimized = tmp_path / "Album_optimized"
    state_root = tmp_path / "mfb_state"
    monkeypatch.setenv("MFB_HOME_ROOT", str(state_root))
    source.mkdir()
    optimized.mkdir()
    skipped = source / "skipped.bin"
    skipped.write_bytes(b"\xff\xd8\xff\xe0jpeg")
    (optimized / "photo.JXL").write_bytes(b"\xff\x0a")
    write_fast_img_marker(
        state_root,
        source,
        optimized,
        src_jpeg_count=2,
        transcoded_count=1,
        skipped_sources={
            "skipped.bin": {
                "src": "recorded-source-blake3",
                "reason": "lossless JPEG transcode failed after strict cascade",
            }
        },
    )

    stats = verify.run_fast_img_delivery_check(
        source,
        optimized,
        io.StringIO(),
        processing_mode="images_only",
    )

    assert stats["source_files"] == 2
    assert stats["optimized_files"] == 1
    assert stats["source_remaining_files"] == 1
    assert stats["skipped_sources"] == 1
    assert stats["count_delta"] == 0
    assert stats["explained_gaps"] == 1
    assert stats["count_status_label"] == "FAST_IMG_JXL_ONLY_DELIVERY"
    assert stats["integrity_failures"] == 0
    assert stats["has_warnings"] is False


def test_fast_img_delivery_accepts_all_sources_skipped_with_no_jxl_outputs(
    tmp_path, monkeypatch
):
    source = tmp_path / "Album"
    optimized = tmp_path / "Album_optimized"
    state_root = tmp_path / "mfb_state"
    monkeypatch.setenv("MFB_HOME_ROOT", str(state_root))
    source.mkdir()
    optimized.mkdir()
    skipped = source / "missing_eoi.bin"
    skipped.write_bytes(b"\xff\xd8\xff\xe0jpeg")
    write_fast_img_marker(
        state_root,
        source,
        optimized,
        src_jpeg_count=1,
        transcoded_count=0,
        skipped_sources={
            "missing_eoi.bin": {
                "src": "recorded-source-blake3",
                "reason": "Skipped: JPEG cannot be reversibly transcoded; source remains unmodified",
            }
        },
    )

    report = io.StringIO()
    stats = verify.run_fast_img_delivery_check(
        source,
        optimized,
        report,
        processing_mode="images_only",
    )

    assert stats["source_files"] == 1
    assert stats["optimized_files"] == 0
    assert stats["source_remaining_files"] == 1
    assert stats["skipped_sources"] == 1
    assert stats["count_delta"] == 0
    assert stats["explained_gaps"] == 1
    assert stats["count_status_label"] == "FAST_IMG_JXL_ONLY_DELIVERY"
    assert stats["integrity_failures"] == 0
    assert stats["has_warnings"] is False
    assert "Expected optimized JXLs:     0" in report.getvalue()


def test_fast_img_delivery_accepts_recorded_failed_sources_remaining(
    tmp_path, monkeypatch
):
    source = tmp_path / "Album"
    optimized = tmp_path / "Album_optimized"
    state_root = tmp_path / "mfb_state"
    monkeypatch.setenv("MFB_HOME_ROOT", str(state_root))
    source.mkdir()
    optimized.mkdir()
    failed = source / "djxl_failed.jpg"
    failed.write_bytes(b"\xff\xd8\xff\xe0jpeg")
    (optimized / "converted.JXL").write_bytes(b"\xff\x0a")
    write_fast_img_marker(
        state_root,
        source,
        optimized,
        src_jpeg_count=2,
        transcoded_count=1,
        failed_sources={
            "djxl_failed.jpg": {
                "src": "failed-source-blake3",
                "reason": "pixel-diff: djxl exited non-zero decoding output.JXL",
            }
        },
    )

    report = io.StringIO()
    stats = verify.run_fast_img_delivery_check(
        source,
        optimized,
        report,
        processing_mode="images_only",
    )

    assert stats["source_files"] == 2
    assert stats["optimized_files"] == 1
    assert stats["source_remaining_files"] == 1
    assert stats["failed_sources"] == 1
    assert stats["count_delta"] == 0
    assert stats["explained_gaps"] == 1
    assert stats["count_status_label"] == "FAST_IMG_JXL_ONLY_DELIVERY"
    assert stats["integrity_failures"] == 0
    assert "Recorded failed JPEGs:       1" in report.getvalue()


def test_fast_img_delivery_check_rejects_missing_marker_proof(tmp_path, monkeypatch):
    source = tmp_path / "Album"
    optimized = tmp_path / "Album_optimized"
    monkeypatch.setenv("MFB_HOME_ROOT", str(tmp_path / "mfb_state"))
    source.mkdir()
    optimized.mkdir()
    (optimized / "photo.JXL").write_bytes(b"\xff\x0a")
    report = io.StringIO()

    stats = verify.run_fast_img_delivery_check(
        source,
        optimized,
        report,
        processing_mode="images_only",
    )

    assert stats["source_count_source"] == "missing_fast_img_marker"
    assert stats["source_files"] == 0
    assert stats["count_status_label"] is None
    assert stats["integrity_failures"] == 1
    assert stats["has_warnings"] is True
    assert "fast-img marker missing" in report.getvalue()


def test_fast_img_delivery_check_rejects_remaining_true_jpeg_and_non_jxl_output(
    tmp_path, monkeypatch
):
    source = tmp_path / "Album"
    optimized = tmp_path / "Album_optimized"
    state_root = tmp_path / "mfb_state"
    monkeypatch.setenv("MFB_HOME_ROOT", str(state_root))
    source.mkdir()
    optimized.mkdir()
    (source / "remaining.bin").write_bytes(b"\xff\xd8\xff\xe0jpeg")
    (optimized / "photo.jpg").write_bytes(b"not-jxl")
    write_fast_img_marker(state_root, source, optimized, src_jpeg_count=1)

    stats = verify.run_fast_img_delivery_check(
        source,
        optimized,
        io.StringIO(),
        processing_mode="images_only",
    )

    assert stats["source_files"] == 1
    assert stats["source_remaining_files"] == 1
    assert stats["optimized_files"] == 0
    assert stats["count_delta"] == -1
    assert stats["count_matches_with_handoff"] is False
    assert stats["count_status_label"] is None
    assert stats["extra"] == 1
    assert stats["integrity_failures"] == 4
    assert stats["has_warnings"] is True


def test_fast_img_jpeg_probe_matches_rust_magic_detector(tmp_path):
    true_jpeg_without_jpg_ext = tmp_path / "camera.bin"
    png_with_jpg_ext = tmp_path / "not-a-jpeg.jpg"
    truncated = tmp_path / "truncated.jpg"
    true_jpeg_without_jpg_ext.write_bytes(b"\xff\xd8\xff\xe1jpeg")
    png_with_jpg_ext.write_bytes(b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR")
    truncated.write_bytes(b"\xff\xd8")

    assert verify.detect_true_format(true_jpeg_without_jpg_ext) == "jpeg"
    assert verify.is_true_jpeg_file(true_jpeg_without_jpg_ext) is True
    assert verify.detect_true_format(png_with_jpg_ext) == "png"
    assert verify.is_true_jpeg_file(png_with_jpg_ext) is False
    assert verify.detect_true_format(truncated) == "unknown"
    assert verify.is_true_jpeg_file(truncated) is False


def test_fast_img_jpeg_probe_surfaces_io_errors(tmp_path):
    missing = tmp_path / "missing.jpg"

    try:
        verify.is_true_jpeg_file(missing)
    except OSError as exc:
        assert str(missing) in str(exc)
    else:
        raise AssertionError("missing JPEG probe unexpectedly returned a silent false")


def test_integrity_collection_uses_true_format_not_spoofed_extension(tmp_path):
    source = tmp_path / "Album"
    source.mkdir()
    disguised_jpeg = source / "camera.bin"
    fake_jpeg = source / "fake.jpg"
    disguised_jpeg.write_bytes(b"\xff\xd8\xff\xe0jpeg")
    fake_jpeg.write_bytes(b"not an image")

    assert verify.is_media_file(disguised_jpeg) is True
    assert verify.is_media_file(fake_jpeg) is False

    collected = verify.collect_media_files(source, processing_mode="images_only")

    assert "camera" in collected
    assert collected["camera"] == [disguised_jpeg]
    assert "fake" not in collected


def test_fast_img_delivery_rejects_spoofed_jxl_extension(tmp_path, monkeypatch):
    source = tmp_path / "Album"
    optimized = tmp_path / "Album_optimized"
    state_root = tmp_path / "mfb_state"
    monkeypatch.setenv("MFB_HOME_ROOT", str(state_root))
    source.mkdir()
    optimized.mkdir()
    (optimized / "photo.JXL").write_bytes(b"not a jpeg xl codestream")
    write_fast_img_marker(state_root, source, optimized, src_jpeg_count=1)

    report = io.StringIO()
    stats = verify.run_fast_img_delivery_check(
        source,
        optimized,
        report,
        processing_mode="images_only",
    )

    assert stats["optimized_files"] == 0
    assert stats["extra"] == 1
    assert stats["integrity_failures"] == 3
    assert stats["has_warnings"] is True
    assert "Non-JXL optimized files" in report.getvalue()


def test_fast_img_delivery_records_media_probe_errors(tmp_path, monkeypatch):
    source = tmp_path / "Album"
    optimized = tmp_path / "Album_optimized"
    state_root = tmp_path / "mfb_state"
    monkeypatch.setenv("MFB_HOME_ROOT", str(state_root))
    source.mkdir()
    optimized.mkdir()
    source_bad = source / "bad-source.jpg"
    optimized_bad = optimized / "bad-output.jxl"
    source_bad.write_bytes(b"\xff\xd8\xff\xe0bad")
    optimized_bad.write_bytes(b"\xff\x0abad")
    write_fast_img_marker(state_root, source, optimized, src_jpeg_count=1)

    def fail_probe(path: Path) -> str:
        raise media_scope.MediaProbeError(f"forced probe failure for {path}")

    monkeypatch.setattr(verify, "detect_true_format", fail_probe)

    report = io.StringIO()
    stats = verify.run_fast_img_delivery_check(
        source,
        optimized,
        report,
        processing_mode="images_only",
    )

    assert stats["source_probe_errors"] == 1
    assert stats["optimized_probe_errors"] == 1
    assert stats["integrity_failures"] >= 2
    assert stats["count_status_label"] is None
    assert "Source format probe errors" in report.getvalue()
    assert "Optimized format probe errors" in report.getvalue()


def test_fast_img_restore_check_accepts_jxl_to_jpeg_roundtrip(tmp_path):
    source = tmp_path / "Album_optimized"
    restored = tmp_path / "Album_restored_jpeg"
    (source / "nested").mkdir(parents=True)
    (restored / "nested").mkdir(parents=True)
    (source / "nested" / "camera.JXL").write_bytes(b"\xff\x0atrue-jxl")
    (restored / "nested" / "camera.jpeg").write_bytes(b"\xff\xd8\xff\xe0true-jpeg")

    report = io.StringIO()
    stats = verify.run_fast_img_restore_check(source, restored, report)

    assert stats["source_files_label"] == "Source JXL files"
    assert stats["optimized_files_label"] == "Restored JPEG files"
    assert stats["source_files"] == 1
    assert stats["optimized_files"] == 1
    assert stats["matched"] == 1
    assert stats["count_delta"] == 0
    assert stats["count_status_label"] == "FAST_IMG_JPEG_RESTORE"
    assert stats["integrity_failures"] == 0
    assert stats["has_warnings"] is False
    assert "Restore mode:   JXL -> JPEG via djxl" in report.getvalue()


def test_fast_img_restore_check_accepts_manifest_verified_deleted_sources(tmp_path):
    source = tmp_path / "Album_optimized"
    restored = tmp_path / "Album_restored_jpeg"
    source.mkdir()
    restored.mkdir()
    (restored / "nested").mkdir()
    (restored / "nested" / "camera.jpg").write_bytes(b"\xff\xd8\xff\xe0true-jpeg")
    (restored / ".mfb_restore_jpeg_manifest.tsv").write_text(
        "\n".join(
            [
                "# MFB_RESTORE_JPEG_MANIFEST_V1",
                "source_rel_hex\toutput_rel_hex\tsource_blake3\toutput_blake3\tsource_deleted",
                f"{_hex_text('nested/camera.JXL')}\t{_hex_text('nested/camera.jpg')}\tsource-blake3\toutput-blake3\ttrue",
                "",
            ]
        ),
        encoding="utf-8",
    )

    report = io.StringIO()
    stats = verify.run_fast_img_restore_check(source, restored, report)

    assert stats["source_files"] == 1
    assert stats["source_remaining_files"] == 0
    assert stats["verified_deleted_sources"] == 1
    assert stats["optimized_files"] == 1
    assert stats["matched"] == 1
    assert stats["count_delta"] == 0
    assert stats["count_status_label"] == "FAST_IMG_JPEG_RESTORE"
    assert stats["integrity_failures"] == 0
    assert stats["has_warnings"] is False
    assert "Manifest verified deleted source JXLs: 1" in report.getvalue()


def test_fast_img_restore_check_rejects_manifest_claim_when_source_still_exists(
    tmp_path,
):
    source = tmp_path / "Album_optimized"
    restored = tmp_path / "Album_restored_jpeg"
    (source / "nested").mkdir(parents=True)
    (restored / "nested").mkdir(parents=True)
    (source / "nested" / "camera.JXL").write_bytes(b"\xff\x0atrue-jxl")
    (restored / "nested" / "camera.jpg").write_bytes(b"\xff\xd8\xff\xe0true-jpeg")
    (restored / ".mfb_restore_jpeg_manifest.tsv").write_text(
        "\n".join(
            [
                "# MFB_RESTORE_JPEG_MANIFEST_V1",
                "source_rel_hex\toutput_rel_hex\tsource_blake3\toutput_blake3\tsource_deleted",
                f"{_hex_text('nested/camera.JXL')}\t{_hex_text('nested/camera.jpg')}\tsource-blake3\toutput-blake3\ttrue",
                "",
            ]
        ),
        encoding="utf-8",
    )

    report = io.StringIO()
    stats = verify.run_fast_img_restore_check(source, restored, report)

    assert stats["source_files"] == 1
    assert stats["source_remaining_files"] == 1
    assert stats["verified_deleted_sources"] == 0
    assert stats["integrity_failures"] >= 1
    assert stats["count_status_label"] is None
    assert stats["has_warnings"] is True
    assert "Restore manifest errors" in report.getvalue()


def test_fast_img_restore_check_rejects_manifest_deleted_source_with_xmp_leftover(
    tmp_path,
):
    source = tmp_path / "Album_optimized"
    restored = tmp_path / "Album_restored_jpeg"
    (source / "nested").mkdir(parents=True)
    (restored / "nested").mkdir(parents=True)
    (source / "nested" / "camera.JXL.xmp").write_text("<x:xmpmeta/>", encoding="utf-8")
    (restored / "nested" / "camera.jpg").write_bytes(b"\xff\xd8\xff\xe0true-jpeg")
    (restored / ".mfb_restore_jpeg_manifest.tsv").write_text(
        "\n".join(
            [
                "# MFB_RESTORE_JPEG_MANIFEST_V1",
                "source_rel_hex\toutput_rel_hex\tsource_blake3\toutput_blake3\tsource_deleted",
                f"{_hex_text('nested/camera.JXL')}\t{_hex_text('nested/camera.jpg')}\tsource-blake3\toutput-blake3\ttrue",
                "",
            ]
        ),
        encoding="utf-8",
    )

    report = io.StringIO()
    stats = verify.run_fast_img_restore_check(source, restored, report)

    assert stats["verified_deleted_sources"] == 0
    assert stats["restore_manifest_errors"] == 1
    assert stats["integrity_failures"] >= 1
    assert stats["count_status_label"] is None
    assert "manifest deleted source left XMP sidecar" in report.getvalue()


def test_fast_img_restore_check_rejects_duplicate_manifest_deleted_source(tmp_path):
    source = tmp_path / "Album_optimized"
    restored = tmp_path / "Album_restored_jpeg"
    source.mkdir()
    restored.mkdir()
    (restored / "camera.jpg").write_bytes(b"\xff\xd8\xff\xe0true-jpeg")
    row = (
        f"{_hex_text('camera.JXL')}\t{_hex_text('camera.jpg')}"
        "\tsource-blake3\toutput-blake3\ttrue"
    )
    (restored / ".mfb_restore_jpeg_manifest.tsv").write_text(
        "\n".join(
            [
                "# MFB_RESTORE_JPEG_MANIFEST_V1",
                "source_rel_hex\toutput_rel_hex\tsource_blake3\toutput_blake3\tsource_deleted",
                row,
                row,
                "",
            ]
        ),
        encoding="utf-8",
    )

    report = io.StringIO()
    stats = verify.run_fast_img_restore_check(source, restored, report)

    assert stats["verified_deleted_sources"] == 1
    assert stats["restore_manifest_errors"] == 1
    assert stats["integrity_failures"] >= 1
    assert stats["count_status_label"] is None
    assert "duplicate manifest source key" in report.getvalue()


def test_fast_img_restore_check_rejects_missing_or_non_jpeg_outputs(tmp_path):
    source = tmp_path / "Album_optimized"
    restored = tmp_path / "Album_restored_jpeg"
    source.mkdir()
    restored.mkdir()
    (source / "missing.JXL").write_bytes(b"\xff\x0atrue-jxl")
    (source / "wrong.JXL").write_bytes(b"\xff\x0atrue-jxl")
    (restored / "wrong.png").write_bytes(b"\x89PNG\r\n\x1a\nnot-jpeg")

    report = io.StringIO()
    stats = verify.run_fast_img_restore_check(source, restored, report)

    assert stats["source_files"] == 2
    assert stats["optimized_files"] == 0
    assert stats["missing"] == 2
    assert stats["mismatched_types"] == 1
    assert stats["integrity_failures"] >= 3
    assert stats["count_status_label"] is None
    assert stats["has_warnings"] is True
    assert "Missing restored JPEG outputs" in report.getvalue()
    assert "Non-JPEG restored outputs" in report.getvalue()


def test_media_scope_routes_disguised_animated_formats_by_content(tmp_path):
    webp = tmp_path / "animated-webp.bin"
    apng = tmp_path / "animated-png.bin"
    still_jpeg = tmp_path / "still-jpeg.bin"
    fake_jpg = tmp_path / "fake.jpg"
    webp.write_bytes(b"RIFF\x18\x00\x00\x00WEBPVP8XANIM")
    apng.write_bytes(
        b"\x89PNG\r\n\x1a\n"
        b"\x00\x00\x00\rIHDR"
        b"\x00\x00\x00\x01\x00\x00\x00\x01\x08\x06\x00\x00\x00"
        b"\x00\x00\x00\x00"
        b"\x00\x00\x00\x08acTL"
    )
    still_jpeg.write_bytes(b"\xff\xd8\xff\xe0jpeg")
    fake_jpg.write_bytes(b"not media")

    assert media_scope.classify_media_owner(webp) == media_scope.PIPELINE_VIDEO
    assert media_scope.classify_media_owner(apng) == media_scope.PIPELINE_VIDEO
    assert media_scope.classify_media_owner(still_jpeg) == media_scope.PIPELINE_IMAGE
    assert media_scope.classify_media_owner(fake_jpg) is None


def test_media_scope_classification_surfaces_missing_file_probe_errors(tmp_path):
    missing = tmp_path / "missing.gif"

    try:
        media_scope.classify_media_owner(missing)
    except OSError as exc:
        assert str(missing) in str(exc)
    else:
        raise AssertionError("missing media classification silently returned None")


def test_media_scope_rejects_malformed_animated_containers(tmp_path):
    malformed_gif = tmp_path / "truncated-gif.bin"
    malformed_apng = tmp_path / "truncated-apng.bin"
    malformed_gif.write_bytes(b"GIF89a\x01\x00")
    malformed_apng.write_bytes(b"\x89PNG\r\n\x1a\n")

    for path in (malformed_gif, malformed_apng):
        try:
            media_scope.classify_media_owner(path)
        except media_scope.MediaProbeError as exc:
            assert str(path) in str(exc)
        else:
            raise AssertionError(
                f"malformed container was routed as valid media: {path}"
            )
