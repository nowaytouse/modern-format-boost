//! Centralized Static Log Messages and Themed Logging
//!
//! This module centralizes frequently used log messages and provides themed
//! logging functions to ensure consistent styling across the workspace.
//!
//! ## Design Principles
//! - **Consistency**: All similar events should look the same in the logs.
//! - **Maintainability**: Messages are defined once, making updates easier.
//! - **Theming**: Logical events (Success, Skip, Ignore, Error) have distinct visual identities.

use crate::modern_ui::{TerminalColor, symbols};

// ─── Error Logging Infrastructure (Moved from error_logging.rs) ───────────────

/// Error severity levels for enhanced visibility
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorSeverity {
    /// Data loss, corruption, or truncation — highest priority
    Critical,
    /// Unexpected behavior with no obvious cause — needs investigation
    Rare,
    /// Metadata stripped, lost, or unreadable
    MetadataLoss,
    /// Broken pipe, EOF mid-stream, process terminated unexpectedly
    PipelineBroken,
    /// FFmpeg/cjxl/ImageMagick returned unexpected exit codes or output
    UpstreamError,
    /// Ordinary recoverable error
    Standard,
}

impl ErrorSeverity {
    /// Short label used in log lines (no color — for file logs)
    #[must_use]
    pub const fn label(&self) -> &'static str {
        match self {
            Self::Critical => "[CRITICAL]",
            Self::Rare => "[RARE ERROR]",
            Self::MetadataLoss => "[METADATA LOSS]",
            Self::PipelineBroken => "[PIPELINE BROKEN]",
            Self::UpstreamError => "[UPSTREAM ERROR]",
            Self::Standard => "[ERROR]",
        }
    }

    /// Colored label for terminal output
    #[must_use]
    pub fn label_colored(&self) -> String {
        match self {
            Self::Critical => "\x1b[1;31m🚨 CRITICAL\x1b[0m".to_string(),
            Self::Rare => "\x1b[1;33m☢️  RARE ERROR\x1b[0m".to_string(),
            Self::MetadataLoss => "\x1b[1;35m📋 METADATA LOSS\x1b[0m".to_string(),
            Self::PipelineBroken => "\x1b[1;36m🔧 PIPELINE BROKEN\x1b[0m".to_string(),
            Self::UpstreamError => "\x1b[33m⛔️ UPSTREAM ERROR\x1b[0m".to_string(),
            Self::Standard => "\x1b[31m❌ ERROR\x1b[0m".to_string(),
        }
    }
}

/// Emit an enhanced error to both terminal (colored) and file log (plain).
///
/// Terminal: `  🚨 CRITICAL  <context>: <detail>`
/// File:     `  [CRITICAL] <context>: <detail>`
pub fn log_enhanced_error(severity: ErrorSeverity, context: &str, detail: &str) {
    // Terminal: colored, indented
    let colored = format!(
        "  {}  \x1b[1m{}\x1b[0m: {}",
        severity.label_colored(),
        context,
        detail
    );
    crate::progress_mode::emit_stderr(&colored);

    // File log: plain text with label
    if crate::progress_mode::has_log_file() {
        let plain = format!("  {}  {}: {}", severity.label(), context, detail);
        crate::progress_mode::write_to_log(&plain);
    }
}

/// Auto-classify an error message by pattern matching.
#[must_use]
pub fn classify_error(msg: &str) -> ErrorSeverity {
    let lower = msg.to_lowercase();

    if lower.contains("data loss") || lower.contains("corrupt") || lower.contains("truncat") {
        return ErrorSeverity::Critical;
    }
    if lower.contains("metadata")
        && (lower.contains("lost") || lower.contains("missing") || lower.contains("strip"))
    {
        return ErrorSeverity::MetadataLoss;
    }
    if lower.contains("broken pipe")
        || lower.contains("unexpected eof")
        || lower.contains("connection reset")
    {
        return ErrorSeverity::PipelineBroken;
    }
    if lower.contains("assertion failed")
        || lower.contains("segmentation fault")
        || lower.contains("bus error")
    {
        return ErrorSeverity::Rare;
    }
    if lower.contains("could find no file") || lower.contains("pattern_type") {
        return ErrorSeverity::Rare;
    }
    if (lower.contains("cjxl")
        || lower.contains("magick")
        || lower.contains("ffmpeg")
        || lower.contains("ffprobe"))
        && (lower.contains("exit code") || lower.contains("failed") || lower.contains("error"))
    {
        return ErrorSeverity::UpstreamError;
    }
    ErrorSeverity::Standard
}

// ── Convenience macros ──

#[macro_export]
macro_rules! log_critical {
    ($ctx:expr, $($arg:tt)*) => {
        $crate::static_logs::log_enhanced_error(
            $crate::static_logs::ErrorSeverity::Critical,
            $ctx, &format!($($arg)*),
        )
    };
}

#[macro_export]
macro_rules! log_rare_error {
    ($ctx:expr, $($arg:tt)*) => {
        $crate::static_logs::log_enhanced_error(
            $crate::static_logs::ErrorSeverity::Rare,
            $ctx, &format!($($arg)*),
        )
    };
}

#[macro_export]
macro_rules! log_metadata_loss {
    ($ctx:expr, $($arg:tt)*) => {
        $crate::static_logs::log_enhanced_error(
            $crate::static_logs::ErrorSeverity::MetadataLoss,
            $ctx, &format!($($arg)*),
        )
    };
}

#[macro_export]
macro_rules! log_pipeline_broken {
    ($ctx:expr, $($arg:tt)*) => {
        $crate::static_logs::log_enhanced_error(
            $crate::static_logs::ErrorSeverity::PipelineBroken,
            $ctx, &format!($($arg)*),
        )
    };
}

#[macro_export]
macro_rules! log_upstream_error {
    ($ctx:expr, $($arg:tt)*) => {
        $crate::static_logs::log_enhanced_error(
            $crate::static_logs::ErrorSeverity::UpstreamError,
            $ctx, &format!($($arg)*),
        )
    };
}

#[macro_export]
macro_rules! log_auto_error {
    ($ctx:expr, $($arg:tt)*) => {{
        let _msg = format!($($arg)*);
        $crate::static_logs::log_enhanced_error(
            $crate::static_logs::classify_error(&_msg),
            $ctx, &_msg,
        )
    }};
}

// ─── Themed Progress Logging (High-Heat Exempt) ──────────────────────────────

/// Logs a file that was successfully converted.
pub fn log_success(label: &str, detail: &str) {
    let colored_label = TerminalColor::success(&format!("[{label}]"));
    tracing::info!("{} {} {}", symbols::SUCCESS, colored_label, detail);
}

#[macro_export]
macro_rules! log_success {
    ($label:expr, $detail:literal, $($arg:tt)+) => {
        $crate::static_logs::log_success($label, &format!($detail, $($arg)+))
    };
    ($label:expr, $detail:literal $(,)?) => {
        $crate::static_logs::log_success($label, &format!($detail))
    };
    ($label:expr, $detail:expr $(,)?) => {
        $crate::static_logs::log_success($label, $detail)
    };
    ($detail:expr $(,)?) => {
        $crate::static_logs::log_success("Success", $detail)
    };
}

/// Logs a file that was skipped.
pub fn log_skip(label: &str, detail: &str) {
    let colored_label = TerminalColor::warning(&format!("[{label}]"));
    tracing::info!("⏭️  {} {}", colored_label, detail);
}

#[macro_export]
macro_rules! log_skip {
    ($label:expr, $detail:literal, $($arg:tt)+) => {
        $crate::static_logs::log_skip($label, &format!($detail, $($arg)+))
    };
    ($label:expr, $detail:literal $(,)?) => {
        $crate::static_logs::log_skip($label, &format!($detail))
    };
    ($label:expr, $detail:expr $(,)?) => {
        $crate::static_logs::log_skip($label, $detail)
    };
    ($detail:expr $(,)?) => {
        $crate::static_logs::log_skip("Skip", $detail)
    };
}

/// Logs a file that was ignored (yielded to peer module).
pub fn log_ignore(detail: &str) {
    tracing::info!("⚪ {}", detail);
}

#[macro_export]
macro_rules! log_ignore {
    ($detail:literal, $($arg:tt)+) => {
        $crate::static_logs::log_ignore(&format!($detail, $($arg)+))
    };
    ($detail:literal $(,)?) => {
        $crate::static_logs::log_ignore(&format!($detail))
    };
    ($detail:expr $(,)?) => {
        $crate::static_logs::log_ignore($detail)
    };
}

/// Logs a file that failed conversion.
pub fn log_failure(label: &str, detail: &str) {
    let colored_label = TerminalColor::error(&format!("[{label}]"));
    tracing::warn!("❌ {} {}", colored_label, detail);
}

#[macro_export]
macro_rules! log_failure {
    ($label:expr, $detail:literal, $($arg:tt)+) => {
        $crate::static_logs::log_failure($label, &format!($detail, $($arg)+))
    };
    ($label:expr, $detail:literal $(,)?) => {
        $crate::static_logs::log_failure($label, &format!($detail))
    };
    ($label:expr, $detail:expr $(,)?) => {
        $crate::static_logs::log_failure($label, $detail)
    };
    ($detail:expr $(,)?) => {
        $crate::static_logs::log_failure("Failure", $detail)
    };
}

// ─── Systemic Health Logging (Anomalies & Integrity) ─────────────────────────

/// Logs an anomaly (minor error or unexpected state).
pub fn log_anomaly(label: &str, detail: &str) {
    let colored_label = TerminalColor::warning(&format!("[{label}]"));
    tracing::warn!("☢️  {} {}", colored_label, detail);
}

#[macro_export]
macro_rules! log_anomaly {
    ($label:expr, $detail:literal, $($arg:tt)+) => {
        $crate::static_logs::log_anomaly($label, &format!($detail, $($arg)+))
    };
    ($label:expr, $detail:literal $(,)?) => {
        $crate::static_logs::log_anomaly($label, &format!($detail))
    };
    ($label:expr, $detail:expr $(,)?) => {
        $crate::static_logs::log_anomaly($label, $detail)
    };
}

/// Logs data corruption or structural integrity issues.
pub fn log_corruption(label: &str, detail: &str) {
    let colored_label = TerminalColor::critical(&format!("[{label}]"));
    tracing::warn!("💀 {} [CORRUPTION] {}", colored_label, detail);
}

#[macro_export]
macro_rules! log_corruption {
    ($label:expr, $detail:literal, $($arg:tt)+) => {
        $crate::static_logs::log_corruption($label, &format!($detail, $($arg)+))
    };
    ($label:expr, $detail:literal $(,)?) => {
        $crate::static_logs::log_corruption($label, &format!($detail))
    };
    ($label:expr, $detail:expr $(,)?) => {
        $crate::static_logs::log_corruption($label, $detail)
    };
}

/// Logs a fatal error that may cause batch termination.
pub fn log_fatal(label: &str, detail: &str) {
    let colored_label = TerminalColor::critical(&format!("[{label}]"));
    tracing::error!("🛑 [FATAL] {} {}", colored_label, detail);
}

#[macro_export]
macro_rules! log_fatal {
    ($label:expr, $detail:literal, $($arg:tt)+) => {
        $crate::static_logs::log_fatal($label, &format!($detail, $($arg)+))
    };
    ($label:expr, $detail:literal $(,)?) => {
        $crate::static_logs::log_fatal($label, &format!($detail))
    };
    ($label:expr, $detail:expr $(,)?) => {
        $crate::static_logs::log_fatal($label, $detail)
    };
    ($detail:expr $(,)?) => {
        $crate::static_logs::log_fatal("Fatal", $detail)
    };
}

/// Logs a detailed step within a stage.
pub fn log_detail(detail: &str) {
    tracing::info!("   {}", detail);
}

#[macro_export]
macro_rules! log_detail {
    ($detail:literal, $($arg:tt)+) => {
        $crate::static_logs::log_detail(&format!($detail, $($arg)+))
    };
    ($detail:literal $(,)?) => {
        $crate::static_logs::log_detail(&format!($detail))
    };
    ($detail:expr $(,)?) => {
        $crate::static_logs::log_detail($detail)
    };
    () => {
        $crate::static_logs::log_detail("")
    };
}

/// Logs a labeled informational message.
pub fn log_info(label: &str, detail: &str) {
    let colored_label = TerminalColor::info(&format!("[{label}]"));
    tracing::info!("   {} {}", colored_label, detail);
}

#[macro_export]
macro_rules! log_info {
    ($label:expr, $detail:literal, $($arg:tt)+) => {
        $crate::static_logs::log_info($label, &format!($detail, $($arg)+))
    };
    ($label:expr, $detail:literal $(,)?) => {
        $crate::static_logs::log_info($label, &format!($detail))
    };
    ($label:expr, $detail:expr $(,)?) => {
        $crate::static_logs::log_info($label, $detail)
    };
}

/// Logs a recommendation or hint.
pub fn log_hint(label: &str, detail: &str) {
    let colored_label = TerminalColor::value(&format!("[{label}]"));
    tracing::info!("💡 {} {}", colored_label, detail);
}

#[macro_export]
macro_rules! log_hint {
    ($label:expr, $detail:literal, $($arg:tt)+) => {
        $crate::static_logs::log_hint($label, &format!($detail, $($arg)+))
    };
    ($label:expr, $detail:literal $(,)?) => {
        $crate::static_logs::log_hint($label, &format!($detail))
    };
    ($label:expr, $detail:expr $(,)?) => {
        $crate::static_logs::log_hint($label, $detail)
    };
    ($detail:expr $(,)?) => {
        $crate::static_logs::log_hint("Hint", $detail)
    };
}

/// Logs a debug message (only visible in debug builds or with `RUST_LOG=debug`).
pub fn log_debug(detail: &str) {
    tracing::debug!("   {}", detail);
}

#[macro_export]
macro_rules! log_debug {
    ($detail:literal, $($arg:tt)+) => {
        $crate::static_logs::log_debug(&format!($detail, $($arg)+))
    };
    ($detail:literal $(,)?) => {
        $crate::static_logs::log_debug(&format!($detail))
    };
    ($detail:expr $(,)?) => {
        $crate::static_logs::log_debug($detail)
    };
}

/// Logs a trace message (extremely verbose).
pub fn log_trace(detail: &str) {
    tracing::trace!("   {}", detail);
}

#[macro_export]
macro_rules! log_trace {
    ($($arg:tt)*) => {
        $crate::static_logs::log_trace(&format!($($arg)*))
    };
}

/// Logs a stage or high-level progress.
pub fn log_stage(icon: &str, stage: &str, detail: &str) {
    tracing::info!("{} {}: {}", icon, stage, detail);
}

/// Logs a file saved message.
pub fn log_file_saved(label: &str, path: &std::path::Path) {
    tracing::info!("💾 {}: {}", label, path.display());
}

#[macro_export]
macro_rules! log_file_saved {
    ($label:expr, $path:expr) => {
        $crate::static_logs::log_file_saved($label, $path)
    };
}

// ─── Centralized Static Messages ─────────────────────────────────────────────
pub mod messages {
    pub const BATCH_START: &str = "🚀 Starting batch processing...";
    pub const BATCH_COMPLETE: &str = "🏁 Batch processing complete.";
    pub const SCANNING_FILES: &str = "🔍 Scanning for media files...";
    pub const INITIALIZING_CACHE: &str = "💾 Initializing analysis cache...";
    pub const LOADING_CHECKPOINT: &str = "🔄 Loading checkpoint...";
    pub const SAVING_CHECKPOINT: &str = "💾 Saving checkpoint...";
    pub const TOOLS_CHECKING: &str = "🛠️  Checking tool availability...";
    pub const FATAL_STOP: &str = "🛑 Fatal error encountered, stopping batch processing.";
    pub const OUTPUT_VERIFY: &str = "🔍 Verifying output completeness...";
    pub const METADATA_SYNC: &str = "📁 Preserving directory metadata...";
    pub const COPYING_UNSUPPORTED: &str = "📦 Copying unsupported files...";
    pub const CONVERSION_SUMMARY: &str = "📊 Conversion Summary:";
    pub const STRATEGY_HINT: &str = "Strategy: deeper paths -> lighter workload -> shorter duration -> smaller files -> lower resolution";
    pub const DOVI_TOOL_MISSING: &str =
        "⚠️  dovi_tool not found — Dolby Vision RPU cannot be preserved, falling back to HDR10";
    pub const DOVI_TOOL_INSTALL: &str = "💡 Install with: cargo install dovi_tool";
    pub const HDR10PLUS_TOOL_MISSING: &str = "⚠️  hdr10plus_tool not found — HDR10+ dynamic metadata cannot be preserved, falling back to HDR10";
    pub const INPUT_REPORT: &str = "   Input:  {path} ({size} bytes)";
    pub const OUTPUT_REPORT: &str = "   Output: {path} ({size} bytes)";
    pub const RESULT_REPORT: &str = "   Result: {message}";
    pub const APPLE_COMPAT_NOT_COPYING: &str =
        "Apple-compat fallback: not copying incompatible original";
    pub const APPLE_COMPAT_NOT_COPYING_DETAILED: &str =
        "   ⚠️  Apple compatibility mode: not copying incompatible original";
    pub const GPU_PROBE_START: &str = "Detecting GPU acceleration...";
    pub const GPU_PROBE_FAILED: &str = "GPU probe failed ({reason}), using CPU encoding";
    pub const GPU_DETECTED: &str = "   ✅ GPU: {gpu_type} detected";
    pub const GPU_NOT_AVAILABLE: &str = "   ⚠️ No GPU acceleration available, using CPU encoding";

    // --- Database (PostgreSQL) ---
    pub const DB_UNAVAILABLE: &str = "⚠️  Database Unavailable";
    pub const DB_CONNECTED: &str =
        "🐘 Database [PostgreSQL]: CONNECTED (Full Learning Mode Active)";
    pub const DB_SCHEMA_READY: &str = "✅ Database Schema Ready.";
    pub const DB_IMPORT_START: &str = "📥 Importing Default High-Value GIF Training Dataset...";
    pub const DB_IMPORT_COMPLETE: &str = "✅ Training Dataset successfully imported.";

    // --- Tools ---
    pub const X265_MISSING: &str = "⚠️  x265 tool not found — HEVC software encoding unavailable";
    pub const X265_INSTALL: &str = "💡 Install with: brew install x265";

    // --- Labels ---
    pub const LABEL_QUALITY: &str = "Quality";
    pub const LABEL_QUALITY_FAIL: &str = "Quality FAILED";
    pub const LABEL_QUALITY_SIZE_FAIL: &str = "QUALITY/SIZE VALIDATION FAILED";
    pub const LABEL_SSIM_CALC_FAILED: &str = "SSIM CALCULATION FAILED";
    pub const LABEL_3D_QUALITY_GATE_FAILED: &str = "3D QUALITY GATE FAILED";
    pub const LABEL_QUALITY_TARGET_FAILED: &str = "QUALITY TARGET FAILED";
    pub const LABEL_SIZE_FAIL: &str = "Size FAILED";
    pub const LABEL_CLEANUP: &str = "Cleanup";
    pub const LABEL_FFMPEG: &str = "FFmpeg";
    pub const LABEL_FFPROBE: &str = "FFprobe";
    pub const LABEL_WEBP: &str = "WebP";
    pub const LABEL_HEIC: &str = "HEIC";
    pub const LABEL_AVIF: &str = "AVIF";
    pub const LABEL_JXL: &str = "JXL";
    pub const LABEL_GIFSKI: &str = "Gifski";
    pub const LABEL_DV: &str = "Dolby Vision";
    pub const LABEL_DV_FAILED: &str = "Dolby Vision FAILED";
    pub const LABEL_HDR10PLUS: &str = "HDR10+";
    pub const LABEL_HDR10PLUS_FAILED: &str = "HDR10+ FAILED";
    pub const LABEL_DELETE: &str = "Delete";
    pub const LABEL_DELETE_FAILED: &str = "Delete FAILED";
    pub const LABEL_CHECKPOINT: &str = "Checkpoint";
    pub const LABEL_LOCK: &str = "Lock";
    pub const LABEL_CACHE: &str = "Cache";
    pub const LABEL_CONFIG: &str = "Configuration";
    pub const LABEL_TOOLS: &str = "Tools";
    pub const LABEL_GHOST_MODE: &str = "Ghost Mode";
    pub const LABEL_LOGGING: &str = "Logging";
    pub const LABEL_RUN_LOG: &str = "Run Log";
    pub const LABEL_TIMESTAMP: &str = "Timestamp";
    pub const LABEL_MEDIA_INFO: &str = "Media Info";
    pub const LABEL_ANOMALY: &str = "Anomaly";
    pub const LABEL_BITRATE: &str = "Bitrate";
    pub const LABEL_COLOR_SPACE: &str = "Color Space";
    pub const LABEL_MS_SSIM: &str = "MS-SSIM";
    pub const LABEL_VMAF: &str = "VMAF";
    pub const LABEL_CAMBI: &str = "CAMBI";
    pub const LABEL_PRECHECK: &str = "Precheck";
    pub const LABEL_METADATA: &str = "Metadata";
    pub const LABEL_PHASE_1: &str = "Phase 1";
    pub const LABEL_PHASE_2: &str = "Phase 2";
    pub const LABEL_PHASE_3: &str = "Phase 3";
    pub const LABEL_PHASE_4: &str = "Phase 4";
    pub const LABEL_STRATEGY: &str = "Strategy";
    pub const LABEL_GPU: &str = "GPU";
    pub const LABEL_GPU_QUALITY: &str = "GPU Quality Ceiling";
    pub const LABEL_DETECTION: &str = "Detection";
    pub const LABEL_HEURISTIC: &str = "HEURISTIC";
    pub const LABEL_SYSTEM: &str = "System";

    pub const VAL_EXCELLENT: &str = "Excellent";
    pub const VAL_VERY_GOOD: &str = "Very Good";
    pub const VAL_GOOD_MEETS_TARGET: &str = "Good (meets target)";
    pub const VAL_BELOW_TARGET: &str = "Below Target";
    pub const VAL_GOOD: &str = "Good";
    pub const VAL_FAILED: &str = "FAILED";
    pub const VAL_READY: &str = "READY FOR 3D GATE";
    pub const VAL_SUCCESS: &str = "SUCCESS";
    pub const LABEL_PHASE_5: &str = "Phase 5";
    pub const LABEL_CALIBRATION: &str = "Calibration";
    pub const LABEL_REPORT: &str = "Report";
    pub const LABEL_COPY: &str = "Copy";
    pub const LABEL_XMP: &str = "XMP";
    pub const LABEL_ENCODER: &str = "Encoder";
    pub const LABEL_BATCH: &str = "Batch";
    pub const LABEL_DISK: &str = "Disk";
    pub const LABEL_THREAD: &str = "Thread";
    pub const LABEL_VERIFY: &str = "Verify";
    pub const LABEL_CONVERSION: &str = "Conversion";
    pub const LABEL_INTENT: &str = "Intent";
    pub const LABEL_NUMERIC: &str = "Numeric";
    pub const LABEL_DYNAMIC: &str = "Dynamic";
    pub const LABEL_PENETRATION: &str = "Penetration";
    pub const LABEL_PROBE: &str = "Probe";
    pub const LABEL_IMAGE: &str = "Image";
    pub const LABEL_VIDEO: &str = "Video";

    // --- Common Messages ---
    pub const QUALITY_GATE_FAILED: &str = "Final quality gate failed";
    pub const SSIM_CALC_FAILED: &str = "SSIM calculation failed";
    pub const APPLE_COMPAT_HEVC: &str =
        "Apple compatibility mode (--apple-compat) is ONLY supported for HEVC.";
    pub const RUN_LOG_OPEN_FAIL: &str = "Could not open run log file";
    pub const DIR_LOCK_FAIL: &str = "Failed to acquire directory lock";
    pub const PROTECTING_ORIGINAL: &str = "🛡️  Original file PROTECTED";
    pub const DISCARDING_OUTPUT: &str = "🗑️  Output discarded";
    pub const CACHE_INIT_FAIL: &str = "Cache initialization failed";
    pub const LOGGING_INIT_FAIL: &str = "Logging initialization failed";

    pub const PROTECT_QUALITY_SIZE: &str = "Original file PROTECTED (quality/size check failed)";
    pub const DISCARD_QUALITY_SIZE: &str = "Output discarded (quality/size check failed)";
    pub const PROTECT_SSIM_NA: &str = "Original file PROTECTED (SSIM not available)";
    pub const DISCARD_SSIM_FAIL: &str = "Output discarded (SSIM calculation failed)";
    pub const PROTECT_QUALITY_LOW: &str = "Original file PROTECTED (quality below threshold)";
    pub const DISCARD_QUALITY_LOW: &str = "Output discarded (quality below threshold)";
}

/// Logs a banner for important announcements with a solid decorative border.
pub fn log_banner(title: &str) {
    let theme_color = "\x1b[38;2;67;160;255m";
    let reset = "\x1b[0m";
    let border = "━".repeat(title.len() + 6);
    tracing::info!("{}{}{}", theme_color, border, reset);
    tracing::info!("{}  ┃ {} ┃  {}", theme_color, title, reset);
    tracing::info!("{}{}{}", theme_color, border, reset);
}

#[macro_export]
macro_rules! log_banner {
    ($title:expr) => {
        $crate::static_logs::log_banner($title)
    };
}

/// Logs a major execution step (e.g. [1/5] Initialization).
pub fn log_step(current: usize, total: usize, title: &str) {
    let theme_color = "\x1b[38;2;67;160;255m";
    let reset = "\x1b[0m";
    tracing::info!(
        "\n{}┯━━━━━ Step [{}/{}] ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━{}",
        theme_color,
        current,
        total,
        reset
    );
    tracing::info!("{}│ 🔷 {}{}", theme_color, title, reset);
    tracing::info!(
        "{}┷━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━{}",
        theme_color,
        reset
    );
}

#[macro_export]
macro_rules! log_step {
    ($current:expr, $total:expr, $title:expr) => {
        $crate::static_logs::log_step($current as usize, $total as usize, $title)
    };
}

/// Logs a sub-step or internal stage.
pub fn log_substep(title: &str) {
    let theme_color = "\x1b[38;2;33;150;243m";
    let reset = "\x1b[0m";
    tracing::info!("{}   🔹 {}{}", theme_color, title, reset);
}

#[macro_export]
macro_rules! log_substep {
    ($title:expr) => {
        $crate::static_logs::log_substep($title)
    };
}

/// Logs a statistic line (e.g. for tables).
pub fn log_stat(label: &str, value: &str) {
    tracing::info!("   ├─ {:<20}: {}", label, value);
}

#[macro_export]
macro_rules! log_stat {
    ($label:expr, $value:expr $(,)?) => {
        $crate::static_logs::log_stat($label, &format!("{}", $value))
    };
    ($label:expr, $fmt:literal, $($arg:tt)+) => {
        $crate::static_logs::log_stat($label, &format!($fmt, $($arg)+))
    };
}

/// Logs a summary section header.
pub fn log_summary_header(title: &str) {
    let theme_color = "\x1b[38;2;67;160;255m";
    let reset = "\x1b[0m";
    tracing::info!("\n{}📊 {} Statistics{}", theme_color, title, reset);
    tracing::info!(
        "{}═══════════════════════════════════════{}",
        theme_color,
        reset
    );
}

#[macro_export]
macro_rules! log_summary_header {
    ($title:expr) => {
        $crate::static_logs::log_summary_header($title)
    };
}

/// Convenience macro for logging static messages.
#[macro_export]
macro_rules! log_static {
    (info, $msg_const:expr) => {
        tracing::info!("{}", $msg_const);
    };
    (warn, $msg_const:expr) => {
        tracing::warn!("{}", $msg_const);
    };
    (error, $msg_const:expr) => {
        tracing::error!("{}", $msg_const);
    };
    (debug, $msg_const:expr) => {
        tracing::debug!("{}", $msg_const);
    };
    (trace, $msg_const:expr) => {
        tracing::trace!("{}", $msg_const);
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_severity_labels() {
        assert_eq!(ErrorSeverity::Critical.label(), "[CRITICAL]");
        assert!(ErrorSeverity::Critical.label_colored().contains("CRITICAL"));
        assert!(ErrorSeverity::Rare.label_colored().contains("RARE ERROR"));
    }

    #[test]
    fn test_classify_error() {
        assert_eq!(classify_error("File is corrupt"), ErrorSeverity::Critical);
        assert_eq!(
            classify_error("metadata is missing"),
            ErrorSeverity::MetadataLoss
        );
        assert_eq!(classify_error("broken pipe"), ErrorSeverity::PipelineBroken);
        assert_eq!(classify_error("assertion failed"), ErrorSeverity::Rare);
        assert_eq!(
            classify_error("ffmpeg failed with exit code 1"),
            ErrorSeverity::UpstreamError
        );
        assert_eq!(classify_error("some random error"), ErrorSeverity::Standard);
    }
}
