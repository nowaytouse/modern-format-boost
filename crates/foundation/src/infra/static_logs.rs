//! Centralized Static Log Messages and Themed Logging
//!
//! This module centralizes frequently used log messages and provides themed
//! logging functions to ensure consistent styling across the workspace.

use crate::modern_ui::{TerminalColor, symbols};

const PLAIN_DETAIL_PREFIXES: &[(&str, &str)] = &[
    ("✅ ", "[OK] "),
    ("❌ ", "[ERR] "),
    ("⚠️  ", "[WARN] "),
    ("⚠️ ", "[WARN] "),
    ("💡 ", "[i] "),
    ("📦 ", "[PKG] "),
    ("💾 ", "[SAVE] "),
    ("📋 ", "[META] "),
    ("⏭️  ", "[SKIP] "),
    ("⏭️ ", "[SKIP] "),
    ("🔄 ", "[..] "),
    ("🔬 ", "[AUDIT] "),
    ("📂 ", "[DIR] "),
    ("✓ ", "[+] "),
    ("✗ ", "[x] "),
    ("📊 ", "[MET] "),
];

// ─── Error Logging Infrastructure (Moved from error_logging.rs)
// ───────────────

/// Error severity levels for enhanced visibility
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorSeverity {
    /// Fatal error that requires immediate termination
    Fatal,
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
            Self::Fatal => "[FATAL]",
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
        crate::media_conversion_gate::ui_error_severity_colored_label(*self)
    }
}

/// Rewrite user-facing detail text for plain mode (strip leading decorative
/// emoji).
#[must_use]
pub fn plain_aware_detail(detail: &str) -> String {
    if !crate::progress_mode::is_plain_mode() {
        return detail.to_string();
    }
    let trimmed = detail.trim_start_matches('\r');
    let lead = &detail[..detail.len() - trimmed.len()];
    let visible = trimmed.trim_start();
    let indent = &trimmed[..trimmed.len() - visible.len()];
    for (emoji, ascii) in PLAIN_DETAIL_PREFIXES {
        if let Some(rest) = visible.strip_prefix(emoji) {
            return format!("{lead}{indent}{ascii}{rest}");
        }
    }
    detail.to_string()
}

/// Get the themed symbol for a given log label.
#[must_use]
pub fn get_symbol_by_label(label: &str) -> &'static str {
    if crate::progress_mode::is_plain_mode() {
        return match label {
            "Quality Integrity Violation" => "[ERR]",
            "Infrastructure Audit" => "[DB]",
            "Batch Audit" => "[PKG]",
            "Checkpoint Audit" => "[CHK]",
            "Verification Audit" | "Pixel-Perfect Verification" => "[VER]",
            "Cache Audit"
            | "Caching Audit"
            | "Cache Inventory Audit"
            | "Cache Storage Audit"
            | "Cache Schema Audit" => "[CACHE]",
            "System Audit" => "[SYS]",
            "Recovery Audit" => "[REC]",
            "Discovery Audit: Streams"
            | "Detection Audit"
            | "Discovery Audit: Storage"
            | "Discovery Audit: Precheck"
            | "Blind Spot Discovery Audit"
            | "Exploration Strategy Audit"
            | "Analysis Audit" => "[FIND]",
            "Operation Audit: Success" | "Readiness Audit" | "Session Summary" => "[OK]",
            "Validation Audit" => "[TEST]",
            "Pipeline Configuration" | "Process Audit: Concurrency" => "[CFG]",
            "System Anomaly" => "[WARN]",
            "Metadata Integrity" => "[META]",
            "Strategy Audit" => "[STRAT]",
            "Mapping Audit" => "[MAP]",
            "Session Finalized" => "[DONE]",
            "MS-SSIM Audit" | "MS-SSIM Structural Audit" => "[MS]",
            "Session Audit: Log Persistence" => "[LOG]",
            "HEIC Forensic Audit" => "[HEIC]",
            "Inference Log Audit" => "[INF]",
            "Feature Power Audit" => "[AUDIT]",
            "UltraHDR Synthesis Audit" | "HDR Synthesis Audit" => "[HDR]",
            "Confidence Metric Audit" => "[CONF]",
            "Structural Bitstream Audit" => "[BITS]",
            "Perceptual Quality Audit" => "[QUAL]",
            "VMAF Perceptual Audit" => "[VMAF]",
            "CAMBI Banding Audit" => "[CAMBI]",
            "Chroma Fidelity Audit" => "[CHROMA]",
            "Gainmap Forensic Audit" => "[GAIN]",
            "Matching Decision Audit" => "[MATCH]",
            "Forensic Hint" => "[i]",
            _ => "",
        };
    }
    match label {
        "Analysis Audit"
        | "Discovery Audit: Streams"
        | "Detection Audit"
        | "Discovery Audit: Storage"
        | "Discovery Audit: Precheck"
        | "Blind Spot Discovery Audit"
        | "Exploration Strategy Audit" => "🔍",
        "Quality Integrity Violation" => "❌",
        "Infrastructure Audit" => "🗄️",
        "Batch Audit" => "📦",
        "Checkpoint Audit" => "📍",
        "Verification Audit" | "Pixel-Perfect Verification" => "📏",
        "Cache Audit"
        | "Caching Audit"
        | "Cache Inventory Audit"
        | "Cache Storage Audit"
        | "Cache Schema Audit" => "💾",
        "System Audit" => "🖥️",
        "Recovery Audit" => "🏥",
        "Operation Audit: Success" | "Readiness Audit" | "Session Summary" => "✅",
        "Validation Audit" => "🧪",
        "Pipeline Configuration" | "Process Audit: Concurrency" => "⚙️",
        "System Anomaly" => "⚠️",
        "Metadata Integrity" => "📋",
        "Strategy Audit" => "🎯",
        "Mapping Audit" => "🗺️",
        "Session Finalized" => "🎉",
        "MS-SSIM Audit" => "⏱️",
        "Session Audit: Log Persistence" => "📜",
        "HEIC Forensic Audit" => "📸",
        "Inference Log Audit" | "MS-SSIM Structural Audit" => "📊",
        "Feature Power Audit" => "🔬",
        "UltraHDR Synthesis Audit" => "☀",
        "Confidence Metric Audit" => "🛡️",
        "Structural Bitstream Audit" => "🏗️",
        "Perceptual Quality Audit" | "VMAF Perceptual Audit" => "🧠",
        "CAMBI Banding Audit" => "🌈",
        "Chroma Fidelity Audit" => "🎨",
        "HDR Synthesis Audit" => "☢️",
        "Gainmap Forensic Audit" => "📉",
        "Matching Decision Audit" => "🤖",
        "Forensic Hint" => "💡",
        _ => "",
    }
}

/// Emit an enhanced error via tracing for full audit-readiness and vertical
/// alignment.
pub fn log_enhanced_error(severity: ErrorSeverity, context: &str, detail: &str) {
    crate::ctrlc_guard::wait_if_prompt_active();
    let symbol = get_symbol_by_label(context);
    let detail = plain_aware_detail(detail);
    let layout = DiagnosticLayout::new(severity.label_colored(), format!("{symbol} {detail}"));
    match severity {
        ErrorSeverity::Critical | ErrorSeverity::PipelineBroken => {
            tracing::error!(target: "static_log", label = context, "{}", layout.render());
        }
        _ => {
            tracing::warn!(target: "static_log", label = context, "{}", layout.render());
        }
    }
}

/// Emit an enhanced debug log.
pub fn log_enhanced_debug(context: &str, detail: &str) {
    crate::ctrlc_guard::wait_if_prompt_active();
    let label = TerminalColor::debug(&format!("[{context}]"));
    let symbol = get_symbol_by_label(context);
    let icon = if symbol.is_empty() {
        crate::media_conversion_gate::ui_icon_pick(symbols::SEARCH, symbols::plain::SEARCH)
    } else {
        symbol.to_string()
    };
    let detail = plain_aware_detail(detail);
    let layout = DiagnosticLayout::new(label, format!("{icon} {detail}"));
    tracing::debug!(target: "static_log", "{}", layout.render());
}

/// Emit an enhanced info log.
pub fn log_enhanced_info(context: &str, detail: &str) {
    crate::ctrlc_guard::wait_if_prompt_active();
    let label = TerminalColor::info(&format!("[{context}]"));
    let symbol = get_symbol_by_label(context);
    let icon = if symbol.is_empty() {
        crate::media_conversion_gate::ui_icon_pick(symbols::INFO, symbols::plain::INFO)
    } else {
        symbol.to_string()
    };
    let detail = plain_aware_detail(detail);
    let layout = DiagnosticLayout::new(label, format!("{icon} {detail}"));
    tracing::info!(target: "static_log", "{}", layout.render());
}

/// Emit an enhanced report info log.
pub fn log_enhanced_report_info(context: &str, detail: &str) {
    crate::ctrlc_guard::wait_if_prompt_active();
    let label = TerminalColor::info(&format!("[{context}]"));
    let symbol = get_symbol_by_label(context);
    let icon = if symbol.is_empty() {
        crate::media_conversion_gate::ui_icon_pick(symbols::INFO, symbols::plain::INFO)
    } else {
        symbol.to_string()
    };
    let detail = plain_aware_detail(detail);
    let layout = DiagnosticLayout::new(label, format!("{icon} {detail}"));
    tracing::info!(target: "mfb::report", "{}", layout.render());
}

/// Emit an enhanced warn log.
pub fn log_enhanced_warn(context: &str, detail: &str) {
    crate::ctrlc_guard::wait_if_prompt_active();
    let label = TerminalColor::warning(&format!("[{context}]"));
    let symbol = get_symbol_by_label(context);
    let icon = if symbol.is_empty() {
        crate::media_conversion_gate::ui_icon_pick(symbols::WARNING, symbols::plain::WARNING)
    } else {
        symbol.to_string()
    };
    let detail = plain_aware_detail(detail);
    let layout = DiagnosticLayout::new(label, format!("{icon} {detail}"));
    tracing::warn!(target: "static_log", "{}", layout.render());
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

/// Emit an enhanced stage log.
pub fn log_stage(symbol: &str, context: &str, detail: &str) {
    let label = TerminalColor::info(&format!("[{context}]"));
    let layout = DiagnosticLayout::new(label, format!("{symbol} {detail}"));
    tracing::info!(target: "static_log", "{}", layout.render());
}

/// Themed Layout Engine for consistent terminal diagnostics.
struct DiagnosticLayout {
    label: String,
    message: String,
}

impl DiagnosticLayout {
    const fn new(label: String, message: String) -> Self {
        Self { label, message }
    }

    /// Renders a fully aligned diagnostic line including thread-local context.
    fn render(&self) -> String {
        let context = crate::progress_mode::format_log_line("");
        let trimmed_context = context.trim_end();

        // Rationale: Labels and file contexts are padded for rigid vertical columns.
        let context_target_width = 30;
        let plain_context = crate::logging::strip_ansi_str(trimmed_context);
        let context_width = visual_width(&plain_context);
        let context_padding = if context_width < context_target_width {
            " ".repeat(context_target_width - context_width)
        } else {
            String::new()
        };

        let label_width = 18;
        let plain_label = crate::logging::strip_ansi_str(&self.label);
        let current_width = visual_width(&plain_label);
        let padding = if current_width < label_width {
            " ".repeat(label_width - current_width)
        } else {
            String::new()
        };

        let lines: Vec<&str> = self.message.split('\n').collect();
        if lines.is_empty() {
            return String::new();
        }

        if lines.len() == 1 {
            if trimmed_context.is_empty() {
                format!("{}{} │ {}", self.label, padding, self.message)
            } else {
                format!(
                    "{}{} │ {}{} │ {}",
                    trimmed_context, context_padding, self.label, padding, self.message
                )
            }
        } else {
            use std::fmt::Write;
            let mut result = String::new();
            if trimmed_context.is_empty() {
                let _ = write!(
                    result,
                    "{label}{pad} │ {first}",
                    label = self.label,
                    pad = padding,
                    first = lines[0]
                );
            } else {
                let _ = write!(
                    result,
                    "{context}{context_pad} │ {label}{pad} │ {first}",
                    context = trimmed_context,
                    context_pad = context_padding,
                    label = self.label,
                    pad = padding,
                    first = lines[0]
                );
            }

            let indent_context = " ".repeat(context_target_width);
            let indent_label = " ".repeat(label_width);
            for line in &lines[1..] {
                result.push('\n');
                if trimmed_context.is_empty() {
                    let _ = write!(result, "{indent_label} │ {line}");
                } else {
                    let _ = write!(result, "{indent_context} │ {indent_label} │ {line}");
                }
            }
            result
        }
    }
}

/// Calculate the visual display width of a string, accounting for multi-byte
/// Unicode characters.
fn visual_width(s: &str) -> usize {
    s.chars()
        .map(|c| {
            let cp = u32::from(c);
            // Basic heuristic for common Emojis and wide characters in this project
            if (0x1F300..=0x1F9FF).contains(&cp) || (0x2600..=0x26FF).contains(&cp) || cp > 0xFF {
                2
            } else {
                1
            }
        })
        .sum()
}

fn truncate_to_visual_width(s: &str, max_width: usize) -> String {
    if visual_width(s) <= max_width {
        return s.to_string();
    }

    let ellipsis = "...";
    let ellipsis_width = visual_width(ellipsis);
    if max_width <= ellipsis_width {
        return ellipsis.chars().take(max_width).collect();
    }

    let budget = max_width.saturating_sub(ellipsis_width);
    let mut out = String::new();
    let mut used = 0usize;

    for ch in s.chars() {
        let ch_width = visual_width(&ch.to_string());
        if used + ch_width > budget {
            break;
        }
        out.push(ch);
        used += ch_width;
    }

    out.push_str(ellipsis);
    out
}

#[macro_export]
macro_rules! log_anomaly {
    ($label:expr, $msg:literal $(,)?) => {
        $crate::infra::static_logs::log_enhanced_error(
            $crate::infra::static_logs::ErrorSeverity::Rare,
            $label, &format!("{} {}", $crate::media_conversion_gate::ui_icon_pick($crate::modern_ui::symbols::WARNING, $crate::modern_ui::symbols::plain::WARNING), format!($msg)),
        )
    };
    ($label:expr, $msg:expr $(,)?) => {
        $crate::infra::static_logs::log_enhanced_error(
            $crate::infra::static_logs::ErrorSeverity::Rare,
            $label, &format!("{} {}", $crate::media_conversion_gate::ui_icon_pick($crate::modern_ui::symbols::WARNING, $crate::modern_ui::symbols::plain::WARNING), $msg),
        )
    };
    ($label:expr, $fmt:literal, $($arg:tt)+) => {
        $crate::infra::static_logs::log_enhanced_error(
            $crate::infra::static_logs::ErrorSeverity::Rare,
            $label, &format!("{} {}", $crate::media_conversion_gate::ui_icon_pick($crate::modern_ui::symbols::WARNING, $crate::modern_ui::symbols::plain::WARNING), format!($fmt, $($arg)+)),
        )
    };
}

#[macro_export]
macro_rules! log_metadata_loss {
    ($ctx:expr, $msg:literal $(,)?) => {
        $crate::infra::static_logs::log_enhanced_error(
            $crate::infra::static_logs::ErrorSeverity::MetadataLoss,
            $ctx, &format!("{} {}", $crate::media_conversion_gate::ui_icon_pick($crate::modern_ui::symbols::SAVE, $crate::modern_ui::symbols::plain::SAVE), format!($msg)),
        )
    };
    ($ctx:expr, $msg:expr $(,)?) => {
        $crate::infra::static_logs::log_enhanced_error(
            $crate::infra::static_logs::ErrorSeverity::MetadataLoss,
            $ctx, &format!("{} {}", $crate::media_conversion_gate::ui_icon_pick($crate::modern_ui::symbols::SAVE, $crate::modern_ui::symbols::plain::SAVE), $msg),
        )
    };
    ($ctx:expr, $($arg:tt)*) => {
        $crate::infra::static_logs::log_enhanced_error(
            $crate::infra::static_logs::ErrorSeverity::MetadataLoss,
            $ctx, &format!("{} {}", $crate::media_conversion_gate::ui_icon_pick($crate::modern_ui::symbols::SAVE, $crate::modern_ui::symbols::plain::SAVE), format!($($arg)*)),
        )
    };
}

#[macro_export]
macro_rules! log_critical_error {
    ($ctx:expr, $msg:expr $(,)?) => {
        $crate::infra::static_logs::log_enhanced_error(
            $crate::infra::static_logs::ErrorSeverity::Critical,
            $ctx, &format!("{} {}", $crate::media_conversion_gate::ui_icon_pick($crate::modern_ui::symbols::STOP, $crate::modern_ui::symbols::plain::STOP), $msg),
        )
    };
    ($ctx:expr, $($arg:tt)*) => {
        $crate::infra::static_logs::log_enhanced_error(
            $crate::infra::static_logs::ErrorSeverity::Critical,
            $ctx, &format!("{} {}", $crate::media_conversion_gate::ui_icon_pick($crate::modern_ui::symbols::STOP, $crate::modern_ui::symbols::plain::STOP), format!($($arg)*)),
        )
    };
}

#[macro_export]
macro_rules! log_pipeline_broken {
    ($ctx:expr, $msg:expr $(,)?) => {
        $crate::infra::static_logs::log_enhanced_error(
            $crate::infra::static_logs::ErrorSeverity::PipelineBroken,
            $ctx, &format!("{} {}", $crate::media_conversion_gate::ui_icon_pick($crate::modern_ui::symbols::ERROR, $crate::modern_ui::symbols::plain::ERROR), $msg),
        )
    };
    ($ctx:expr, $($arg:tt)*) => {
        $crate::infra::static_logs::log_enhanced_error(
            $crate::infra::static_logs::ErrorSeverity::PipelineBroken,
            $ctx, &format!("{} {}", $crate::media_conversion_gate::ui_icon_pick($crate::modern_ui::symbols::ERROR, $crate::modern_ui::symbols::plain::ERROR), format!($($arg)*)),
        )
    };
}

#[macro_export]
macro_rules! log_upstream_error {
    ($ctx:expr, $msg:expr $(,)?) => {
        $crate::infra::static_logs::log_enhanced_error(
            $crate::infra::static_logs::ErrorSeverity::UpstreamError,
            $ctx, &format!("{} {}", $crate::media_conversion_gate::ui_icon_pick($crate::modern_ui::symbols::ERROR, $crate::modern_ui::symbols::plain::ERROR), $msg),
        )
    };
    ($ctx:expr, $($arg:tt)*) => {
        $crate::infra::static_logs::log_enhanced_error(
            $crate::infra::static_logs::ErrorSeverity::UpstreamError,
            $ctx, &format!("{} {}", $crate::media_conversion_gate::ui_icon_pick($crate::modern_ui::symbols::ERROR, $crate::modern_ui::symbols::plain::ERROR), format!($($arg)*)),
        )
    };
}

#[macro_export]
macro_rules! log_auto_error {
    ($ctx:expr, $msg:expr $(,)?) => {
        $crate::infra::static_logs::log_enhanced_error(
            $crate::infra::static_logs::classify_error(&format!("{}", $msg)),
            $ctx, &format!("{}", $msg),
        )
    };
    ($ctx:expr, $($arg:tt)*) => {{
        let _msg = format!($($arg)*);
        $crate::infra::static_logs::log_enhanced_error(
            $crate::infra::static_logs::classify_error(&_msg),
            $ctx, &_msg,
        )
    }};
}

// ─── Themed Progress Logging
// ──────────────────────────────────────────────────

fn format_path_reason(path: Option<&std::path::Path>, reason: &str) -> String {
    crate::media_conversion_gate::delivery_log_detail_with_optional_path(path, reason)
}

/// Pipeline id for cross-layer audit lines (Rust `media_scope` / `verify`).
#[must_use]
pub fn audit_pipeline_from_label(label: &str) -> &'static str {
    let lower = label.to_lowercase();
    if lower.contains("image") {
        "img"
    } else if lower.contains("video") {
        "vid"
    } else {
        "batch"
    }
}

/// Stable ``ignore_class`` tokens for verify / ``media_scope`` reconciliation.
pub mod audit_ignore_class {
    /// Vid intentionally ignores a proven static single-frame asset.
    pub const VID_STATIC_SINGLE_FRAME: &str = "vid_static_single_frame";
    /// Vid ignores unknown/zero frame count (not proven animated).
    pub const VID_STATIC_UNKNOWN_FRAMES: &str = "vid_static_unknown_frames";
    /// Vid rejects asset outside video domain (orchestrator may route to img).
    pub const VID_OUT_OF_DOMAIN: &str = "vid_out_of_domain";
    /// Img **ignores** confirmed animated media (static-only pipeline). **Not**
    /// a relay/forward to `vid`.
    pub const IMG_ANIMATED_HANDOFF: &str = "img_animated_handoff";
    /// Img refuses static conversion due to analysis uncertainty.
    pub const IMG_ANALYSIS_UNCERTAINTY: &str = "img_analysis_uncertainty";
    /// Strict delivery: missing/non-finite entropy blocks static conversion.
    pub const IMG_STRICT_ENTROPY: &str = "img_strict_entropy_missing";
    /// Animation probe ambiguous, contradictory, or failed — fail-closed static
    /// skip.
    pub const IMG_ANIMATION_AMBIGUITY: &str = "img_animation_ambiguity";
}

/// Machine-readable line for Rust `verify` / session log reconciliation
/// (`target: mfb::audit`).
///
/// ``mfb_audit_schema=1`` documents the field vocabulary; bump when adding
/// required keys.
pub fn emit_mfb_audit(
    outcome: &str,
    pipeline: &str,
    path: Option<&std::path::Path>,
    reason: &str,
    ignore_class: Option<&str>,
) {
    let ignore_class = ignore_class.filter(|c| !c.is_empty());
    if let Some(p) = path {
        if let Some(ic) = ignore_class {
            tracing::info!(
                target: "mfb::audit",
                mfb_audit_schema = 1_u8,
                outcome = outcome,
                pipeline = pipeline,
                path = %p.display(),
                reason = %reason,
                ignore_class = ic,
                "MFB_AUDIT"
            );
        } else {
            tracing::info!(
                target: "mfb::audit",
                mfb_audit_schema = 1_u8,
                outcome = outcome,
                pipeline = pipeline,
                path = %p.display(),
                reason = %reason,
                "MFB_AUDIT"
            );
        }
    } else if let Some(ic) = ignore_class {
        tracing::info!(
            target: "mfb::audit",
            mfb_audit_schema = 1_u8,
            outcome = outcome,
            pipeline = pipeline,
            reason = %reason,
            ignore_class = ic,
            "MFB_AUDIT"
        );
    } else {
        tracing::info!(
            target: "mfb::audit",
            mfb_audit_schema = 1_u8,
            outcome = outcome,
            pipeline = pipeline,
            reason = %reason,
            "MFB_AUDIT"
        );
    }
}

/// File-level audit for batch outcomes (failed / ignored without path in
/// layout).
pub fn log_file_outcome_audit(
    pipeline: &'static str,
    outcome: &str,
    path: &std::path::Path,
    reason: &str,
) {
    log_file_outcome_audit_with_class(pipeline, outcome, path, reason, None);
}

/// File-level audit with optional structured ``ignore_class`` for verify
/// reconciliation.
pub fn log_file_outcome_audit_with_class(
    pipeline: &'static str,
    outcome: &str,
    path: &std::path::Path,
    reason: &str,
    ignore_class: Option<&str>,
) {
    emit_mfb_audit(outcome, pipeline, Some(path), reason, ignore_class);
}

/// Batch pipeline start marker (`mfb::audit` / session logs).
pub fn log_batch_start_audit(pipeline: &'static str, label: &str, file_count: usize) {
    tracing::info!(
        target: "mfb::audit",
        mfb_audit_schema = 1_u8,
        outcome = "batch_start",
        pipeline = pipeline,
        label = label,
        file_count = file_count,
        "MFB_AUDIT"
    );
}

/// End-of-batch summary for cross-layer reconciliation (`verify` / session
/// logs).
pub fn log_batch_complete_audit(
    pipeline: &'static str,
    succeeded: usize,
    skipped: usize,
    ignored: usize,
    failed: usize,
    total: usize,
) {
    tracing::info!(
        target: "mfb::audit",
        mfb_audit_schema = 1_u8,
        outcome = "batch_complete",
        pipeline = pipeline,
        succeeded = succeeded,
        skipped = skipped,
        ignored = ignored,
        failed = failed,
        total = total,
        "MFB_AUDIT"
    );
}

#[derive(Clone, Copy)]
enum StaticLogLevel {
    Info,
    Warn,
}

fn emit_static_outcome_log(
    level: StaticLogLevel,
    path: Option<&std::path::Path>,
    outcome: &str,
    reason: &str,
    rendered: &str,
) {
    if let Some(p) = path {
        match level {
            StaticLogLevel::Info => tracing::info!(
                target: "static_log",
                path = %p.display(),
                outcome = outcome,
                reason = %reason,
                "{rendered}"
            ),
            StaticLogLevel::Warn => tracing::warn!(
                target: "static_log",
                path = %p.display(),
                outcome = outcome,
                reason = %reason,
                "{rendered}"
            ),
        }
    } else {
        match level {
            StaticLogLevel::Info => tracing::info!(
                target: "static_log",
                outcome = outcome,
                reason = %reason,
                "{rendered}"
            ),
            StaticLogLevel::Warn => tracing::warn!(
                target: "static_log",
                outcome = outcome,
                reason = %reason,
                "{rendered}"
            ),
        }
    }
}

pub fn log_success(label: &str, detail: &str) {
    log_success_at(label, None, detail);
}

/// File-aware success log with explicit pipeline id for cross-layer audit
/// (`img` / `vid`).
pub fn log_success_at_with_pipeline(
    label: &str,
    pipeline: &'static str,
    path: Option<&std::path::Path>,
    reason: &str,
) {
    let detail = plain_aware_detail(&format_path_reason(path, reason));
    let colored_label = TerminalColor::success(&format!("[{label}]"));
    let msg = format!(
        "{} {}",
        crate::media_conversion_gate::ui_icon_pick(symbols::SUCCESS, symbols::plain::SUCCESS),
        detail
    );
    let layout = DiagnosticLayout::new(colored_label, msg);
    let rendered = layout.render();
    emit_mfb_audit("converted", pipeline, path, reason, None);
    emit_static_outcome_log(StaticLogLevel::Info, path, "converted", reason, &rendered);
}

/// File-aware success log: always writes structured fields to the log file
/// (grep-friendly).
pub fn log_success_at(label: &str, path: Option<&std::path::Path>, reason: &str) {
    log_success_at_with_pipeline(label, audit_pipeline_from_label(label), path, reason);
}

pub fn log_skip(label: &str, detail: &str) {
    log_skip_at(label, None, detail);
}

/// File-aware skip log with explicit pipeline id for cross-layer audit (`img` /
/// `vid`).
pub fn log_skip_at_with_pipeline(
    label: &str,
    pipeline: &'static str,
    path: Option<&std::path::Path>,
    reason: &str,
) {
    let detail = format_path_reason(path, reason);
    let colored_label = TerminalColor::warning(&format!("[{label}]"));
    let msg = format!(
        "{} SKIP: {}",
        crate::media_conversion_gate::ui_icon_pick(symbols::SKIP, symbols::plain::SKIP),
        detail
    );
    let layout = DiagnosticLayout::new(colored_label, msg);
    let rendered = layout.render();
    emit_mfb_audit("skipped", pipeline, path, reason, None);
    emit_static_outcome_log(StaticLogLevel::Warn, path, "skipped", reason, &rendered);
}

/// File-aware skip log: always writes structured fields to the log file
/// (grep-friendly).
pub fn log_skip_at(label: &str, path: Option<&std::path::Path>, reason: &str) {
    log_skip_at_with_pipeline(label, audit_pipeline_from_label(label), path, reason);
}

pub fn log_ignore(label: &str, detail: &str) {
    log_ignore_at(label, None, detail);
}

/// File-aware ignore log with explicit pipeline id for cross-layer audit (`img`
/// / `vid`).
pub fn log_ignore_at_with_pipeline(
    label: &str,
    pipeline: &'static str,
    path: Option<&std::path::Path>,
    reason: &str,
    ignore_class: Option<&str>,
) {
    let detail = format_path_reason(path, reason);
    let colored_label = TerminalColor::ignore(&format!("[{label}]"));
    let msg = format!(
        "{} IGNORE: {}",
        crate::media_conversion_gate::ui_icon_pick(symbols::IGNORE, symbols::plain::IGNORE),
        detail
    );
    let layout = DiagnosticLayout::new(colored_label, msg);
    let rendered = layout.render();
    emit_mfb_audit("ignored", pipeline, path, reason, ignore_class);
    emit_static_outcome_log(StaticLogLevel::Info, path, "ignored", reason, &rendered);
}

/// File-aware ignore log: always writes structured fields to the log file
/// (grep-friendly).
pub fn log_ignore_at(label: &str, path: Option<&std::path::Path>, reason: &str) {
    log_ignore_at_with_pipeline(label, audit_pipeline_from_label(label), path, reason, None);
}

#[macro_export]
macro_rules! log_success {
    ($label:expr, $detail:expr $(,)?) => {
        $crate::infra::static_logs::log_success($label, &format!("{}", $detail))
    };
    ($label:expr, $detail:literal, $($arg:tt)+) => {
        $crate::infra::static_logs::log_success($label, &format!($detail, $($arg)+))
    };
    ($detail:expr $(,)?) => {
        $crate::infra::static_logs::log_success($crate::infra::static_logs::messages::LABEL_SUCCESS, &format!("{}", $detail))
    };
}

#[macro_export]
macro_rules! log_skip {
    ($label:expr, $detail:expr $(,)?) => {
        $crate::infra::static_logs::log_skip($label, &format!("{}", $detail))
    };
    ($label:expr, $detail:literal, $($arg:tt)+) => {
        $crate::infra::static_logs::log_skip($label, &format!($detail, $($arg)+))
    };
    ($detail:expr $(,)?) => {
        $crate::infra::static_logs::log_skip(
            &format!(
                "{} Skip Audit",
                $crate::media_conversion_gate::ui_icon_pick("📋", "[SKIP]")
            ),
            &format!("{}", $detail),
        )
    };
}

#[macro_export]
macro_rules! log_ignore {
    ($msg:expr $(,)?) => {
        $crate::infra::static_logs::log_ignore(
            &format!(
                "{} Ignore Audit",
                $crate::media_conversion_gate::ui_icon_pick("🙈", "[SKIP]")
            ),
            &format!("{}", $msg),
        )
    };
    ($fmt:literal, $($arg:tt)+) => {
        $crate::infra::static_logs::log_ignore(
            &format!(
                "{} Ignore Audit",
                $crate::media_conversion_gate::ui_icon_pick("🙈", "[SKIP]")
            ),
            &format!($fmt, $($arg)+),
        )
    };
}

pub fn log_detail(detail: &str) {
    let colored_label = TerminalColor::ignore("[Detail]");
    let msg = plain_aware_detail(detail);
    let layout = DiagnosticLayout::new(colored_label, msg);
    tracing::info!(target: "mfb::detail", "{}", layout.render());
}

#[macro_export]
macro_rules! log_detail {
    ($detail:literal, $($arg:tt)+) => {
        $crate::infra::static_logs::log_detail(&format!($detail, $($arg)+))
    };
    ($detail:literal $(,)?) => {
        $crate::infra::static_logs::log_detail(&format!($detail))
    };
    ($detail:expr $(,)?) => {
        $crate::infra::static_logs::log_detail(&format!("{}", $detail))
    };
    () => {
        $crate::infra::static_logs::log_detail("")
    };
}

pub fn log_report_detail(detail: &str) {
    let colored_label = TerminalColor::ignore("[Detail]");
    let msg = plain_aware_detail(detail);
    let layout = DiagnosticLayout::new(colored_label, msg);
    tracing::info!(target: "mfb::report", "{}", layout.render());
}

#[macro_export]
macro_rules! log_report_detail {
    ($detail:literal, $($arg:tt)+) => {
        $crate::infra::static_logs::log_report_detail(&format!($detail, $($arg)+))
    };
    ($detail:literal $(,)?) => {
        $crate::infra::static_logs::log_report_detail(&format!($detail))
    };
    ($detail:expr $(,)?) => {
        $crate::infra::static_logs::log_report_detail(&format!("{}", $detail))
    };
    () => {
        $crate::infra::static_logs::log_report_detail("")
    };
}

#[macro_export]
macro_rules! log_info {
    (label = $label:expr, $detail:literal, $($arg:tt)+) => {
        $crate::infra::static_logs::log_enhanced_info($label, &format!($detail, $($arg)+))
    };
    (label = $label:expr, $detail:expr $(,)?) => {
        $crate::infra::static_logs::log_enhanced_info($label, &format!("{}", $detail))
    };
    ($label:expr, $detail:literal, $($arg:tt)+) => {
        $crate::infra::static_logs::log_enhanced_info($label, &format!($detail, $($arg)+))
    };
    ($label:expr, $detail:literal $(,)?) => {
        $crate::infra::static_logs::log_enhanced_info($label, &format!("{}", $detail))
    };
    ($label:expr, $detail:expr $(,)?) => {
        $crate::infra::static_logs::log_enhanced_info($label, &format!("{}", $detail))
    };
    ($detail:literal, $($arg:tt)+) => {
        $crate::infra::static_logs::log_enhanced_info("Info", &format!($detail, $($arg)+))
    };
    ($detail:expr $(,)?) => {
        $crate::infra::static_logs::log_enhanced_info("Info", &format!("{}", $detail))
    };
}

#[macro_export]
macro_rules! log_warn {
    (label = $label:expr, $detail:literal, $($arg:tt)+) => {
        $crate::infra::static_logs::log_enhanced_warn($label, &format!($detail, $($arg)+))
    };
    (label = $label:expr, $detail:expr $(,)?) => {
        $crate::infra::static_logs::log_enhanced_warn($label, &format!("{}", $detail))
    };
    ($label:expr, $detail:literal, $($arg:tt)+) => {
        $crate::infra::static_logs::log_enhanced_warn($label, &format!($detail, $($arg)+))
    };
    ($label:expr, $detail:literal $(,)?) => {
        $crate::infra::static_logs::log_enhanced_warn($label, &format!("{}", $detail))
    };
    ($label:expr, $detail:expr $(,)?) => {
        $crate::infra::static_logs::log_enhanced_warn($label, &format!("{}", $detail))
    };
    ($detail:literal, $($arg:tt)+) => {
        $crate::infra::static_logs::log_enhanced_warn("Warning", &format!($detail, $($arg)+))
    };
    ($detail:expr $(,)?) => {
        $crate::infra::static_logs::log_enhanced_warn("Warning", &format!("{}", $detail))
    };
}

#[macro_export]
macro_rules! log_error {
    (label = $label:expr, $detail:literal, $($arg:tt)+) => {
        $crate::infra::static_logs::log_enhanced_error($crate::infra::static_logs::ErrorSeverity::Standard, $label, &format!($detail, $($arg)+))
    };
    (label = $label:expr, $detail:expr $(,)?) => {
        $crate::infra::static_logs::log_enhanced_error($crate::infra::static_logs::ErrorSeverity::Standard, $label, &format!("{}", $detail))
    };
    ($label:expr, $detail:literal, $($arg:tt)+) => {
        $crate::infra::static_logs::log_enhanced_error($crate::infra::static_logs::ErrorSeverity::Standard, $label, &format!($detail, $($arg)+))
    };
    ($label:expr, $detail:literal $(,)?) => {
        $crate::infra::static_logs::log_enhanced_error($crate::infra::static_logs::ErrorSeverity::Standard, $label, &format!("{}", $detail))
    };
    ($label:expr, $detail:expr $(,)?) => {
        $crate::infra::static_logs::log_enhanced_error($crate::infra::static_logs::ErrorSeverity::Standard, $label, &format!("{}", $detail))
    };
    ($detail:literal, $($arg:tt)+) => {
        $crate::infra::static_logs::log_enhanced_error($crate::infra::static_logs::ErrorSeverity::Standard, "Error", &format!($detail, $($arg)+))
    };
    ($detail:expr $(,)?) => {
        $crate::infra::static_logs::log_enhanced_error($crate::infra::static_logs::ErrorSeverity::Standard, "Error", &format!("{}", $detail))
    };
}

/// Logs a debug message.
#[macro_export]
macro_rules! log_debug {
    (label = $label:expr, $detail:literal, $($arg:tt)+) => {
        $crate::infra::static_logs::log_enhanced_debug($label, &format!($detail, $($arg)+))
    };
    (label = $label:expr, $detail:expr $(,)?) => {
        $crate::infra::static_logs::log_enhanced_debug($label, &format!("{}", $detail))
    };
    ($label:expr, $detail:literal, $($arg:tt)+) => {
        $crate::infra::static_logs::log_enhanced_debug($label, &format!($detail, $($arg)+))
    };
    ($label:expr, $detail:literal $(,)?) => {
        $crate::infra::static_logs::log_enhanced_debug($label, &format!($detail))
    };
    ($label:expr, $detail:expr $(,)?) => {
        $crate::infra::static_logs::log_enhanced_debug($label, &format!("{}", $detail))
    };
    ($detail:literal, $($arg:tt)+) => {
        $crate::infra::static_logs::log_enhanced_debug("Debug", &format!($detail, $($arg)+))
    };
    ($detail:literal $(,)?) => {
        $crate::infra::static_logs::log_enhanced_debug("Debug", &format!($detail))
    };
    ($detail:expr $(,)?) => {
        $crate::infra::static_logs::log_enhanced_debug("Debug", &format!("{}", $detail))
    };
}

/// Logs a failure message.
#[macro_export]
macro_rules! log_failure {
    (label = $label:expr, $detail:literal, $($arg:tt)+) => {
        $crate::infra::static_logs::log_enhanced_error($crate::infra::static_logs::ErrorSeverity::Standard, $label, &format!($detail, $($arg)+))
    };
    (label = $label:expr, $detail:literal $(,)?) => {
        $crate::infra::static_logs::log_enhanced_error($crate::infra::static_logs::ErrorSeverity::Standard, $label, &format!($detail))
    };
    (label = $label:expr, $detail:expr $(,)?) => {
        $crate::infra::static_logs::log_enhanced_error($crate::infra::static_logs::ErrorSeverity::Standard, $label, &format!("{}", $detail))
    };
    ($label:expr, $detail:literal, $($arg:tt)+) => {
        $crate::infra::static_logs::log_enhanced_error($crate::infra::static_logs::ErrorSeverity::Standard, $label, &format!($detail, $($arg)+))
    };
    ($label:expr, $detail:expr $(,)?) => {
        $crate::infra::static_logs::log_enhanced_error($crate::infra::static_logs::ErrorSeverity::Standard, $label, &format!("{}", $detail))
    };
    ($detail:expr $(,)?) => {
        $crate::infra::static_logs::log_enhanced_error($crate::infra::static_logs::ErrorSeverity::Standard, "Failure", &format!("{}", $detail))
    };
}

/// Logs a fatal error and terminates.
#[macro_export]
macro_rules! log_fatal {
    ($ctx:expr, $msg:literal $(,)?) => {
        $crate::infra::static_logs::log_enhanced_error(
            $crate::infra::static_logs::ErrorSeverity::Fatal,
            $ctx, &format!("{}", format!($msg)),
        )
    };
    ($ctx:expr, $msg:expr $(,)?) => {
        $crate::infra::static_logs::log_enhanced_error(
            $crate::infra::static_logs::ErrorSeverity::Fatal,
            $ctx, &format!("{}", $msg),
        )
    };
    ($ctx:expr, $($arg:tt)*) => {
        $crate::infra::static_logs::log_enhanced_error(
            $crate::infra::static_logs::ErrorSeverity::Fatal,
            $ctx, &format!($($arg)*),
        )
    };
}

/// Logs a statistic.
#[macro_export]
macro_rules! log_stat {
    ($label:expr, $value:expr $(,)?) => {
        $crate::infra::static_logs::log_enhanced_info($label, &format!(": {}", $value))
    };
    ($label:expr, $($arg:tt)+) => {
        $crate::infra::static_logs::log_enhanced_info($label, &format!(": {}", format!($($arg)+)))
    };
}

/// Logs a report statistic to `mfb::report` target.
#[macro_export]
macro_rules! log_report_stat {
    ($label:expr, $value:expr $(,)?) => {
        $crate::infra::static_logs::log_enhanced_report_info($label, &format!(": {}", $value))
    };
    ($label:expr, $($arg:tt)+) => {
        $crate::infra::static_logs::log_enhanced_report_info($label, &format!(": {}", format!($($arg)+)))
    };
}

/// Logs a hint or suggestion.
#[macro_export]
macro_rules! log_hint {
    (label = $label:expr, $detail:literal, $($arg:tt)+) => {
        $crate::infra::static_logs::log_enhanced_info($label, &format!(
            "{} Hint: {}",
            $crate::media_conversion_gate::ui_icon_pick($crate::modern_ui::symbols::INFO, $crate::modern_ui::symbols::plain::INFO),
            format!($detail, $($arg)+)
        ))
    };
    (label = $label:expr, $detail:literal $(,)?) => {
        $crate::infra::static_logs::log_enhanced_info($label, &format!(
            "{} Hint: {}",
            $crate::media_conversion_gate::ui_icon_pick($crate::modern_ui::symbols::INFO, $crate::modern_ui::symbols::plain::INFO),
            format!($detail)
        ))
    };
    (label = $label:expr, $detail:expr $(,)?) => {
        $crate::infra::static_logs::log_enhanced_info($label, &format!(
            "{} Hint: {}",
            $crate::media_conversion_gate::ui_icon_pick($crate::modern_ui::symbols::INFO, $crate::modern_ui::symbols::plain::INFO),
            $detail
        ))
    };
    ($label:expr, $detail:literal, $($arg:tt)+) => {
        $crate::infra::static_logs::log_enhanced_info($label, &format!(
            "{} Hint: {}",
            $crate::media_conversion_gate::ui_icon_pick($crate::modern_ui::symbols::INFO, $crate::modern_ui::symbols::plain::INFO),
            format!($detail, $($arg)+)
        ))
    };
    ($label:expr, $detail:literal $(,)?) => {
        $crate::infra::static_logs::log_enhanced_info($label, &format!(
            "{} Hint: {}",
            $crate::media_conversion_gate::ui_icon_pick($crate::modern_ui::symbols::INFO, $crate::modern_ui::symbols::plain::INFO),
            format!($detail)
        ))
    };
    ($label:expr, $detail:expr $(,)?) => {
        $crate::infra::static_logs::log_enhanced_info($label, &format!(
            "{} Hint: {}",
            $crate::media_conversion_gate::ui_icon_pick($crate::modern_ui::symbols::INFO, $crate::modern_ui::symbols::plain::INFO),
            $detail
        ))
    };
    ($detail:expr $(,)?) => {
        $crate::infra::static_logs::log_enhanced_info("Hint", &format!(
            "{} {}",
            $crate::media_conversion_gate::ui_icon_pick($crate::modern_ui::symbols::INFO, $crate::modern_ui::symbols::plain::INFO),
            $detail
        ))
    };
}

/// Logs a corruption error.
#[macro_export]
macro_rules! log_corruption {
    ($ctx:expr, $msg:literal $(,)?) => {
        $crate::infra::static_logs::log_enhanced_error(
            $crate::infra::static_logs::ErrorSeverity::Critical,
            $ctx, &format!("{} Corruption: {}", $crate::media_conversion_gate::ui_icon_pick($crate::modern_ui::symbols::BUG, $crate::modern_ui::symbols::plain::BUG), format!($msg)),
        )
    };
    ($ctx:expr, $msg:expr $(,)?) => {
        $crate::infra::static_logs::log_enhanced_error(
            $crate::infra::static_logs::ErrorSeverity::Critical,
            $ctx, &format!("{} Corruption: {}", $crate::media_conversion_gate::ui_icon_pick($crate::modern_ui::symbols::BUG, $crate::modern_ui::symbols::plain::BUG), $msg),
        )
    };
    ($ctx:expr, $($arg:tt)*) => {
        $crate::infra::static_logs::log_enhanced_error(
            $crate::infra::static_logs::ErrorSeverity::Critical,
            $ctx, &format!("{} Corruption: {}", $crate::media_conversion_gate::ui_icon_pick($crate::modern_ui::symbols::BUG, $crate::modern_ui::symbols::plain::BUG), format!($($arg)*)),
        )
    };
}

/// Logs a critical error.
#[macro_export]
macro_rules! log_critical {
    ($ctx:expr, $msg:literal $(,)?) => {
        $crate::infra::static_logs::log_enhanced_error(
            $crate::infra::static_logs::ErrorSeverity::Critical,
            $ctx, &format!("{}", format!($msg)),
        )
    };
    ($ctx:expr, $msg:expr $(,)?) => {
        $crate::infra::static_logs::log_enhanced_error(
            $crate::infra::static_logs::ErrorSeverity::Critical,
            $ctx, &format!("{}", $msg),
        )
    };
    ($ctx:expr, $($arg:tt)*) => {
        $crate::infra::static_logs::log_enhanced_error(
            $crate::infra::static_logs::ErrorSeverity::Critical,
            $ctx, &format!($($arg)*),
        )
    };
}

/// Logs a rare error.
#[macro_export]
macro_rules! log_rare_error {
    ($ctx:expr, $msg:literal $(,)?) => {
        $crate::infra::static_logs::log_enhanced_error(
            $crate::infra::static_logs::ErrorSeverity::Rare,
            $ctx, &format!("{}", format!($msg)),
        )
    };
    ($ctx:expr, $msg:expr $(,)?) => {
        $crate::infra::static_logs::log_enhanced_error(
            $crate::infra::static_logs::ErrorSeverity::Rare,
            $ctx, &format!("{}", $msg),
        )
    };
    ($ctx:expr, $($arg:tt)*) => {
        $crate::infra::static_logs::log_enhanced_error(
            $crate::infra::static_logs::ErrorSeverity::Rare,
            $ctx, &format!($($arg)*),
        )
    };
}

/// Logs a summary header.
#[macro_export]
macro_rules! log_summary_header {
    ($title:expr $(,)?) => {
        $crate::infra::static_logs::log_report_header($title)
    };
}

/// Specialized logging for beautiful, audit-ready report headers.
pub fn log_report_header(title: &str) {
    let top = "╔═══════════════════════════════════════════════════════════════╗";
    let bot = "╚═══════════════════════════════════════════════════════════════╝";
    let title_upper = truncate_to_visual_width(&title.to_uppercase(), 61);
    let title_width = visual_width(&title_upper);
    let pad = 61usize.saturating_sub(title_width) / 2;
    let trailing_pad = 61usize.saturating_sub(pad + title_width);
    let mid = format!(
        "║{} {} {}║",
        " ".repeat(pad),
        title_upper,
        " ".repeat(trailing_pad)
    );

    let colored_header =
        format!("\x1b[1;34m{top}\x1b[0m\n\x1b[1;34m{mid}\x1b[0m\n\x1b[1;34m{bot}\x1b[0m");

    let layout = DiagnosticLayout::new(TerminalColor::info("[REPORT]"), colored_header);
    tracing::info!(target: "mfb::report", "{}", layout.render());
}

/// Specialized logging for beautiful, audit-ready report data items.
pub fn log_report_item(key: &str, value: &str) {
    let msg = format!("\x1b[36m{key:<18}\x1b[0m : \x1b[1;97m{value}\x1b[0m");
    let layout = DiagnosticLayout::new(TerminalColor::info("   │"), msg);
    tracing::info!(target: "mfb::report", "{}", layout.render());
}

#[macro_export]
macro_rules! log_report_item {
    ($key:expr, $val:expr $(,)?) => {
        $crate::infra::static_logs::log_report_item($key, &format!("{}", $val))
    };
    ($key:expr, $fmt:literal, $($arg:tt)+) => {
        $crate::infra::static_logs::log_report_item($key, &format!($fmt, $($arg)+))
    };
}

/// Logs a formal diagnostic section header.
#[macro_export]
macro_rules! log_section {
    ($title:expr) => {
        $crate::log_section!($title, "INFO")
    };
    ($title:expr, $level:expr $(,)?) => {
        $crate::log_static!("");
        $crate::log_static!("┌─────────────────────────────────────────────────────────────┐");
        $crate::log_static!(&format!("│  {} {:<45} │", $title, format!("({})", $level)));
        $crate::log_static!("└─────────────────────────────────────────────────────────────┘");
    };
}

/// Logs a standard table header for diagnostic reports.
#[macro_export]
macro_rules! log_table_header {
    ($($col:expr),+ $(,)?) => {
        let mut header = String::from("   ");
        $(
            header.push_str(&format!("{:<15}", $col));
        )+
        $crate::log_static!(&header);
        $crate::log_static!(&format!("   {}", "─".repeat(header.len().saturating_sub(3))));
    };
}

/// Raw static logging (primarily for report generation)
#[macro_export]
macro_rules! log_static {
    (warn, $msg:expr $(,)?) => {
        $crate::log_warn!($crate::infra::static_logs::messages::LABEL_RUN_LOG, $msg)
    };
    (info, $msg:expr $(,)?) => {
        $crate::log_info!($crate::infra::static_logs::messages::LABEL_RUN_LOG, $msg)
    };
    ($msg:expr $(,)?) => {
        $crate::log_info!($crate::infra::static_logs::messages::LABEL_RUN_LOG, $msg)
    };
}

pub mod messages {
    pub const LABEL_QUALITY: &str = "Analysis Audit";
    pub const LABEL_QUALITY_FAIL: &str = "Quality Integrity Violation";
    pub const LABEL_CLEANUP: &str = "Cleanup";
    pub const LABEL_FFMPEG: &str = "FFmpeg";
    pub const LABEL_WEBP: &str = "WebP";
    pub const LABEL_AVIF: &str = "AVIF";
    pub const LABEL_JXL: &str = "JXL";
    pub const LABEL_HEIC: &str = "HEIC";
    pub const LABEL_GIF: &str = "GIF";
    pub const LABEL_JPEG: &str = "JPEG";
    pub const LABEL_IO: &str = "I/O";
    pub const LABEL_VID_SAFEGUARD: &str = "VID-SAFEGUARD";
    pub const LABEL_RECONCILIATION: &str = "Reconciliation";
    pub const LABEL_DATABASE: &str = "Infrastructure Audit";
    pub const LABEL_DB: &str = "Infrastructure Audit";
    pub const LABEL_BATCH: &str = "Batch Audit";
    pub const LABEL_CHECKPOINT: &str = "Checkpoint Audit";
    pub const LABEL_VERIFY: &str = "Verification Audit";
    pub const LABEL_CACHE: &str = "Cache Audit";
    pub const LABEL_SYSTEM: &str = "System Audit";
    pub const LABEL_RECOVERY: &str = "Recovery Audit";
    pub const LABEL_IDENTIFY: &str = "Discovery Audit: Streams";
    pub const LABEL_SUCCESS: &str = "Operation Audit: Success";
    pub const LABEL_TEST: &str = "Validation Audit";
    pub const LABEL_LOCK: &str = "Lock";
    pub const LABEL_CONFIG: &str = "Pipeline Configuration";
    pub const LABEL_PHASE_2: &str = "Phase 2";
    pub const LABEL_PHASE_3: &str = "Phase 3";
    pub const LABEL_ANOMALY: &str = "System Anomaly";
    pub const LABEL_DETECTION: &str = "Detection Audit";
    pub const LABEL_TOOLS: &str = "Tools";
    pub const LABEL_METADATA: &str = "Metadata Integrity";
    pub const LABEL_HEURISTIC: &str = "Heuristic";
    pub const LABEL_NUMERIC: &str = "Numeric";
    pub const LABEL_IMAGE: &str = "Image";
    pub const LABEL_PENETRATION: &str = "Penetration";
    pub const LABEL_XMP: &str = "XMP";
    pub const LABEL_INTENT: &str = "Intent";
    pub const LABEL_STRATEGY: &str = "Strategy Audit";
    pub const LABEL_MAPPING: &str = "Mapping Audit";
    pub const LABEL_READY: &str = "Readiness Audit";
    pub const LABEL_DONE: &str = "Session Finalized";
    pub const LABEL_CHECKPOINT_LOCK: &str = "Checkpoint Lock";
    pub const LABEL_CHECKPOINT_CLEANUP: &str = "Checkpoint Cleanup";
    pub const LABEL_COPY: &str = "Copy";
    pub const LABEL_REPORT: &str = "Session Summary";
    pub const LABEL_TIMESTAMP_RESTORE: &str = "Timestamp Restore";
    pub const LABEL_APPLE_PHOTOS: &str = "Apple Photos";
    pub const LABEL_REPORT_HEADER: &str = "Report Header";
    pub const LABEL_REPORT_FOOTER: &str = "Report Footer";
    pub const LABEL_SUMMARY_DATA: &str = "Summary Data";
    pub const LABEL_FINAL_REPORT: &str = "Final Report";
    pub const CONVERSION_SUMMARY: &str = "Conversion Summary";
    pub const LABEL_MS_SSIM: &str = "MS-SSIM Audit";
    pub const LABEL_CALIBRATION: &str = "Calibration";
    pub const LABEL_CONVERSION: &str = "Conversion";
    pub const LABEL_VIDEO: &str = "Video";
    pub const LABEL_ENCODER: &str = "Encoder";
    pub const LABEL_DISK: &str = "Discovery Audit: Storage";
    pub const LABEL_THREAD: &str = "Process Audit: Concurrency";
    pub const LABEL_PHASE_1: &str = "Phase 1";
    pub const LABEL_PHASE_4: &str = "Phase 4";
    pub const LABEL_PHASE_5: &str = "Phase 5";
    pub const LABEL_COLOR_SPACE: &str = "Color Space";
    pub const LABEL_GPU: &str = "GPU";
    pub const LABEL_DYNAMIC: &str = "Dynamic";
    pub const LABEL_FFPROBE: &str = "FFProbe";
    pub const LABEL_GPU_QUALITY: &str = "GPU Quality";
    pub const LABEL_CORRUPTION: &str = "Corruption";
    pub const LABEL_CAMBI: &str = "CAMBI";
    pub const LABEL_VMAF: &str = "VMAF";
    pub const LABEL_SSIM_CALC_FAILED: &str = "SSIM Calc Failed";
    pub const LABEL_PRECHECK: &str = "Discovery Audit: Precheck";
    pub const LABEL_PROBE: &str = "Probe";
    pub const LABEL_RUN_LOG: &str = "Session Audit: Log Persistence";
    pub const LABEL_GHOST_MODE: &str = "Ghost Mode";
    pub const LABEL_LOGGING: &str = "Logging";
    pub const LABEL_DV_FAILED: &str = "Dolby Vision Failed";
    pub const LABEL_HDR10PLUS_FAILED: &str = "HDR10+ Failed";
    pub const LABEL_DELETE_FAILED: &str = "Delete Failed";
    pub const LABEL_DV_TOOL: &str = "Dolby Vision";
    pub const LABEL_ICC: &str = "Metadata Recovery";
    pub const LABEL_PSNR: &str = "PSNR";
    pub const LABEL_SSIM: &str = "SSIM";
    pub const LABEL_SSIM_ESTIMATED: &str = "Estimated SSIM";
    pub const LABEL_HEIC_AUDIT: &str = "HEIC Forensic Audit";
    pub const LABEL_INFERENCE_AUDIT: &str = "Inference Log Audit";
    pub const LABEL_FEATURE_AUDIT: &str = "Feature Power Audit";
    pub const LABEL_BLIND_SPOT_AUDIT: &str = "Blind Spot Discovery Audit";
    pub const LABEL_ULTRAHDR_SYNTHESIS: &str = "UltraHDR Synthesis Audit";

    pub const LABEL_PARALLEL_AUDIT: &str = "Parallel Audit";
    pub const LABEL_PROCESS_AUDIT: &str = "Process Audit";
    pub const LABEL_CACHING_AUDIT: &str = "Caching Audit";
    pub const LABEL_INTEGRITY_AUDIT: &str = "Integrity Audit";
    pub const LABEL_INVENTORY_AUDIT: &str = "Inventory Audit";
    pub const LABEL_INGEST_AUDIT: &str = "Ingest Audit";
    pub const LABEL_AV1_STRATEGY: &str = "AV1 Strategy";
    pub const LABEL_SYNC_AUDIT: &str = "Sync Audit";
    pub const LABEL_HEALTH_AUDIT: &str = "Health Audit";
    pub const LABEL_INFRASTRUCTURE_AUDIT: &str = "Infrastructure Audit";
    pub const LABEL_SESSION_SUMMARY: &str = "Session Summary";
    pub const LABEL_STATS: &str = "Stats";
    pub const LABEL_RECOVERY_AUDIT: &str = "Recovery Audit";
    pub const LABEL_CACHE_AUDIT: &str = "Cache Audit";
    pub const LABEL_PERSISTENCE_AUDIT: &str = "Integrity Audit: Persistence";
    pub const LABEL_ITERATIONS: &str = "Total iterations";
    pub const LABEL_TIME_ELAPSED: &str = "Time elapsed";
    pub const LABEL_FINAL_CRF: &str = "Final CRF";
    pub const LABEL_FINAL_SSIM: &str = "Final SSIM";
    pub const LABEL_FINAL_PSNR: &str = "Final PSNR";
    pub const LABEL_OVERALL_CONFIDENCE: &str = "Overall Confidence";
    pub const LABEL_SAMPLING_COVERAGE: &str = "Sampling Coverage";
    pub const LABEL_PREDICTION_ACCURACY: &str = "Prediction Accuracy";
    pub const LABEL_SAFETY_MARGIN: &str = "Safety Margin";
    pub const LABEL_SSIM_RELIABILITY: &str = "SSIM Reliability";
    pub const LABEL_3D_QUALITY_GATE_FAILED: &str = "3D Quality Gate Failed";
    pub const LABEL_QUALITY_TARGET_FAILED: &str = "Quality Target Failed";
    pub const LABEL_QUALITY_SIZE_FAIL: &str = "Quality Size Fail";
    pub const LABEL_EXPLORATION_AUDIT: &str = "Exploration Strategy Audit";
    pub const LABEL_CONFIDENCE_AUDIT: &str = "Confidence Metric Audit";
    pub const LABEL_CACHE_INVENTORY: &str = "Cache Inventory Audit";
    pub const LABEL_CACHE_STORAGE: &str = "Cache Storage Audit";
    pub const LABEL_CACHE_SCHEMA: &str = "Cache Schema Audit";
    pub const LABEL_PIXEL_PERFECT: &str = "Pixel-Perfect Verification";
    pub const LABEL_STRUCTURAL_AUDIT: &str = "Structural Bitstream Audit";
    pub const LABEL_PERCEPTUAL_AUDIT: &str = "Perceptual Quality Audit";
    pub const LABEL_MS_SSIM_AUDIT: &str = "MS-SSIM Structural Audit";
    pub const LABEL_VMAF_AUDIT: &str = "VMAF Perceptual Audit";
    pub const LABEL_CAMBI_AUDIT: &str = "CAMBI Banding Audit";
    pub const LABEL_CHROMA_AUDIT: &str = "Chroma Fidelity Audit";
    pub const LABEL_HDR_SYNTHESIS: &str = "HDR Synthesis Audit";
    pub const LABEL_GAINMAP_AUDIT: &str = "Gainmap Forensic Audit";
    pub const LABEL_DECISION_AUDIT: &str = "Matching Decision Audit";
    pub const LABEL_HINT: &str = "Forensic Hint";

    pub const MSG_CONVERSION_CORE_MSG: &str =
        "{codec} (CRF {crf_display}{explored_msg}, {iterations} iter{ssim_msg}): {size_tag}";
    pub const MSG_CONVERSION_QUAL_LABEL: &str = "[OK] {q} | {core_msg}";
    pub const MSG_CONVERSION_QUAL_NONE: &str = "[OK] {core_msg}";
    pub const MSG_CONVERSION_FALLBACK_FAILURE_READ: &str =
        "Failed to read fallback failure metadata: {}";
    pub const MSG_CONVERSION_FALLBACK_COPIED_READ: &str =
        "Failed to read copied fallback metadata: {}";

    pub const MSG_IMAGE_DETECTION_ISOBMFF_ANIM_FAIL: &str =
        "ISOBMFF animation detection failed for {}: {}";

    pub const MSG_VQD_BPP_ANOMALY: &str =
        "BPP NaN/Inf! Refusing to forge data. Information invalidated.";
    pub const MSG_VQD_EFFICIENCY_ANOMALY: &str =
        "Efficiency NaN/Inf! Refusing to forge data. Information invalidated.";
    pub const MSG_VQD_CRF_FAIL: &str =
        "Video Analysis: Metadata search for CRF failed; falling back to BPP heuristic";
    pub const MSG_VQD_CRF_PARAMS_AUDIT: &str = "ENCODER PARAMS AUDIT: Extracted CRF value '{}' is out of valid u8 range | Forensic: \
         Numeric overflow or invalid tag value; defaulting to heuristic fallback";

    pub const MSG_MSSSIM_VERIFIED: &str =
        "Forensic: MS-SSIM verified in {:.2}s (Sampled {}/{} frames)";
    pub const MSG_MSSSIM_SPEEDUP: &str =
        "Performance: Parallel throughput boosted by {:.1}x (Scaling Strategy: Active)";
    pub const MSG_MSSSIM_CHANNEL_PANIC: &str =
        "{} channel thread panicked during MS-SSIM calculation";
    pub const MSG_MSSSIM_RELIABILITY: &str =
        "Component Reliability: Y={:.4} | U={:.4} | V={:.4} (Channel Parity Audit)";
    pub const MSG_MSSSIM_FALLBACK: &str =
        "[WARN] MS-SSIM failed for channel {}, falling back to SSIM parity audit";
    pub const MSG_MSSSIM_BOTH_FAIL: &str = "Both MS-SSIM and SSIM parity failed for channel {}";
    pub const MSG_MSSSIM_RETRIEVE_FAIL: &str = "Failed to retrieve {} channel score from monitor";
    pub const MSG_MSSSIM_RETRIEVE_FAIL_SSIM: &str =
        "Failed to retrieve {} channel SSIM score from monitor";

    pub const MSG_IMAGE_DETECTION_OPEN_FAIL: &str =
        "Failed to open file for ftyp brand resolution at '{}'. Refusing to forge HEIC.";
    pub const MSG_IMAGE_DETECTION_READ_FAIL: &str =
        "Failed to read ftyp box at '{}': {}. Information invalidated.";
    pub const MSG_IMAGE_DETECTION_TRUNCATED_FTYP: &str =
        "ftyp box too short or missing at '{}'. Refusing to forge HEIC.";
    pub const MSG_IMAGE_DETECTION_TRUNCATED_SIZE: &str =
        "Truncated ftyp box size at '{}'. Information invalidated.";
    pub const MSG_IMAGE_DETECTION_OVERFLOW: &str =
        "ISOBMFF box_size overflow! Refusing to forge data. Information invalidated.";
    pub const MSG_IMAGE_DETECTION_TRUNCATED_BRANDS: &str =
        "ftyp box truncated before brands at '{}'. Refusing to forge HEIC.";
    pub const MSG_IMAGE_DETECTION_OOB_BRANDS: &str =
        "ftyp box brands out of bounds at '{}'. Information invalidated.";
    pub const MSG_IMAGE_DETECTION_GIF_TRUNCATED: &str = "GIF DECODE AUDIT: Truncated image descriptor at '{}' | Forensic: Unexpected EOF during \
         descriptor parse; breaking loop to prevent out-of-bounds access";

    pub const MSG_GIF_TOO_SMALL: &str = "GIF AUDIT: File too small | Forensic: len={} (min 13 \
                                         required for header + logical screen descriptor); \
                                         refusing to parse";
    pub const MSG_GIF_INVALID_MAGIC: &str =
        "GIF AUDIT: Invalid magic | Forensic: Found '{}'; expected GIF87a/GIF89a";
    pub const MSG_GIF_OVERFLOW_FRAME: &str = "GIF AUDIT: Frame count overflow | Forensic: \
                                              payload_count={} exceeds u32::MAX; file is highly \
                                              anomalous";
    pub const MSG_GIF_OVERFLOW_DELAY: &str = "GIF AUDIT: Delay count overflow | Forensic: \
                                              delay_count={} exceeds u32::MAX; file is highly \
                                              anomalous";

    pub const MSG_DATE_ANOMALY_FILENAME: &str = "METADATA ANOMALY: Missing 'FileName' in exiftool output | Forensic: Field is None in \
         JSON response; defaulting to empty string (may cause matching failures)";
    pub const MSG_DATE_ANOMALY_SOURCE: &str = "METADATA ANOMALY: Missing 'SourceFile' in exiftool output | Forensic: Field is None in \
         JSON response; defaulting to empty string (may cause path resolution issues)";

    pub const MSG_IQD_PIXEL_FEATURE: &str =
        "Forensic: Initiating pixel-level feature extraction for {}x{} {} media";
    pub const MSG_IQD_INVALID_RGBA: &str = "Invalid RGBA data: expected {} bytes for {}x{}, got {}";
    pub const MSG_IQD_INVALID_DIM: &str = "Invalid dimensions: width or height is 0";
    pub const MSG_VQD_INVALID_DIM: &str = "Invalid dimensions: width or height is 0";
    pub const MSG_IQD_CLASSIFIED: &str = "Forensic: Content classified as {} (Confidence: {:.0}%) \
                                          | Decision: Based on multi-dimensional pixel heuristics";

    pub const DB_FALLBACK: &str = "Component Reliability: Cascading to heuristic-only mode | \
                                   Forensic: Local KNN service unavailable; system remains \
                                   operational via conservative priors";
    pub const MSG_DB_FALLBACK: &str = "Component Reliability: Cascading to heuristic-only mode | \
                                       Forensic: Local KNN service unavailable; system remains \
                                       operational via conservative priors";
    pub const DB_INIT_HINT: &str =
        "Forensic: Verify PostgreSQL service state and MFB_PG_CONNSTR environment variable";
    pub const MSG_DB_INIT_HINT: &str =
        "Forensic: Verify PostgreSQL service state and MFB_PG_CONNSTR environment variable";
    pub const DB_CONN_SUCCESS: &str = "Database Audit: Connection to forensic registry established";
    pub const MSG_DB_CONN_SUCCESS: &str =
        "Database Audit: Connection to forensic registry established";
    pub const DB_INIT_SCHEMA: &str = "Database Audit: Synchronizing forensic registry schema...";
    pub const DB_SEED_START: &str =
        "Database Audit: Seeding reference samples into forensic registry...";
    pub const DB_SEED_COMPLETE: &str = "Database Audit: Reference seeding finalized ({} samples)";
    pub const DB_SEED_FAIL: &str = "Database Audit: Reference seeding failed: {}";
    pub const DB_BACKFILL_PGVECTOR: &str =
        "Database Audit: Backfilling pgvector embeddings for legacy records...";

    pub const MSG_SIGNAL_VOID_AUDIO_SILENT: &str = "Signal Void: Audio stream is silent or empty \
                                                    | Forensic: Normalizing to silent-video-bias \
                                                    priors";
    pub const MSG_SIGNAL_VOID_FRAME_COUNT: &str = "Signal Void: Frame count is 0 or negative | \
                                                   Forensic: Information invalidated; refusing to \
                                                   forge data";
    pub const MSG_LOOP_CLEANUP_FAIL: &str =
        "Forensic: Failed to cleanup temporary loop artifacts: {}";

    pub const MSG_DB_IMMATURE: &str =
        "Image Quality Database is immature (needs >={} total, >={} per class). Bypassing KNN.";
    pub const MSG_DB_INFERENCE_LOG_FAIL: &str = "Failed to log inference results: {}";
    pub const MSG_DB_CORRUPTION_PIXELS: &str =
        "Database Audit: Potential corruption in pixel features; skipping entry";
    pub const MSG_DB_DISABLED: &str = "Database Audit: Manual override - Image Quality DB disabled";
    pub const MSG_DB_FORCE_KNN: &str =
        "Database Audit: Manual override - Forcing Quality KNN lookup";
    pub const MSG_DB_KNN_EMPTY: &str =
        "Database Audit: KNN search returned zero neighbors for quality estimation";
    pub const MSG_DB_KNN_FAIL: &str =
        "Database Audit: KNN search failed; falling back to heuristic quality scoring";
    pub const MSG_DB_UNAVAILABLE: &str =
        "Database Audit: Service unavailable; falling back to heuristic quality scoring";
    pub const MSG_DB_INIT_START: &str =
        "Database Audit: Initializing Image Quality Database schema...";

    pub const MSG_BATCH_PIXEL_READ_FAIL: &str =
        "Batch Audit: Failed to read pixel data for asset: {}";
    pub const MSG_BATCH_CACHE_READ_FAIL: &str =
        "Batch Audit: Failed to read cached analysis for asset: {}";
    pub const MSG_CONVERSION_SYSTEM_TIME_FAIL: &str =
        "Conversion Audit: Failed to retrieve system time: {}";
    pub const MSG_CONVERSION_SIZE_INCREASE: &str = "Conversion resulted in larger file size ({})";

    pub const MSG_CONVERSION_SIZE_DIFF_OVERFLOW: &str =
        "Conversion Audit: Size difference overflow for {}; information invalidated";
    pub const MSG_CONVERSION_COPIED_METADATA_FAIL: &str =
        "Conversion Audit: Failed to read copied metadata for {}: {}";
    pub const MSG_CONVERSION_FALLBACK_FAIL: &str =
        "Conversion Audit: Fallback strategy failed for {}: {}";
    pub const MSG_CONVERSION_METADATA_FAIL: &str =
        "Conversion Audit: Metadata extraction failed for {}: {}";
    pub const MSG_CONVERSION_EXPLORED_FROM: &str = " (explored from CRF {from})";
    pub const MSG_CONVERSION_SIZE_TAG_POS: &str = "+{size_diff}";
    pub const MSG_CONVERSION_SIZE_TAG_NEG: &str = "-{reduction_pct}%";
    pub const MSG_METADATA_PRESERVE_FAIL: &str =
        "Metadata Audit: Failed to preserve forensic tags: {err}";
    pub const MSG_CONVERSION_SSIM: &str = " | SSIM: {ssim}";
    pub const MSG_CONVERSION_CRF_LOSSLESS: &str = "{crf} (Lossless)";
    pub const MSG_CONVERSION_CRF_NORMAL: &str = "{crf}";
    pub const MSG_CONVERSION_DUPLICATE: &str = "DUPLICATE";
    pub const MSG_CONVERSION_EXISTS: &str = "EXISTS";
    pub const MSG_CONVERSION_RESULT_INFO: &str = "{format_name} {action} {info}: {size_tag}";
    pub const MSG_CONVERSION_RESULT_BASE: &str = "{format_name} {action}: {size_tag}";
    pub const MSG_CONVERSION_VALIDATE_EXIST: &str = "Validation: Input file does not exist: {}";
    pub const MSG_CONVERSION_VALIDATE_OUTPUT_UTF8: &str =
        "Validation: Output path is not valid UTF-8: {}";
    pub const MSG_CONVERSION_VALIDATE_READ: &str = "Validation: Input file is not readable: {}";
    pub const MSG_CONVERSION_VALIDATE_REGULAR: &str = "Validation: Input is not a regular file: {}";
    pub const MSG_CONVERSION_VALIDATE_SYMLINK: &str = "Validation: Input is a broken symlink: {}";
    pub const MSG_CONVERSION_VALIDATE_UTF8: &str = "Validation: Input path is not valid UTF-8: {}";
    pub const MSG_CONVERSION_DIM_FAIL: &str =
        "Validation: Failed to extract dimensions for {}; refusing to forge metadata";

    pub const MSG_METADATA_RESTORE_INSPECT_FAIL: &str =
        "Metadata Audit: Failed to inspect target for restoration: {}";
    pub const MSG_METADATA_XMP_FALLBACK_FAIL: &str =
        "Metadata Audit: XMP fallback preservation failed";
    pub const MSG_METADATA_XMP_FALLBACK_SUCCESS: &str =
        "Metadata Audit: XMP fallback preservation successful";
    pub const MSG_METADATA_LINUX_BIRTH_FAIL: &str =
        "Metadata Audit: Linux birth time retrieval failed: {}";
    pub const MSG_METADATA_MACOS_PERM_FAIL: &str =
        "Metadata Audit: macOS permission restoration failed: {}";
    pub const MSG_METADATA_REAPPLY_CREATION_FAIL: &str =
        "Metadata Audit: Failed to reapply creation time: {}";
    pub const MSG_METADATA_SET_ADDED_FAIL: &str = "Metadata Audit: Failed to set 'added' time: {}";
    pub const MSG_METADATA_SET_CREATION_FAIL: &str =
        "Metadata Audit: Failed to set creation time: {}";
    pub const MSG_METADATA_SET_CREATION_SUCCESS: &str =
        "Metadata Audit: {} creation time synchronized successfully";
    pub const MSG_METADATA_WINDOWS_CREATION_FAIL: &str =
        "Metadata Audit: Windows creation time restoration failed: {}";
    pub const MSG_METADATA_SET_ADDED_SUCCESS: &str =
        "Metadata Audit: 'Added' time synchronized successfully";
    pub const MSG_METADATA_CREATION_TIME: &str = "Metadata Audit: Creation time detected: {:?}";
    pub const MSG_METADATA_INTERNAL_FAIL: &str = "Metadata Audit: Internal failure: {}";
    pub const MSG_METADATA_MACOS_COPY_FAIL: &str = "Metadata Audit: macOS metadata copy failed: {}";
    pub const MSG_METADATA_REAPPLY_CREATION: &str =
        "Metadata Audit: Reapplying creation time: {:?}";
    pub const MSG_METADATA_READ_CREATION_FAIL: &str =
        "Metadata Audit: Failed to read creation time";
    pub const MSG_METADATA_APPLY_VERIFY_FAIL: &str =
        "Metadata Audit: Application verification failed: {}";
    pub const MSG_METADATA_RESTORE_VERIFY_FAIL: &str =
        "Metadata Audit: Restoration verification failed: {}";
    pub const MSG_METADATA_SET_FILE_TIMES_FAIL: &str =
        "Metadata Audit: Failed to set file timestamps: {}";
    pub const MSG_METADATA_SET_TIMES_SUCCESS: &str =
        "Metadata Audit: {} timestamps synchronized successfully";
    pub const MSG_METADATA_TREE_FAIL: &str =
        "Metadata Audit: Recursive tree preservation failed: {}";
    pub const MSG_METADATA_SRC_FAIL: &str = "Metadata Audit: Source file access failed";
    pub const MSG_METADATA_TREE_SUCCESS: &str =
        "Metadata Audit: Recursive tree preservation successful";
    pub const MSG_METADATA_TREE_PRESERVE: &str =
        "Metadata Audit: {} initiating recursive tree preservation...";

    pub const MSG_PROBE_NB_FRAMES_MISSING: &str =
        "Probe Audit: 'nb_frames' missing in stream metadata; result may be unreliable";
    pub const MSG_PROBE_BITRATE_MISSING: &str = "Probe Audit: Bitrate information missing";
    pub const MSG_PROBE_DURATION_FAIL: &str = "Probe Audit: Failed to extract duration for {}";
    pub const MSG_PROBE_DURATION_MISSING: &str = "Probe Audit: Duration information missing";
    pub const MSG_PROBE_FILE_NOT_FOUND: &str = "Probe Audit: Target file not found: {}";
    pub const MSG_PROBE_NOT_A_FILE: &str = "Probe Audit: Target is not a regular file: {}";
    pub const MSG_PROBE_TOOL_MISSING: &str = "ffprobe not found; please install FFmpeg";

    pub const VAL_EXCELLENT: &str = "Excellent";
    pub const VAL_VERY_GOOD: &str = "Very Good";
    pub const VAL_GOOD: &str = "Good";
    pub const VAL_READY: &str = "Ready";
    pub const VAL_SUCCESS: &str = "Success";
    pub const VAL_FAILED: &str = "Failed";
    pub const VAL_GOOD_MEETS_TARGET: &str = "Good (Meets Target)";
    pub const VAL_BELOW_TARGET: &str = "Below Target";

    pub const APPLE_COMPAT_NOT_COPYING: &str = "Apple compatibility mode: not copying";
    pub const APPLE_COMPAT_NOT_COPYING_DETAILED: &str =
        "Apple compatibility mode: not copying (using slower re-encode)";
    pub const APPLE_COMPAT_HEVC: &str =
        "Apple compatibility mode only supports HEVC output. Please specify --codec hevc.";
    pub const GPU_PROBE_FAILED: &str = "GPU probe failed: {reason}";
    pub const GPU_PROBE_START: &str = "Starting GPU hardware acceleration probe";
    pub const GPU_DETECTED: &str = "GPU hardware acceleration detected and verified";
    pub const GPU_NOT_AVAILABLE: &str = "GPU hardware acceleration not available or disabled";
    pub const DOVI_TOOL_MISSING: &str = "dovi_tool not found; cannot extract RPU";
    pub const DOVI_TOOL_INSTALL: &str =
        "Install dovi_tool (https://github.com/quietvoid/dovi_tool) to enable DV preservation";
    pub const HDR10PLUS_TOOL_MISSING: &str = "hdr10plus_tool not found; cannot extract metadata";

    pub const PROTECT_QUALITY_LOW: &str = "PROTECT_QUALITY_LOW";
    pub const DISCARD_QUALITY_LOW: &str = "DISCARD_QUALITY_LOW";
    pub const PROTECT_QUALITY_SIZE: &str = "PROTECT_QUALITY_SIZE";
    pub const DISCARD_QUALITY_SIZE: &str = "DISCARD_QUALITY_SIZE";
    pub const QUALITY_GATE_FAILED: &str =
        "Quality gate failed: output size or metrics below safe thresholds";
    pub const PROTECTING_ORIGINAL: &str = "Protecting original file (fallback source)";
    pub const DISCARDING_OUTPUT: &str = "Discarding candidate output (failed safety audit)";
    pub const PROTECT_SSIM_NA: &str = "PROTECT_SSIM_NA";
    pub const DISCARD_SSIM_FAIL: &str = "DISCARD_SSIM_FAIL";
    pub const SSIM_CALC_FAILED: &str = "SSIM calculation failed; defaulting to safe fallback";
    pub const VERIFICATION_COMPLETE: &str = "Verification complete";

    pub const LOSSLESS_FALLBACK_MAGICK: &str = "Lossless fallback: ImageMagick";
    pub const JXL_STRIPPED_TAIL_RETRY: &str = "JXL: retry with stripped tail metadata";
    pub const AVIF_MATHEMATICAL_LOSSLESS_WARNING: &str =
        "AVIF: mathematical lossless warning (colorspace shift possible)";
    pub const JXL_FINALIST_FAILURE_KEEP_BASELINE: &str =
        "JXL finalist verification failed; keeping the d0 baseline";
    pub const SIPS_TRY_FIRST: &str = "SIPS: trying primary conversion";
    pub const SIPS_SUCCESS: &str = "SIPS conversion successful";
    pub const SIPS_FAIL_TRY_MAGICK: &str = "SIPS failed; falling back to ImageMagick";
    pub const MAGICK_SUCCESS: &str = "ImageMagick conversion successful";
    pub const MAGICK_FAIL_TRY_CJXL: &str = "ImageMagick failed; falling back to native cjxl";
    pub const RUN_LOG_OPEN_FAIL: &str = "Failed to open run log file for writing";
    pub const COPYING_UNSUPPORTED: &str =
        "Copying requested but format is unsupported for direct copy";
    pub const OUTPUT_VERIFY: &str = "Verifying output file integrity";
    pub const METADATA_SYNC: &str = "Synchronizing metadata from original file";
    pub const INGEST_COMPLETE: &str = "Database Audit: Batch ingestion finalized";
    pub const STRESS_TEST_COMPLETE: &str =
        "Stress Test Audit: Completed; all concurrent handles joined successfully";
    pub const DV_RPU_PRESERVED: &str = "Dolby Vision detected: RPU will be preserved via dovi_tool";
    pub const ALGO_VERSION_DIST: &str = "Algorithm Version Distribution:";
    pub const DIR_TIMESTAMPS_RESTORED: &str = "Integrity Audit: Directory timestamps restored";
    pub const ICC_PATCH_SUCCESS: &str = "Recovery Audit: ICC patch synchronization successful";
    pub const PARALLEL_SPEEDUP: &str = "Parallel speedup:";
    pub const CALCULATING_MSSSIM: &str = "🔄 Calculating MS-SSIM scores for Y/U/V channels...";
    pub const MSSSIM_FINALIZED: &str = "✅ MS-SSIM parallel calculation finalized";
    pub const MSSSIM_SCORES: &str = "✅ Scores:";
    pub const DB_HEALTH_START: &str = "Starting deep database health diagnostic scan...";
    pub const DB_HEALTH_FINALIZED: &str =
        "Database infrastructure audit finalized: Health Report generated.";
    pub const DB_INVENTORY_CROSS_REF: &str =
        "Cross-referencing table statistics and record counts:";
    pub const REASON_STR: &str = "Reason:";
    pub const DOLBY_VISION_DETECTED: &str =
        "Dolby Vision detected: RPU will be preserved via dovi_tool";
    pub const ORIGINAL_DELETED_VERIFIED: &str = "Original deleted (integrity verified)";
    pub const HDR_RECOVERY_DOWNGRADE: &str =
        "HDR Recovery: Downgrading to HDR10 static layer (dynamic metadata extraction failed)";
    pub const PSNR_INFINITY: &str = "PSNR: ∞ dB (Identical - mathematically lossless)";
    pub const NO_IMAGES_FOUND: &str = "📂 No image files found in";
    pub const JXL_OPTIMAL_SKIP: &str =
        "Source is static JPEG XL (already optimal) - skipping to avoid generational loss";
    pub const MSG_GIF_MSSSIM_UNSUPPORTED: &str =
        "GIF format - MS-SSIM not supported (palette-based). No fallback.";
    pub const MSG_SSIM_NA_DETAIL: &str =
        "cannot validate quality │ may indicate codec compatibility issues (VP8/VP9/alpha channel)";
    pub const MSG_3D_GATE_FAILED: &str = "3D quality gate failed";
    pub const MSG_QUALITY_TARGET_FAILED: &str = "QUALITY TARGET FAILED";
    pub const MSG_HDR10PLUS_HARVEST_SUCCESS: &str =
        "HDR Audit: HDR10+ dynamic telemetry successfully harvested (preserved via dhdr10-info)";
    pub const MSG_CORRUPTION_SKIP: &str =
        "[Corruption] JPEG file is truncated or missing EOI, skipping expensive fallback.";
    pub const MSG_MS_SSIM_INIT: &str =
        "Initiating multi-channel MS-SSIM analysis (Y+U+V) for structural fidelity audit...";
    pub const MSG_HDR_INIT: &str = "Initiating HEIC to HDR JXL synthesis pipeline for target: {}";
    pub const MSG_HDR_SYNTH_ACTIVE: &str =
        "Forensic: Executing HDR GainMap synthesis (P3 Conversion: {})";
    pub const MSG_GAINMAP_EXTRACTED: &str =
        "Forensic: Gainmap isolated successfully ({}) | Reference Plane: {}";
    pub const MSG_GAINMAP_PARAMS: &str = "Forensic: Harvested ISO 21496-1 Gainmap metadata: {:.2} \
                                          (max) / {:.2} (min) | Gamma: {:.2}";
    pub const MSG_ENCODE_FINALIZED: &str =
        "Encoder Audit: x265 CPU encoding cycle finalized (Payload: {})";
    pub const MSG_VERIFICATION_INIT: &str =
        "Initiating cross-codec verification: comparing reference bitstream vs candidate output";
    pub const MSG_VERIFICATION_SUCCESS: &str =
        "✅ Verification Successful: perceptual integrity and structural consistency verified.";

    pub const MSG_EXPLORE_VMAF_SAMPLED: &str = "Exploration Audit: VMAF sampled: {}";
    pub const MSG_EXPLORE_FORCE_MS_SSIM: &str =
        "Exploration Audit: Forcing MS-SSIM due to lack of VMAF data";
    pub const MSG_EXPLORE_FFMPEG_ERR: &str = "Exploration Audit: FFmpeg error during probe: {}";
    pub const MSG_EXPLORE_SSIM_FAIL: &str = "Exploration Audit: SSIM calculation failed for probe";
    pub const MSG_EXPLORE_GOAL_QUALITY: &str = "Exploration Audit: Converged on target quality";
    pub const MSG_EXPLORE_SEPARATOR: &str =
        "────────────────────────────────────────────────────────────────────────────────";
    pub const MSG_HIGHLY_COMPRESSED_WARNING: &str =
        "⚠️  Highly compressed source detected. Visual quality may be impacted.";
    pub const MSG_STAGE_A: &str = "Stage A: Initial Heuristic Alignment";
    pub const MSG_STAGE_B1: &str = "Stage B1: Coarse Bidirectional Exploration";
    pub const MSG_STAGE_B2: &str = "Stage B2: Fine-Tuning Convergence";

    pub const MSG_ANALYZER_HEIC_AUX: &str =
        "Analyzer Audit: HEIC auxiliary image stream discovered for {}";
    pub const MSG_ANALYZER_HEIC_GAINMAP: &str =
        "Analyzer Audit: HEIC ISO 21496-1 Gainmap found for {}";
    pub const MSG_ANALYZER_HEIC_VENDOR: &str =
        "Analyzer Audit: HEIC vendor-specific metadata found for {}";
    pub const MSG_ANALYZER_CACHE_HIT: &str = "Analyzer Audit: Cache hit for {}";
    pub const MSG_ANALYZER_WEBP_JOINT_AUDIT: &str =
        "Analyzer Audit: Initiating joint WebP/Video forensic audit for {}";
    pub const MSG_ANALYZER_STATIC_GIF: &str =
        "Analyzer Audit: GIF confirmed as static image (1 frame) for {}";
    pub const MSG_ANALYZER_STATIC_MEDIA: &str =
        "Analyzer Audit: Media confirmed as static (non-animated) for {}";
    pub const MSG_ANALYZER_JXLLINFO_SUGGESTION: &str =
        "Forensic: Install jxlinfo for faster JXL metadata extraction";
    pub const MSG_JPEG_DIM_FAIL: &str = "Analyzer Audit: Failed to extract JPEG dimensions for {}";
    pub const MSG_JPEG_TRUNCATED: &str = "Analyzer Audit: JPEG file is truncated at {}";
    pub const MSG_ANALYZER_CACHE_STORE_ERR: &str =
        "Analyzer Audit: Failed to store results in cache: {}";
    pub const MSG_ANALYZER_DJXL_FAIL: &str = "Analyzer Audit: djxl bitstream research failed";

    pub const MSG_FORMAT_TIFF_BIGTIFF_SMALL: &str =
        "Format Audit: BigTIFF file too small for header: {}";
    pub const MSG_FORMAT_TIFF_BYTE_ORDER: &str = "Format Audit: Invalid TIFF byte order for {}";
    pub const MSG_FORMAT_TIFF_SMALL: &str = "Format Audit: TIFF file too small for header: {}";

    pub const MSG_METADATA_FALLBACK_FAIL: &str = "Metadata Audit: Fallback strategy failed: {}";
    pub const MSG_METADATA_REPAIR_MAGICK_FAIL: &str =
        "Metadata Audit: ImageMagick repair failed: {}";
    pub const MSG_METADATA_HINT: &str = "💡 Forensic Hint: {}";
    pub const MSG_METADATA_REPAIR_SUCCESS: &str =
        "Metadata Audit: Corruption repaired via ImageMagick: {}";
    pub const MSG_METADATA_EMERGENCY_FAIL: &str =
        "Metadata Audit: Emergency recovery failed for {}";
    pub const MSG_METADATA_FALLBACK_DETECT_FAIL: &str =
        "Metadata Audit: Fallback detection failed for {}";
    pub const MSG_METADATA_REPAIR_MAGICK_UNAVAILABLE: &str =
        "Metadata Audit: ImageMagick not found; cannot repair corruption: {}";
    pub const MSG_METADATA_EMERGENCY_SUCCESS: &str =
        "Metadata Audit: Emergency recovery successful";
    pub const MSG_METADATA_FAIL: &str = "Metadata Audit: Operation failed: {}";
    pub const MSG_METADATA_FALLBACK_START: &str =
        "Metadata Audit: Cascading to fallback strategy...";
    pub const MSG_METADATA_QT_SET_FAIL: &str =
        "Metadata Audit: Failed to set QuickTime metadata tags: {}";
    pub const MSG_METADATA_EMERGENCY_RECOVERY: &str =
        "Metadata Audit: Initiating emergency bitstream recovery...";
    pub const MSG_METADATA_EXIFTOOL_NOT_FOUND: &str =
        "exiftool not found; please install it for forensic metadata preservation";
    pub const MSG_METADATA_DELIVERY_SKIP_MISSING_SOURCE: &str =
        "Metadata delivery: source missing or unreadable; skipping preservation ({})";
    pub const MSG_METADATA_DELIVERY_SKIP_NO_SOURCE_EXIF: &str =
        "Metadata delivery: source had no writable EXIF/tags; continuing ({})";
    pub const MSG_METADATA_DELIVERY_SKIP_XATTR_ABSENCE: &str =
        "Metadata delivery: no xattr API or empty xattrs on source; continuing ({})";
    pub const MSG_METADATA_DELIVERY_XATTR_PARTIAL: &str =
        "Metadata delivery: xattr copy partial; continuing ({})";
    pub const MSG_METADATA_DELIVERY_TIMESTAMP_PARTIAL: &str =
        "Metadata delivery: timestamp sync partial; continuing ({})";
    pub const MSG_METADATA_QT_DATE_FAIL: &str =
        "Metadata Audit: Failed to extract QuickTime creation date";
    pub const MSG_METADATA_REPAIR_START: &str = "Metadata Audit: Attempting bitstream repair...";

    pub const MSG_VQD_INVALID_DUR: &str =
        "Video Analysis: Invalid duration detected; refusing to forge metadata";
    pub const MSG_VQD_INVALID_FPS: &str =
        "Video Analysis: Invalid FPS detected; refusing to forge metadata";
    pub const MSG_VQD_INVALID_FPS_NUM: &str =
        "Video Analysis: Invalid FPS numerator detected; refusing to forge metadata";
    pub const MSG_VQD_MISSING_BITRATE: &str = "Video Analysis: Bitrate information missing";
    pub const MSG_VQD_MISSING_COLOR: &str =
        "Video Analysis: Color space information missing; falling back to {}";
    pub const MSG_VQD_MISSING_DUR: &str = "Video Analysis: Duration information missing";
    pub const MSG_VQD_MISSING_FPS: &str = "Video Analysis: FPS information missing";
    pub const MSG_VQD_MISSING_HEIGHT: &str = "Video Analysis: Height information missing";
    pub const MSG_VQD_MISSING_WIDTH: &str = "Video Analysis: Width information missing";

    pub const MSG_PROCESS_FAIL: &str = "Process Audit: Command execution failed: {}";
    pub const MSG_PROCESS_SPAWN: &str = "Process Audit: Spawning process: {}";

    pub const MSG_ENCODER_AVAILABLE: &str = "Encoder Audit: x265 encoder available and verified";
    pub const MSG_ENCODER_MISSING: &str =
        "Encoder Audit: x265 encoder not found; please install x265";
    pub const MSG_ENCODE_BITSTREAM_FAIL: &str = "Encoder Audit: Bitstream generation failed";
    pub const MSG_ENCODE_CONCURRENCY_FAIL: &str =
        "Encoder Audit: Internal concurrency failure: {:?}";
    pub const MSG_ENCODE_DIRECT_AUDIT: &str = "Encoder Audit: Direct encoding finalized in {}s";
    pub const MSG_ENCODE_IPC_FAIL: &str = "Encoder Audit: IPC establishment failed";
    pub const MSG_ENCODE_PIPED_AUDIT: &str = "Encoder Audit: Piped encoding finalized in {}s";
    pub const MSG_MUX_START: &str = "Muxing Audit: Initiating encapsulation for {}...";

    pub const MSG_MSSSIM_BOTH_FAIL_ERR: &str = "Both MS-SSIM and SSIM failed: {}";
    pub const MSG_MSSSIM_GIF_UNSUPPORTED: &str = "GIF format: MS-SSIM not supported";
    pub const MSG_MSSSIM_SCORE_FAIL: &str = "Failed to retrieve MS-SSIM score for channel {}";
    pub const MSG_MSSSIM_SSIM_SCORE_FAIL: &str = "Failed to retrieve SSIM score for channel {}";
    pub const MSG_MSSSIM_THREAD_PANIC: &str = "MS-SSIM thread for channel {} panicked";

    pub const MSG_BATCH_FATAL_STOP: &str =
        "Batch Audit: Fatal error encountered; terminating batch processing loop";
    pub const MSG_COPY_UNSUPPORTED: &str =
        "Copy Audit: Format is unsupported for direct copy; falling back to re-encode";
    pub const MSG_METADATA_DIR_DONE: &str =
        "Metadata Audit: Recursive directory timestamp restoration finalized";
    pub const MSG_METADATA_DIR_SYNC: &str = "Metadata Audit: Synchronizing directory timestamps...";
    pub const MSG_STRATEGY_DESCRIPTION: &str = "Strategy Description: {}";
    pub const MSG_THREAD_STRATEGY: &str = "Thread Strategy: {}";
    pub const MSG_VERIFY_COMPLETENESS: &str =
        "Verification: All assets accounted for in forensic registry";
    pub const MSG_VERIFY_MISMATCH: &str = "Verification: Record mismatch detected for {}";

    pub const MSG_BATCH_DEPTH_FAIL: &str = "Batch Audit: Recursion depth limit exceeded";
    pub const MSG_BATCH_MOD_TIME_FAIL: &str =
        "Batch Audit: Failed to retrieve modification time for {}";
    pub const MSG_BATCH_PAUSE_DISK_FULL: &str = "Batch Audit: Disk full; pausing processing loop";
    pub const MSG_BATCH_SECURITY_SYMLINK: &str =
        "Batch Audit: Refusing to follow suspicious symlink: {}";
    pub const MSG_FFMPEG_READER_PANIC: &str = "Pipeline Audit: FFmpeg reader thread panicked";
    pub const MSG_FFMPEG_STDERR_PANIC: &str =
        "Pipeline Audit: FFmpeg stderr monitor thread panicked";
    pub const MSG_GPU_ACCURATE_BOUNDARY: &str = "GPU Audit: Converged on accurate boundary";
    pub const MSG_GPU_DONE_TIME: &str = "GPU Audit: Hardware acceleration probe finalized at {}";
    pub const MSG_GPU_FINE_TUNED_NARROW: &str = "GPU Audit: Fine-tuning narrow search window";
    pub const MSG_GPU_LOW_SSIM_EXPAND: &str =
        "GPU Audit: Low SSIM detected; expanding exploration window";
    pub const MSG_GPU_MAPPING_EXAMPLE: &str = "GPU Mapping Example: {}";
    pub const MSG_GPU_MAPPING_REPORT: &str = "GPU Mapping Report generated";
    pub const MSG_GPU_MAPPING_UNCALIBRATED: &str = "GPU Mapping: Uncalibrated hardware detected";
    pub const MSG_GPU_PROBE_NOTE: &str = "GPU Audit: {}";
    pub const MSG_GPU_REPORT_AV1: &str = "GPU Audit: AV1 hardware acceleration supported ({})";
    pub const MSG_GPU_REPORT_H264: &str = "GPU Audit: H.264 hardware acceleration supported ({})";
    pub const MSG_GPU_REPORT_HEVC: &str = "GPU Audit: HEVC hardware acceleration supported ({})";
    pub const MSG_GPU_START_TIME: &str = "GPU Audit: Hardware acceleration probe initiated at {}";
    pub const MSG_GPU_VT_INFO: &str = "GPU Audit: VideoToolbox hardware acceleration discovered";
    pub const MSG_HOME_DETERMINE_FAIL: &str = "Audit: Failed to determine user home directory: {}";
    pub const MSG_PATH_NON_UTF8: &str = "Audit: Path is not valid UTF-8: {}";
    pub const MSG_PHASE_2_SSIM: &str = "Phase 2 Audit: SSIM-based refinement active";
    pub const MSG_PHASE_2_ULTIMATE: &str = "Phase 2 Audit: Ultimate quality refinement active";
    pub const MSG_PHASE_2_UPWARD: &str = "Phase 2 Audit: Upward quality refinement active";
    pub const MSG_PROGRESS_DONE: &str = "Progress: Done";
    pub const MSG_PROGRESS_LOCKED: &str = "Progress: Locked";
    pub const MSG_QUALITY_APPLE_COMPAT_HEVC: &str =
        "Quality Audit: Apple compatible HEVC output is already present";
    pub const MSG_QUALITY_BPP_TARGET: &str = "Quality Audit: BPP target reached: {}";
    pub const MSG_QUALITY_CUTTING_EDGE: &str = "Quality Audit: Cutting-edge strategy selected";
    pub const MSG_QUALITY_HDR_CONFIRMED: &str = "Quality Audit: HDR synthesis confirmed";
    pub const MSG_QUALITY_MODERN: &str = "Quality Audit: Modern strategy selected";
    pub const MSG_QUALITY_OPTIMAL_PARAM: &str = "Quality Audit: Optimal parameter found: {}";
    pub const MSG_QUALITY_REPORT_HIGH: &str = "Quality Audit: High-fidelity output verified";
    pub const MSG_QUALITY_REPORT_MEDIUM: &str = "Quality Audit: Medium-fidelity output verified";
    pub const MSG_QUALITY_REPORT_OUTPUT: &str = "Quality Audit: Output report generated";
    pub const MSG_QUALITY_REPORT_SOURCE: &str = "Quality Audit: Source report generated";
    pub const MSG_QUALITY_SKIP_REASON: &str = "Quality Audit: Skipping {} (already optimal)";
    pub const MSG_QUALITY_SKIP_REASON_IMAGE: &str =
        "Quality Audit: Skipping image {} (already optimal)";
    pub const MSG_QUALITY_STREAM_TIMELINE: &str = "Quality Audit: Stream timeline: {}s";
    pub const MSG_QUALITY_TEMPORAL_VELOCITY: &str = "Quality Audit: Temporal velocity: {} fps";
    pub const MSG_SSIM_ERR_LINES: &str = "SSIM Audit: Error lines detected: {}";
    pub const MSG_SSIM_FAST_MODE: &str = "SSIM Audit: Fast mode active";
    pub const MSG_SSIM_INPUT_DISPLAY: &str = "SSIM Audit: Input: {}";
    pub const MSG_SSIM_PIX_FMT_ERR: &str = "SSIM Audit: Pixel format error";
    pub const MSG_SSIM_STRATEGY: &str = "SSIM Audit: Strategy: {}";
    pub const MSG_SSIM_U_BYPASS: &str = "SSIM Audit: U-channel bypass active";
    pub const MSG_SSIM_VMAF_FIX: &str = "SSIM Audit: VMAF fix applied";
    pub const MSG_SSIM_VMAF_MISSING: &str = "SSIM Audit: VMAF data missing";
    pub const MSG_SSIM_VMAF_OLD: &str = "SSIM Audit: Legacy VMAF model detected";
    pub const MSG_SSIM_VMAF_SCORE: &str = "SSIM Audit: VMAF score: {}";
    pub const MSG_SSIM_V_BYPASS: &str = "SSIM Audit: V-channel bypass active";
    pub const MSG_UI_LOGGER_REINIT: &str = "UI Audit: Logger reinitialized";
    pub const MSG_UI_LOGGER_UNINIT: &str = "UI Audit: Logger uninitialized";
    pub const MSG_XMP_DOC_ID_SCAN: &str = "XMP Audit: Scanning for DocumentID: {}";
    pub const MSG_XMP_EXTRACT_FAIL: &str = "XMP Audit: Extraction failed for {}: {}";
    pub const MSG_XMP_FIND_MATCH: &str = "XMP Audit: Finding match for {}";
    pub const MSG_XMP_MATCH_FOUND: &str = "XMP Audit: Match found for {}";
    pub const MSG_XMP_NO_MATCH: &str = "XMP Audit: No match found";
    pub const MSG_XMP_STRATEGY_1: &str = "XMP Audit: Strategy 1 active for {}";
    pub const MSG_XMP_STRATEGY_2: &str = "XMP Audit: Strategy 2 active for {}";
    pub const MSG_XMP_STRATEGY_2_5: &str = "XMP Audit: Strategy 2.5 active for {}";
    pub const MSG_XMP_STRATEGY_3: &str = "XMP Audit: Strategy 3 active for {}";
    pub const MSG_XMP_STRATEGY_4: &str = "XMP Audit: Strategy 4 active for {}";
    pub const MSG_XMP_STRATEGY_5: &str = "XMP Audit: Strategy 5 active for {}";
    pub const MSG_XMP_STRATEGY_6: &str = "XMP Audit: Strategy 6 active for {}";
    pub const MSG_XMP_STRATEGY_7: &str = "XMP Audit: Strategy 7 active for {}";
    pub const MSG_XMP_STRATEGY_8: &str = "XMP Audit: Strategy 8 active for {}";
    pub const MSG_XMP_UNREADABLE: &str = "XMP Audit: File is unreadable: {}";

    pub const MSG_BRANDING_DESCRIPTION: &str = "Modern Format Boost - Forensic Media Pipeline";
    pub const MSG_PHASE_2_STRATEGY: &str = "Phase 2 Strategy: {}";
    pub const MSG_PROGRESS_INACTIVE: &str = "Progress: Inactive";
    pub const MSG_PROGRESS_UNKNOWN: &str = "Progress: Unknown";
    pub const MSG_SSIM_SCORE: &str = "SSIM Score: {}";

    pub const MSG_MAIN_DB_HEALTH_ABORT: &str =
        "Infrastructure Audit: Critical corruption detected; aborting sequence: {}";
    pub const MSG_MAIN_DB_HEALTH_CONN: &str =
        "Infrastructure Audit: Database connectivity status: {}";
    pub const MSG_MAIN_DB_HEALTH_CONN_FAIL: &str =
        "Infrastructure Audit: Connection refused or timed out";
    pub const MSG_MAIN_DB_HEALTH_CONN_OK: &str =
        "Infrastructure Audit: High-availability connection established";
    pub const MSG_MAIN_DB_HEALTH_CORRUPTION: &str =
        "Infrastructure Audit: Block-level corruption discovered in {}";
    pub const MSG_MAIN_DB_HEALTH_CORRUPTION_ALERT: &str =
        "Infrastructure Audit: DATA LOSS IMMINENT - Integrity violation in {}";
    pub const MSG_MAIN_DB_HEALTH_ENGINE: &str =
        "Infrastructure Audit: Backend engine identified as {}";
    pub const MSG_MAIN_DB_HEALTH_INTEGRITY_OK: &str =
        "Infrastructure Audit: Full relational integrity verified";
    pub const MSG_MAIN_DB_HEALTH_MATURITY: &str =
        "Infrastructure Audit: Deployment maturity level: {}";
    pub const MSG_MAIN_DB_HEALTH_TABLE: &str =
        "Infrastructure Audit: Validating schema for table {}...";
    pub const MSG_MAIN_IMAGE_MAPPING: &str =
        "Image Processing Strategy: Mapping forensic parameters for {}";
    pub const MSG_MAIN_LOCK_FAIL: &str =
        "Concurrency Audit: Failed to acquire exclusive lock for {}: {}";
    pub const MSG_MAIN_VID_MAPPING_LOSSLESS: &str =
        "Video Mapping Strategy: Executing bit-perfect lossless transcoding for {}";
    pub const MSG_MAIN_VID_MAPPING_LOSSY_BASE: &str =
        "Video Mapping Strategy: Standard perceptual compression active for {}";
    pub const MSG_MAIN_VID_MAPPING_LOSSY_MATCH: &str =
        "Video Mapping Strategy: High-fidelity quality-matching active for {}";
    pub const MSG_MAIN_VID_STRATEGY_ASSET: &str = "Strategy Audit: Analyzing asset {}";
    pub const MSG_MAIN_VID_STRATEGY_AUDIT: &str =
        "Strategy Audit: Initiating video processing decision matrix...";
    pub const MSG_MAIN_VID_STRATEGY_BASIS: &str = "Strategy Audit: Rationale for selection: {}";
    pub const MSG_MAIN_VID_STRATEGY_DETECTION: &str =
        "Strategy Audit: Source detected as {} with {} encoding";
    pub const MSG_MAIN_VID_STRATEGY_TARGET: &str = "Strategy Audit: Target mapped to {} profile";
}

/// Logs the start of a task with a distinct visual marker.
pub fn log_task_start(task: &str) {
    log_task_start_path(None, task);
}

/// Logs task start with optional full path for forensic grep in run logs.
pub fn log_task_start_path(path: Option<&std::path::Path>, label: &str) {
    crate::ctrlc_guard::wait_if_prompt_active();
    let marker = TerminalColor::info("▶");
    if let Some(p) = path {
        tracing::info!(
            target: "mfb::progress",
            path = %p.display(),
            outcome = "started",
            "{} Processing: {}",
            marker,
            label
        );
    } else {
        tracing::info!(
            target: "mfb::progress",
            outcome = "started",
            "{} Processing: {}",
            marker,
            label
        );
    }
}

#[cfg(test)]
mod ui_contract_tests {
    use super::{ErrorSeverity, get_symbol_by_label, plain_aware_detail};
    use std::sync::Mutex;

    static PLAIN_MODE_TEST_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn error_severity_plain_label_colored() {
        let _guard = PLAIN_MODE_TEST_LOCK.lock().expect("plain-mode test lock");
        crate::progress_mode::set_plain_mode(true);
        assert_eq!(ErrorSeverity::Critical.label_colored(), "[CRITICAL]");
        crate::progress_mode::set_plain_mode(false);
    }

    #[test]
    fn plain_aware_detail_rewrites_leading_emoji() {
        let _guard = PLAIN_MODE_TEST_LOCK.lock().expect("plain-mode test lock");
        crate::progress_mode::set_plain_mode(true);
        assert_eq!(
            plain_aware_detail("✅ Verification OK"),
            "[OK] Verification OK"
        );
        assert_eq!(
            plain_aware_detail("⚠️  Highly compressed"),
            "[WARN] Highly compressed"
        );
        assert_eq!(
            plain_aware_detail("\r      📊 SSIM: 0.99"),
            "\r      [MET] SSIM: 0.99"
        );
        crate::progress_mode::set_plain_mode(false);
        assert_eq!(
            plain_aware_detail("✅ Verification OK"),
            "✅ Verification OK"
        );
    }

    #[test]
    fn get_symbol_by_label_plain_mode_returns_ascii() {
        let _guard = PLAIN_MODE_TEST_LOCK.lock().expect("plain-mode test lock");
        crate::progress_mode::set_plain_mode(true);
        assert_eq!(get_symbol_by_label("Operation Audit: Success"), "[OK]");
        assert_eq!(get_symbol_by_label("Forensic Hint"), "[i]");
        crate::progress_mode::set_plain_mode(false);
        assert_eq!(get_symbol_by_label("Operation Audit: Success"), "✅");
    }
}
