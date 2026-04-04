//! Global Constants for `modern_format_boost`
//!
//! This module centralizes core magic numbers, business rules, and
//! environment variable toggles to ensure consistency across the workspace.

// --- Size & Storage Defaults ---

/// Default size tolerance (1MB) = 1,048,576 bytes
pub const DEFAULT_SIZE_TOLERANCE_BYTES: u64 = 1_048_576;

/// Default size tolerance percentage (1%)
pub const DEFAULT_SIZE_TOLERANCE_RATIO: f64 = 0.01;

/// Minimum output size for images to be considered valid for deletion of original.
pub const MIN_OUTPUT_SIZE_BEFORE_DELETE_IMAGE: u64 = 1024;

/// Minimum output size for videos to be considered valid for deletion of original.
pub const MIN_OUTPUT_SIZE_BEFORE_DELETE_VIDEO: u64 = 4096;

// --- Unified Video Duration Thresholds ---

/// Animation and short clip threshold (30s).
pub const ANIMATION_CLIP_THRESHOLD_SECS: f32 = 30.0;

/// Maximum duration for CRF 0.00 lossless-first probing (Meme vs High Value).
pub const MEME_LOSSLESS_DURATION_LIMIT: f32 = 120.0;
pub const HIGH_VALUE_LOSSLESS_DURATION_LIMIT: f32 = 30.0;

/// Video length categories
pub const LONG_VIDEO_THRESHOLD_SECS: f32 = 300.0;
pub const VERY_LONG_VIDEO_THRESHOLD_SECS: f32 = 600.0;
pub const HEAVY_VIDEO_THRESHOLD_SECS: f32 = 1200.0;
pub const VMAF_SKIP_THRESHOLD_SECS: f32 = 1800.0;
pub const VMAF_SKIP_THRESHOLD_ULTIMATE_SECS: f32 = 3600.0;

// --- Loop Intent System (Tree & KNN) ---

/// Platform markers that indicate a strong likelihood of being a GIF/sticker.
pub const LOOP_PLATFORM_MARKERS: &[&str] =
    &["GIPHY", "TENOR", "STICKER", "TELEGRAM", "TIKTOK", "DISCORD"];

/// Directory keywords that suggest an asset is a meme or reaction.
pub const MEME_DIRECTORY_KEYWORDS: &[&str] = &[
    "meme",
    "memes",
    "sticker",
    "stickers",
    "emoji",
    "emojis",
    "reaction",
    "reactions",
    "sticker_pack",
    "sticker_pkg",
    "sticker_collection",
    "meme_collection",
    "funny",
    "humor",
];

// 1. Dynamic Multipliers (Relative to KNN P90 Baseline)
/// Multiplier for the "Safe Zone" beyond the P90 baseline.
pub const LOOP_KNN_P90_SAFE_ZONE_MULTIPLIER: f64 = 7.5;
/// Multiplier for the "Video Bias" threshold beyond the P90 baseline.
pub const LOOP_KNN_P90_BIAS_THRESHOLD_MULTIPLIER: f64 = 1.5;

// 2. Physical "Bottom-line" Thresholds
/// Default baseline for "Sticker Safe Zone" (seconds) if KNN is unavailable.
pub const DEFAULT_LOOP_BASELINE_DURATION_SECS: f64 = 2.0;
/// Max dimension (w or h) typically used for stickers/emojis.
pub const STICKER_MAX_DIMENSION: u32 = 512;
/// "Bottom-line" size control: assets below this size are likely stickers.
pub const STICKER_MAX_SIZE_BYTES: u64 = 1_572_864; // 1.5 MB

// 3. Physical Intensity & Bitrate Analysis
/// Threshold for "Physical Intensity" (Pixels per second normalized).
pub const PHYSICAL_INTENSITY_PASS_STRENGTH: f64 = 1.5;
/// WebP compression ratio below which an asset is considered "High Quality Master".
pub const MODERN_FORMAT_HIGH_BITRATE_RATIO: f64 = 8.0;
/// FPS below which an animation is considered "PPT-like" slow-playback.
pub const MODERN_FORMAT_PPT_FPS_THRESHOLD: f64 = 5.0;

// 4. Modern Format Compression Ratio Thresholds (Layer 4-A)
/// High compression ratio (15.0) indicating simple graphics / memes.
pub const MODERN_FORMAT_LOW_BITRATE_RATIO: f64 = 15.0;
/// Low compression ratio (5.0) indicating extremely high quality / noise.
pub const MODERN_FORMAT_ULTRA_HIGH_BITRATE_RATIO: f64 = 5.0;

// 4. Fallback & Force Rules
/// Hidden Layer 1-C developer hard-pass threshold (seconds).
/// The core tree also uses this as the upper bound for its short-asset soft prior.
pub const HARD_PASS_SHORT_GIF_THRESHOLD_SECS: f64 = 10.0;
/// Hidden long-silent/video-bias threshold (seconds).
/// The core tree also uses this as the lower bound for its long-silent soft penalty.
pub const MODERN_FORMAT_VIDEO_BIAS_THRESHOLD_SECS: f64 = 15.0;

// 5. Environment Variable Names
/// Toggle for modern format conversion bias ("1" = on, "0" = off).
pub const ENV_MODERN_FORMAT_CONVERT_BIAS: &str = "MODERN_FORMAT_CONVERT_BIAS";
/// Hidden developer toggle for Layer 1-C short-asset hard-pass ("1" = enable, default off).
pub const ENV_FORCE_SHORT_GIFS: &str = "MODERN_FORMAT_FORCE_SHORT_GIFS";
/// Hidden developer toggle for Layer 1-D long-silent interceptor ("1" = enable, default off).
pub const ENV_INTERCEPT_LONG_SILENT: &str = "MODERN_FORMAT_INTERCEPT_LONG_SILENT";
/// Override for the sticker duration safe-limit (seconds).
pub const ENV_STICKER_LIMIT_SECS: &str = "MODERN_FORMAT_STICKER_LIMIT_SECS";
/// Bypass for the entire database-driven feedback loop (Dynamic weights, KNN, Logging).
pub const ENV_DISABLE_DB_FEEDBACK: &str = "MODERN_FORMAT_DISABLE_DB_FEEDBACK";
/// Independent kill-switch for the static image quality DB (does not affect GIF/Video KNN).
pub const ENV_DISABLE_IMAGE_QUALITY_DB: &str = "MODERN_FORMAT_DISABLE_IMAGE_QUALITY_DB";

// --- Database Maturity Thresholds ---
// KNN results are unreliable when training data is too sparse or non-diverse.
// These thresholds gate both the GIF/Video KNN and the static image quality KNN.

/// Minimum total labeled samples required for GIF/Video KNN to engage.
/// Below this count, data is too sparse to be representative.
pub const MIN_GIF_SAMPLES_TOTAL: i64 = 150;
/// Minimum samples per class (high/video) for GIF/Video KNN.
/// Without both sides of the decision boundary, KNN will be biased toward one class.
pub const MIN_GIF_SAMPLES_PER_CLASS: i64 = 30;

/// Minimum total labeled samples required for static image KNN to engage.
pub const MIN_QUALITY_SAMPLES_TOTAL: i64 = 50;
/// Minimum samples per class (high/low) for static image KNN.
pub const MIN_QUALITY_SAMPLES_PER_CLASS: i64 = 10;

// --- Formats & Extensions ---

/// Modern animated image/container extensions.
pub const MODERN_ANIMATED_EXTENSIONS: &[&str] = &["webp", "avif", "apng", "heic", "heif", "jxl"];

// --- FFmpeg & Encoder Metadata ---

// 1. Tags
/// Apple Compatibility Tag (Common for HEVC)
pub const FFMPEG_TAG_HVC1: &str = "hvc1";
/// Standard HEVC Tag (Broad compatibility)
pub const FFMPEG_TAG_HEV1: &str = "hev1";
/// Standard AV1 Tag
pub const FFMPEG_TAG_AV01: &str = "av01";

// 2. Presets (x26x / SVT-AV1)
pub const FFMPEG_PRESET_ULTRAFAST: &str = "ultrafast";
pub const FFMPEG_PRESET_MEDIUM: &str = "medium";
pub const FFMPEG_PRESET_SLOW: &str = "slow";
pub const FFMPEG_PRESET_SLOWER: &str = "slower";
pub const FFMPEG_PRESET_VERYSLOW: &str = "veryslow";

// 3. Encoder Names
pub const FFMPEG_ENCODER_X264: &str = "libx264";
pub const FFMPEG_ENCODER_X265: &str = "libx265";
pub const FFMPEG_ENCODER_SVTAV1: &str = "libsvtav1";

// 4. Default Search Parameters
/// Starting CRF for quality-matched exploration.
pub const DEFAULT_CRF_EXPLORE_START: f32 = 18.0;
/// CRF adjustment step for iterative search.
pub const CRF_SEARCH_STEP: f32 = 1.0;

// --- Loop Intent Decision Tree Thresholds (Log-Odds) ---

pub const TREE_DECISION_LOG_ODDS_THRESHOLD: f64 = 0.95;
pub const TREE_Z_SCORE_CAP: f64 = 2.5;
pub const SHORT_FAST_POSITIVE_LOG_ODDS: f64 = 0.72;
pub const LONG_SLOW_NEGATIVE_LOG_ODDS: f64 = 0.82;
pub const SCENE_CUT_NEGATIVE_LOG_ODDS: f64 = 1.35;
pub const COMPACT_SILENT_POSITIVE_LOG_ODDS: f64 = 0.62;
pub const LARGE_MEDIA_NEGATIVE_LOG_ODDS: f64 = 0.55;
pub const PLATFORM_MARKER_POSITIVE_LOG_ODDS: f64 = 0.52;
pub const PLAY_ONCE_NEGATIVE_LOG_ODDS: f64 = 0.92;
pub const TRANSPARENCY_POSITIVE_LOG_ODDS: f64 = 0.34;
pub const LOCALIZED_MOTION_POSITIVE_LOG_ODDS: f64 = 0.16;
pub const DIRECTORY_CONTEXT_POSITIVE_LOG_ODDS: f64 = 0.12;
pub const FILENAME_CONTEXT_POSITIVE_LOG_ODDS: f64 = 0.10;
pub const MODERN_MASTER_NEGATIVE_LOG_ODDS: f64 = 0.35;
pub const SHORT_CLIP_PRIOR_LOG_ODDS: f64 = 0.42;
pub const EXTENDED_SHORT_ASSET_PRIOR_LOG_ODDS: f64 = 0.20;
pub const LONG_SILENT_PRIOR_NEGATIVE_LOG_ODDS: f64 = 0.26;
pub const LAYER6_HIGH_SCORE_THRESHOLD: f64 = 0.70;
pub const LAYER6_RELAXED_CONFIDENCE_THRESHOLD: f64 = 0.68;

// --- Video Quality & Compression Boundaries ---

/// CRF threshold for "Visually Lossless" classification.
pub const CRF_THRESHOLD_VISUALLY_LOSSLESS: f32 = 15.0;
/// CRF threshold for "High Quality" classification.
pub const CRF_THRESHOLD_HIGH_QUALITY: f32 = 23.0;
/// CRF threshold for "Standard Quality" classification.
pub const CRF_THRESHOLD_STANDARD: f32 = 30.0;

/// Bits Per Pixel (BPP) threshold for "Visually Lossless" classification.
pub const BPP_THRESHOLD_VISUALLY_LOSSLESS: f64 = 2.0;
/// Bits Per Pixel (BPP) threshold for "High Quality" classification.
pub const BPP_THRESHOLD_HIGH_QUALITY: f64 = 0.5;
/// Bits Per Pixel (BPP) threshold for "Standard Quality" classification.
pub const BPP_THRESHOLD_STANDARD: f64 = 0.1;

/// Resolution threshold for 4K UHD height.
pub const HEIGHT_UHD_4K: u32 = 2160;
/// Resolution threshold for 4K UHD width.
pub const WIDTH_UHD_4K: u32 = 3840;

// --- Image Detection & Safety Limits ---

/// Maximum memory allocation for decoding large images (2GB).
pub const MAX_IMAGE_DECODE_ALLOC_BYTES: u64 = 2 * 1024 * 1024 * 1024;
/// Maximum file size for proactive image analysis (512MB).
pub const MAX_IMAGE_ANALYSIS_FILE_SIZE: u64 = 512 * 1024 * 1024;

/// PNG Quantization Heuristic: Lower score boundary for "Lossless" gray zone.
pub const PNG_QUANT_THRESHOLD_LOW: f64 = 0.40;
/// PNG Quantization Heuristic: Upper score boundary for "Lossy" classification.
pub const PNG_QUANT_THRESHOLD_HIGH: f64 = 0.58;

// --- Video Explorer Iteration & Search Limits ---

pub const ABSOLUTE_MIN_CRF: f32 = 0.0;
pub const ABSOLUTE_MAX_CRF: f32 = 51.0;

pub const STAGE_B1_MAX_ITERATIONS: u32 = 20;
pub const STAGE_B2_MAX_ITERATIONS: u32 = 25;
pub const STAGE_B_BIDIRECTIONAL_MAX_ITERATIONS: u32 = 18;
pub const BINARY_SEARCH_MAX_ITERATIONS: u32 = 12;
pub const GLOBAL_MAX_ITERATIONS: u32 = 500;

/// Files below this size are considered "small" for compression verification (10MB).
pub const SMALL_FILE_THRESHOLD_BYTES: u64 = 10 * 1024 * 1024;

/// Minimum absolute metadata margin allowed (2KB).
pub const METADATA_MARGIN_MIN_BYTES: u64 = 2048;
/// Maximum absolute metadata margin allowed (100KB).
pub const METADATA_MARGIN_MAX_BYTES: u64 = 102_400;
/// Target metadata overhead percentage (0.5%).
pub const METADATA_MARGIN_RATIO: f64 = 0.005;

pub const ULTIMATE_REQUIRED_ZERO_GAINS: u32 = 100;
pub const NORMAL_REQUIRED_ZERO_GAINS: u32 = 4;
pub const LONG_VIDEO_REQUIRED_ZERO_GAINS: u32 = 3;

// --- Additional Quality & Duration Boundaries ---

/// FPS below which an animation is considered "PPT-like" slow-playback. (Duplicated from loop intent section for visibility)
pub const PPT_FPS_THRESHOLD: f64 = 5.0;
/// Negligible duration for effectively static images (seconds).
pub const NEGLIGIBLE_DURATION_SECS: f64 = 0.01;

/// HQ HD Resolution: Width (1280).
pub const HQ_HD_WIDTH: u32 = 1280;
/// HQ HD Resolution: Height (720).
pub const HQ_HD_HEIGHT: u32 = 720;
/// Total pixels for HD (1280x720 = 921,600).
pub const HQ_PIX_COUNT_HD: u64 = 921_600;

// --- Extension Constants ---

pub const EXT_MOV: &str = "MOV";
pub const EXT_MP4: &str = "MP4";
pub const EXT_MKV: &str = "mkv";
pub const EXT_WEBP: &str = "webp";
pub const EXT_GIF: &str = "gif";
pub const EXT_JXL: &str = "jxl";
pub const EXT_AVIF: &str = "avif";
pub const EXT_APNG: &str = "apng";
pub const EXT_PNG: &str = "png";

// --- External Tools & Binary Names ---

pub const TOOL_FFMPEG: &str = "ffmpeg";
pub const TOOL_FFPROBE: &str = "ffprobe";
pub const TOOL_CJXL: &str = "cjxl";
pub const TOOL_DJXL: &str = "djxl";
pub const TOOL_JXLINFO: &str = "jxlinfo";
pub const TOOL_WEBPMUX: &str = "webpmux";
pub const TOOL_GIFSKI: &str = "gifski";
pub const TOOL_MAGICK: &str = "magick";
pub const TOOL_IDENTIFY: &str = "identify";
pub const TOOL_SIPS: &str = "sips";
pub const TOOL_EXIFTOOL: &str = "exiftool";
pub const TOOL_DWEBP: &str = "dwebp";
pub const TOOL_X265: &str = "x265";
pub const TOOL_AVIFENC: &str = "avifenc";
pub const TOOL_DOVI: &str = "dovi_tool";
pub const TOOL_HDR10PLUS: &str = "hdr10plus_tool";

// --- SVT-AV1 Defaults ---

/// Default preset for SVT-AV1 (6 = Balanced).
pub const FFMPEG_SVTAV1_DEFAULT_PRESET: &str = "6";

// --- FFmpeg Command Flags & Arguments ---

pub const FFMPEG_ARG_OVERWRITE: &str = "-y";
pub const FFMPEG_ARG_HIDE_BANNER: &str = "-hide_banner";
pub const FFMPEG_ARG_THREADS: &str = "-threads";
pub const FFMPEG_ARG_INPUT: &str = "-i";
pub const FFMPEG_ARG_MAP: &str = "-map";
pub const FFMPEG_ARG_FORMAT: &str = "-f";
pub const FFMPEG_ARG_FRAMES_VIDEO: &str = "-frames:v";
pub const FFMPEG_ARG_CODEC_VIDEO: &str = "-c:v";
pub const FFMPEG_ARG_CRF: &str = "-crf";
pub const FFMPEG_ARG_PRESET: &str = "-preset";
pub const FFMPEG_ARG_PIX_FMT: &str = "-pix_fmt";
pub const FFMPEG_ARG_VSYNC: &str = "-vsync";
pub const FFMPEG_ARG_NO_AUDIO: &str = "-an";
pub const FFMPEG_ARG_FILTER_COMPLEX: &str = "-filter_complex";
pub const FFMPEG_ARG_FILTER_LAVFI: &str = "-lavfi";
pub const FFMPEG_ARG_SELECT_STREAMS: &str = "-select_streams";
pub const FFMPEG_ARG_COUNT_FRAMES: &str = "-count_frames";
pub const FFMPEG_ARG_SHOW_ENTRIES: &str = "-show_entries";
pub const FFMPEG_ARG_OUTPUT_FORMAT: &str = "-of";
pub const FFMPEG_ARG_PLAYS: &str = "-plays";
pub const FFMPEG_ARG_X265_PARAMS: &str = "-x265-params";
pub const FFMPEG_ARG_PROFILE_VIDEO: &str = "-profile:v";
pub const FFMPEG_ARG_TAG_VIDEO: &str = "-tag:v";
pub const FFMPEG_ARG_TUNE: &str = "-tune";
pub const FFMPEG_ARG_RC: &str = "-rc";
pub const FFMPEG_ARG_QUALITY: &str = "-quality";
pub const FFMPEG_ARG_LOG_LEVEL: &str = "-v";
 
// --- JXL Argument Constants ---
pub const JXL_ARG_DISTANCE: &str = "-d";
pub const JXL_ARG_EFFORT: &str = "-e";
pub const JXL_ARG_THREADS: &str = "-j";
pub const JXL_ARG_LOSSLESS_JPEG: &str = "--lossless_jpeg=1";
pub const JXL_ARG_COLOR_SPACE: &str = "color_space";
pub const JXL_ARG_COMPRESS_BOXES: &str = "--compress_boxes=0";
pub const JXL_ARG_ALLOW_JPEG_RECON: &str = "--allow_jpeg_reconstruction";
pub const JXL_ARG_ICC_PATHNAME: &str = "icc_pathname";
 
// --- ImageMagick Argument Constants ---
pub const MAGICK_ARG_STRIP: &str = "-strip";
pub const MAGICK_ARG_DEPTH: &str = "-depth";
pub const MAGICK_ARG_DEFINE: &str = "-define";
pub const MAGICK_ARG_SET: &str = "-set";
pub const MAGICK_ARG_COLORSPACE: &str = "colorspace";

// --- FFmpeg Value Constants ---

pub const FFMPEG_VAL_VFR: &str = "vfr";
pub const FFMPEG_VAL_ERROR: &str = "error";
pub const FFMPEG_VAL_CSV_CLEAN: &str = "csv=p=0";
pub const FFMPEG_VAL_JSON: &str = "json";

pub const VAL_QUIET: &str = "quiet";
pub const VAL_MEDIUM: &str = "medium";
pub const VAL_MAIN: &str = "main";
pub const VAL_HIGH: &str = "high";
pub const VAL_P4: &str = "p4";
pub const VAL_HQ: &str = "hq";
pub const VAL_VBR: &str = "vbr";
