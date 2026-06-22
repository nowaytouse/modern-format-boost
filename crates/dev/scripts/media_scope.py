"""
Shared media-scope rules for Python orchestration (drag-and-drop, verify).

Single source of truth for:
- Animation detection (WebP / GIF / PNG-APNG) aligned with Rust preflight heuristics
- Pipeline routing (image = static img tool, video = vid / animated handoff)
- Integrity gap classification (true missing vs pipeline handoff)

Rust counterparts (for cross-reference when reading logs):
- img: ``fast_static_skip_or_ignore``, ``media_conversion_gate::animation_reject_reason`` → IGNORE
- vid: static / single-frame → ``video_ignored``; animated WebP → conversion or fail
- Probe (M15): ``probe_layer_audit`` / ``recovery_*`` / ``color_depth_or_baseline`` → ``[delivery fallback:…]`` in logs
- Substrate (M16): ``conversion`` / ``media_penetration`` / ``vid`` ingest·DB-health / explore batch audits → same tag
- Tooling (M17): ``process_runner`` / ``progress`` / ``msssim_progress`` / ``ffmpeg_process`` / ``explore_strategy`` → same tag
- Quality intel (M18): ``analysis_cache`` / ``quality_matcher`` / ``*_quality_detector`` → same tag
- FFprobe/HDR/explore aux (M19): ``ffprobe`` / ``ffprobe_json`` / ``hdr`` / explore precheck·SSIM → same tag
- GPU coarse explore (M20): ``gpu_coarse_search`` / explore iteration overflow → same tag
- Resume/DB/detection (M21): ``checkpoint`` / ``image_detection`` / ``database`` → same tag
- Image analysis/pipeline (M22): ``image_jpeg_analysis`` / ``image_heic_analysis`` / ``image_formats`` / ``batch`` / ``cli_runner`` → same tag
- Metadata/JXL/XMP (M23): ``metadata`` / ``jxl_utils`` / ``xmp_merger`` → same tag
- Substrate ext (M24): ``loop_intent`` / ``gpu_accel`` / ``io_utils`` / ``file_copier`` / ``stream_size`` / ``x265_encoder`` → same tag
- Quality/runtime (M25): ``msssim_parallel`` / ``msssim_sampling`` / ``float_compare`` / ``crf_constants`` / ``media_meta_utils`` / ``common_utils`` / ``error_handler`` → same tag
- Infra/numeric (M26): ``numeric_cast`` / ``system_memory`` / ``modern_ui`` / ``progress_mode`` / ``path_validator`` / ``date_analysis`` / ``smart_file_copier`` / ``ctrlc_guard`` / ``lru_cache`` / ``safety`` / ``path_safety`` / ``image_metrics`` / ``x265_params`` / ``file_sorter`` → same tag
- Session logging (M27): ``logging`` log rotation / retention → same tag
- Unwrap-or routing (M28): ``file_copier`` / ``smart_file_copier`` / ``loop_intent`` metadata ext / ``lru_cache`` clock / ``ctrlc_guard`` / ``error_handler`` → gate helpers (no silent ext/strip defaults)
- Resume/GPU/CLI/DB unwrap-or (M29): ``checkpoint`` / ``gpu_accel`` / ``cli_runner`` / ``database`` ingest paths → ``unix_epoch_*`` / ``gpu_*`` / ``pipeline_outcome_reason`` / ``db_labeled_by_or_default``
- Metadata/encode unwrap-or (M30): ``xmp_merger`` / ``metadata`` / ``x265_encoder`` / ``hdr`` / ``jxl_utils`` / ``media_penetration`` → ``path_stem_root_segment`` / ``encode_stderr_last_line_or_unknown`` / ``dv_profile8_compat_id_or_default``
- JPEG/explore unwrap-or (M31): ``image_jpeg_analysis`` / ``image_detection`` / ``explore_strategy`` / ``video_explorer`` / ``precheck`` → ``probe_jpeg_*`` / ``explore_metric_numeric_end`` / ``probe_ffprobe_codec_name_lowercase``
- GPU/SSIM explore unwrap-or (M32): ``gpu_coarse_search`` / ``ssim_calculator`` / ``stream_analysis`` / ``hdr`` ICC → ``explore_metric_numeric_end`` / ``x265_params_segment_or_empty`` / ``explore_best_crf_or_backtrack_anchor`` / ``probe_buffer_prefix_or_empty``
- Probe/quality intel unwrap-or (M33): ``common_utils`` / ``ffprobe`` / ``animated_image_quality_features`` / ``video_quality_features`` / ``media_penetration`` / ``video_quality_detector`` / ``depth_channel`` / ``scenario_quality_lookup`` → ``probe_stdout_first_token`` / ``probe_optional_f64_or_zero`` / ``animated_delay_variation_or_default``
- DB/precision/loop probe unwrap-or (M34): ``database`` / ``multi_scenario_db`` / ``media_precision`` / ``loop_intent`` lavfi → ``db_physics_embedding_or_empty`` / ``db_optional_bool_or_false`` / ``utf8_suffix_or_empty`` / ``path_extension_lowercase_or_empty_unchecked``
- Quality embed/UI/GPU unwrap-or (M35): ``image_quality_db`` / ``database`` stats / ``modern_ui`` / ``unified_progress`` / ``gpu_accel`` VAAPI / ``metadata`` rel parent → ``db_numeric_stats_triple_or_zero`` / ``ui_spinner_glyph_at`` / ``probe_optional_f64_or_zero`` / ``gpu_vaapi_device_path_or_default``
- Runtime/CLI/tooling/KNN unwrap-or (M36): ``progress_mode`` / ``logging`` / ``cli_runner`` / ``tool_builders`` / ``date_analysis`` / ``database_vector`` / ``database`` distance quantiles / ``gpu_accel`` PSNR map → ``delivery_run_logs_dir_or_dot`` / ``delivery_disk_check_path_or_input`` / ``delivery_tool_executable_or_default`` / ``db_sorted_distance_at``
- Loop intent / inference JSON / numeric sort (M37): ``loop_intent`` / ``database`` signal snapshot / ``gpu_accel`` calibration → ``loop_reference_profile_or_default`` / ``loop_optional_secs_or_baseline`` / ``json_finite_f64_or_null`` / ``f64_sort_cmp``
- Inference snapshot / env / metadata I/O (M38): ``build_signal_snapshot`` / ``algorithm_runtime`` / ``metadata`` timestamp restore → ``json_optional_*_or_null`` / ``algorithm_env_usize_or_default`` / ``io_error_or_metadata_label``
- Delivery seal (M39): contract M1–M39 + ``MEDIA_CONVERSION_DELIVERY_SEAL.md`` + heatmap baseline; production numeric-forgery scan; sole ``log_anomaly!`` in ``media_conversion_gate::delivery_fallback_audit``
- Blind-spot guards (M40): ``quality_matcher`` chroma / ``gpu_accel`` encode improvement % / ``animated_image_quality_features`` compression estimate → ``probe_chroma_factor_or_default`` / ``explore_encode_size_improvement_pct`` / ``probe_compression_ratio_or_estimate``
- Explore/JXL substrate (M41): ``gpu_coarse_search`` ultimate sample rate / ``video_explorer`` size·elapsed / ``dynamic_mapping`` offset / ``jxl_explorer`` telemetry → ``explore_ultimate_gate_sample_rate`` / ``explore_latest_encoded_size_or_zero`` / ``explore_elapsed_secs_or_zero`` / ``explore_dynamic_mapping_offset_or_zero`` / ``jxl_best_telemetry_or_zero`` / ``jxl_screened_output_size_or_max``
- Progress/loop guards (M42): ``progress`` explore mutex fields / ``loop_intent`` duration ramps → ``progress_explore_*_or_zero`` / ``loop_missing_duration_z_neutral`` / ``loop_*_proximity_ramp_*`` / ``loop_baseline_median_frames_or_zero``
- Final allowlist clearance (M43): ``ctrlc_guard`` / ``media_meta_utils`` GIF GCT / ``gpu_accel`` compression potential → ``runtime_elapsed_secs_or_zero`` / ``gif_palette_byte_size_or_zero`` / ``gpu_compression_potential_adjustment_or_zero``; dev ``ALLOWLIST`` empty
- Session logging mutex (M44): ``logging`` / ``progress_mode`` run-log mutexes → ``logging_mutex_guard_or_recover``; invalid ``RUST_LOG`` → ``delivery_logging_path_audit``; ``modern_ui`` tracing file path → ``path_parent_or_dot`` / ``path_tracing_log_file_name_or_app_log`` / ``ui_result_box_width_or_title_default`` (U11)
- Log dir + path safety (M45): ``LogConfig`` ``MFB_LOG_DIR`` → ``delivery_log_dir_from_env_or_temp``; ``path_safety`` ImageMagick/search temp → ``path_magick_relativized_lossy`` / ``path_search_temp_*``; ``progress`` coarse message / ``c_api`` ingest mutex → ``delivery_progress_mutex_string_or_empty`` / ``mutex_guard_or_recover``
- Progress + log detail (M46): ``progress`` active line / explore status mutex → ``mutex_guard_or_recover`` / ``progress_explore_optional_f64_or_none`` / ``ui_f64_display_or_placeholder``; ``static_logs`` path detail → ``delivery_log_detail_with_optional_path``; ``unified_progress`` SSIM → ``ui_ssim_inline_or_na``
- Explore metric display (M47): ``video_explorer`` / ``gpu_coarse_search`` / ``image_analyzer`` heuristic logs → ``ui_f64_or_na`` / ``ui_f64_pair_*`` / ``ui_optional_f64_display_or_map`` (no inline ``N/A`` placeholders)
- Quality-intel metric display (M48): ``image_quality_detector`` / ``video_quality_detector`` / ``image_jpeg_analysis`` / ``quality_verifier_enhanced`` → ``ui_f64_or_na`` / ``ui_optional_u32_or_na`` / ``ui_optional_u64_or_na`` / ``ui_f64_percent_or_na`` / ``ui_duration_secs_label_or_na``
- Confidence + terminal (M49): ``quality_matcher`` / ``image_detection`` / ``media_precision`` / ``ctrlc_guard`` → ``ui_confidence_pct_whole_or_na`` / ``ui_bit_depth_format_label_or_na`` / ``mutex_guard_or_recover`` (no inline ``|| \"N/A\".to_string()`` on CRF/confidence errors)
- Probe stderr (M50): ``image_analyzer`` / ``image_detection`` → ``probe_imagemagick_animation_detected_audit`` / ``ui_penetration_warning_stderr``
- Quality errors + report headers (M51): ``video_quality_detector`` / ``image_quality_detector`` → ``ui_quality_user_error`` / ``ui_visual_artifact_audit_title``
- Infra user errors (M52): ``path_validator`` / ``multi_scenario_db`` / ``cli_runner`` / ``flag_validator`` / ``stream_size`` / ``pure_media_verifier`` / ``video_explorer`` → ``ui_user_facing_error`` / ``ui_log_summary_title_with_icon``
- Core error surfaces (M53): ``unified_error`` / ``app_error`` / ``quality_matcher`` → ``ui_user_facing_error`` / ``ui_user_facing_warning``
- Safety + explore CRF icons (M54): ``safety`` / ``video_explorer`` CRF bisect → ``ui_safety_*_blocked`` / ``ui_explore_crf_*_mark`` (no raw emoji / inline ``symbols::pick`` in safety blocks)
- Static log severity banners (M55): ``static_logs::ErrorSeverity::label_colored`` → ``ui_error_severity_colored_label`` (no inline ``symbols::pick`` in severity match)
- Static logs icon picks (M56): ``static_logs`` macros/outcome logs → ``ui_icon_pick`` (no ``symbols::pick`` in module)
- Video explore stderr icons (M57): ``video_explorer`` → ``ui_icon_pick`` / ``ui_explore_crf_*_mark`` (no ``modern_ui::symbols::pick`` in module)
- GPU coarse search icons (M58): ``video_explorer/gpu_coarse_search`` → ``ui_icon_pick`` (no ``symbols::pick`` in module)
- Explore strategy icons (M59): ``explore_strategy`` → ``ui_icon_pick``
- FFmpeg/MS-SSIM icons (M60): ``ffmpeg_process`` / ``msssim_progress`` / ``msssim_parallel`` → ``ui_icon_pick``
- Progress mode icons (M61): ``progress_mode`` / ``jxl_utils`` stderr → ``ui_icon_pick``
- Progress + logging icons (M62): ``progress.rs`` / ``logging.rs`` → ``ui_icon_pick``
- Batch report icons (M63): ``report.rs`` → ``ui_icon_pick``
- Quality/DB audit icons (M64): ``quality_matcher`` HINT + ``database.rs`` audit → ``ui_icon_pick``
- Delivery I/O icons (M65): ``file_copier`` / ``image_analyzer`` / ``video_detection`` / ``cli_runner`` / ``stream_size`` → ``ui_icon_pick``
- GPU accel icons (M66): ``gpu_accel`` → ``ui_icon_pick``
- Precision preservation policy (M67): ``media_precision`` / ``lossless_converter`` / ``gpu_coarse_search`` / ``animated_image`` honor real sample depth and routed precision helpers; no silent hardcoded ``yuv420p*`` / ``rgb48le`` fallbacks in production encode paths
Python session + tracing audit:
- Session log: ``ROUTED pipeline=…`` and ``target: mfb::audit`` run logs
- Handoff preserve: **never silent**; **only** after img/vid (`phase=post_img_vid`) — scan → ``y``/``n`` → audit
- Rust log files emit canonical ``MFB_AUDIT k=v`` keys (stable order, ``mfb_audit_schema=1``;
  optional ``ignore_class`` for vid static-ignore reconciliation)
- ``preserve_handoff_gaps()`` refuses wrong ``phase`` (no copy)
"""

from __future__ import annotations

import os
import re
import shutil
import subprocess
import unicodedata
from collections.abc import Callable
from dataclasses import dataclass
from pathlib import Path


class MediaProbeError(RuntimeError):
    """Raised when content bytes identify media but parsing cannot prove routing."""


# ── Extension sets (verify + drag-and-drop) ───────────────────────────────────

IMG_EXTS = {
    ".arw",
    ".bmp",
    ".cr2",
    ".cr3",
    ".crw",
    ".cur",
    ".dng",
    ".erf",
    ".gif",
    ".heic",
    ".heif",
    ".hif",
    ".ico",
    ".j2k",
    ".jpg",
    ".jpeg",
    ".jpe",
    ".jfif",
    ".jp2",
    ".jxl",
    ".kdc",
    ".mef",
    ".mos",
    ".mrw",
    ".nef",
    ".orf",
    ".pef",
    ".png",
    ".raf",
    ".raw",
    ".rw2",
    ".srw",
    ".svg",
    ".tif",
    ".tiff",
    ".avif",
    ".wbmp",
    ".webp",
    ".x3f",
}

VID_EXTS = {
    ".3g2",
    ".3gp",
    ".amv",
    ".apng",
    ".asf",
    ".avif",
    ".avi",
    ".divx",
    ".drc",
    ".dv",
    ".f4v",
    ".flv",
    ".gif",
    ".heic",
    ".heif",
    ".hif",
    ".jxl",
    ".m2p",
    ".m2t",
    ".m2ts",
    ".m2v",
    ".m4v",
    ".mkv",
    ".mov",
    ".mp4",
    ".mpeg",
    ".mpg",
    ".mts",
    ".mxf",
    ".nsv",
    ".ogv",
    ".png",
    ".rm",
    ".rmvb",
    ".roq",
    ".svi",
    ".tp",
    ".trp",
    ".ts",
    ".vob",
    ".webm",
    ".webp",
    ".wmv",
    ".xvid",
}

ANIMATION_CAPABLE_IMAGE_EXTS = {
    ".apng",
    ".avif",
    ".gif",
    ".heic",
    ".heif",
    ".hif",
    ".jxl",
    ".png",
    ".webp",
}

PURE_VIDEO_EXTS = VID_EXTS - ANIMATION_CAPABLE_IMAGE_EXTS

OUTPUT_EXTS = {
    ".jxl",
    ".avif",
    ".heic",
    ".heif",
    ".mp4",
    ".mov",
    ".mkv",
    ".webm",
    ".jpg",
    ".jpeg",
    ".png",
    ".webp",
    ".gif",
    ".tiff",
    ".tif",
    ".bmp",
    ".apng",
}

MEDIA_EXTS = IMG_EXTS | VID_EXTS
ALL_KNOWN_EXTS = MEDIA_EXTS | OUTPUT_EXTS
SKIP_EXTS = {".xmp", ".ds_store", ".thumbs.db", ".desktop.ini"}

TRUE_FORMAT_EXTENSIONS: dict[str, set[str]] = {
    "jpeg": {"jpg", "jpeg", "jpe", "jfif"},
    "png": {"png"},
    "heic": {"heic", "hif"},
    "heif": {"heif", "hif"},
    "avif": {"avif"},
    "webp": {"webp"},
    "gif": {"gif"},
    "bmp": {"bmp"},
    "jxl": {"jxl"},
    "tiff": {"tif", "tiff"},
    "qoi": {"qoi"},
    "jp2": {"jp2", "j2k", "jpf", "jpx", "jpm", "mj2"},
    "ico": {"ico", "cur"},
    "exr": {"exr"},
    "flif": {"flif"},
    "psd": {"psd"},
    "pnm": {"pnm", "pbm", "pgm", "ppm"},
    "dds": {"dds"},
    "mp4": {"mp4", "m4v", "m4a", "m4b", "m4p", "m4r", "3gp", "3g2"},
    "mov": {"mov", "qt"},
    "mkv": {"mkv"},
    "webm": {"webm"},
}

PURE_VIDEO_FORMATS = {"mp4", "mov", "mkv", "webm"}

AVIF_BRANDS = {b"avif", b"avis", b"avio", b"MA1B", b"MA1A", b"av01"}
HEIC_BRANDS = {
    b"heic",
    b"heix",
    b"heim",
    b"heis",
    b"hevc",
    b"hevx",
    b"hev1",
    b"hvc1",
    b"hvc2",
    b"hvc3",
    b"hvc4",
    b"hevm",
    b"hevs",
    b"hev2",
}
HEIF_BRANDS = {b"heif", b"miaf", b"miPr", b"mif2", b"hefb", b"hefc"}
JP2_BRANDS = {b"mjp2", b"mjpb", b"mjd2", b"mpx3", b"mpx4", b"mpxh"}
MP4_BRANDS = {
    b"mp41",
    b"mp42",
    b"isom",
    b"iso2",
    b"iso3",
    b"iso4",
    b"iso5",
    b"iso6",
    b"iso7",
    b"iso8",
    b"iso9",
    b"dash",
    b"cmfc",
    b"m4v ",
    b"m4a ",
    b"m4b ",
    b"m4p ",
    b"m4r ",
    b"mp71",
    b"avc1",
    b"avc2",
    b"avc3",
    b"mp4v",
    b"3gp4",
    b"3gp5",
    b"3gp6",
    b"3gp1",
    b"3gp2",
    b"3gp3",
    b"3g2a",
    b"3g2b",
    b"3g2c",
    b"M4A ",
    b"M4B ",
    b"M4P ",
    b"M4V ",
    b"M4VH",
    b"M4VP",
    b"mmp4",
    b"dvc ",
    b"dvcp",
    b"dvpp",
    b"dv5p",
    b"dv5n",
    b"dvh5",
    b"dvh6",
    b"dvhp",
    b"dvhe",
    b"dvhq",
    b"dv6n",
    b"dv6p",
    b"vvcb",
    b"vvcg",
    b"vvcs",
    b"evc1",
    b"lvc1",
    b"avc4",
    b"avc5",
    b"avc6",
    b"avc7",
    b"avc8",
    b"hvc5",
    b"hvc6",
    b"hvc7",
    b"hvc8",
    b"vp08",
    b"vp09",
    b"av01",
    b"av02",
    b"mi11",
    b"mi12",
    b"mi1q",
    b"mi1r",
    b"mi21",
    b"mi31",
    b"dvh1",
    b"dvr1",
    b"simu",
    b"ccff",
}

PIPELINE_IMAGE = "image"
PIPELINE_VIDEO = "video"

# Machine-readable audit prefix (Rust tracing target ``mfb::audit`` + Python session log).
MFB_AUDIT_PREFIX = "MFB_AUDIT"

# Structured ``ignore_class`` tokens (Rust ``static_logs::audit_ignore_class``).
VID_STATIC_IGNORE_CLASSES = frozenset(
    {
        "vid_static_single_frame",
        "vid_static_unknown_frames",
    }
)

VID_HANDOFF_IGNORE_CLASSES = frozenset(
    {
        *VID_STATIC_IGNORE_CLASSES,
        "vid_out_of_domain",
    }
)

IMG_IGNORE_CLASSES = frozenset(
    {
        "img_animated_handoff",
        "img_analysis_uncertainty",
        "img_strict_entropy_missing",
        "img_animation_ambiguity",
    }
)

IMG_ANIMATED_HANDOFF_CLASSES = frozenset({"img_animated_handoff"})


def parse_mfb_audit_key_values(line: str) -> dict[str, str] | None:
    """
    Parse ``MFB_AUDIT k=v ...`` from a log line (order-independent; values may be quoted).
    Returns ``{}`` if the prefix is present but has no pairs; ``None`` if there is no audit segment.
    """
    if MFB_AUDIT_PREFIX not in line:
        return None
    idx = line.find(MFB_AUDIT_PREFIX)
    rest = line[idx + len(MFB_AUDIT_PREFIX) :].strip()
    if not rest:
        return {}
    out: dict[str, str] = {}
    pos = 0
    n = len(rest)
    while pos < n:
        while pos < n and rest[pos].isspace():
            pos += 1
        if pos >= n:
            break
        eq_pos = rest.find("=", pos)
        if eq_pos < 0:
            break
        key = rest[pos:eq_pos].strip()
        if not key:
            break
        pos = eq_pos + 1
        if pos >= n:
            out[key] = ""
            break
        if rest[pos] == '"':
            pos += 1
            buf: list[str] = []
            escaped = False
            while pos < n:
                ch = rest[pos]
                if escaped:
                    buf.append(ch)
                    escaped = False
                    pos += 1
                elif ch == "\\":
                    escaped = True
                    pos += 1
                elif ch == '"':
                    pos += 1
                    break
                else:
                    buf.append(ch)
                    pos += 1
            out[key] = "".join(buf)
        else:
            end = pos
            while end < n and not rest[end].isspace():
                end += 1
            out[key] = rest[pos:end]
            pos = end
    return out


# Only ``drag_and_drop_processor.run_img_vid_pipeline`` may set this before handoff preserve.
HANDOFF_PRESERVE_PHASE_POST_IMG_VID = "post_img_vid"

ROUTED_LINE_RE = re.compile(
    r"ROUTED\s+pipeline=(image|video)\s+path=(.+)$", re.IGNORECASE
)
PRESERVE_HANDOFF_LINE_RE = re.compile(r"PRESERVE_HANDOFF\s+path=(.+)$", re.IGNORECASE)
HANDOFF_PRESERVE_DECLINED_RE = re.compile(r"HANDOFF_PRESERVE_DECLINED\b", re.IGNORECASE)
PIPELINE_EXIT_RE = re.compile(
    r"(?P<pipeline>IMG|VID)_PIPELINE_EXIT\s+"
    r"code=(?P<code>-?\d+)\s+"
    r"succeeded=(?P<succeeded>\d+)\s+"
    r"skipped=(?P<skipped>\d+)\s+"
    r"ignored=(?P<ignored>\d+)\s+"
    r"failed=(?P<failed>\d+)",
    re.IGNORECASE,
)
# Tracing file log: ``outcome=ignored path=...`` or structured span fields in text
MFB_AUDIT_LOG_RE = re.compile(
    r"outcome=(?P<outcome>\w+)\s+.*?path=(?P<path>\S+)(?:\s+reason=(?P<reason>.*?))?(?:\s+\||$)",
    re.IGNORECASE,
)
# tracing-subscriber field order may vary — prefer parse_mfb_audit_key_values for new logs
MFB_AUDIT_FIELDS_RE = re.compile(
    r"path=(?P<path>\"[^\"]+\"|\S+).*?outcome=(?P<outcome>\w+).*?pipeline=(?P<pipeline>img|vid|batch|unknown)(?:.*?reason=(?P<reason>\"[^\"]*\"|\S+))?",
    re.IGNORECASE,
)
IGNORE_LINE_RE = re.compile(r"\[IGNORE\]\s+(.+?)\s+—\s+(.+)$", re.IGNORECASE)
SKIP_LINE_RE = re.compile(r"\[SKIP\]\s+(.+?)\s+—\s+(.+)$", re.IGNORECASE)


def read_prefix(path: Path, limit: int = 1024 * 1024) -> bytes:
    with open(path, "rb") as f:
        return f.read(limit)


def detect_true_format(path: Path) -> str:
    """Mirror Rust format_detect::detect_true_format magic-byte detection."""
    with open(path, "rb") as f:
        data = f.read(32)

    if len(data) >= 2 and data[0:2] == b"\xff\x0a":
        return "jxl"
    if len(data) >= 2 and data[0:2] == b"BM":
        return "bmp"
    if len(data) < 3:
        return "unknown"
    if data[0:3] == b"\xff\xd8\xff":
        return "jpeg"
    if len(data) >= 8 and data[0:8] == b"\x89PNG\r\n\x1a\n":
        return "png"
    if len(data) >= 12 and data[0:4] == b"RIFF" and data[8:12] == b"WEBP":
        return "webp"
    if len(data) >= 12 and data[4:8] == b"ftyp":
        brand = data[8:12]
        if brand in AVIF_BRANDS:
            return "avif"
        if brand in HEIC_BRANDS:
            return "heic"
        if brand in HEIF_BRANDS:
            return "heif"
        if brand in {b"mif1", b"msf1"}:
            return resolve_mif1_from_compatible_brands(path)
        if brand in MP4_BRANDS:
            return "mp4"
        if brand == b"qt  ":
            return "mov"
        if brand in JP2_BRANDS:
            return "jp2"
        return "unknown"
    if len(data) >= 4 and data[0:4] == b"\x1a\x45\xdf\xa3":
        return "webm" if b"webm" in data else "mkv"
    if len(data) >= 4 and data[0:4] == b"GIF8":
        return "gif"
    if len(data) >= 12 and data[0:12] == b"\x00\x00\x00\x0cJXL \r\n\x87\n":
        return "jxl"
    if data[0:4] in {
        b"II*\x00",
        b"MM\x00*",
        b"II+\x00",
        b"MM\x00+",
    }:
        return "tiff"
    if data[0:4] == b"qoif":
        return "qoi"
    if len(data) >= 12 and data[0:4] == b"\x00\x00\x00\x0c" and data[4:8] == b"jP  ":
        return "jp2"
    if data[0:4] == b"\xff\x4f\xff\x51":
        return "jp2"
    if data[0:4] in {b"\x00\x00\x01\x00", b"\x00\x00\x02\x00"}:
        return "ico"
    if data[0:4] == b"\x76\x2f\x31\x01":
        return "exr"
    if data[0:4] == b"FLIF":
        return "flif"
    if data[0:4] == b"8BPS":
        return "psd"
    if (
        len(data) >= 2
        and data[0:1] == b"P"
        and data[1] in range(ord("1"), ord("6") + 1)
        and (len(data) < 3 or chr(data[2]).isspace())
    ):
        return "pnm"
    if data[0:4] == b"DDS ":
        return "dds"
    return "unknown"


def resolve_mif1_from_compatible_brands(path: Path) -> str:
    with open(path, "rb") as f:
        data = f.read(1024 * 1024)
    if len(data) < 16 or data[4:8] != b"ftyp":
        return "unknown"
    box_size = int.from_bytes(data[0:4], "big", signed=False)
    ftyp_end = min(box_size, len(data))
    if ftyp_end < 16:
        return "unknown"
    compat = data[16:ftyp_end]
    heic_found = False
    heif_found = False
    for idx in range(0, len(compat) - 3, 4):
        brand = compat[idx : idx + 4]
        if brand in AVIF_BRANDS:
            return "avif"
        if brand in HEIC_BRANDS:
            heic_found = True
        if brand in HEIF_BRANDS:
            heif_found = True
        if brand in MP4_BRANDS:
            return "mp4"
        if brand == b"qt  ":
            return "mov"
        if brand in JP2_BRANDS:
            return "jp2"
    if heic_found:
        return "heic"
    if heif_found:
        return "heif"
    return "unknown"


def extension_matches_true_format(path: Path, true_format: str) -> bool:
    ext = path.suffix.lower().lstrip(".")
    return ext in TRUE_FORMAT_EXTENSIONS.get(true_format, set())


def is_animated_webp(path: Path) -> bool:
    data = read_prefix(path)
    if len(data) < 12 or data[0:4] != b"RIFF" or data[8:12] != b"WEBP":
        raise MediaProbeError(
            f"WebP animation probe failed for {path}: missing RIFF/WEBP header"
        )
    return b"ANIM" in data or b"ANMF" in data


def is_animated_png(path: Path) -> bool:
    data = read_prefix(path)
    if not data.startswith(b"\x89PNG\r\n\x1a\n"):
        raise MediaProbeError(
            f"PNG animation probe failed for {path}: missing PNG signature"
        )
    if len(data) < 24 or data[12:16] != b"IHDR":
        raise MediaProbeError(
            f"PNG animation probe failed for {path}: malformed or truncated IHDR"
        )
    return data.startswith(b"\x89PNG\r\n\x1a\n") and b"acTL" in data


def is_probably_animated_isobmff(path: Path) -> bool:
    """Heuristic for HEIF/AVIF motion brands (training + vid routing hints)."""
    data = read_prefix(path, 4096)
    if len(data) < 16 or data[4:8] != b"ftyp":
        raise MediaProbeError(
            f"ISOBMFF animation probe failed for {path}: malformed ftyp header"
        )
    box_size = int.from_bytes(data[0:4], "big", signed=False)
    if not 16 <= box_size <= len(data):
        raise MediaProbeError(
            f"ISOBMFF animation probe failed for {path}: invalid ftyp box size {box_size}"
        )
    brand_bytes = data[8:box_size]
    brands = {
        brand_bytes[idx : idx + 4] for idx in range(0, max(len(brand_bytes) - 3, 0), 4)
    }
    return bool(brands & {b"avis", b"msf1"})


def parse_jxlinfo_animation_hint(output: str) -> bool | None:
    normalized = output.lower()
    for line in normalized.splitlines():
        if "have_animation:" in line:
            token = line.split("have_animation:", 1)[1].split()
            if not token:
                continue
            if token[0] == "1":
                return True
            if token[0] == "0":
                return False
        if "animation length:" in line:
            token = line.split("animation length:", 1)[1].split()
            if not token:
                continue
            try:
                return float(token[0]) > 0.0
            except ValueError:
                return None
    if any(line.startswith("jpeg xl image") for line in normalized.splitlines()):
        return False
    return None


def is_animated_jxl(path: Path) -> bool:
    if detect_true_format(path) != "jxl":
        raise MediaProbeError(
            f"JXL animation probe failed for {path}: true format is not JXL"
        )
    jxlinfo = shutil.which("jxlinfo")
    if not jxlinfo:
        raise MediaProbeError(
            f"JXL animation probe failed for {path}: jxlinfo not found"
        )
    try:
        result = subprocess.run(
            [jxlinfo, str(path)],
            capture_output=True,
            text=True,
            timeout=15,
            check=False,
        )
    except (OSError, subprocess.TimeoutExpired) as exc:
        raise MediaProbeError(f"JXL animation probe failed for {path}: {exc}") from exc
    if result.returncode != 0:
        stderr = " ".join(result.stderr.split())
        raise MediaProbeError(
            f"JXL animation probe failed for {path}: jxlinfo exited {result.returncode}"
            f"{f' ({stderr})' if stderr else ''}"
        )
    parsed = parse_jxlinfo_animation_hint(f"{result.stdout}\n{result.stderr}")
    if parsed is None:
        raise MediaProbeError(
            f"JXL animation probe failed for {path}: jxlinfo output had no animation verdict"
        )
    return parsed


def is_animated_gif(path: Path) -> bool:
    data = path.read_bytes()

    if len(data) < 13 or data[:6] not in {b"GIF87a", b"GIF89a"}:
        raise MediaProbeError(
            f"GIF animation probe failed for {path}: malformed or truncated header"
        )

    pos = 6
    if pos + 7 > len(data):
        raise MediaProbeError(
            f"GIF animation probe failed for {path}: truncated logical screen"
        )
    packed = data[pos + 4]
    has_gct = (packed & 0x80) != 0
    gct_size = 3 * (1 << ((packed & 0x07) + 1)) if has_gct else 0
    pos += 7 + gct_size
    if pos > len(data):
        raise MediaProbeError(
            f"GIF animation probe failed for {path}: truncated global color table"
        )

    image_descriptors = 0
    gce_count = 0
    saw_trailer = False

    while pos < len(data):
        byte = data[pos]
        if byte == 0x2C:
            image_descriptors += 1
            if pos + 10 > len(data):
                raise MediaProbeError(
                    f"GIF animation probe failed for {path}: truncated image descriptor"
                )
            img_packed = data[pos + 9]
            has_lct = (img_packed & 0x80) != 0
            lct_size = 3 * (1 << ((img_packed & 0x07) + 1)) if has_lct else 0
            pos += 10 + lct_size
            if pos >= len(data):
                raise MediaProbeError(
                    f"GIF animation probe failed for {path}: missing image data"
                )
            pos += 1
            while True:
                if pos >= len(data):
                    raise MediaProbeError(
                        f"GIF animation probe failed for {path}: unterminated image data"
                    )
                block_size = data[pos]
                pos += 1
                if block_size == 0:
                    break
                if pos + block_size > len(data):
                    raise MediaProbeError(
                        f"GIF animation probe failed for {path}: truncated image data block"
                    )
                pos += block_size
        elif byte == 0x21:
            if pos + 2 > len(data):
                raise MediaProbeError(
                    f"GIF animation probe failed for {path}: truncated extension block"
                )
            label = data[pos + 1]
            if label == 0xF9:
                gce_count += 1
            pos += 2
            while True:
                if pos >= len(data):
                    raise MediaProbeError(
                        f"GIF animation probe failed for {path}: unterminated extension block"
                    )
                block_size = data[pos]
                pos += 1
                if block_size == 0:
                    break
                if pos + block_size > len(data):
                    raise MediaProbeError(
                        f"GIF animation probe failed for {path}: truncated extension data"
                    )
                pos += block_size
        elif byte == 0x3B:
            saw_trailer = True
            break
        else:
            raise MediaProbeError(
                f"GIF animation probe failed for {path}: unexpected block byte 0x{byte:02x}"
            )

    if not saw_trailer:
        raise MediaProbeError(f"GIF animation probe failed for {path}: missing trailer")

    frame_count = gce_count if gce_count > 1 else image_descriptors
    return frame_count > 1


def true_format_owner(path: Path, true_format: str) -> str | None:
    if true_format == "unknown":
        return None
    if true_format in PURE_VIDEO_FORMATS:
        return PIPELINE_VIDEO
    if true_format == "gif":
        return PIPELINE_VIDEO if is_animated_gif(path) else PIPELINE_IMAGE
    if true_format == "webp":
        return PIPELINE_VIDEO if is_animated_webp(path) else PIPELINE_IMAGE
    if true_format == "png":
        return PIPELINE_VIDEO if is_animated_png(path) else PIPELINE_IMAGE
    if true_format in {"avif", "heic", "heif"}:
        return PIPELINE_VIDEO if is_probably_animated_isobmff(path) else PIPELINE_IMAGE
    if true_format == "jxl":
        return PIPELINE_VIDEO if is_animated_jxl(path) else PIPELINE_IMAGE
    return PIPELINE_IMAGE


def true_format_matches_processing_mode(
    path: Path, true_format: str, processing_mode: str
) -> bool:
    owner = true_format_owner(path, true_format)
    if owner is None:
        return False
    if processing_mode == "images_only":
        return owner == PIPELINE_IMAGE
    if processing_mode == "videos_only":
        return owner == PIPELINE_VIDEO
    return True


def classify_media_owner(path: Path) -> str | None:
    """
    Return ``image``, ``video``, or None (not managed media).
    Matches drag-and-drop ``classify_media_path`` / verify routing.
    """
    if path.name.startswith(".") or path.suffix.lower() in SKIP_EXTS:
        return None
    true_format = detect_true_format(path)
    return true_format_owner(path, true_format)


def matches_processing_mode(path: Path, processing_mode: str) -> bool:
    owner = classify_media_owner(path)
    if owner is None:
        return False
    if processing_mode == "images_only":
        return owner == PIPELINE_IMAGE
    if processing_mode == "videos_only":
        return owner == PIPELINE_VIDEO
    return True


def animation_label(path: Path) -> str | None:
    """Human-readable animation proof for reports."""
    true_format = detect_true_format(path)
    if true_format == "webp" and is_animated_webp(path):
        return "animated WebP (ANIM/ANMF)"
    if true_format == "gif" and is_animated_gif(path):
        return "animated GIF"
    if true_format == "png" and is_animated_png(path):
        return "animated PNG (APNG/acTL)"
    if true_format in {"avif", "heic", "heif"} and is_probably_animated_isobmff(path):
        return f"animated {true_format.upper()} sequence (ISOBMFF)"
    if true_format == "jxl" and is_animated_jxl(path):
        return "animated JXL"
    return None


def _audit_path_matches_rel(
    audit_path: str, rel_s: str, source_dir: Path | None
) -> bool:
    rel_norm = rel_s.replace("\\", "/")
    try:
        if source_dir is not None:
            key = (
                Path(audit_path).resolve().relative_to(source_dir.resolve()).as_posix()
            )
            return key == rel_norm
    except (OSError, ValueError):
        pass
    norm = audit_path.replace("\\", "/")
    return norm.endswith("/" + rel_norm) or norm.endswith(rel_norm) or rel_norm in norm


def lookup_rust_outcomes_for_rel(
    rust_outcomes: dict[str, list[dict[str, str]]] | None,
    rel_s: str,
    *,
    source_dir: Path | None = None,
) -> list[dict[str, str]]:
    """All structured mfb::audit records for a source-relative path (img + vid)."""
    if not rust_outcomes:
        return []
    hits: list[dict[str, str]] = []
    for audit_path, records in rust_outcomes.items():
        if _audit_path_matches_rel(audit_path, rel_s, source_dir):
            hits.extend(records)
    return hits


def lookup_rust_outcome(
    rust_outcomes: dict[str, list[dict[str, str]]] | None,
    rel_s: str,
    *,
    source_dir: Path | None = None,
) -> dict[str, str] | None:
    """Return the best mfb::audit record for classification (prefer img, else last vid)."""
    hits = lookup_rust_outcomes_for_rel(rust_outcomes, rel_s, source_dir=source_dir)
    if not hits:
        return None
    for record in reversed(hits):
        if record.get("pipeline") == "img":
            return record
    return hits[-1]


def is_vid_static_ignore(rust: dict[str, str] | None) -> bool:
    """True when vid audit shows intentional ignore of a static / single-frame asset."""
    if not rust or rust.get("pipeline") != "vid" or rust.get("outcome") != "ignored":
        return False
    ignore_class = (rust.get("ignore_class") or "").strip()
    if ignore_class in VID_STATIC_IGNORE_CLASSES:
        return True
    # Legacy logs without ignore_class — use specific reason phrases only (not bare "static").
    reason = (rust.get("reason") or "").lower()
    return (
        "static image detected (1 frame)" in reason
        or "vid ignores static media" in reason
        or "vid ignores potentially non-animated" in reason
        or "single-frame" in reason
        or "non-animated" in reason
    )


def is_vid_expected_handoff(rust: dict[str, str] | None) -> bool:
    """True when vid intentionally ignored an asset (static handoff, out-of-domain, etc.)."""
    if not rust or rust.get("pipeline") != "vid" or rust.get("outcome") != "ignored":
        return False
    ignore_class = (rust.get("ignore_class") or "").strip()
    if ignore_class in VID_HANDOFF_IGNORE_CLASSES:
        return True
    if is_vid_static_ignore(rust):
        return True
    reason = (rust.get("reason") or "").lower()
    return "outside video domain" in reason or "outside this tool domain" in reason


def is_img_animation_ambiguity(rust: dict[str, str] | None) -> bool:
    """True when img refused static-only confirmation (AVIF/HEIC multi-stream ambiguity)."""
    if not rust or rust.get("pipeline") != "img" or rust.get("outcome") != "ignored":
        return False
    ignore_class = (rust.get("ignore_class") or "").strip()
    return ignore_class == "img_animation_ambiguity"


def is_img_animated_handoff(rust: dict[str, str] | None) -> bool:
    """True when img audit shows intentional ignore of animated media (vid owns transcode)."""
    if not rust or rust.get("pipeline") != "img" or rust.get("outcome") != "ignored":
        return False
    ignore_class = (rust.get("ignore_class") or "").strip()
    if ignore_class in IMG_ANIMATED_HANDOFF_CLASSES:
        return True
    reason = (rust.get("reason") or "").lower()
    return "img strictly processes static images only" in reason or (
        "animated media detected" in reason and "refusing static conversion" in reason
    )


def is_img_classified_ignore(rust: dict[str, str] | None) -> bool:
    if not rust or rust.get("pipeline") != "img" or rust.get("outcome") != "ignored":
        return False
    ignore_class = (rust.get("ignore_class") or "").strip()
    return ignore_class in IMG_IGNORE_CLASSES


def classify_missing_entry(
    path: Path,
    processing_mode: str,
    *,
    routing: dict[str, str] | None = None,
    rust_outcomes: dict[str, list[dict[str, str]]] | None = None,
    source_dir: Path | None = None,
) -> tuple[str, str]:
    """
    Classify a source file absent from the optimized tree.

    Returns:
    - ``true_missing`` — static img-owned; optimized output expected (``both`` / ``images_only``).
    - ``pipeline_handoff`` — expected gap: no output by design (scope / vid static ignore).
    - ``vid_pipeline_failed`` — vid was supposed to transcode but failed/skipped.
    - ``vid_pipeline_unverified`` — video-route gap but no ``mfb::audit`` (re-run verify with logs).
    """
    owner = classify_media_owner(path)
    anim = animation_label(path)
    true_format = detect_true_format(path)
    rel_s = (
        path.relative_to(source_dir).as_posix()
        if source_dir is not None
        else path.as_posix()
    )
    routed = (routing or {}).get(rel_s)
    audit_hits = lookup_rust_outcomes_for_rel(
        rust_outcomes, rel_s, source_dir=source_dir
    )
    img_rust = next(
        (r for r in reversed(audit_hits) if r.get("pipeline") == "img"), None
    )
    vid_rust = next(
        (r for r in reversed(audit_hits) if r.get("pipeline") == "vid"), None
    )
    rust = img_rust or vid_rust
    reason = (rust.get("reason") or "")[:200] if rust else ""

    if vid_rust and vid_rust.get("outcome") in {"failed", "skipped"}:
        vid_reason = (vid_rust.get("reason") or "")[:200]
        return (
            "vid_pipeline_failed",
            f"vid pipeline {vid_rust.get('outcome', '')}"
            + (f": {vid_reason}" if vid_reason else ""),
        )

    if rust and rust.get("pipeline") == "vid":
        outcome = rust.get("outcome", "")
        if outcome in {"failed", "skipped"}:
            return (
                "vid_pipeline_failed",
                f"vid pipeline {outcome}" + (f": {reason}" if reason else ""),
            )
        if outcome == "ignored" and not is_vid_expected_handoff(rust):
            return (
                "vid_pipeline_failed",
                "vid ignored without expected handoff classification"
                + (f": {reason}" if reason else ""),
            )
        if outcome == "ignored" and is_vid_expected_handoff(rust):
            if processing_mode == "videos_only":
                return (
                    "pipeline_handoff",
                    "vid ignored static/single-frame asset — "
                    "videos_only does not require optimized output for this file",
                )
            if processing_mode == "both" and owner == PIPELINE_IMAGE:
                return (
                    "pipeline_handoff",
                    "vid ignored static asset — both mode expects img pipeline output "
                    "(check img batch / rsync for this path)",
                )
            if processing_mode == "both":
                return (
                    "pipeline_handoff",
                    f"vid ignored as static ({reason or 'single-frame'}); "
                    "not an animated transcode failure",
                )

    if rust and rust.get("pipeline") == "img":
        outcome = rust.get("outcome", "")
        if outcome in {"failed", "skipped"}:
            return (
                "true_missing",
                f"img pipeline {outcome}" + (f": {reason}" if reason else ""),
            )
        if outcome == "ignored":
            if is_img_animation_ambiguity(img_rust or rust):
                return (
                    "pipeline_handoff",
                    "img could not confirm static-only (AVIF/HEIC/JXL ambiguity) — "
                    "no optimized output until re-probed or manual review"
                    + (f": {reason}" if reason else ""),
                )
            if is_img_animated_handoff(img_rust or rust):
                if processing_mode == "both" and (
                    anim or routed == PIPELINE_VIDEO or owner == PIPELINE_VIDEO
                ):
                    return (
                        "pipeline_handoff",
                        "img ignored animated asset — both mode expects vid transcode output",
                    )
                if processing_mode == "videos_only" and (
                    anim or routed == PIPELINE_VIDEO or owner == PIPELINE_VIDEO
                ):
                    return (
                        "pipeline_handoff",
                        "img ignored animated asset — videos_only expects vid transcode",
                    )
            return (
                "true_missing",
                "img ignored without classified handoff — investigate img batch logs"
                + (f": {reason}" if reason else ""),
            )

    if processing_mode == "images_only" and (owner == PIPELINE_VIDEO or anim):
        detail = (
            f"{anim or 'video-scoped asset'}: not in images_only verify scope "
            f"(owner={owner or 'video'}); if present in tree, run vid or use both mode"
        )
        return ("pipeline_handoff", detail)

    if processing_mode == "videos_only" and owner == PIPELINE_IMAGE:
        return (
            "pipeline_handoff",
            f"{true_format} is static image-owned — excluded from videos_only scope "
            "(matches_processing_mode); no optimized counterpart required",
        )

    if processing_mode == "both" and (
        owner == PIPELINE_VIDEO or routed == PIPELINE_VIDEO or anim
    ):
        if rust and rust.get("outcome") in {"failed", "skipped"}:
            return (
                "vid_pipeline_failed",
                f"vid pipeline {rust['outcome']}" + (f": {reason}" if reason else ""),
            )
        if rust is None:
            routed_note = routed or owner or "video"
            return (
                "vid_pipeline_unverified",
                f"{anim or 'video-route asset'}: missing mfb::audit for pipeline={routed_note}; "
                "re-run verify with --session-audit and bundle run logs",
            )
        routed_note = routed or owner or "video"
        return (
            "vid_pipeline_failed",
            f"{anim or 'video-route asset'}: session routed pipeline={routed_note}; "
            "both mode expects vid transcode output — no optimized counterpart",
        )

    if processing_mode == "videos_only" and (
        owner == PIPELINE_VIDEO or routed == PIPELINE_VIDEO or anim
    ):
        if rust and rust.get("outcome") in {"failed", "skipped"}:
            return (
                "vid_pipeline_failed",
                f"vid pipeline {rust['outcome']}" + (f": {reason}" if reason else ""),
            )
        if rust is None:
            routed_note = routed or owner or "video"
            return (
                "vid_pipeline_unverified",
                f"{anim or 'video-route asset'}: missing mfb::audit (routed={routed_note}); "
                "re-run verify with session/bundle logs",
            )
        routed_note = routed or owner or "video"
        return (
            "vid_pipeline_failed",
            f"{anim or 'video-route asset'}: videos_only expects vid transcode "
            f"(routed={routed_note}) — no optimized counterpart",
        )

    return (
        "true_missing",
        "no optimized counterpart (static image pipeline expected output)",
    )


def format_session_audit_routed(pipeline: str, rel_path: str) -> str:
    return f"ROUTED pipeline={pipeline} path={rel_path}"


def format_session_audit_preserve_handoff(rel_path: str) -> str:
    return f"PRESERVE_HANDOFF path={rel_path}"


def format_audit_event(category: str, **fields: str | int) -> str:
    """Single-line structured session audit (grep-friendly)."""
    parts = [f"{k}={v}" for k, v in fields.items()]
    tail = " ".join(parts)
    return f"{category} {tail}".strip()


def audit_handoff_blocked(reason: str, **extra: str) -> str:
    return format_audit_event("HANDOFF_PRESERVE_BLOCKED", reason=reason, **extra)


def integrity_stem_key(rel: Path | str) -> str:
    """Stem key used by Rust verify for source/optimized pairing."""
    rel_path = Path(rel)
    raw = str(rel_path.with_suffix(""))
    return unicodedata.normalize("NFC", raw).casefold().strip()


def collect_optimized_stem_keys(optimized_root: Path) -> set[str]:
    keys: set[str] = set()
    optimized_root = optimized_root.resolve()
    if not optimized_root.is_dir():
        return keys
    for dirpath, _, files in os.walk(optimized_root):
        for fname in files:
            if fname.startswith("."):
                continue
            p = Path(dirpath) / fname
            if p.suffix.lower() in SKIP_EXTS:
                continue
            try:
                true_format = detect_true_format(p)
            except OSError as exc:
                raise MediaProbeError(
                    f"optimized stem true-format probe failed for {p}: {exc}"
                ) from exc
            if true_format == "unknown":
                continue
            try:
                rel = p.relative_to(optimized_root)
            except ValueError:
                continue
            keys.add(integrity_stem_key(rel))
    return keys


def optimized_has_stem_match(optimized_root: Path, rel_path: str | Path) -> bool:
    """True if optimized tree already has any output for this source stem."""
    return integrity_stem_key(rel_path) in collect_optimized_stem_keys(optimized_root)


def format_bytes(size_bytes: int) -> str:
    if size_bytes < 1024:
        return f"{size_bytes} B"
    if size_bytes < 1024 * 1024:
        return f"{size_bytes / 1024:.1f} KB"
    if size_bytes < 1024 * 1024 * 1024:
        return f"{size_bytes / (1024 * 1024):.1f} MB"
    return f"{size_bytes / (1024 * 1024 * 1024):.2f} GB"


@dataclass(frozen=True)
class HandoffPreserveCandidate:
    """A video-route source with no optimized stem match (needs explicit user copy)."""

    rel_path: str
    size_bytes: int


def list_handoff_preserve_candidates(
    source_root: Path,
    optimized_root: Path,
    video_rel_paths: set[str] | list[str],
) -> list[HandoffPreserveCandidate]:
    """Enumerate copies that would run only after the user confirms handoff preserve."""
    source_root = source_root.resolve()
    optimized_root = optimized_root.resolve()
    if not source_root.is_dir() or not optimized_root.is_dir():
        return []

    stem_keys = collect_optimized_stem_keys(optimized_root)
    candidates: list[HandoffPreserveCandidate] = []

    for rel_s in sorted(video_rel_paths):
        rel = Path(rel_s)
        src = source_root / rel
        if not src.is_file():
            continue
        if integrity_stem_key(rel) in stem_keys:
            continue
        try:
            size = src.stat().st_size
        except OSError as exc:
            raise MediaProbeError(
                f"handoff preserve candidate size probe failed for {src}: {exc}"
            ) from exc
        candidates.append(HandoffPreserveCandidate(rel.as_posix(), size))

    return candidates


def preserve_handoff_gaps(
    source_root: Path,
    optimized_root: Path,
    video_rel_paths: set[str] | list[str],
    *,
    only_candidates: list[HandoffPreserveCandidate] | None = None,
    phase: str = "",
    audit_log: Callable[[str], None] | None = None,
) -> list[str]:
    """
    Copy user-confirmed handoff candidates into the adjacent optimized tree.

    *phase* must be ``HANDOFF_PRESERVE_PHASE_POST_IMG_VID`` (set only after img/vid).
    Pass *only_candidates* after an interactive ``y`` so nothing is copied silently.
    """
    if phase != HANDOFF_PRESERVE_PHASE_POST_IMG_VID:
        if audit_log:
            audit_log(
                audit_handoff_blocked(
                    "invalid_phase",
                    phase=phase or "(empty)",
                    required=HANDOFF_PRESERVE_PHASE_POST_IMG_VID,
                )
            )
        return []

    source_root = source_root.resolve()
    optimized_root = optimized_root.resolve()
    if not source_root.is_dir() or not optimized_root.is_dir():
        if audit_log:
            audit_log(
                format_audit_event(
                    "HANDOFF_PRESERVE_ABORT", reason="missing_source_or_optimized_dir"
                )
            )
        return []

    if only_candidates is not None:
        rels_to_copy = [c.rel_path for c in only_candidates]
    else:
        rels_to_copy = [
            c.rel_path
            for c in list_handoff_preserve_candidates(
                source_root, optimized_root, video_rel_paths
            )
        ]

    preserved: list[str] = []
    for rel_s in rels_to_copy:
        rel = Path(rel_s)
        src = source_root / rel
        dst = optimized_root / rel
        if not src.is_file():
            continue
        try:
            dst.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(src, dst)
        except OSError as exc:
            if audit_log:
                audit_log(
                    format_audit_event(
                        "HANDOFF_PRESERVE_COPY_FAIL",
                        path=rel.as_posix(),
                        error=str(exc),
                    )
                )
            raise
        if audit_log:
            audit_log(format_session_audit_preserve_handoff(rel.as_posix()))
        preserved.append(rel.as_posix())

    return preserved


def load_session_pipeline_exits(audit_paths: list[Path]) -> list[dict[str, str | int]]:
    """Parse ``IMG/VID_PIPELINE_EXIT`` lines from session verbose logs."""
    exits: list[dict[str, str | int]] = []
    for audit_path in audit_paths:
        if not audit_path.is_file():
            continue
        try:
            with open(audit_path, encoding="utf-8", errors="ignore") as f:
                for line in f:
                    m = PIPELINE_EXIT_RE.search(line.strip())
                    if m:
                        exits.append(
                            {
                                "pipeline": m.group("pipeline").lower(),
                                "code": int(m.group("code")),
                                "succeeded": int(m.group("succeeded")),
                                "skipped": int(m.group("skipped")),
                                "ignored": int(m.group("ignored")),
                                "failed": int(m.group("failed")),
                            }
                        )
        except OSError as exc:
            raise MediaProbeError(
                f"pipeline exit log unreadable: {audit_path}: {exc}"
            ) from exc
    return exits


def session_handoff_preserve_was_declined(audit_paths: list[Path]) -> bool:
    for audit_path in audit_paths:
        if not audit_path.is_file():
            continue
        try:
            with open(audit_path, encoding="utf-8", errors="ignore") as f:
                for line in f:
                    if HANDOFF_PRESERVE_DECLINED_RE.search(line):
                        return True
        except OSError as exc:
            raise MediaProbeError(
                f"handoff preserve decline log unreadable: {audit_path}: {exc}"
            ) from exc
    return False


def load_session_preserve_handoff(audit_paths: list[Path]) -> set[str]:
    preserved: set[str] = set()
    for audit_path in audit_paths:
        if not audit_path.is_file():
            continue
        try:
            with open(audit_path, encoding="utf-8", errors="ignore") as f:
                for line in f:
                    m = PRESERVE_HANDOFF_LINE_RE.search(line.strip())
                    if m:
                        preserved.add(m.group(1).strip())
        except OSError as exc:
            raise MediaProbeError(
                f"handoff preserve log unreadable: {audit_path}: {exc}"
            ) from exc
    return preserved


def scan_directory_routing(root: Path) -> tuple[set[str], set[str]]:
    """Walk *root* and return (image_rel_paths, video_rel_paths)."""
    image_paths: set[str] = set()
    video_paths: set[str] = set()
    root = root.resolve()
    for dirpath, _, files in os.walk(root):
        for fname in files:
            if fname.startswith("."):
                continue
            p = Path(dirpath) / fname
            owner = classify_media_owner(p)
            if owner is None:
                continue
            rel = os.path.relpath(p, root)
            if owner == PIPELINE_IMAGE:
                image_paths.add(rel)
            else:
                video_paths.add(rel)
    return image_paths, video_paths


def is_video_like(path: Path) -> bool:
    return classify_media_owner(path) == PIPELINE_VIDEO


def load_session_routing(audit_paths: list[Path]) -> dict[str, str]:
    """Parse ``ROUTED pipeline=…`` lines from session verbose logs."""
    routing: dict[str, str] = {}
    for audit_path in audit_paths:
        if not audit_path.is_file():
            continue
        try:
            with open(audit_path, encoding="utf-8", errors="ignore") as f:
                for line in f:
                    m = ROUTED_LINE_RE.search(line.strip())
                    if m:
                        routing[m.group(2).strip()] = m.group(1).lower()
        except OSError as exc:
            raise MediaProbeError(
                f"routing audit log unreadable: {audit_path}: {exc}"
            ) from exc
    return routing


def load_rust_outcomes_from_logs(
    log_paths: list[Path], source_root: Path | None = None
) -> dict[str, list[dict[str, str]]]:
    """
    Index per-file outcomes from Rust run logs (``mfb::audit``, stderr-style IGNORE/SKIP).
    Keys are paths as found in logs; values are outcome records.
    """
    by_path: dict[str, list[dict[str, str]]] = {}
    source_abs = str(source_root.resolve()) if source_root else None

    def add(
        path_str: str,
        outcome: str,
        pipeline: str,
        reason: str,
        ignore_class: str = "",
    ) -> None:
        path_str = path_str.strip()
        if source_abs:
            try:
                if not str(Path(path_str).resolve()).startswith(source_abs):
                    return
            except OSError:
                pass
        record: dict[str, str] = {
            "outcome": outcome,
            "pipeline": pipeline,
            "reason": reason,
        }
        if ignore_class:
            record["ignore_class"] = ignore_class
        by_path.setdefault(path_str, []).append(record)

    for log_path in log_paths:
        if not log_path.is_file():
            continue
        try:
            with open(log_path, encoding="utf-8", errors="ignore") as f:
                for line in f:
                    stripped = line.strip()
                    if MFB_AUDIT_PREFIX in stripped or "mfb::audit" in stripped:
                        if "outcome=batch_complete" in stripped:
                            continue
                        kvs = parse_mfb_audit_key_values(stripped)
                        if kvs:
                            outcome = kvs.get("outcome", "")
                            if outcome == "batch_complete":
                                continue
                            path_raw = kvs.get("path")
                            if path_raw:
                                pipeline = kvs.get("pipeline") or (
                                    "img" if "image_processing" in stripped else "vid"
                                )
                                reason = kvs.get("reason") or ""
                                ignore_class = kvs.get("ignore_class") or ""
                                add(path_raw, outcome, pipeline, reason, ignore_class)
                                continue
                        m_fields = MFB_AUDIT_FIELDS_RE.search(stripped)
                        m_legacy = (
                            MFB_AUDIT_LOG_RE.search(stripped) if not m_fields else None
                        )
                        m = m_fields or m_legacy
                        if m and m.group("outcome") != "batch_complete":
                            path_raw = m.group("path").strip('"')
                            pipeline = (
                                m.group("pipeline")
                                if m_fields and m.groupdict().get("pipeline")
                                else (
                                    "img" if "image_processing" in stripped else "vid"
                                )
                            )
                            reason = (
                                (m.groupdict().get("reason") or "").strip().strip('"')
                            )
                            add(path_raw, m.group("outcome"), pipeline, reason)
                    if m := IGNORE_LINE_RE.search(stripped):
                        add(m.group(1).strip(), "ignored", "img", m.group(2).strip())
                    if m := SKIP_LINE_RE.search(stripped):
                        add(m.group(1).strip(), "skipped", "img", m.group(2).strip())
                    if "image_processing{" in stripped and "outcome=" in stripped:
                        m = MFB_AUDIT_LOG_RE.search(stripped)
                        if m:
                            add(
                                m.group("path"),
                                m.group("outcome"),
                                "img",
                                (m.group("reason") or "").strip(),
                            )
        except OSError as exc:
            raise MediaProbeError(
                f"rust outcome log unreadable: {log_path}: {exc}"
            ) from exc
    return by_path


def reconcile_handoff(
    handoff_entries: list[tuple[str, Path, str]],
    routing: dict[str, str],
    rust_outcomes: dict[str, list[dict[str, str]]],
    source_dir: Path,
    optimized_dir: Path | None = None,
    preserved_rel_paths: set[str] | None = None,
    preserve_declined: bool = False,
    pipeline_exits: list[dict[str, str | int]] | None = None,
) -> list[str]:
    """
    Build human-readable cross-layer reconciliation lines for the integrity report.
    """
    lines: list[str] = []
    if pipeline_exits:
        lines.append("  Session pipeline exits:")
        for ex in pipeline_exits:
            lines.append(
                f"    {ex['pipeline']}: code={ex['code']} ok={ex['succeeded']} "
                f"skip={ex['skipped']} ignore={ex['ignored']} fail={ex['failed']}"
            )
        lines.append("")
    for _key, src_path, note in handoff_entries:
        rel = src_path.relative_to(source_dir)
        rel_s = rel.as_posix()
        owner = classify_media_owner(src_path)
        routed = routing.get(rel_s) or routing.get(str(rel))
        lines.append(f"  ▶ {rel_s}")
        lines.append(f"      scope note: {note}")
        lines.append(
            f"      Python classify_media_owner: {owner} | "
            f"session ROUTED: {routed or '(not in session audit)'}"
        )

        # Match rust log by suffix path (logs use absolute paths)
        rust_hits: list[dict[str, str]] = []
        for log_path, records in rust_outcomes.items():
            if log_path.endswith(rel_s) or rel_s in log_path:
                rust_hits.extend(records)
        if rust_hits:
            for rec in rust_hits[:3]:
                lines.append(
                    f"      Rust log: outcome={rec['outcome']} "
                    f"pipeline={rec.get('pipeline', '?')} — {rec.get('reason', '')[:120]}"
                )
        else:
            lines.append(
                "      Rust log: (no img/vid outcome line for this path — "
                "check bundle img_run / vid_run or run was before structured audit)"
            )

        if routed == PIPELINE_VIDEO and owner == PIPELINE_VIDEO:
            rust_outcome = rust_hits[-1]["outcome"] if rust_hits else None
            if rust_outcome == "ignored" and is_vid_static_ignore(rust_hits[-1]):
                lines.append(
                    "      ✓ vid ignored static/single-frame — expected gap in "
                    "videos_only; not a transcode failure"
                )
            elif rust_outcome in {"failed", "skipped"}:
                lines.append(
                    "      ✗ vid pipeline failed/skipped — both mode expects transcode; "
                    "this is a real integrity gap (not img handoff ignore)"
                )
            else:
                lines.append(
                    "      ⚠ video-route asset missing optimized output; "
                    "check vid batch logs (both mode expects vid transcode, not img ignore)"
                )
        elif routed == PIPELINE_IMAGE and owner == PIPELINE_VIDEO:
            lines.append(
                "      ⚠ Layer mismatch: file is animated but session routed as image"
            )
        elif routed is None:
            lines.append(
                "      ⚠ Session routing log missing this path (re-run with current drag-and-drop)"
            )

        if preserve_declined:
            lines.append(
                "      Session audit: user declined HANDOFF_PRESERVE (no copies performed)"
            )
        elif preserved_rel_paths and rel_s in preserved_rel_paths:
            lines.append("      Session audit: PRESERVE_HANDOFF ran for this path")
        if optimized_dir:
            exact = optimized_dir / rel
            if exact.is_file():
                lines.append(
                    f"      Optimized tree: {rel_s} present "
                    f"({exact.stat().st_size} bytes)"
                )
            elif not (preserved_rel_paths and rel_s in preserved_rel_paths):
                lines.append(
                    "      Optimized tree: still missing — re-run batch or "
                    "call finalize_handoff_preservation()"
                )
        lines.append("")
    return lines


def _self_check_media_scope() -> None:
    """Lightweight invariants for classify / ignore_class (run: python3 -m media_scope)."""
    static_record = {
        "pipeline": "vid",
        "outcome": "ignored",
        "ignore_class": "vid_static_single_frame",
        "reason": "unrelated static keyword in failure text",
    }
    assert is_vid_static_ignore(static_record)

    failed_record = {
        "pipeline": "vid",
        "outcome": "failed",
        "reason": "static layout error during transcode",
    }
    assert not is_vid_static_ignore(failed_record)

    legacy_record = {
        "pipeline": "vid",
        "outcome": "ignored",
        "reason": "Static image detected (1 frame) - vid ignores static media",
    }
    assert is_vid_static_ignore(legacy_record)

    broad_legacy = {
        "pipeline": "vid",
        "outcome": "ignored",
        "reason": "static layout error only",
    }
    assert not is_vid_static_ignore(broad_legacy)

    predicted_handoff = {
        "pipeline": "vid",
        "outcome": "ignored",
        "ignore_class": "vid_static_single_frame",
        "reason": "quality below target static transcode",
    }
    assert is_vid_static_ignore(predicted_handoff)

    cat, _ = classify_missing_entry(
        Path("/src/foo.webp"),
        "both",
        routing={"foo.webp": PIPELINE_VIDEO},
        rust_outcomes={
            "/src/foo.webp": [
                {
                    "pipeline": "vid",
                    "outcome": "failed",
                    "reason": "static parse error",
                }
            ]
        },
        source_dir=Path("/src"),
    )
    assert cat == "vid_pipeline_failed"

    anim_ignored_no_class = {
        "pipeline": "vid",
        "outcome": "ignored",
        "reason": "explore quality below target",
    }
    cat2, _ = classify_missing_entry(
        Path("/src/anim.webp"),
        "both",
        routing={"anim.webp": PIPELINE_VIDEO},
        rust_outcomes={
            "/src/anim.webp": [anim_ignored_no_class],
        },
        source_dir=Path("/src"),
    )
    assert cat2 == "vid_pipeline_failed"

    img_anim = {
        "pipeline": "img",
        "outcome": "ignored",
        "ignore_class": "img_animated_handoff",
        "reason": "Animated media detected - img strictly processes static images only",
    }
    assert is_img_animated_handoff(img_anim)
    cat3, _ = classify_missing_entry(
        Path("/src/anim.webp"),
        "both",
        routing={"anim.webp": PIPELINE_VIDEO},
        rust_outcomes={"/src/anim.webp": [img_anim]},
        source_dir=Path("/src"),
    )
    assert cat3 == "pipeline_handoff"

    vid_oob = {
        "pipeline": "vid",
        "outcome": "ignored",
        "ignore_class": "vid_out_of_domain",
        "reason": "outside video domain after content check",
    }
    assert is_vid_expected_handoff(vid_oob)
    assert not is_vid_static_ignore(vid_oob)

    img_unclassified = {
        "pipeline": "img",
        "outcome": "ignored",
        "reason": "unknown preflight reject",
    }
    assert not is_img_classified_ignore(img_unclassified)
    cat4, _ = classify_missing_entry(
        Path("/src/x.png"),
        "both",
        routing={},
        rust_outcomes={"/src/x.png": [img_unclassified]},
        source_dir=Path("/src"),
    )
    assert cat4 == "true_missing"

    img_entropy = {
        "pipeline": "img",
        "outcome": "ignored",
        "ignore_class": "img_analysis_uncertainty",
        "reason": "Analysis uncertainty for PNG (entropy missing)",
    }
    cat5, _ = classify_missing_entry(
        Path("/src/x.png"),
        "images_only",
        rust_outcomes={"/src/x.png": [img_entropy]},
        source_dir=Path("/src"),
    )
    assert cat5 == "true_missing"

    cat6, _ = classify_missing_entry(
        Path("/src/clip.mp4"),
        "both",
        routing={"clip.mp4": PIPELINE_VIDEO},
        rust_outcomes=None,
        source_dir=Path("/src"),
    )
    assert cat6 == "vid_pipeline_unverified"


if __name__ == "__main__":
    _self_check_media_scope()
    print("media_scope self-checks OK")
