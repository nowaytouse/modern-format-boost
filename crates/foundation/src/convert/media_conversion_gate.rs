//! Strict gates for **media conversion** processing (`img` / `vid` delivery
//! layer) and **probe/detection** paths that feed delivery (`video_detection`,
//! `video_explorer`, `image_analyzer`).
//!
//! Training tier rules live in [`crate::training_tier_audit`]; algorithm seals
//! live in [`crate::algorithm_seal`]. All audits emit `[delivery
//! fallback:branch]` for unified log search.

#![allow(clippy::missing_panics_doc, clippy::panic, clippy::single_match_else)]

use crate::builder_base::ToolBuilder;
use crate::image_analyzer::ImageAnalysis;
use crate::jxl_explorer::JxlExploreResult;
use crate::video_explorer::ExploreResult;
use crate::video_explorer::precision;
use std::borrow::Cow;
use std::cmp::Ordering;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Explicit audit for delivery-layer fallbacks (never silent). Sole conversion
/// `log_anomaly` site (M100/M101).
pub(crate) fn delivery_fallback_audit(branch: &'static str, detail: impl AsRef<str>) {
    let detail = detail.as_ref();
    tracing::warn!(target: "mfb.conversion", branch, "{detail}");
    crate::log_anomaly!(
        crate::infra::static_logs::messages::LABEL_CONVERSION,
        &format!("[delivery fallback:{branch}] {detail}")
    );
}

/// Delivery path/layout fallback (stem/parent/collision): audit only under
/// strict (M87/M97).
pub fn delivery_path_layout_fallback_audit(branch: &'static str, detail: impl AsRef<str>) {
    delivery_strict_batch_audit(branch, detail);
}

/// Quality CRF heuristic missing optional field: audit only under strict (M85;
/// probe layer is SSOT).
pub fn quality_heuristic_fallback_audit(branch: &'static str, detail: impl AsRef<str>) {
    probe_quality_batch_audit(branch, detail);
}

/// Missing `content_type` in CRF quality analysis (Unknown adjustment + audit,
/// M104).
pub fn quality_content_type_missing_audit() {
    quality_heuristic_fallback_audit(
        "quality_content_type_unknown",
        "content_type missing in quality analysis; using Unknown CRF adjustment",
    );
}

/// Content type for CRF analysis when analysis omitted it (Unknown + audit,
/// M104/M182).
#[must_use]
pub fn quality_content_type_for_crf_or_unknown(
    content_type: Option<crate::quality_matcher::ContentType>,
) -> crate::quality_matcher::ContentType {
    content_type.unwrap_or_else(|| {
        quality_content_type_missing_audit();
        crate::quality_matcher::ContentType::Unknown
    })
}

/// JPEG quantization table cell after IJG scaling (1 + audit when u16 cast
/// fails).
#[must_use]
pub fn delivery_jpeg_qt_cell_u16_or_one(scaled_value: f64, quality: u8) -> u16 {
    crate::numeric_cast::f64_to_u16_strict(scaled_value, "generate_standard_qt").unwrap_or_else(
        || {
            delivery_numeric_fallback_audit(
                "delivery_jpeg_qt",
                format!(
                    "JPEG QT cell out of u16 range after IJG scaling (quality={quality}, \
                     raw={scaled_value}); using floor 1"
                ),
            );
            1
        },
    )
}

/// Probe/detection recovery note (e.g. animated promote): strict via probe
/// layer (M88/M97).
pub fn probe_detection_recovery_audit(branch: &'static str, detail: impl AsRef<str>) {
    probe_layer_batch_audit(branch, detail);
}

/// Single strict-gated path audit entry (M96 SSOT). Domain wrappers delegate
/// here.
///
/// With strict delivery disabled the audit is downgraded to `tracing::debug!`,
/// never dropped: the fallback substitution still happens, so a trace of it
/// must survive.
pub fn delivery_strict_path_audit(branch: &'static str, path: &Path, detail: impl AsRef<str>) {
    if crate::algorithm_runtime::strict_media_conversion_delivery_enabled() {
        delivery_path_audit(branch, path, detail);
    } else {
        tracing::debug!(
            target: "mfb.audit",
            branch,
            path = %path.display(),
            "{}",
            detail.as_ref()
        );
    }
}

/// Single strict-gated batch audit entry (M96/M98 SSOT). Domain wrappers and
/// gate `*_or_default` helpers delegate here.
///
/// With strict delivery disabled the audit is downgraded to `tracing::debug!`,
/// never dropped: the fallback substitution still happens, so a trace of it
/// must survive.
pub fn delivery_strict_batch_audit(branch: &'static str, detail: impl AsRef<str>) {
    if crate::algorithm_runtime::strict_media_conversion_delivery_enabled() {
        delivery_batch_audit(branch, detail);
    } else {
        tracing::debug!(target: "mfb.audit", branch, "{}", detail.as_ref());
    }
}

/// `img` / `vid` API path-scoped encode/reconcile fallback (delegates to
/// [`delivery_strict_path_audit`]).
pub fn delivery_api_path_fallback_audit(
    branch: &'static str,
    path: &Path,
    detail: impl AsRef<str>,
) {
    delivery_strict_path_audit(branch, path, detail);
}

/// `img` / `vid` API batch fallback without a stable path (delegates to
/// [`delivery_strict_batch_audit`]).
pub fn delivery_api_batch_fallback_audit(branch: &'static str, detail: impl AsRef<str>) {
    delivery_strict_batch_audit(branch, detail);
}

/// JXL encode/ICC recovery fallback: audit only under strict (M90/M97).
pub fn delivery_jxl_path_fallback_audit(
    branch: &'static str,
    path: &Path,
    detail: impl AsRef<str>,
) {
    delivery_strict_path_audit(branch, path, detail);
}

/// JXL batch fallback without a stable path: audit only under strict (M92/M97).
pub fn delivery_jxl_batch_fallback_audit(branch: &'static str, detail: impl AsRef<str>) {
    delivery_strict_batch_audit(branch, detail);
}

/// JXL near-best margin cast fallback (None + strict JXL audit).
#[must_use]
pub fn delivery_jxl_margin_u64_or_one(value: Option<u64>, detail: impl AsRef<str>) -> Option<u64> {
    if value.is_none() {
        delivery_jxl_batch_fallback_audit("jxl_near_best_margin", detail);
    }
    value
}

/// Substrate path audit (delegates to [`delivery_strict_path_audit`], M95/M96).
fn delivery_substrate_path_audit(branch: &'static str, path: &Path, detail: impl AsRef<str>) {
    delivery_strict_path_audit(branch, path, detail);
}

/// Substrate batch audit (delegates to [`delivery_strict_batch_audit`],
/// M95/M96).
fn delivery_substrate_batch_audit(branch: &'static str, detail: impl AsRef<str>) {
    delivery_strict_batch_audit(branch, detail);
}

/// Explore size-target failure label when `failure_reason` is empty (strict
/// audit).
#[must_use]
pub fn explore_size_target_failure_reason_or_default(reason: Option<&str>) -> String {
    if let Some(r) = reason {
        return r.to_string();
    }
    explore_delivery_explore_outcome_audit(
        "explore_size_target_reason",
        "size_target_met failure_reason empty; using default label",
    );
    String::from("size target not met")
}

/// Canonical path key for duplicate/processed tracking; logs when
/// canonicalization is unavailable.
///
/// For paths that don't exist yet (e.g., output paths being reserved),
/// canonicalization will fail - this is expected behavior and we silently fall
/// back to display path. For existing files, canonicalization provides a stable
/// key across symlinks and relative paths.
#[must_use]
pub fn processed_path_key(path: &Path) -> String {
    match path.canonicalize() {
        Ok(canonical) => canonical.to_str().map_or_else(
            || {
                delivery_strict_batch_audit(
                    "path_utf8",
                    format!(
                        "{}: canonical path is not UTF-8; using display path for tracking",
                        canonical.display()
                    ),
                );
                path.display().to_string()
            },
            ToString::to_string,
        ),
        Err(err) => {
            // Non-existent outputs are expected during reservation; audit only when the
            // path exists.
            if path.exists() {
                delivery_strict_batch_audit(
                    "path_canonicalize",
                    format!(
                        "{} ({err}): using display path for tracking",
                        path.display()
                    ),
                );
            }
            path.display().to_string()
        }
    }
}

/// Canonical path for external tools; logs and returns non-canonical input when
/// needed.
#[must_use]
pub fn canonicalize_for_tool_input(input: &Path) -> PathBuf {
    canonicalize_path_or_preserve(input, |detail| {
        delivery_strict_batch_audit("tool_input_canonicalize", detail);
    })
}

/// Checkpoint progress-dir root; same resolution policy as
/// [`canonicalize_for_tool_input`] (M120).
#[must_use]
pub fn canonicalize_for_checkpoint_path(input: &Path) -> PathBuf {
    canonicalize_path_or_preserve(input, |detail| {
        delivery_checkpoint_batch_audit("checkpoint_path_canonicalize", detail);
    })
}

fn canonicalize_path_or_preserve(input: &Path, audit: impl FnOnce(String)) -> PathBuf {
    match std::fs::canonicalize(input) {
        Ok(path) => path,
        Err(err) => {
            audit(format!(
                "{} ({err}): using non-canonical path",
                input.display()
            ));
            input.to_path_buf()
        }
    }
}

/// Optional scalar for audit/format strings only (`0` label means missing; not
/// a measured value).
#[must_use]
pub const fn delivery_audit_optional_u32(value: Option<u32>) -> u32 {
    match value {
        Some(v) => v,
        None => 0,
    }
}

/// Optional scalar for audit/format strings only (`0` label means missing; not
/// a measured value).
#[must_use]
pub const fn delivery_audit_optional_u64(value: Option<u64>) -> u64 {
    match value {
        Some(v) => v,
        None => 0,
    }
}

/// Frame count (`u64` probe) for delivery / reconciliation messages.
#[must_use]
pub fn delivery_frame_count_label_u64(frame_count: Option<u64>, context: &str) -> String {
    frame_count.map_or_else(
        || delivery_frame_count_label(None, context),
        |count| count.to_string(),
    )
}

/// Frame count for delivery / reconciliation messages (never silent `unknown`).
#[must_use]
pub fn delivery_frame_count_label(frame_count: Option<u32>, context: &str) -> String {
    frame_count.map_or_else(
        || {
            delivery_strict_batch_audit(
                "animation_frame_count",
                format!("{context}: frame_count missing in probe metadata"),
            );
            String::from("unreported")
        },
        |count| count.to_string(),
    )
}

/// File name for user-facing delivery logs.
#[must_use]
pub fn path_file_name_for_log(path: &Path) -> String {
    path.file_name().map_or_else(
        || {
            delivery_strict_batch_audit(
                "path_file_name",
                format!(
                    "{}: missing file_name; using display path in log",
                    path.display()
                ),
            );
            path.display().to_string()
        },
        |name| name.to_string_lossy().into_owned(),
    )
}

/// First stderr line for tool failure reporting.
#[must_use]
pub fn stderr_first_line_label(stderr: &[u8], path: &Path, branch: &'static str) -> String {
    String::from_utf8_lossy(stderr).lines().next().map_or_else(
        || {
            delivery_strict_batch_audit(
                branch,
                format!("{}: tool stderr has no first line", path.display()),
            );
            String::from("(no stderr line)")
        },
        str::to_string,
    )
}

/// Non-empty optional label (HDR signals, etc.).
#[must_use]
pub fn optional_nonempty_label(
    branch: &'static str,
    label: Option<&str>,
    fallback: &'static str,
    context: &str,
) -> String {
    label.filter(|value| !value.is_empty()).map_or_else(
        || {
            delivery_strict_batch_audit(
                branch,
                format!("{context}: using fallback label \"{fallback}\""),
            );
            fallback.to_string()
        },
        str::to_string,
    )
}

/// Extension string for delivery messages.
#[must_use]
pub fn path_extension_label(path: &Path) -> String {
    path.extension().and_then(|ext| ext.to_str()).map_or_else(
        || {
            delivery_strict_batch_audit(
                "path_extension",
                format!("{}: missing extension in delivery message", path.display()),
            );
            String::from("unknown")
        },
        str::to_string,
    )
}

/// Base directory for batch copy/verify when unset.
#[must_use]
pub fn base_dir_or_default<'a>(base: Option<&'a Path>, branch: &'static str) -> &'a Path {
    base.unwrap_or_else(|| {
        delivery_strict_batch_audit(
            branch,
            "base_dir unset; using \".\" for directory operations",
        );
        Path::new(".")
    })
}

/// Volume to probe for free disk space when output dir is unset.
#[must_use]
pub fn disk_space_probe_path<'a>(output_dir: Option<&'a Path>, input: &'a Path) -> &'a Path {
    output_dir.unwrap_or_else(|| {
        delivery_strict_batch_audit(
            "disk_space_probe",
            format!(
                "output_dir unset; probing free space at input {}",
                input.display()
            ),
        );
        input
    })
}

/// Output file stem for delivery paths.
#[must_use]
pub fn output_stem_for_delivery(input: &Path) -> String {
    if let Some(stem) = input.file_stem().and_then(|s| s.to_str()) {
        return stem.to_string();
    }
    delivery_path_layout_fallback_audit(
        "output_stem",
        format!(
            "{}: missing file stem; using literal \"output\"",
            input.display()
        ),
    );
    String::from("output")
}

/// Resolve output directory (shared by `img` / `vid` delivery).
#[must_use]
pub fn resolve_output_dir_for_delivery(
    input: &Path,
    base_dir: Option<&Path>,
    user_out: Option<&Path>,
) -> PathBuf {
    if let (Some(user_out), Some(base)) = (user_out, base_dir) {
        let rel = strip_prefix_or_self(input, base, "strip_base_dir");
        let parent = path_parent_or_dot(rel);
        return user_out.join(parent);
    }
    user_out.map_or_else(
        || {
            input.parent().map_or_else(
                || {
                    delivery_strict_batch_audit(
                        "output_parent",
                        format!(
                            "{}: no parent; placing output under cwd (.)",
                            input.display()
                        ),
                    );
                    PathBuf::from(".")
                },
                Path::to_path_buf,
            )
        },
        Path::to_path_buf,
    )
}

/// Probe / detection layer audit (same telemetry as delivery; pre-conversion
/// routing, M93/M98).
pub fn probe_layer_audit(branch: &'static str, path: &Path, detail: impl AsRef<str>) {
    if crate::algorithm_runtime::image_quality_heuristic_enabled() {
        delivery_strict_path_audit(branch, path, detail);
    } else {
        tracing::debug!(target: "mfb.audit", branch, path = %path.display(), "{}", detail.as_ref());
    }
}

/// Probe / detection batch audit (no single input path, M93/M98).
pub fn probe_layer_batch_audit(branch: &'static str, detail: impl AsRef<str>) {
    if crate::algorithm_runtime::image_quality_heuristic_enabled() {
        delivery_strict_batch_audit(branch, detail);
    } else {
        tracing::debug!(target: "mfb.audit", branch, "{}", detail.as_ref());
    }
}

/// Batch/resume checkpoint fallback (lock, progress, ps/hostname).
pub fn delivery_checkpoint_batch_audit(branch: &'static str, detail: impl AsRef<str>) {
    delivery_substrate_batch_audit(branch, detail);
}

/// Checkpoint fallback tied to progress or lock path.
pub fn delivery_checkpoint_path_audit(branch: &'static str, path: &Path, detail: impl AsRef<str>) {
    delivery_substrate_path_audit(branch, path, detail);
}

/// KNN / loop-intent / ingest DB fallback (no asset path).
pub fn delivery_db_batch_audit(branch: &'static str, detail: impl AsRef<str>) {
    delivery_substrate_batch_audit(branch, detail);
}

/// DB usize cast for percentile/index derivation (None + DB audit when
/// missing).
#[must_use]
pub fn delivery_db_usize_or_zero(value: Option<usize>, detail: &'static str) -> Option<usize> {
    if value.is_none() {
        delivery_db_batch_audit("delivery_db_numeric", detail);
    }
    value
}

/// DB JSON payload parse with audited None fallback.
#[must_use]
pub fn delivery_db_json_or_default<T, E>(
    parsed: Result<T, E>,
    context: &str,
    branch: &'static str,
) -> Option<T>
where
    E: std::fmt::Display,
{
    match parsed {
        Ok(v) => Some(v),
        Err(e) => {
            delivery_db_batch_audit(branch, format!("{context}; error: {e}"));
            None
        }
    }
}

/// DB fallback with asset path (ingest, similarity, metadata refinement).
pub fn delivery_db_path_audit(branch: &'static str, path: &Path, detail: impl AsRef<str>) {
    delivery_substrate_path_audit(branch, path, detail);
}

/// Static image format analysis fallback (JPEG / HEIC / TIFF / WebP probes).
pub fn probe_image_format_audit(branch: &'static str, path: &Path, detail: impl AsRef<str>) {
    probe_layer_audit(branch, path, detail);
}

/// Image format analysis when no stable path is in scope (in-memory buffer
/// parse).
pub fn probe_image_format_batch_audit(branch: &'static str, detail: impl AsRef<str>) {
    probe_layer_batch_audit(branch, detail);
}

/// Batch / CLI pipeline orchestration fallback (directory walk, pool init).
pub fn delivery_pipeline_batch_audit(branch: &'static str, detail: impl AsRef<str>) {
    delivery_strict_batch_audit(branch, detail);
}

/// Batch / CLI fallback tied to a specific file or directory path.
pub fn delivery_pipeline_path_audit(branch: &'static str, path: &Path, detail: impl AsRef<str>) {
    delivery_strict_path_audit(branch, path, detail);
}

/// Metadata / timestamp / xattr preservation fallback (path-scoped).
pub fn delivery_metadata_path_audit(branch: &'static str, path: &Path, detail: impl AsRef<str>) {
    delivery_substrate_path_audit(branch, path, detail);
}

/// Metadata preservation fallback when no single asset path is in scope.
pub fn delivery_metadata_batch_audit(branch: &'static str, detail: impl AsRef<str>) {
    delivery_substrate_batch_audit(branch, detail);
}

/// JXL ICC / health / encode fallback during img delivery (path-scoped, M97).
pub fn delivery_jxl_path_audit(branch: &'static str, path: &Path, detail: impl AsRef<str>) {
    delivery_strict_path_audit(branch, path, detail);
}

/// JXL delivery fallback without a stable path in scope (M97).
pub fn delivery_jxl_batch_audit(branch: &'static str, detail: impl AsRef<str>) {
    delivery_strict_batch_audit(branch, detail);
}

/// Loop-intent / penetration heuristic fallback (path-scoped).
pub fn delivery_intent_path_audit(branch: &'static str, path: &Path, detail: impl AsRef<str>) {
    delivery_substrate_path_audit(branch, path, detail);
}

/// Loop-intent fallback without a stable asset path in scope.
pub fn delivery_intent_batch_audit(branch: &'static str, detail: impl AsRef<str>) {
    delivery_substrate_batch_audit(branch, detail);
}

/// IO / filesystem substrate fallback (path-scoped).
pub fn delivery_io_path_audit(branch: &'static str, path: &Path, detail: impl AsRef<str>) {
    delivery_substrate_path_audit(branch, path, detail);
}

/// IO / copy substrate fallback without a stable path in scope.
pub fn delivery_io_batch_audit(branch: &'static str, detail: impl AsRef<str>) {
    delivery_substrate_batch_audit(branch, detail);
}

/// GPU detection / encode helper fallback (path-scoped; not explore coarse
/// search).
pub fn delivery_gpu_path_audit(branch: &'static str, path: &Path, detail: impl AsRef<str>) {
    delivery_substrate_path_audit(branch, path, detail);
}

/// GPU subsystem fallback without a stable path in scope.
pub fn delivery_gpu_batch_audit(branch: &'static str, detail: impl AsRef<str>) {
    delivery_substrate_batch_audit(branch, detail);
}

/// Elapsed time since UNIX epoch (`None` when system clock is before epoch).
#[must_use]
pub fn unix_duration_since_epoch_optional() -> Option<Duration> {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(duration) => Some(duration),
        Err(_) => {
            delivery_runtime_batch_audit(
                "delivery_numeric",
                "NUMERIC AUDIT: SystemTime before UNIX_EPOCH; refusing fabricated zero duration",
            );
            None
        }
    }
}

/// Legacy wall-clock duration helper (panics when clock invalid; prefer
/// [`unix_duration_since_epoch_optional`]).
#[must_use]
pub fn unix_duration_since_epoch_or_zero() -> Duration {
    unix_duration_since_epoch_optional().unwrap_or_else(|| {
        panic!(
            "unix_duration_since_epoch required for wall-clock SSOT; clock before UNIX_EPOCH or \
             unavailable"
        );
    })
}

/// Seconds since UNIX epoch for checkpoint/resume timestamps (`None` when clock
/// invalid).
#[must_use]
pub fn unix_epoch_secs_optional() -> Option<u64> {
    unix_duration_since_epoch_optional().map(|d| d.as_secs())
}

/// Legacy wall-clock seconds helper (panics when clock invalid; prefer
/// [`unix_epoch_secs_optional`]).
#[must_use]
pub fn unix_epoch_secs_or_zero() -> u64 {
    unix_epoch_secs_optional().unwrap_or_else(|| {
        panic!(
            "unix_epoch_secs required for wall-clock SSOT; clock before UNIX_EPOCH or unavailable"
        );
    })
}

static GPU_CONCURRENCY_CACHE: Mutex<
    Option<(
        Instant,
        crate::performance_schedule::PerfGovernorTier,
        usize,
    )>,
> = Mutex::new(None);

const GPU_CONCURRENCY_CACHE_TTL: Duration = Duration::from_secs(2);

/// GPU encode concurrency: explicit env wins; otherwise tier cap from
/// `performance_schedule`.
#[must_use]
pub fn gpu_concurrency_max_or_default() -> usize {
    match std::env::var(crate::constants::ENV_GPU_CONCURRENCY) {
        Ok(raw) => match raw.parse::<usize>() {
            Ok(value) if value >= 1 => return value,
            Ok(_) => delivery_substrate_batch_audit(
                "gpu_concurrency_env",
                format!(
                    "{} must be at least 1; using performance tier cap",
                    crate::constants::ENV_GPU_CONCURRENCY
                ),
            ),
            Err(e) => delivery_substrate_batch_audit(
                "gpu_concurrency_env",
                format!(
                    "failed to parse {}='{raw}': {e}; using performance tier cap",
                    crate::constants::ENV_GPU_CONCURRENCY
                ),
            ),
        },
        Err(std::env::VarError::NotPresent) => {}
        Err(e) => delivery_substrate_batch_audit(
            "gpu_concurrency_env",
            format!(
                "failed to read {}: {e}; using performance tier cap",
                crate::constants::ENV_GPU_CONCURRENCY
            ),
        ),
    }
    let tier = crate::performance_schedule::current_perf_tier();
    let now = Instant::now();
    let mut cache =
        mutex_guard_or_recover("gpu_concurrency_cap_cache", GPU_CONCURRENCY_CACHE.lock());
    if let Some((cached_at, cached_tier, value)) = *cache
        && cached_tier == tier
        && now.duration_since(cached_at) < GPU_CONCURRENCY_CACHE_TTL
    {
        return value;
    }
    let value = crate::performance_schedule::gpu_concurrency_cap(tier);
    *cache = Some((now, tier, value));
    value
}

/// Output extension segment for GPU temp filenames (policy-silent `mp4` when
/// missing, M95).
#[must_use]
pub fn gpu_output_extension_segment(output: &Path) -> String {
    output
        .extension()
        .and_then(|ext| ext.to_str())
        .map_or_else(|| String::from("mp4"), str::to_string)
}

/// Batch CLI skip/ignore reason (audit only in strict delivery when missing).
#[must_use]
pub fn pipeline_outcome_reason<'a>(
    reason: Option<&'a str>,
    fallback: &'static str,
    context: &str,
) -> Cow<'a, str> {
    reason.filter(|value| !value.is_empty()).map_or_else(
        || {
            delivery_pipeline_batch_audit(
                "pipeline_outcome",
                format!("{context}: missing outcome reason; using \"{fallback}\""),
            );
            Cow::Borrowed(fallback)
        },
        Cow::Borrowed,
    )
}

/// DB ingest label when `labeled_by` is absent (audit only in strict delivery).
#[must_use]
pub fn db_labeled_by_or_default(labeled_by: Option<&str>) -> &str {
    labeled_by.unwrap_or_else(|| {
        delivery_db_batch_audit(
            "db_labeled_by",
            "DB AUDIT: missing labeled_by on loop sample refresh; using loop_samples_refresh",
        );
        "loop_samples_refresh"
    })
}

/// Video encode / stream-size fallback (path-scoped).
pub fn delivery_encode_path_audit(branch: &'static str, path: &Path, detail: impl AsRef<str>) {
    delivery_substrate_path_audit(branch, path, detail);
}

/// Stream-size duration missing/unparseable: audit only under strict delivery
/// (M76).
pub fn stream_size_duration_fallback_audit(path: &Path, detail: impl AsRef<str>) {
    delivery_encode_path_audit("delivery_encode", path, detail);
}

/// Stream-size ffprobe/metadata failures: audit only under strict delivery
/// (M78).
pub fn stream_size_probe_failure_audit(path: &Path, detail: impl AsRef<str>) {
    delivery_encode_path_audit("delivery_encode", path, detail);
}

/// Video encode / stream-size fallback without a stable path in scope.
pub fn delivery_encode_batch_audit(branch: &'static str, detail: impl AsRef<str>) {
    delivery_substrate_batch_audit(branch, detail);
}

/// MS-SSIM / quality-verify channel fallback (parallel compute, sampling).
pub fn delivery_msssim_fallback_audit(branch: &'static str, detail: impl AsRef<str>) {
    delivery_substrate_batch_audit(branch, detail);
}

/// Numeric guard fallback (float compare, CRF cache, parse defaults).
pub fn delivery_numeric_fallback_audit(branch: &'static str, detail: impl AsRef<str>) {
    delivery_substrate_batch_audit(branch, detail);
}

/// Runtime / detection / cache-dir fallback (path-scoped).
pub fn delivery_runtime_path_audit(branch: &'static str, path: &Path, detail: impl AsRef<str>) {
    delivery_substrate_path_audit(branch, path, detail);
}

/// Runtime / error-handler fallback without a stable path in scope.
pub fn delivery_runtime_batch_audit(branch: &'static str, detail: impl AsRef<str>) {
    delivery_substrate_batch_audit(branch, detail);
}

/// Exiftool JSON string field when absent (empty + runtime audit).
#[must_use]
pub fn delivery_exiftool_field_or_empty(
    value: Option<String>,
    audit_message: impl AsRef<str>,
) -> String {
    value.unwrap_or_else(|| {
        delivery_runtime_batch_audit("delivery_runtime", audit_message);
        String::new()
    })
}

/// Training corpus path when env map misses (original path + DB audit).
#[must_use]
pub fn delivery_training_source_path_or_input(path: &Path) -> PathBuf {
    crate::common_utils::resolve_training_source_path(path).unwrap_or_else(|| {
        delivery_db_batch_audit(
            "training_source_map",
            format!(
                "training source map miss for {}; using original path",
                path.display()
            ),
        );
        path.to_path_buf()
    })
}

/// External tool path for builders, or bare name for PATH lookup
/// (strict-gated).
#[must_use]
pub fn delivery_tool_path_or_bare_name(name: &str) -> PathBuf {
    crate::common_utils::resolve_tool_path(name).unwrap_or_else(|| {
        delivery_strict_batch_audit(
            "tool_path_unresolved",
            format!(
                "external tool '{name}' not resolved (which/user-local/homebrew/cargo fallbacks); \
                 using bare name for PATH lookup"
            ),
        );
        PathBuf::from(name)
    })
}

/// Video quality CRF from encoder tags, or precomputed BPP estimate (audited).
#[must_use]
pub fn probe_video_crf_from_params_or_estimate(extracted: Option<u8>, estimate: u8) -> u8 {
    extracted.unwrap_or_else(|| {
        probe_quality_batch_audit(
            "video_quality_crf_extract_failed",
            crate::infra::static_logs::messages::MSG_VQD_CRF_FAIL,
        );
        estimate
    })
}

/// `PATH` for subprocess discovery when unset (empty + runtime audit).
#[must_use]
pub fn delivery_path_env_or_empty() -> std::ffi::OsString {
    std::env::var_os("PATH").unwrap_or_else(|| {
        delivery_runtime_batch_audit(
            "process_lock_path",
            "PROCESS LOCK AUDIT: PATH environment variable missing; using empty PATH for tool \
             discovery",
        );
        std::ffi::OsString::new()
    })
}

/// System RAM probe for x265/thread policy when detection fails (None + audit).
#[must_use]
pub fn delivery_system_memory_mb_or_zero() -> Option<(u64, u64)> {
    let res = crate::system_memory::get_memory_mb();
    if res.is_none() {
        delivery_runtime_batch_audit(
            "delivery_system",
            "SYSTEM AUDIT: RAM detection failed during x265 profiling | Forensic: Memory probe \
             returned None; falling back to conservative LowMemory profile to prevent OOM \
             termination",
        );
    }
    res
}

/// `rsync` executable path when `which` fails (bare name + audit).
#[must_use]
pub fn delivery_rsync_executable_or_default() -> String {
    match which::which("rsync") {
        Ok(path) => match path.to_str() {
            Some(path) => path.to_owned(),
            None => {
                delivery_runtime_batch_audit(
                    "parallel_rsync",
                    format!(
                        "RSYNC AUDIT: resolved rsync path is not UTF-8 ({}); using bare 'rsync'",
                        path.display()
                    ),
                );
                String::from("rsync")
            }
        },
        Err(e) => {
            delivery_runtime_batch_audit(
                "parallel_rsync",
                format!("RSYNC AUDIT: rsync not found on PATH ({e}); using bare 'rsync'"),
            );
            String::from("rsync")
        }
    }
}

/// Size-change permille for terminal UI when u32 cast overflows (cap + audit).
#[must_use]
pub fn delivery_runtime_permille_u32_or_max(old: u64, new: u64) -> u32 {
    let permille_u128 = (u128::from(new) * 10_000) / u128::from(old);
    u32::try_from(permille_u128).unwrap_or_else(|_| {
        delivery_runtime_batch_audit(
            "delivery_numeric",
            format!(
                "RATIO AUDIT: Size change permille overflows u32 | Forensic: old_bytes={old}, \
                 new_bytes={new}, permille_raw={permille_u128}; capping display at u32::MAX"
            ),
        );
        u32::MAX
    })
}

/// Spinner / counter frame index when atomic value exceeds `usize` (0 + audit).
#[must_use]
pub fn delivery_spinner_frame_index_or_zero(raw: u64, context: &str) -> usize {
    usize::try_from(raw).unwrap_or_else(|_| {
        delivery_runtime_batch_audit(
            "delivery_numeric",
            format!("{context}: frame counter overflowed usize; resetting display index to 0"),
        );
        0
    })
}

/// ffprobe stream `nb_frames` sort priority (`u64::MAX` when absent — not a
/// measured frame count).
#[must_use]
pub fn probe_ffprobe_stream_nb_frames_sort_or_zero(nb: Option<u64>) -> u64 {
    nb.unwrap_or_else(|| {
        probe_layer_batch_audit(
            "ffprobe_nb_frames_sort",
            crate::infra::static_logs::messages::MSG_PROBE_NB_FRAMES_MISSING,
        );
        u64::MAX
    })
}

/// Checkpoint lock metadata start time when process start is unavailable.
#[must_use]
pub fn delivery_checkpoint_lock_start_time_or_now(start_time: Option<u64>, now: u64) -> u64 {
    start_time.unwrap_or_else(|| {
        delivery_checkpoint_batch_audit(
            "checkpoint_lock",
            "CHECKPOINT AUDIT: process start time unavailable; using current time for lock \
             metadata",
        );
        now
    })
}

/// Progress / terminal UX I/O degradation during delivery runs.
pub fn delivery_progress_batch_audit(branch: &'static str, detail: impl AsRef<str>) {
    delivery_substrate_batch_audit(branch, detail);
}

/// Path validation failure before conversion I/O.
pub fn delivery_path_validate_batch_audit(branch: &'static str, detail: impl AsRef<str>) {
    delivery_substrate_batch_audit(branch, detail);
}

/// Session log rotation / retention degradation during delivery runs.
pub fn delivery_logging_path_audit(branch: &'static str, path: &Path, detail: impl AsRef<str>) {
    delivery_runtime_path_audit(branch, path, detail);
}

/// Recovery-mode channel type when bitstream fallback omits layout.
#[must_use]
pub fn recovery_channel_type_label(channel: Option<String>, path: &Path) -> String {
    channel.unwrap_or_else(|| {
        delivery_strict_batch_audit(
            "recovery_channel_unknown",
            format!(
                "{}: channel type missing in recovery probe; using \"unknown\"",
                path.display()
            ),
        );
        String::from("unknown")
    })
}

/// ffprobe recovery mode format label when extension is missing.
#[must_use]
pub fn recovery_format_name(path: &Path) -> String {
    path.extension()
        .and_then(|ext| ext.to_str())
        .filter(|ext| !ext.is_empty())
        .map_or_else(
            || {
                delivery_strict_batch_audit(
                    "recovery_format_unknown",
                    format!(
                        "{}: extension missing in recovery probe; using \"unknown\"",
                        path.display()
                    ),
                );
                String::from("unknown")
            },
            str::to_ascii_lowercase,
        )
}

/// Measured bit depth for geometry heuristics (`None` when container omits bit
/// depth).
#[must_use]
pub fn color_depth_optional(depth: Option<u8>, context: &str) -> Option<u8> {
    if let Some(d) = depth {
        Some(d)
    } else {
        delivery_strict_batch_audit(
            "color_depth_baseline",
            format!("{context}: bit depth missing; refusing forged 8-bit baseline"),
        );
        None
    }
}

/// Legacy symbol (panics when depth absent; prefer [`color_depth_optional`]).
#[must_use]
pub fn color_depth_or_baseline(depth: Option<u8>, context: &str) -> u8 {
    color_depth_optional(depth, context).unwrap_or_else(|| {
        gate_legacy_optional_misuse(
            "color_depth_baseline",
            format!("{context}: bit depth required; use color_depth_optional"),
        );
    })
}

/// ffprobe `r_frame_rate` when present on stream (`None` if absent).
#[must_use]
pub fn probe_r_frame_rate_optional(path: &Path) -> Option<&'static str> {
    let _ = path;
    probe_layer_audit(
        "jxl_r_frame_rate_baseline",
        path,
        "r_frame_rate missing from ffprobe output; refusing forged 0/1 rate",
    );
    None
}

/// Legacy symbol (panics when rate absent; prefer
/// [`probe_r_frame_rate_optional`]).
#[must_use]
pub fn probe_r_frame_rate_baseline(path: &Path) -> &'static str {
    probe_r_frame_rate_optional(path).unwrap_or_else(|| {
        gate_legacy_optional_misuse(
            "jxl_r_frame_rate_baseline",
            format!(
                "{}: r_frame_rate required; use probe_r_frame_rate_optional",
                path.display()
            ),
        );
    })
}

/// Parsed FPS when rational rate string is invalid (`None` = refuse to forge
/// 0.0 FPS).
#[must_use]
pub fn probe_fps_parse_optional(
    rate: &str,
    path: &Path,
    err: impl std::fmt::Display,
) -> Option<f64> {
    probe_layer_audit(
        "jxl_frame_rate_parse_failed",
        path,
        format!("failed to parse frame rate '{rate}': {err}; refusing fabricated 0.0 FPS"),
    );
    None
}

/// Parsed FPS when rational rate string is invalid (legacy; prefer
/// `probe_fps_parse_optional`).
// Keep the absent marker explicit: conversion metrics must not use `unwrap_or(default)`.
#[allow(clippy::manual_unwrap_or)]
#[must_use]
pub fn probe_fps_parse_or_zero(rate: &str, path: &Path, err: impl std::fmt::Display) -> f64 {
    match probe_fps_parse_optional(rate, path, err) {
        Some(v) => v,
        None => f64::NAN,
    }
}

/// GIF frame count for explore stream analysis (`None` when parse fails or zero
/// frames).
#[must_use]
pub fn explore_gif_frame_count_optional<E: std::fmt::Display>(
    frames: Result<usize, E>,
    path: &Path,
) -> Option<usize> {
    match frames {
        Ok(v) if v > 0 => Some(v),
        Ok(_) => {
            explore_precheck_degraded_audit(
                "explore_audit",
                format!(
                    "GIF frame count zero for {}; refusing fabricated frame count",
                    path.display()
                ),
            );
            None
        }
        Err(e) => {
            explore_precheck_degraded_audit(
                "explore_audit",
                format!("GIF frame count failed for {}: {}", path.display(), e),
            );
            None
        }
    }
}

/// Legacy GIF frame count helper (panics when absent; prefer
/// `explore_gif_frame_count_optional`).
#[must_use]
pub fn explore_gif_frame_count_or_zero<E: std::fmt::Display>(
    frames: Result<usize, E>,
    path: &Path,
) -> usize {
    explore_gif_frame_count_optional(frames, path).unwrap_or_else(|| {
        panic!(
            "explore_gif_frame_count required for {} after gate audit",
            path.display()
        );
    })
}

/// WebP frame count for explore stream analysis (`None` when parse fails or
/// zero frames).
#[must_use]
pub fn explore_webp_frame_count_optional<E: std::fmt::Display>(
    frames: Result<u32, E>,
    path: &Path,
) -> Option<u32> {
    match frames {
        Ok(v) if v > 0 => Some(v),
        Ok(_) => {
            explore_precheck_degraded_audit(
                "explore_audit",
                format!(
                    "WebP frame count zero for {}; refusing fabricated frame count",
                    path.display()
                ),
            );
            None
        }
        Err(e) => {
            explore_precheck_degraded_audit(
                "explore_audit",
                format!("WebP frame count failed for {}: {}", path.display(), e),
            );
            None
        }
    }
}

/// Legacy WebP frame count helper (panics when absent; prefer
/// `explore_webp_frame_count_optional`).
#[must_use]
pub fn explore_webp_frame_count_or_zero<E: std::fmt::Display>(
    frames: Result<u32, E>,
    path: &Path,
) -> u32 {
    explore_webp_frame_count_optional(frames, path).unwrap_or_else(|| {
        panic!(
            "explore_webp_frame_count required for {} after gate audit",
            path.display()
        );
    })
}

/// Whether a path may be animated when byte sniff fails (falls back to cached
/// format label).
#[must_use]
pub fn probe_path_can_be_animated_or_label(path: &Path, format_label: &str) -> bool {
    match crate::image_detection::detect_format_from_bytes(path) {
        Ok(format) => matches!(
            format,
            crate::image_detection::DetectedFormat::GIF
                | crate::image_detection::DetectedFormat::WebP
                | crate::image_detection::DetectedFormat::PNG
                | crate::image_detection::DetectedFormat::AVIF
                | crate::image_detection::DetectedFormat::JXL
                | crate::image_detection::DetectedFormat::HEIC
                | crate::image_detection::DetectedFormat::HEIF
        ),
        Err(err) => {
            probe_image_format_audit(
                "analysis_cache_animation_capable",
                path,
                format!(
                    "Animation-capable check: format sniff failed; using cached label \
                     {format_label}: {err}"
                ),
            );
            analysis_format_can_be_animated(format_label)
        }
    }
}

/// GPU coarse-search summary when `best_crf` was never recorded.
#[must_use]
pub fn explore_gpu_search_summary_from_best_crf(
    best_crf: Option<f32>,
    max_crf: f32,
    iterations: u32,
) -> (f32, bool, bool) {
    if let Some(crf) = best_crf {
        (crf, true, iterations > 8)
    } else {
        explore_gpu_coarse_fallback_audit(
            "search_summary",
            format!("GPU search summary: best_crf missing; using max_crf {max_crf:.2}"),
        );
        (max_crf, false, false)
    }
}

/// Perceptual gate failure text when explore result omits `failure_reason`.
#[must_use]
pub fn explore_perceptual_gate_failure_reason_or_default(
    reason: Option<&str>,
    default: &'static str,
    branch: &'static str,
) -> String {
    if let Some(r) = reason {
        r.to_string()
    } else {
        explore_delivery_explore_outcome_audit(
            branch,
            format!("perceptual gate failure_reason empty; using \"{default}\""),
        );
        default.to_string()
    }
}

/// Calibration log SSIM label (`SSIM: {score}` or audited unknown).
#[must_use]
pub fn ui_ssim_colon_label_or_unknown(value: Option<f64>, context: &str) -> String {
    if let Some(ssim) = value.filter(|v| v.is_finite()) {
        format!("SSIM: {ssim:.4}")
    } else {
        explore_delivery_explore_outcome_audit(
            "calibration_ssim",
            format!("{context}: SSIM missing or non-finite; using unknown label"),
        );
        String::from("SSIM: unknown")
    }
}

/// Skip MS-SSIM verification when duration is unknown (audited).
#[must_use]
pub fn explore_ms_ssim_skip_when_duration_unknown(
    duration: Option<f64>,
    threshold_secs: f64,
    force_ms_ssim_long: bool,
) -> bool {
    match duration {
        None => {
            explore_delivery_explore_outcome_audit(
                "ms_ssim_duration",
                "Cannot detect video duration; skipping MS-SSIM verification",
            );
            true
        }
        Some(d) => d >= threshold_secs && !force_ms_ssim_long,
    }
}

/// CLI unsupported-input extension label (`(none)` when missing).
#[must_use]
pub fn delivery_cli_extension_display_or_none(path: &Path) -> String {
    if let Some(ext) = path.extension().and_then(|ext| ext.to_str()) {
        ext.to_string()
    } else {
        delivery_pipeline_batch_audit(
            "delivery_pipeline_cli",
            format!(
                "{}: no extension on unsupported input; reporting (none)",
                path.display()
            ),
        );
        String::from("(none)")
    }
}

/// CLI output byte count label when path exists but size metadata is missing.
#[must_use]
pub fn delivery_cli_output_size_label_or_unknown(
    output_size: Option<u64>,
    input_path: &str,
    output_path: &str,
) -> String {
    if let Some(size) = output_size {
        size.to_string()
    } else {
        delivery_pipeline_batch_audit(
            "delivery_pipeline_cli",
            format!(
                "Output path is present but output size is missing (input={input_path}, \
                 output={output_path})"
            ),
        );
        String::from("unknown")
    }
}

/// Probe-layer file name for penetration/UI logs (`?` when path has no
/// `file_name`).
#[must_use]
pub fn probe_path_file_name_for_log(path: &Path) -> String {
    if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
        name.to_string()
    } else {
        probe_layer_audit(
            "checkpoint_progress",
            path,
            format!(
                "FIELD AUDIT: Failed to get file name for path '{}' | Forensic: Path is malformed \
                 or root; information invalidated",
                path.display()
            ),
        );
        String::from("?")
    }
}

/// Normal-mode fusion SSIM floor from explore reference and configured minimum.
#[must_use]
pub fn explore_fusion_ssim_floor(explore_ssim: Option<f64>, min_ssim_config: f64) -> f64 {
    let sanity = crate::constants::EXPLORATION_FUSION_SANITY_FLOOR;
    let config_floor = min_ssim_config.max(sanity);
    match explore_ssim.filter(|v| v.is_finite()) {
        Some(ref_ssim) => {
            (ref_ssim - crate::constants::EXPLORATION_FUSION_ALLOWED_DROP).max(config_floor)
        }
        None => config_floor,
    }
}

/// `QualityCheck: FAILED (...)` line when explore gate omits `failure_reason`.
#[must_use]
pub fn explore_quality_check_failed_line(
    failure_reason: Option<&str>,
    default_reason: &'static str,
    branch: &'static str,
) -> String {
    let reason =
        explore_perceptual_gate_failure_reason_or_default(failure_reason, default_reason, branch);
    format!("   QualityCheck: FAILED ({reason})")
}

/// Explore-phase SSIM reference for grade logs (`none` when unset).
#[must_use]
pub fn ui_explore_ssim_ref_or_none(value: Option<f64>) -> String {
    match value.filter(|v| v.is_finite()) {
        Some(v) => format!("{v:.6}"),
        None => String::from("none"),
    }
}

/// GPU coarse-search anchor CRF from warm-start hint (audited baseline when
/// hint missing).
#[must_use]
pub fn explore_search_anchor_crf_or_baseline(
    warm_start_crf: Option<f32>,
    baseline_crf: f32,
) -> f32 {
    if let Some(hint) = warm_start_crf {
        (hint - 2.0).max(crate::constants::ABSOLUTE_MIN_CRF)
    } else {
        explore_gpu_coarse_fallback_audit(
            "search_anchor_crf",
            format!("missing warm_start_crf; using baseline_crf {baseline_crf:.2}"),
        );
        baseline_crf
    }
}

/// Image classifier content-type name when no rule matched (audited `UNKNOWN`).
#[must_use]
pub fn probe_classifier_content_name_or_unknown(context: &str) -> String {
    probe_quality_batch_audit(
        "classifier_content_type",
        format!("{context}: no classifier rule matched; using UNKNOWN"),
    );
    String::from("UNKNOWN")
}

/// Palette-based color diversity ratio in `[0, 1]` from indexed palette size.
#[must_use]
pub fn probe_palette_color_diversity_ratio(palette_size: usize) -> f64 {
    (crate::numeric_cast::usize_to_f64(palette_size) / crate::constants::PALETTE_MAX_DENSITY_F64)
        .min(1.0)
}

/// Path mtime for batch ordering (`None` when metadata unavailable).
#[must_use]
pub fn delivery_path_modified_unix_secs_optional(path: &Path) -> Option<u64> {
    let metadata = match std::fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(e) => {
            delivery_pipeline_path_audit(
                "delivery_pipeline_batch",
                path,
                format!("failed to read metadata for modified-time ordering: {e}"),
            );
            return None;
        }
    };
    let modified = match metadata.modified() {
        Ok(modified) => modified,
        Err(e) => {
            delivery_pipeline_path_audit(
                "delivery_pipeline_batch",
                path,
                format!("failed to read modified time for ordering: {e}"),
            );
            return None;
        }
    };
    match modified.duration_since(UNIX_EPOCH) {
        Ok(duration) => Some(duration.as_secs()),
        Err(e) => {
            delivery_pipeline_path_audit(
                "delivery_pipeline_batch",
                path,
                format!("modified time is before UNIX_EPOCH: {e}"),
            );
            None
        }
    }
}

/// Legacy path mtime helper (panics when absent; prefer
/// [`delivery_path_modified_unix_secs_optional`]).
#[must_use]
pub fn delivery_path_modified_unix_secs_or_zero(path: &Path) -> u64 {
    delivery_path_modified_unix_secs_optional(path).unwrap_or_else(|| {
        delivery_pipeline_path_audit(
            "delivery_pipeline_batch",
            path,
            crate::infra::static_logs::messages::MSG_BATCH_MOD_TIME_FAIL
                .replace("{}", &path.display().to_string()),
        );
        panic!(
            "delivery_path_modified_unix_secs_or_zero is legacy — use optional ({})",
            path.display()
        );
    })
}

/// Relative directory depth from batch root (`None` when `strip_prefix` fails).
#[must_use]
pub fn delivery_batch_relative_depth_optional(root: &Path, path: &Path) -> Option<usize> {
    match path.strip_prefix(root) {
        Ok(relative) => relative.parent().map(|parent| parent.components().count()),
        Err(e) => {
            delivery_pipeline_path_audit(
                "delivery_pipeline_batch",
                path,
                format!(
                    "failed to compute relative depth from root {}: {e}",
                    root.display()
                ),
            );
            None
        }
    }
}

/// Legacy batch depth helper (panics when absent; prefer
/// [`delivery_batch_relative_depth_optional`]).
#[must_use]
pub fn delivery_batch_relative_depth_or_zero(root: &Path, path: &Path) -> usize {
    delivery_batch_relative_depth_optional(root, path).unwrap_or_else(|| {
        delivery_pipeline_batch_audit(
            "delivery_pipeline_batch",
            crate::infra::static_logs::messages::MSG_BATCH_DEPTH_FAIL,
        );
        panic!(
            "delivery_batch_relative_depth_or_zero is legacy — use optional (root={}, path={})",
            root.display(),
            path.display()
        );
    })
}

/// Video probe frame count, estimating from duration×fps when probe omits it.
#[must_use]
pub fn delivery_video_frame_count_or_estimate(
    frame_count: Option<u64>,
    duration_secs: Option<f64>,
    frame_rate: Option<f64>,
) -> Option<u64> {
    match frame_count {
        Some(fc) if fc > 0 => Some(fc),
        _ => match (duration_secs, frame_rate) {
            (Some(dur), Some(fps)) => {
                crate::numeric_cast::f64_to_u64_strict((dur * fps).round().max(1.0_f64), "frames")
            }
            _ => None,
        },
    }
}

/// Batch video sort work metric (`pixels * frames`) with overflow audit.
#[must_use]
pub fn delivery_video_sort_work_or_none(pixels: u64, frames: u64, path: &Path) -> Option<u64> {
    if let Some(work) = pixels.checked_mul(frames.max(1)) {
        Some(work)
    } else {
        delivery_pipeline_path_audit(
            "delivery_pipeline_cli",
            path,
            format!(
                "NUMERIC ANOMALY: Estimated video sort work overflowed for '{}' (pixels={}, \
                 frames={}) | Forensic: u64 overflow during complexity estimation; work metric \
                 unavailable for sorting",
                path.display(),
                pixels,
                frames
            ),
        );
        None
    }
}

/// Diagnostic table cell when optional layer/label text is missing (M201).
///
/// Preserves `Some("")` like the prior `map_or("?", |v| v)` (only `None` →
/// `"?"`).
#[must_use]
pub const fn delivery_db_diag_cell_or_unknown(value: Option<&str>) -> &str {
    match value {
        Some(s) => s,
        None => "?",
    }
}

/// `duration_p90`: empirical collection percentile only (M201/M218; no
/// `feature_stats` substitution).
#[must_use]
pub fn delivery_db_duration_p90_or_feature_stats(
    empirical: Option<f64>,
    feature_stats_p90: Option<f64>,
) -> Option<f64> {
    let _ = collection_duration_p90_feature_stats(feature_stats_p90, "delivery_db_duration_p90");
    empirical.filter(|v| v.is_finite())
}

/// Audited rejection of `feature_stats` collection `duration_p90` (never
/// substitutes empirical P90) (M218).
#[must_use]
pub fn collection_duration_p90_feature_stats(
    feature_stats_p90: Option<f64>,
    context: &str,
) -> Option<f64> {
    if let Some(value) = feature_stats_p90.filter(|v| v.is_finite()) {
        delivery_db_batch_audit(
            "collection_duration_p90_feature_stats",
            format!(
                "{context}: ignoring feature_stats duration_p90={value}; empirical sample P90 \
                 required"
            ),
        );
    }
    None
}

/// Audit when a loop reference `feature_stats` key is absent (M216).
pub fn loop_profile_feature_absent(feature_key: &str) {
    delivery_db_batch_audit(
        "loop_profile_feature_absent",
        format!(
            "loop reference feature `{feature_key}` missing from feature_stats; refusing \
             fabricated slot"
        ),
    );
}

/// Loop sample `aspect_ratio` from stored metadata or width/height (M201).
#[must_use]
pub fn delivery_db_loop_aspect_ratio_or_derived(
    stored: Option<f64>,
    width: u32,
    height: u32,
) -> Option<f64> {
    if let Some(ratio) = stored {
        return Some(ratio);
    }
    if height > 0 {
        Some(f64::from(width) / f64::from(height))
    } else {
        delivery_db_batch_audit(
            "delivery_db_numeric",
            "NUMERIC ANOMALY: cannot derive aspect_ratio from dimensions (height=0)",
        );
        None
    }
}

/// KNN neighbor count for `inference_log` INSERT (`None` when cast overflows)
/// (M201).
#[must_use]
pub fn delivery_db_knn_neighbor_count_i32(count: Option<usize>) -> Option<i32> {
    let Some(s) = count else {
        return None;
    };
    if let Some(v) = crate::numeric_cast::usize_to_i32_strict(s, "db_knn_count") {
        Some(v)
    } else {
        delivery_db_batch_audit(
            "delivery_db_knn",
            "NUMERIC ANOMALY: KNN neighbor count overflow during inference logging | Forensic: \
             usize to i32 conversion failure; refusing to forge telemetry data",
        );
        None
    }
}

/// Skip/failure conversion result output path label (copied dest or input)
/// (M202).
#[must_use]
pub fn conversion_fallback_output_path_display(
    copied_dest: Option<&std::path::Path>,
    input: &std::path::Path,
) -> String {
    match copied_dest {
        Some(p) => p.display().to_string(),
        None => input.display().to_string(),
    }
}

/// `ImageMagick` `identify`: magick binary first, then system `identify`
/// (M202).
pub fn probe_identify_output_magick_then_system(
    input: &std::path::Path,
    format: &str,
) -> std::io::Result<std::process::Output> {
    crate::image_builders::IdentifyBuilder::new()
        .use_magick(true)
        .format(format)
        .input(input)
        .build()
        .output()
        .or_else(|_| {
            crate::image_builders::IdentifyBuilder::new()
                .use_magick(false)
                .format(format)
                .input(input)
                .build()
                .output()
        })
}

/// CLI batch `base_dir` when output is set but `base_dir` omitted (M202).
#[must_use]
pub fn delivery_cli_base_dir_or_input_when_output(
    base_dir: Option<std::path::PathBuf>,
    output: Option<&std::path::Path>,
    input: &std::path::Path,
) -> Option<std::path::PathBuf> {
    if base_dir.is_some() {
        return base_dir;
    }
    if output.is_some() {
        Some(input.to_path_buf())
    } else {
        None
    }
}

/// ffprobe stream bit-depth string fields (`bits_per_raw_sample` then
/// `bits_per_sample`) (M203).
#[must_use]
pub fn probe_ffprobe_bit_depth_string_fields(video_stream: &serde_json::Value) -> Option<&str> {
    video_stream
        .get("bits_per_raw_sample")
        .and_then(serde_json::Value::as_str)
        .or_else(|| {
            video_stream
                .get("bits_per_sample")
                .and_then(serde_json::Value::as_str)
        })
}

/// Positive frame rate from avg then `r_frame_rate` (M203).
#[must_use]
pub fn probe_ffprobe_fps_avg_or_r_frame_rate(
    avg_frame_rate: Option<f64>,
    r_frame_rate: Option<f64>,
) -> Option<f64> {
    avg_frame_rate
        .filter(|rate| rate.is_finite() && *rate > 0.0_f64)
        .or_else(|| r_frame_rate.filter(|rate| rate.is_finite() && *rate > 0.0_f64))
}

/// ffprobe stream dimension field with `coded_*` fallback (M203).
#[must_use]
pub fn probe_ffprobe_stream_u64_field(
    video_stream: &serde_json::Value,
    field_name: &str,
) -> Option<u64> {
    video_stream[field_name].as_u64().or_else(|| {
        if field_name == "width" {
            video_stream["coded_width"].as_u64()
        } else if field_name == "height" {
            video_stream["coded_height"].as_u64()
        } else {
            None
        }
    })
}

/// Zero-dimension recovery: WebP canvas then bitstream/header chain (M203).
#[must_use]
pub fn probe_ffprobe_zero_dimension_recovery(
    path: &std::path::Path,
    format_name: &str,
) -> Option<(u32, u32)> {
    let webp_dims = if format_name.contains("webp") {
        match crate::image_formats::webp::canvas_dimensions_from_path(path) {
            Ok(value) => value,
            Err(err) => {
                probe_layer_audit(
                    "ffprobe_zero_dimension_webp_canvas_failed",
                    path,
                    format!("WebP canvas dimension recovery failed: {err}"),
                );
                None
            }
        }
    } else {
        None
    };
    let fallback_dims = match crate::conversion::dimensions_without_ffprobe(path) {
        Ok(value) => value,
        Err(err) => {
            probe_layer_audit(
                "ffprobe_zero_dimension_bitstream_recovery_failed",
                path,
                format!("bitstream dimension recovery failed: {err}"),
            );
            None
        }
    };
    webp_dims.or(fallback_dims)
}

/// Encoder settings from ffprobe stream tags (`x265/x264/encoder_settings`)
/// (M203).
#[must_use]
pub fn probe_ffprobe_encoder_settings_from_tags(tags: &serde_json::Value) -> Option<String> {
    tags.get("x265-params")
        .and_then(serde_json::Value::as_str)
        .or_else(|| tags.get("x264-params").and_then(serde_json::Value::as_str))
        .or_else(|| {
            tags.get("encoder_settings")
                .and_then(serde_json::Value::as_str)
        })
        .map(str::to_string)
}

/// Raw HDR coordinate u64 with audited reject on overflow (M203).
#[must_use]
pub fn probe_ffprobe_hdr_coord_raw_u64(v: f64) -> Option<u64> {
    crate::numeric_cast::f64_to_u64_strict(v, "hdr_coord_raw").or_else(|| {
        probe_layer_batch_audit(
            "ffprobe_hdr_coord_invalid",
            format!("numerical anomaly in raw HDR coord: {v}"),
        );
        None
    })
}

/// Raw HDR luminance u64 with audited reject on overflow (M204).
#[must_use]
pub fn probe_ffprobe_hdr_luma_raw_u64(v: f64) -> Option<u64> {
    crate::numeric_cast::f64_to_u64_strict(v, "hdr_luma_raw").or_else(|| {
        probe_layer_batch_audit(
            "ffprobe_hdr_luma_invalid",
            format!("numerical anomaly in raw HDR luma: {v}"),
        );
        None
    })
}

/// GIF/format tag loop count key (`loop_count` then `loop`) (M204).
#[must_use]
pub fn probe_ffprobe_format_loop_count_tag(
    tags: &serde_json::Map<String, serde_json::Value>,
) -> Option<&serde_json::Value> {
    tags.get("loop_count").or_else(|| tags.get("loop"))
}

/// Parse HDR chromaticity rational string to 50k-scaled u64 (M204).
#[must_use]
pub fn probe_ffprobe_parse_hdr_rational_to_50k(s: &str) -> Option<u64> {
    if let Some((num, den)) = s.split_once('/') {
        let n: f64 = crate::numeric_cast::parse_strict(num.trim(), "hdr_num")?;
        let d: f64 = crate::numeric_cast::parse_strict(den.trim(), "hdr_den")?;
        if crate::numeric_cast::is_effectively_zero(
            d,
            crate::numeric_cast::FloatContext::FfmpegMeasurement,
        ) {
            return None;
        }
        crate::numeric_cast::f64_to_u64_strict(
            (n / d) * crate::constants::HDR_COORD_SCALING_FACTOR,
            "hdr_coord",
        )
    } else {
        let v: f64 = crate::numeric_cast::parse_strict(s.trim(), "hdr_val")?;
        if v <= 1.0 {
            crate::numeric_cast::f64_to_u64_strict(
                v * crate::constants::HDR_COORD_SCALING_FACTOR,
                "hdr_coord",
            )
        } else {
            probe_ffprobe_hdr_coord_raw_u64(v)
        }
    }
}

/// Parse HDR luminance rational string to 10k-scaled u64 (M204).
#[must_use]
pub fn probe_ffprobe_parse_luminance_to_10k(s: &str) -> Option<u64> {
    if let Some((num, den)) = s.split_once('/') {
        let n: f64 = crate::numeric_cast::parse_strict(num.trim(), "hdr_num")?;
        let d: f64 = crate::numeric_cast::parse_strict(den.trim(), "hdr_den")?;
        if crate::numeric_cast::is_effectively_zero(
            d,
            crate::numeric_cast::FloatContext::FfmpegMeasurement,
        ) {
            return None;
        }
        crate::numeric_cast::f64_to_u64_strict(
            (n / d) * crate::constants::HDR_LUMA_SCALING_FACTOR,
            "hdr_luma",
        )
    } else {
        let v: f64 = crate::numeric_cast::parse_strict(s.trim(), "hdr_val")?;
        if v <= crate::constants::HDR_LUMA_SCALING_FACTOR {
            crate::numeric_cast::f64_to_u64_strict(
                v * crate::constants::HDR_LUMA_SCALING_FACTOR,
                "hdr_luma",
            )
        } else {
            probe_ffprobe_hdr_luma_raw_u64(v)
        }
    }
}

/// Mastering-display chromaticity: rational string then scaled f64 fallback
/// (M204).
#[must_use]
pub fn probe_ffprobe_hdr_side_data_chromaticity_u64(
    sd: &serde_json::Value,
    field: &str,
) -> Option<u64> {
    sd[field]
        .as_str()
        .and_then(probe_ffprobe_parse_hdr_rational_to_50k)
        .or_else(|| {
            sd[field].as_f64().and_then(|v| {
                crate::numeric_cast::f64_to_u64_strict(
                    v * crate::constants::HDR_COORD_SCALING_FACTOR,
                    "hdr_coord_f64",
                )
            })
        })
}

/// Mastering-display luminance: rational string then scaled f64 fallback
/// (M204).
#[must_use]
pub fn probe_ffprobe_hdr_side_data_luminance_u64(
    sd: &serde_json::Value,
    field: &str,
) -> Option<u64> {
    sd[field]
        .as_str()
        .and_then(probe_ffprobe_parse_luminance_to_10k)
        .or_else(|| {
            sd[field].as_f64().and_then(|v| {
                crate::numeric_cast::f64_to_u64_strict(
                    v * crate::constants::HDR_LUMA_SCALING_FACTOR,
                    "hdr_luma_f64",
                )
            })
        })
}

/// Content light level max (`max_content` then `MaxCLL`) (M204).
#[must_use]
pub fn probe_ffprobe_cll_max_content_u64(sd: &serde_json::Value) -> Option<u64> {
    sd["max_content"].as_u64().or_else(|| sd["MaxCLL"].as_u64())
}

/// Content light level average (`max_average` then `MaxFALL`) (M204).
#[must_use]
pub fn probe_ffprobe_cll_max_average_u64(sd: &serde_json::Value) -> Option<u64> {
    sd["max_average"]
        .as_u64()
        .or_else(|| sd["MaxFALL"].as_u64())
}

/// Stream bit depth from `bits_per_raw_sample` then `bits_per_sample` strings
/// (M204).
#[must_use]
pub fn probe_ffprobe_stream_bit_depth_u8_from_fields(
    bits_per_raw_sample: Option<&str>,
    bits_per_sample: Option<&str>,
) -> Option<u8> {
    crate::numeric_cast::parse_option_strict(bits_per_raw_sample, "bits_per_raw_sample")
        .or_else(|| crate::numeric_cast::parse_option_strict(bits_per_sample, "bits_per_sample"))
}

/// Positive finite `f64` probe value (M205).
#[must_use]
pub fn probe_positive_f64(value: f64) -> Option<f64> {
    if value.is_finite() && value > 0.0 {
        Some(value)
    } else {
        None
    }
}

/// ffprobe avg/`r_frame_rate` with optional native-detected fps fallback
/// (M205).
#[must_use]
pub fn probe_ffprobe_fps_with_detected(
    avg_frame_rate: Option<f64>,
    r_frame_rate: Option<f64>,
    detected_fps: Option<f32>,
) -> Option<f64> {
    probe_ffprobe_fps_avg_or_r_frame_rate(avg_frame_rate, r_frame_rate).or_else(|| {
        detected_fps
            .map(f64::from)
            .filter(|fps| fps.is_finite() && *fps > 0.0)
    })
}

/// Video quality ingest duration chain (M205).
#[must_use]
pub fn probe_video_quality_duration_secs(
    probe_duration: Option<f64>,
    frame_count: Option<u64>,
    fps: Option<f64>,
) -> Option<f64> {
    probe_duration.and_then(probe_positive_f64).or_else(|| {
        frame_count.filter(|count| *count > 0).and_then(|count| {
            fps.and_then(probe_positive_f64)
                .map(|fps| crate::numeric_cast::u64_to_f64(count) / fps)
        })
    })
}

/// Video quality ingest fps chain (M205).
#[must_use]
pub fn probe_video_quality_fps(
    avg_frame_rate: Option<f64>,
    r_frame_rate: Option<f64>,
    frame_count: Option<u64>,
    duration_secs: f64,
) -> Option<f64> {
    probe_ffprobe_fps_avg_or_r_frame_rate(avg_frame_rate, r_frame_rate).or_else(|| {
        frame_count.filter(|count| *count > 0).and_then(|count| {
            if !duration_secs.is_finite() || duration_secs <= 0.0 {
                None
            } else {
                Some(crate::numeric_cast::u64_to_f64(count) / duration_secs)
            }
        })
    })
}

/// Video quality ingest frame-count chain (M205).
#[must_use]
pub fn probe_video_quality_frame_count(
    probe_frame_count: Option<u64>,
    duration_secs: f64,
    fps: Option<f64>,
) -> Option<u64> {
    probe_frame_count.filter(|count| *count > 0).or_else(|| {
        let fps = fps.and_then(probe_positive_f64)?;
        let estimated = duration_secs * fps;
        if estimated.is_finite() && estimated >= 1.0 {
            crate::numeric_cast::f64_to_u64_strict(estimated.round(), "video_quality_frame_count")
        } else {
            None
        }
    })
}

/// Video quality ingest bitrate Mbps chain (M205).
#[must_use]
pub fn probe_video_quality_bitrate_mbps(
    video_bit_rate: Option<u64>,
    format_bit_rate: Option<u64>,
    file_size_bytes: u64,
    duration_secs: f64,
) -> Option<f64> {
    video_bit_rate
        .filter(|bit_rate| *bit_rate > 0)
        .map(|bit_rate| crate::numeric_cast::u64_to_f64(bit_rate) / 1_000_000.0)
        .or_else(|| {
            format_bit_rate
                .filter(|bit_rate| *bit_rate > 0)
                .map(|bit_rate| crate::numeric_cast::u64_to_f64(bit_rate) / 1_000_000.0)
        })
        .or_else(|| {
            if file_size_bytes == 0 || !duration_secs.is_finite() || duration_secs <= 0.0 {
                None
            } else {
                Some(
                    (crate::numeric_cast::u64_to_f64(file_size_bytes) * 8.0)
                        / duration_secs
                        / 1_000_000.0,
                )
            }
        })
}

/// Animated-image frame count: ffprobe → detector → GIF timing (M205).
#[must_use]
pub fn probe_animated_frame_count_u32(
    probe_frame_count: Option<u64>,
    detected_frame_count: Option<u32>,
    gif_frame_count: Option<u32>,
) -> Option<u32> {
    match probe_frame_count.map(|count| (count, u32::try_from(count))) {
        Some((_raw_count, Ok(count))) if count > 1 => return Some(count),
        Some((_, Ok(_))) | None => {}
        Some((raw_count, Err(e))) => probe_layer_batch_audit(
            "animated_frame_count",
            format!("probe frame count {raw_count} does not fit u32: {e}; trying detector count"),
        ),
    }
    detected_frame_count
        .filter(|count| *count > 1)
        .or_else(|| gif_frame_count.filter(|count| *count > 1))
}

/// Animated-image duration chain (M205).
#[must_use]
pub fn probe_animated_duration_secs(
    gif_duration_secs: Option<f64>,
    analysis_duration_secs: Option<f32>,
    probe_duration: Option<f64>,
    frame_count: u32,
    fps: Option<f64>,
) -> Option<f64> {
    gif_duration_secs
        .and_then(probe_positive_f64)
        .or_else(|| {
            analysis_duration_secs.and_then(|duration| probe_positive_f64(f64::from(duration)))
        })
        .or_else(|| probe_duration.and_then(probe_positive_f64))
        .or_else(|| probe_duration_secs_from_frame_count_fps(frame_count, fps))
}

/// Animated-image fps chain (M205).
#[must_use]
pub fn probe_animated_fps(
    gif_fps: Option<f64>,
    avg_frame_rate: Option<f64>,
    r_frame_rate: Option<f64>,
    detected_fps: Option<f32>,
    frame_count: u32,
    duration_secs: f64,
) -> Option<f64> {
    gif_fps
        .and_then(probe_positive_f64)
        .or_else(|| probe_ffprobe_fps_with_detected(avg_frame_rate, r_frame_rate, detected_fps))
        .or_else(|| probe_fps_from_frame_count_duration(frame_count, duration_secs))
}

/// Animated-image average frame delay ms chain (M205).
#[must_use]
pub fn probe_animated_average_frame_delay_ms(
    gif_delay_ms: Option<f64>,
    pts_deltas: &[f64],
    frame_count: u32,
    duration_secs: f64,
) -> Option<f64> {
    gif_delay_ms
        .and_then(probe_positive_f64)
        .or_else(|| probe_animated_average_pts_delay_ms(pts_deltas))
        .or_else(|| probe_average_frame_delay_ms_from_duration(frame_count, duration_secs))
}

/// GIF timing variation or PTS-derived normalized variation (M205).
#[must_use]
pub fn probe_animated_timing_variation_or_pts(
    gif_variation: Option<f64>,
    pts_deltas: &[f64],
) -> Option<f64> {
    gif_variation
        .and_then(probe_positive_f64)
        .or_else(|| probe_animated_normalized_pts_delay_variation(pts_deltas))
}

/// Duration from frame count and fps (M205).
#[must_use]
pub fn probe_duration_secs_from_frame_count_fps(frame_count: u32, fps: Option<f64>) -> Option<f64> {
    let fps = fps.and_then(probe_positive_f64)?;
    if frame_count > 0 {
        Some(f64::from(frame_count) / fps)
    } else {
        None
    }
}

/// Fps from frame count and duration (M205).
#[must_use]
pub fn probe_fps_from_frame_count_duration(frame_count: u32, duration_secs: f64) -> Option<f64> {
    if frame_count > 0 && duration_secs > 0.0 && duration_secs.is_finite() {
        Some(f64::from(frame_count) / duration_secs)
    } else {
        None
    }
}

/// Average frame delay from duration and frame count (M205).
#[must_use]
pub fn probe_average_frame_delay_ms_from_duration(
    frame_count: u32,
    duration_secs: f64,
) -> Option<f64> {
    if frame_count > 0 && duration_secs > 0.0 && duration_secs.is_finite() {
        Some((duration_secs * 1000.0) / f64::from(frame_count))
    } else {
        None
    }
}

/// Average PTS delta delay in milliseconds (M205).
#[must_use]
pub fn probe_animated_average_pts_delay_ms(pts_deltas: &[f64]) -> Option<f64> {
    let deltas = probe_positive_finite_pts_deltas(pts_deltas);
    if deltas.is_empty() {
        return None;
    }
    Some(deltas.iter().sum::<f64>() * 1000.0 / crate::numeric_cast::usize_to_f64(deltas.len()))
}

/// Normalized PTS delay variation in `[0, 1]` (M205).
#[must_use]
pub fn probe_animated_normalized_pts_delay_variation(pts_deltas: &[f64]) -> Option<f64> {
    let deltas = probe_positive_finite_pts_deltas(pts_deltas);
    if deltas.len() < 2 {
        return None;
    }
    let mean = deltas.iter().sum::<f64>() / crate::numeric_cast::usize_to_f64(deltas.len());
    if !mean.is_finite() || mean <= f64::EPSILON {
        return None;
    }
    let variance = deltas
        .iter()
        .map(|delta| (delta - mean).powi(2))
        .sum::<f64>()
        / crate::numeric_cast::usize_to_f64(deltas.len());
    Some((variance.sqrt() / mean).clamp(0.0, 1.0))
}

/// First `Some` of two byte buffers (e.g. PNG then APNG header read) (M206).
#[must_use]
pub fn probe_header_bytes_png_or_apng(
    png_prefix: Option<Vec<u8>>,
    apng_prefix: Option<Vec<u8>>,
) -> Option<Vec<u8>> {
    png_prefix.or(apng_prefix)
}

/// WebP dimensions from header bytes, else canvas probe on path (M206).
#[must_use]
pub fn probe_webp_dimensions_from_bytes_or_path(
    from_bytes: Option<(u32, u32)>,
    from_path: Option<(u32, u32)>,
) -> Option<(u32, u32)> {
    from_bytes.or(from_path)
}

/// Bitstream media info with WebP canvas fallback when extension is webp
/// (M206).
#[must_use = "media probe fallback results must be propagated or audited"]
pub fn probe_bitstream_media_info_or_webp_canvas(
    path: &std::path::Path,
    media_info: anyhow::Result<Option<crate::conversion::BitstreamMediaInfo>>,
) -> anyhow::Result<Option<crate::conversion::BitstreamMediaInfo>> {
    if let Some(info) = media_info? {
        return Ok(Some(info));
    }
    if path
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("webp"))
    {
        return Ok(
            crate::image_formats::webp::canvas_dimensions_from_path(path)?.map(
                |(width, height)| crate::conversion::BitstreamMediaInfo {
                    width,
                    height,
                    channel_type: None,
                    bit_depth: None,
                },
            ),
        );
    }
    Ok(None)
}

/// ffprobe `bit_rate` or derive from file size and duration (M206).
#[must_use]
pub fn probe_ffprobe_bit_rate_or_derived_from_size(
    bit_rate: Option<u64>,
    file_size: u64,
    duration: Option<f64>,
) -> Option<u64> {
    if let Some(rate) = bit_rate {
        return Some(rate);
    }
    let dur = duration.filter(|d| *d > 0.0)?;
    let bits = file_size.saturating_mul(8);
    let derived = crate::numeric_cast::f64_to_u64_strict(
        crate::numeric_cast::u64_to_f64(bits) / dur,
        "derived_bitrate",
    )?;
    (derived > 0).then_some(derived)
}

fn probe_positive_finite_pts_deltas(pts_deltas: &[f64]) -> Vec<f64> {
    pts_deltas
        .iter()
        .copied()
        .filter(|delta| delta.is_finite() && *delta > 0.0)
        .collect()
}

/// Loop threshold duration: p50 else capped p75 (M203).
#[must_use]
pub fn loop_duration_p50_or_capped_p75(p50: Option<f64>, p75: Option<f64>) -> Option<f64> {
    p50.or_else(|| p75.map(|value| value.min(crate::constants::LOOP_INTENT_MAX_DURATION)))
}

/// Encoder/software label from precision field then tag map (M203).
#[must_use]
pub fn loop_encoder_software_label<S: std::hash::BuildHasher>(
    original_encoder: Option<String>,
    tags: &std::collections::HashMap<String, String, S>,
) -> Option<String> {
    original_encoder
        .or_else(|| tags.get(crate::constants::TAG_SOFTWARE).cloned())
        .or_else(|| tags.get(crate::constants::TAG_ENCODER).cloned())
}

/// Layer-6 unit probability with tree fallback (M203).
#[must_use]
pub fn loop_inference_unit_probability_or_tree_fallback(
    layer6_score: Option<f64>,
    fallback_probability: Option<f64>,
) -> Option<f64> {
    layer6_score
        .and_then(crate::algorithm_seal::loop_unit_probability)
        .or_else(|| fallback_probability.and_then(crate::algorithm_seal::loop_unit_probability))
}

/// Inference log resolution path: tracking then tree (M203).
#[must_use]
pub fn loop_inference_resolution_path_or_tree(
    tracking: Option<String>,
    tree: Option<String>,
) -> Option<String> {
    tracking.or(tree)
}

/// width×height pixel count with audited overflow (`None` on u64 overflow)
/// (M202).
#[must_use]
pub fn delivery_pipeline_pixel_count_u64_or_none(
    width: u32,
    height: u32,
    path: &std::path::Path,
    branch: &'static str,
    overflow_detail: impl AsRef<str>,
) -> Option<u64> {
    u64::from(width).checked_mul(u64::from(height)).or_else(|| {
        delivery_pipeline_path_audit(branch, path, overflow_detail);
        None
    })
}

/// Conversion skip message size tag when `i128` delta does not fit `i64`.
#[must_use]
pub fn conversion_size_increase_diff_tag(diff_bytes: i128) -> String {
    if let Some(s) = crate::numeric_cast::i128_to_i64_strict(diff_bytes, "size_diff") {
        crate::infra::static_logs::messages::MSG_CONVERSION_SIZE_TAG_POS
            .replace("{size_diff}", &crate::modern_ui::format_size_diff(s))
    } else {
        delivery_strict_batch_audit(
            "conversion_size_diff",
            "size delta i128→i64 overflow; using > i64::MAX label",
        );
        String::from("> i64::MAX")
    }
}

/// Conversion success message with optional quality label prefix.
#[must_use]
pub fn conversion_message_with_quality_label(
    core_msg: &str,
    quality_label: Option<&str>,
) -> String {
    const TOKEN_CORE_MSG: &str = "{core_msg}";
    const TOKEN_Q: &str = "{q}";

    match quality_label.filter(|q| !q.is_empty()) {
        Some(q) => crate::infra::static_logs::messages::MSG_CONVERSION_QUAL_LABEL
            .replace(TOKEN_Q, q)
            .replace(TOKEN_CORE_MSG, core_msg),
        None => crate::infra::static_logs::messages::MSG_CONVERSION_QUAL_NONE
            .replace(TOKEN_CORE_MSG, core_msg),
    }
}

/// Conversion result body before quality-label wrapping.
#[must_use]
pub fn conversion_result_core_msg(
    format_name: &str,
    action: &str,
    size_tag: &str,
    extra_info: Option<&str>,
) -> String {
    const TOKEN_ACTION: &str = "{action}";
    const TOKEN_FORMAT_NAME: &str = "{format_name}";
    const TOKEN_INFO: &str = "{info}";
    const TOKEN_SIZE_TAG: &str = "{size_tag}";

    match extra_info {
        Some(info) => crate::infra::static_logs::messages::MSG_CONVERSION_RESULT_INFO
            .replace(TOKEN_FORMAT_NAME, format_name)
            .replace(TOKEN_ACTION, action)
            .replace(TOKEN_INFO, info)
            .replace(TOKEN_SIZE_TAG, size_tag),
        None => crate::infra::static_logs::messages::MSG_CONVERSION_RESULT_BASE
            .replace(TOKEN_FORMAT_NAME, format_name)
            .replace(TOKEN_ACTION, action)
            .replace(TOKEN_SIZE_TAG, size_tag),
    }
}

/// HEIC deep-analysis canvas when `detect_image` succeeds (`None` = no canvas,
/// not fake 0×0).
#[must_use]
pub fn probe_detection_canvas_optional(
    path: &Path,
    detection: Option<&crate::image_detection::DetectionResult>,
) -> Option<(u32, u32, bool, Option<u8>)> {
    detection
        .map(|d| (d.width, d.height, d.has_alpha, d.bit_depth))
        .or_else(|| {
            probe_layer_audit(
                "heic_fallback_dimensions",
                path,
                "HEIC fallback: detect_image returned no canvas; refusing fabricated 0×0 \
                 dimensions",
            );
            None
        })
}

/// Legacy canvas helper (panics when absent; prefer
/// `probe_detection_canvas_optional`).
#[must_use]
pub fn probe_detection_canvas_or_zero(
    path: &Path,
    detection: Option<&crate::image_detection::DetectionResult>,
) -> (u32, u32, bool, Option<u8>) {
    probe_detection_canvas_optional(path, detection).unwrap_or_else(|| {
        panic!(
            "probe_detection_canvas required for {} after gate audit",
            path.display()
        );
    })
}

/// HDR synthesis log label when HEIC input has no `file_name`.
#[must_use]
pub fn probe_hdr_heic_input_label(path: &Path) -> String {
    if let Some(name) = path.file_name() {
        name.to_string_lossy().into_owned()
    } else {
        hdr_metadata_fallback_audit(
            "hdr_heic_input",
            path,
            "HDR synthesis: missing file_name; using unknown_heic label",
        );
        String::from("unknown_heic")
    }
}

/// HDR temp sidecar extension when path omits one (audited `bin`).
#[must_use]
pub fn probe_hdr_sidecar_extension_or_bin(path: &Path) -> String {
    if let Some(ext) = path.extension().and_then(|ext| ext.to_str()) {
        ext.to_string()
    } else {
        hdr_metadata_fallback_audit(
            "hdr_sidecar_ext",
            path,
            "HDR AUDIT: missing extension for temp sidecar; using bin",
        );
        String::from("bin")
    }
}

/// Fixed-width binary slice read with IO audit when out of range (`None` =
/// refuse to forge).
#[must_use]
pub fn probe_io_fixed_slice_or_none<'a>(
    data: &'a [u8],
    pos: usize,
    width: usize,
    name: &str,
) -> Option<&'a [u8]> {
    if let Some(slice) = data.get(pos..pos + width) {
        Some(slice)
    } else {
        delivery_io_batch_audit(
            "delivery_io",
            format!(
                "Required {width} bytes for '{name}' missing at pos {pos}! Refusing to forge data."
            ),
        );
        None
    }
}

/// Last non-progress `FFmpeg` stderr line, or fixed unknown label.
#[must_use]
pub fn probe_ffmpeg_stderr_tail_line_or_unknown(stderr: &str) -> String {
    match stderr.lines().rev().find(|line| {
        let trimmed = line.trim();
        !trimmed.is_empty()
            && !trimmed.starts_with("frame=")
            && !trimmed.starts_with("fps=")
            && !trimmed.starts_with("size=")
    }) {
        None => "Unknown FFmpeg error".to_string(),
        Some(s) => s.trim().to_string(),
    }
}

/// Training CLI `PostgreSQL` DSN: CLI → `MFB_PG_CONNSTR` → audited built-in
/// default.
#[must_use]
pub fn delivery_training_pg_connstr_or_default(cli_conn: Option<String>) -> String {
    if let Some(value) = cli_conn.filter(|v| !v.trim().is_empty()) {
        return value;
    }
    match std::env::var("MFB_PG_CONNSTR") {
        Ok(value) => {
            let trimmed = value.trim();
            if !trimmed.is_empty() {
                return trimmed.to_string();
            }
        }
        Err(std::env::VarError::NotPresent) => {}
        Err(e) => {
            delivery_db_batch_audit(
                "delivery_training_pg",
                format!("failed to read MFB_PG_CONNSTR: {e}; using built-in PG_DEFAULT_CONNSTR"),
            );
        }
    }
    delivery_db_batch_audit(
        "delivery_training_pg",
        "MFB_PG_CONNSTR unset or empty; using built-in PG_DEFAULT_CONNSTR",
    );
    crate::database::PG_DEFAULT_CONNSTR.to_string()
}

/// Subprocess log tail line for timeout errors (`None` / blank → `"<empty>"`).
#[must_use]
pub fn delivery_subprocess_log_tail_or_empty(line: Option<&str>) -> &str {
    match line {
        Some(s) if !s.trim().is_empty() => s.trim(),
        _ => "<empty>",
    }
}

/// Path basename for logs (`None` → `"<unknown>"`).
#[must_use]
pub fn delivery_path_basename_for_log_or_unknown(path: &Path) -> String {
    match path.file_name().and_then(|s| s.to_str()) {
        Some(name) => name.to_string(),
        None => "<unknown>".to_string(),
    }
}

/// `argv[0]` basename when present, else the full argument string.
#[must_use]
pub fn delivery_argv0_basename_or_full(arg0: &str) -> &str {
    match Path::new(arg0)
        .file_name()
        .and_then(std::ffi::OsStr::to_str)
    {
        Some(base) => base,
        None => arg0,
    }
}

/// statvfs free-byte product clamped to `u64` with audit on overflow.
#[must_use]
pub fn delivery_system_avail_bytes_from_u128(avail_u128: u128) -> u64 {
    match u64::try_from(avail_u128) {
        Ok(v) => v,
        Err(_) => {
            delivery_runtime_batch_audit(
                "delivery_system",
                "statvfs free bytes exceeded u64::MAX; clamping to u64::MAX",
            );
            u64::MAX
        }
    }
}

/// GIF/loop DB synthesis: `u64` frame count → `usize`, or 0 with stderr notice.
#[must_use]
pub fn delivery_db_u64_to_usize_or_zero_with_notice(frames: u64, field: &'static str) -> usize {
    if let Some(n) = crate::numeric_cast::u64_to_usize_strict(frames, field) {
        n
    } else {
        delivery_db_batch_audit(
            "delivery_db_numeric",
            format!("GIF {field}: frame count usize overflow; truncating to 0"),
        );
        crate::ui_stderr::line(
            crate::modern_ui::symbols::ANOMALY,
            crate::modern_ui::symbols::plain::ANOMALY,
            format!("[ANOMALY] GIF frame count overflow! Truncating to 0 ({field})."),
        );
        0
    }
}

/// Quality-model Python interpreter from env, or audited `python3` default.
#[must_use]
pub fn delivery_quality_model_python_command_or_default() -> String {
    match std::env::var(crate::constants::ENV_MFB_QUALITY_MODEL_PYTHON) {
        Ok(value) => {
            let cmd = value.trim().to_string();
            if !cmd.is_empty() {
                return cmd;
            }
            delivery_runtime_batch_audit(
                "delivery_quality_model",
                "MFB_QUALITY_MODEL_PYTHON empty; using default python3 interpreter",
            );
        }
        Err(std::env::VarError::NotPresent) => {
            delivery_runtime_batch_audit(
                "delivery_quality_model",
                "MFB_QUALITY_MODEL_PYTHON unset; using default python3 interpreter",
            );
        }
        Err(e) => {
            delivery_runtime_batch_audit(
                "delivery_quality_model",
                format!("failed to read MFB_QUALITY_MODEL_PYTHON: {e}; using default python3"),
            );
        }
    }
    "python3".to_string()
}

/// Resolved `ImageMagick` CLI path, or default tool name when discovery fails.
#[must_use]
pub fn delivery_imagemagick_cli_path_or_default() -> PathBuf {
    match crate::common_utils::resolve_imagemagick_cli() {
        Some((path, _)) => path,
        None => PathBuf::from(crate::constants::TOOL_MAGICK),
    }
}

/// GPU probe failure reason when diagnostics list is empty.
#[must_use]
pub const fn delivery_gpu_probe_failure_reason_or_default(diagnostics: &[String]) -> &str {
    match diagnostics.first() {
        Some(reason) => reason.as_str(),
        None => "no supported encoder found",
    }
}

/// Owned x265 params base segment (`None` → empty, no audit).
#[must_use]
pub fn x265_params_base_owned_or_empty(base: Option<&str>) -> String {
    match base {
        Some(s) => s.to_string(),
        None => String::new(),
    }
}

/// Static lossy AVIF quality CLI arg + strategy reason when estimation fails.
#[must_use]
pub fn img_static_lossless_quality_arg_or_default(
    estimated_quality: Option<u8>,
) -> (String, String) {
    if let Some(q) = estimated_quality {
        (
            format!(" -q {q}"),
            format!("Static lossy image (non-JPEG), recommend AVIF (quality matched to {q})"),
        )
    } else {
        delivery_api_batch_fallback_audit(
            "img_estimated_quality",
            "Static lossy image quality estimation failed; using encoder defaults",
        );
        (
            String::new(),
            String::from(
                "Static lossy image (non-JPEG), quality estimation failed; recommend AVIF (using \
                 encoder defaults)",
            ),
        )
    }
}

/// jxlinfo dimension tuple when stdout parse fails (`None`, not fake 0×0).
#[must_use]
pub fn probe_jxlinfo_dimensions_optional(
    path: &Path,
    err: impl std::fmt::Display,
) -> Option<(u32, u32, bool, Option<u8>)> {
    probe_layer_audit(
        "jxlinfo_parse_failed",
        path,
        format!("failed to parse jxlinfo output: {err}; refusing fabricated 0×0 dimensions"),
    );
    None
}

/// Legacy jxlinfo dimensions helper (returns `None`; prefer
/// [`probe_jxlinfo_dimensions_optional`]).
#[must_use]
pub fn probe_jxlinfo_dimensions_or_zero(
    path: &Path,
    err: impl std::fmt::Display,
) -> (u32, u32, bool, Option<u8>) {
    probe_jxlinfo_dimensions_optional(path, err).unwrap_or_else(|| {
        panic!(
            "probe_jxlinfo_dimensions_or_zero is legacy — use probe_jxlinfo_dimensions_optional \
             ({})",
            path.display()
        );
    })
}

/// Pixel heuristic unavailable during compression probe; treat as lossy.
#[must_use]
pub fn probe_pixel_lossless_or_false(path: &Path, err: impl std::fmt::Display) -> bool {
    probe_layer_audit(
        "pixel_lossless_heuristic_failed",
        path,
        format!("pixel heuristic unavailable: {err}; treating as lossy"),
    );
    false
}

/// Temp output suffix epoch when system clock is before UNIX epoch.
#[must_use]
pub fn delivery_temp_suffix_epoch_nanos(err: impl std::fmt::Display) -> u128 {
    delivery_strict_batch_audit(
        "temp_suffix_clock_skew",
        format!("system time unavailable for temp suffix ({err}); using counter-only entropy"),
    );
    0x_DEAD_BEEF_CAFE_BABE_u128
}

/// Encoder settings search string for precision metadata (comment tag
/// fallback).
#[must_use]
pub fn probe_encoder_settings_search_string<S: std::hash::BuildHasher>(
    encoder_settings: Option<&str>,
    tags: &HashMap<String, String, S>,
) -> String {
    if let Some(settings) = encoder_settings {
        return settings.to_string();
    }
    tags.get("comment").cloned().unwrap_or_else(|| {
        delivery_strict_batch_audit(
            "encoder_settings_comment_missing",
            "no encoder_settings or comment tag; using empty search string",
        );
        String::new()
    })
}

/// idet TFF/BFF token parse (`None` when token missing or unparseable).
#[must_use]
pub fn probe_idet_count_optional(
    path: &Path,
    field: &'static str,
    token: Option<&str>,
) -> Option<u64> {
    let raw = token?;
    match raw.parse::<u64>() {
        Ok(value) => Some(value),
        Err(e) => {
            probe_layer_audit(
                "idet_field_parse_failed",
                path,
                format!(
                    "failed to parse idet {field} value '{raw}': {e}; refusing fabricated count"
                ),
            );
            None
        }
    }
}

/// Legacy idet count helper (panics; prefer [`probe_idet_count_optional`]).
#[must_use]
pub fn probe_idet_count_or_zero(path: &Path, field: &'static str, token: Option<&str>) -> u64 {
    probe_idet_count_optional(path, field, token).unwrap_or_else(|| {
        panic!(
            "probe_idet_count_or_zero is legacy — use probe_idet_count_optional ({}, \
             field={field})",
            path.display()
        );
    })
}

/// Exploration CRF when bidirectional fine-tune succeeds (`Err` if refinement
/// absent).
pub fn explore_boundary_crf_optional(
    refined: Option<f32>,
    boundary_crf: f32,
    input: &Path,
) -> crate::unified_error::Result<f32> {
    let _ = boundary_crf;
    refined.ok_or_else(|| {
        explore_precheck_batch_audit(
            "explore_boundary_crf",
            format!(
                "{}: bidirectional fine-tune returned no refinement; refusing silent boundary \
                 substitution (boundary_crf={boundary_crf:.2})",
                input.display()
            ),
        );
        crate::unified_error::UnifiedError::ResultAnomaly(format!(
            "explore_boundary_crf: no refinement for {}",
            input.display()
        ))
    })
}

/// Legacy symbol (panics on absent refinement; prefer
/// [`explore_boundary_crf_optional`]).
#[must_use]
pub fn explore_boundary_crf_or_refined(
    refined: Option<f32>,
    boundary_crf: f32,
    input: &Path,
) -> f32 {
    explore_boundary_crf_optional(refined, boundary_crf, input).unwrap_or_else(|err| {
        gate_legacy_optional_misuse("explore_boundary_crf", err);
    })
}

/// Path-scoped delivery audit emitter (always logs). Call only from
/// [`delivery_strict_path_audit`] (M99/M100).
pub(crate) fn delivery_path_audit(branch: &'static str, path: &Path, detail: impl AsRef<str>) {
    delivery_fallback_audit(branch, format!("{}: {}", path.display(), detail.as_ref()));
}

/// Batch delivery audit emitter (always logs). Call only from
/// [`delivery_strict_batch_audit`] (M99/M100).
pub(crate) fn delivery_batch_audit(branch: &'static str, detail: impl AsRef<str>) {
    delivery_fallback_audit(branch, detail.as_ref());
}

/// Non-fatal cleanup failure during delivery.
pub fn delivery_cleanup_audit(path: &Path, context: &str, err: impl std::fmt::Display) {
    delivery_strict_path_audit("cleanup_failed", path, format!("{context}: {err}"));
}

/// Explore / quality-gate rejection on the delivery layer (strict-gated,
/// M89/M96).
pub fn explore_quality_gate_audit(branch: &'static str, input: &Path, detail: impl AsRef<str>) {
    delivery_strict_path_audit(branch, input, detail);
}

/// Protect/discard summary after an explore quality skip (strict-gated, M89/M96).
pub fn explore_quality_skip_summary_audit(summary_label: &str, protect: &str, discard: &str) {
    delivery_strict_batch_audit(
        "explore_quality_skip_summary",
        format!("{summary_label} │ Protecting: {protect} │ Discarding: {discard}"),
    );
}

/// GPU coarse / boundary explore failure with input path (strict-gated,
/// M89/M96).
pub fn explore_gpu_coarse_degraded_audit(
    branch: &'static str,
    input: &Path,
    detail: impl AsRef<str>,
) {
    delivery_strict_path_audit(branch, input, detail);
}

/// HDR metadata extraction fallback (DV / HDR10+ degrade path).
pub fn hdr_metadata_fallback_audit(branch: &'static str, path: &Path, detail: impl AsRef<str>) {
    delivery_strict_path_audit(branch, path, detail);
}

/// Apple-compat best-effort output kept despite failed quality/size gates.
pub fn apple_compat_fallback_audit(branch: &'static str, input: &Path, detail: impl AsRef<str>) {
    delivery_strict_path_audit(branch, input, detail);
}

/// WebP frame-duration pad when webpmux frame count disagrees with parsed
/// delays.
pub fn webp_frame_duration_pad_audit(
    path: &Path,
    frame_count: u32,
    parsed_durations: usize,
    pad_ms: u32,
) {
    delivery_api_path_fallback_audit(
        "webp_frame_duration_pad",
        path,
        format!(
            "webpmux reports {frame_count} frames but {parsed_durations} durations parsed; \
             padding tail with {pad_ms}ms"
        ),
    );
}

/// Animated WebP RIFF header frame count when structurally readable (`None`
/// when parse fails or zero).
#[must_use]
pub fn probe_webp_header_frame_count_optional(
    parsed: crate::unified_error::Result<u32>,
    path: &Path,
) -> Option<u32> {
    match parsed {
        Ok(0) => {
            probe_layer_audit(
                "webp_header_frame_count_zero",
                path,
                "animated WebP header reports zero frames; refusing fabricated count",
            );
            None
        }
        Ok(n) => Some(n),
        Err(err) => {
            probe_layer_audit(
                "webp_header_frame_count",
                path,
                format!("animated WebP frame count parse failed: {err}; refusing fabricated count"),
            );
            None
        }
    }
}

/// Animated WebP minimum frame policy when animation is confirmed but header
/// count is unreliable.
#[must_use]
pub fn probe_webp_animated_frame_count_or_minimum(
    parsed: crate::unified_error::Result<u32>,
    path: &Path,
) -> u32 {
    match probe_webp_header_frame_count_optional(parsed, path) {
        Some(n) if n > 1 => n,
        _ => {
            probe_layer_audit(
                "webp_animated_min_frames",
                path,
                "animated WebP without reliable header count; applying minimum frame_count=2 \
                 policy",
            );
            2
        }
    }
}

/// Legacy WebP header frame count (`0` when absent; prefer
/// [`probe_webp_header_frame_count_optional`]).
#[must_use]
pub fn probe_webp_header_frame_count_or_zero(
    parsed: crate::unified_error::Result<u32>,
    path: &Path,
) -> u32 {
    probe_webp_header_frame_count_optional(parsed, path).unwrap_or_else(|| {
        panic!(
            "probe_webp_header_frame_count required for {} after gate audit",
            path.display()
        );
    })
}

/// Exploration CRF snapped to cache grid; invalid inputs → `NaN` (never fake
/// `0.0` anchor).
#[must_use]
pub fn explore_seal_crf_or_zero(crf: f32, context: &str) -> f32 {
    match precision::crf_to_cache_key(crf) {
        Some(key) => precision::cache_key_to_crf(key),
        None => {
            explore_crf_cache_key_rejected_audit(
                "explore_seal_crf",
                format!(
                    "{context}: CRF {crf} has no cache key; refusing fabricated 0.0 grid anchor"
                ),
            );
            f32::NAN
        }
    }
}

const RUNTIME_CPU_PARALLELISM_DEFAULT: usize = 4;

/// Host CPU count when `available_parallelism` is unavailable (4 + audit in
/// strict delivery).
#[must_use]
pub fn runtime_available_parallelism_or_default(context: &str) -> usize {
    std::thread::available_parallelism().map_or_else(
        |err| {
            delivery_strict_batch_audit(
                "runtime_cpu_count",
                format!(
                    "{context}: available_parallelism failed ({err}); using \
                     {RUNTIME_CPU_PARALLELISM_DEFAULT}"
                ),
            );
            RUNTIME_CPU_PARALLELISM_DEFAULT
        },
        std::num::NonZero::get,
    )
}

/// Host CPU count capped for libvmaf / IO pools (default 4, max `cap` + audit).
#[must_use]
pub fn runtime_available_parallelism_capped_or_default(cap: usize, context: &str) -> usize {
    runtime_available_parallelism_or_default(context).min(cap)
}

/// Animated-container promotion frame count when probe reports ≤1 frame
/// (minimum 2 + audit).
#[must_use]
pub fn probe_animated_promoted_frame_count_or_min_two(frames: Option<u32>, _path: &Path) -> u32 {
    // Minimum 2 frames is intentional vid routing policy, not a forged measurement
    // (no audit).
    match frames.filter(|count| *count > 1) {
        Some(count) => count.max(2),
        None => 2,
    }
}

/// Training/delivery feature enabled unless env is explicitly opted out
/// (`0`/`false`/`no`/`off`).
#[must_use]
pub fn delivery_env_enabled_unless_opt_out(env_key: &str) -> bool {
    std::env::var(env_key).map_or(true, |raw| {
        let value = raw.trim().to_ascii_lowercase();
        !matches!(value.as_str(), "" | "0" | "false" | "no" | "off")
    })
}

/// Output file size from metadata, or pre-measured encode estimate when
/// metadata is unreadable.
#[must_use]
pub fn delivery_output_file_len_or_estimate(path: &Path, estimate_bytes: u64) -> u64 {
    match std::fs::metadata(path) {
        Ok(meta) => meta.len(),
        Err(err) => {
            delivery_strict_path_audit(
                "output_size_metadata",
                path,
                format!("metadata read failed ({err}); using encode estimate {estimate_bytes}"),
            );
            estimate_bytes
        }
    }
}

/// C API probe JSON → heap `CString` (null + audit when serialization fails).
#[must_use]
pub fn ffi_probe_json_c_string_or_null(
    value: &serde_json::Value,
    api: &str,
) -> Option<std::ffi::CString> {
    let json = match serde_json::to_string(value) {
        Ok(json) => json,
        Err(err) => {
            probe_layer_batch_audit(
                "ffi_probe_cstring",
                format!("{api}: failed to serialize probe JSON ({err})"),
            );
            return None;
        }
    };
    match std::ffi::CString::new(json) {
        Ok(cstr) => Some(cstr),
        Err(err) => {
            probe_layer_batch_audit(
                "ffi_probe_cstring",
                format!("{api}: probe JSON contains interior NUL ({err})"),
            );
            None
        }
    }
}

/// Last-resort probe JSON when both payload and error envelope fail to
/// CString-encode (Z8).
fn ffi_probe_json_fatal_ptr(api: &str) -> *mut std::ffi::c_char {
    let fallback = format!(r#"{{"ok":false,"error":"{api}: probe JSON encode failed"}}"#);
    match std::ffi::CString::new(fallback.clone()) {
        Ok(cstr) => cstr.into_raw(),
        Err(err) => {
            probe_layer_batch_audit(
                "ffi_probe_json_fatal",
                format!("{api}: probe JSON CString failed: {err}"),
            );
            let sanitized: String = fallback.chars().filter(|c| *c != '\0').collect();
            std::ffi::CString::new(sanitized)
                .unwrap_or_else(|_| {
                    match std::ffi::CString::new(
                        r#"{"ok":false,"error":"probe JSON encode failed"}"#,
                    ) {
                        Ok(cstr) => cstr,
                        Err(e) => panic!("static probe fatal json: {e}"),
                    }
                })
                .into_raw()
        }
    }
}

/// C API probe JSON response pointer (error JSON when serialization fails —
/// never silent `null`).
#[must_use]
pub fn ffi_probe_json_ptr_or_null(value: &serde_json::Value, api: &str) -> *mut std::ffi::c_char {
    match ffi_probe_json_c_string_or_null(value, api) {
        Some(cstr) => cstr.into_raw(),
        None => {
            let err_payload = serde_json::json!({
                "ok": false,
                "error": format!("{api}: probe JSON serialization failed"),
            });
            match ffi_probe_json_c_string_or_null(&err_payload, api) {
                Some(cstr) => cstr.into_raw(),
                None => ffi_probe_json_fatal_ptr(api),
            }
        }
    }
}

fn split_ffi_ingest_paths(paths_str: &str) -> Vec<String> {
    paths_str
        .split('|')
        .map(str::trim)
        .filter(|segment| !segment.is_empty())
        .map(str::to_owned)
        .collect()
}

/// C API ingest path list: JSON `Vec<String>` when valid; pipe-delimited
/// fallback (strict-gated).
#[must_use]
pub fn ffi_ingest_path_list_or_delimited(paths_str: &str) -> Vec<String> {
    match serde_json::from_str::<Vec<String>>(paths_str) {
        Ok(paths) => {
            let non_empty: Vec<String> = paths
                .into_iter()
                .map(|path| path.trim().to_owned())
                .filter(|path| !path.is_empty())
                .collect();
            if non_empty.is_empty() {
                delivery_strict_batch_audit(
                    "ffi_ingest_paths_empty_json",
                    "ingest paths JSON array was empty; using pipe-delimited parse",
                );
                split_ffi_ingest_paths(paths_str)
            } else {
                non_empty
            }
        }
        Err(err) => {
            delivery_strict_batch_audit(
                "ffi_ingest_paths_not_json",
                format!(
                    "ingest paths payload is not a JSON string array ({err}); using \
                     pipe-delimited parse"
                ),
            );
            split_ffi_ingest_paths(paths_str)
        }
    }
}

/// JXL explore previous screened candidate size (audited baseline on first
/// probe).
#[must_use]
pub fn jxl_previous_candidate_size_or_fallback(
    last_size: Option<u64>,
    fallback: u64,
    context: &str,
) -> u64 {
    match last_size {
        Some(size) => size,
        None => {
            delivery_jxl_batch_fallback_audit(
                "jxl_previous_candidate_size",
                format!(
                    "{context}: no prior screened candidate; using input_size baseline {fallback}"
                ),
            );
            fallback
        }
    }
}

/// GIF logical screen dimensions from header prefix (`None` when unreadable or
/// zero).
#[must_use]
pub fn loop_gif_logical_screen_optional(path: &Path) -> Option<(u32, u32)> {
    use std::io::Read;
    let Ok(mut file) = std::fs::File::open(path) else {
        probe_layer_audit(
            "loop_gif_header_open",
            path,
            "failed to open GIF for logical screen read; refusing fabricated 0×0 dimensions",
        );
        return None;
    };
    let mut head = [0u8; 10];
    if file.read_exact(&mut head).is_err() {
        probe_layer_audit(
            "loop_gif_header_read",
            path,
            "failed to read GIF logical screen bytes; refusing fabricated 0×0 dimensions",
        );
        return None;
    }
    let width = u32::from(u16::from_le_bytes([head[6], head[7]]));
    let height = u32::from(u16::from_le_bytes([head[8], head[9]]));
    if width > 0 && height > 0 {
        Some((width, height))
    } else {
        probe_layer_audit(
            "loop_gif_header_zero_dims",
            path,
            "GIF logical screen is 0×0; refusing fabricated dimensions",
        );
        None
    }
}

/// Legacy GIF logical screen helper (`(0, 0)` when absent; prefer
/// [`loop_gif_logical_screen_optional`]).
#[must_use]
pub fn loop_gif_logical_screen_or_zero(path: &Path) -> (u32, u32) {
    loop_gif_logical_screen_optional(path).unwrap_or_else(|| {
        panic!(
            "loop_gif_logical_screen_or_zero is legacy — use loop_gif_logical_screen_optional ({})",
            path.display()
        );
    })
}

/// Loop-intent filename segment for numeric-density features (empty when
/// unknown).
#[must_use]
pub const fn loop_filename_or_empty_for_density(name: Option<&str>) -> &str {
    match name {
        Some(s) => s,
        None => "",
    }
}

/// Clear percentile slots on a distribution that is not backed by an empirical
/// histogram.
pub const fn strip_distribution_percentile_slots(stats: &mut crate::database::DistributionStats) {
    stats.p10 = None;
    stats.p25 = None;
    stats.p50 = None;
    stats.p75 = None;
    stats.p90 = None;
}

/// Require an empirical feature-map slot when building a DB-backed loop
/// reference profile.
///
/// # Errors
/// Returns an error when `mapped` is `None` (no silent substitution).
pub fn algorithm_feature_distribution_required(
    mapped: Option<crate::database::DistributionStats>,
    feature_key: &str,
) -> anyhow::Result<crate::database::DistributionStats> {
    mapped.ok_or_else(|| {
        loop_profile_feature_absent(feature_key);
        anyhow::anyhow!("loop reference feature `{feature_key}` missing from feature_stats")
    })
}

/// Log-line file-type icon prefix from filename extension (empty when no
/// extension).
#[must_use]
pub fn ui_log_file_type_icon_prefix(filename: &str) -> String {
    filename.rfind('.').map_or_else(String::new, |ext_start| {
        let ext = filename[ext_start + 1..].to_ascii_lowercase();
        match ext.as_str() {
            "gif" => ui_icon_pick("🎞️ ", "[ANIM] "),
            "jpg" | "jpeg" | "png" | "webp" | "avif" | "heic" | "heif" | "jxl" | "bmp" | "tiff"
            | "tif" | "ico" | "svg" | "psd" | "raw" | "cr2" | "nef" | "arw" | "dng" | "orf"
            | "rw2" | "exr" | "qoi" | "flif" | "jp2" | "j2k" => ui_icon_pick("🖼️  ", "[IMG] "),
            "mp4" | "mov" | "avi" | "mkv" | "webm" | "flv" | "wmv" | "m4v" | "mpg" | "mpeg"
            | "3gp" | "ogv" | "ts" | "mts" | "m2ts" => ui_icon_pick("🎬 ", "[VID] "),
            _ => String::new(),
        }
    })
}

/// GIF encode FPS from probe (audits when not derived from extracted frames /
/// duration).
#[must_use]
pub fn gif_encode_fps_from_probe(
    input: &Path,
    duration_secs: f64,
    extracted_count: usize,
    avg_frame_rate: Option<f64>,
    r_frame_rate: Option<f64>,
) -> Option<f64> {
    if duration_secs > 0.0 && extracted_count > 0 {
        return Some(crate::numeric_cast::usize_to_f64(extracted_count) / duration_secs);
    }
    if let Some(avg_fps) = avg_frame_rate.filter(|rate| *rate > 0.0) {
        return Some(avg_fps);
    }
    if let Some(r_fps) = r_frame_rate.filter(|rate| *rate > 0.0) {
        return Some(r_fps);
    }
    delivery_api_path_fallback_audit(
        "gif_fps_unavailable",
        input,
        format!("no FPS source (duration={duration_secs:.3}s, extracted_frames={extracted_count})"),
    );
    None
}

/// Last non-empty stderr line for tool failure messages.
#[must_use]
pub fn tool_stderr_last_line_label(stderr: &str, context: impl AsRef<str>) -> String {
    let context = context.as_ref();
    stderr
        .lines()
        .rev()
        .find_map(|line| {
            let trimmed = line.trim();
            (!trimmed.is_empty()).then(|| trimmed.to_string())
        })
        .unwrap_or_else(|| {
            delivery_strict_batch_audit(
                "tool_stderr_empty",
                format!("{context}: stderr has no non-empty line"),
            );
            String::from("No error output")
        })
}

/// Inline SSIM segment for progress bars (empty when unset; strict-gated audit,
/// M113).
#[must_use]
pub fn ui_ssim_inline_or_empty(value: Option<f64>, context: &str) -> String {
    value.filter(|v| v.is_finite()).map_or_else(
        || {
            delivery_progress_batch_audit(
                "ui_ssim_empty",
                format!("{context}: SSIM unset or non-finite; omitting inline SSIM segment"),
            );
            String::new()
        },
        |s| format!("SSIM {s:.4}"),
    )
}

/// SSIM token for exploration progress bar output (delegates to
/// [`ui_ssim_inline_or_empty`]).
#[must_use]
pub fn explore_progress_ssim_token(ssim: Option<f64>, context: &str) -> String {
    ui_ssim_inline_or_empty(ssim, context)
}

/// SSIM inline segment when SSIM has not been measured yet (no audit; expected
/// during encode).
#[must_use]
pub fn ui_ssim_inline_when_unmeasured(value: Option<f64>) -> String {
    value
        .filter(|v| v.is_finite())
        .map_or_else(String::new, |s| format!("SSIM {s:.4}"))
}

/// Per-iteration explore progress SSIM (encode in flight; absence is not a
/// delivery fallback).
#[must_use]
pub fn explore_progress_ssim_token_pending(ssim: Option<f64>) -> String {
    ui_ssim_inline_when_unmeasured(ssim)
}

/// Infrastructure label when an optional version string is missing (e.g.
/// pgvector).
#[must_use]
pub fn infra_version_label_or_audit(
    branch: &'static str,
    version: Option<&str>,
    fallback: &'static str,
) -> String {
    version.map_or_else(
        || {
            delivery_strict_batch_audit(
                branch,
                format!("version missing; reporting \"{fallback}\""),
            );
            fallback.to_string()
        },
        str::to_string,
    )
}

/// Explore quality failure reason when structured fields are empty.
#[must_use]
pub fn explore_quality_fail_reason(
    quality_failure: Option<&str>,
    enhanced_failure: Option<&str>,
    input: &Path,
) -> String {
    quality_failure.or(enhanced_failure).map_or_else(
        || {
            explore_delivery_explore_outcome_audit(
                "explore_fail_reason",
                format!(
                    "{}: quality_passed and enhanced_verify_fail_reason both empty",
                    input.display()
                ),
            );
            String::from("quality/size check failed")
        },
        str::to_string,
    )
}

/// MS-SSIM score for explore failure logs (`{score:.4}`).
#[must_use]
pub fn explore_ms_ssim_score_display(score: Option<f64>, context: &str) -> String {
    score.map_or_else(
        || {
            explore_gpu_coarse_explore_audit(
                "ms_ssim_score",
                format!("{context}: MS-SSIM score missing in explore result"),
            );
            String::from("unreported")
        },
        |s| format!("{s:.4}"),
    )
}

/// MS-SSIM score with `score=` prefix for gate summaries.
#[must_use]
pub fn explore_ms_ssim_score_prefixed(score: Option<f64>, context: &str) -> String {
    score.map_or_else(
        || {
            explore_gpu_coarse_explore_audit(
                "ms_ssim_score",
                format!("{context}: MS-SSIM score missing for quality gate summary"),
            );
            String::from("score=unreported")
        },
        |s| format!("score={s:.4}"),
    )
}

/// Ultimate-mode 3D quality summary when metrics are absent.
#[must_use]
pub fn explore_ultimate_summary_display(summary: Option<String>, context: &str) -> String {
    summary.unwrap_or_else(|| {
        explore_gpu_coarse_explore_audit(
            "ultimate_quality_summary",
            format!("{context}: ultimate quality summary unavailable"),
        );
        String::from("3D metrics unavailable")
    })
}

/// Resolve [`crate::ffprobe_json::ColorInfo`] for cjxl prep; probes via ffprobe
/// when not supplied.
#[must_use]
pub fn color_info_for_cjxl_prep<'a>(
    input: &Path,
    provided: Option<&'a crate::ffprobe_json::ColorInfo>,
    storage: &'a mut crate::ffprobe_json::ColorInfo,
) -> &'a crate::ffprobe_json::ColorInfo {
    if let Some(info) = provided {
        return info;
    }
    *storage = crate::ffprobe_json::extract_color_info(input);
    storage
}

/// ffprobe stream `pix_fmt` (empty when missing — alpha-aux heuristic;
/// policy-silent, M92).
#[must_use]
pub fn ffprobe_pix_fmt_or_empty(
    pix_fmt: Option<&str>,
    _stream_index: usize,
    _context: &str,
) -> String {
    pix_fmt.map_or_else(String::new, str::to_ascii_lowercase)
}

/// Process exit code for tool failure logs (path-scoped).
#[must_use]
pub fn process_exit_code_label(code: Option<i32>, tool: &str, path: &Path) -> i32 {
    process_exit_code_for_context(code, tool, path.display().to_string())
}

/// Loud audit when an external tool process exits non-zero (process runner).
pub fn delivery_tool_process_failed_audit(tool: &str, command_line: &str, code: Option<i32>) {
    let code_label = process_exit_code_for_context(code, tool, command_line);
    delivery_substrate_batch_audit(
        "tool_process_failed",
        crate::infra::static_logs::messages::MSG_PROCESS_FAIL
            .replacen("{}", &code_label.to_string(), 1)
            .replacen("{}", command_line, 1),
    );
}

/// ETA overflow/invalid during coarse batch progress.
pub fn delivery_progress_eta_unknown_audit() {
    delivery_strict_batch_audit(
        "progress_eta_invalid",
        "ETA calculation produced an invalid duration; rendering unknown ETA",
    );
}

/// Exploration / coarse progress terminal failure.
pub fn delivery_progress_explore_failed(prefix: &str, error: &str) {
    delivery_substrate_batch_audit(
        "progress_explore_failed",
        format!("{prefix} failed: {error}"),
    );
}

/// Progress UI field when a mutex is poisoned (empty string fallback).
#[must_use]
pub fn delivery_progress_mutex_string_or_empty(
    branch: &'static str,
    guard: Result<
        std::sync::MutexGuard<'_, String>,
        std::sync::PoisonError<std::sync::MutexGuard<'_, String>>,
    >,
) -> String {
    guard.map_or_else(
        |_| {
            delivery_substrate_batch_audit(branch, "progress mutex poisoned; using empty string");
            String::new()
        },
        |value| value.clone(),
    )
}

/// MS-SSIM progress percent calculation invalid (NaN/Inf/overflow).
pub fn delivery_msssim_progress_pct_invalid_audit() {
    delivery_substrate_batch_audit(
        "msssim_progress_pct_invalid",
        "MS-SSIM progress calculation failed: NaN/Inf/overflow detected",
    );
}

/// MS-SSIM channel score map mutex poisoned.
pub fn delivery_msssim_channel_mutex_audit(branch: &'static str) {
    delivery_substrate_batch_audit(
        branch,
        "failed to acquire lock for channel scores (poisoned)",
    );
}

/// Exploration CRF cache key rejected (negative / non-finite / out of range).
pub fn explore_crf_cache_key_rejected_audit(branch: &'static str, detail: impl AsRef<str>) {
    delivery_strict_batch_audit(branch, detail);
}

/// SSIM measurement failed; PSNR fallback attempted (explore quality path).
pub fn explore_ssim_measurement_fallback_audit(path: &Path, detail: impl AsRef<str>) {
    delivery_strict_path_audit("explore_ssim_measurement_failed", path, detail);
}

/// Analysis / quality cache entry invalidated (checksum, corruption).
pub fn analysis_cache_invalidate_audit(branch: &'static str, path: &Path, detail: impl AsRef<str>) {
    probe_layer_audit(branch, path, detail);
}

/// Batch audit for analysis-cache DB lifecycle (CLI init, age prune, algorithm
/// upgrade).
pub fn analysis_cache_lifecycle_batch_audit(branch: &'static str, detail: impl AsRef<str>) {
    probe_layer_batch_audit(branch, detail);
}

/// Video analysis cache load failure (`detect_video_with_cache`).
pub fn video_cache_load_failed_audit(path: &Path, err: impl std::fmt::Display) {
    probe_layer_audit(
        "video_cache_load_failed",
        path,
        format!("failed to load cached video analysis: {err}"),
    );
}

/// Video analysis cache store failure (SSOT for `detect_video_with_cache` and
/// `conversion_api`).
pub fn video_cache_store_failed_audit(path: &Path, phase: &str, err: impl std::fmt::Display) {
    probe_layer_audit(
        "video_cache_store_failed",
        path,
        format!("phase={phase}; failed to store video analysis: {err}"),
    );
}

/// Image analyzer cache load failure (`analyze_image_with_cache`).
pub fn analyzer_cache_load_failed_audit(path: &Path, err: impl std::fmt::Display) {
    probe_layer_audit(
        "analyzer_cache_load_failed",
        path,
        format!("cache load failed during analysis: {err}"),
    );
}

/// Image analyzer cache store failure.
pub fn analyzer_cache_store_failed_audit(path: &Path, phase: &str, err: impl std::fmt::Display) {
    probe_layer_audit(
        "analyzer_cache_store_failed",
        path,
        format!("phase={phase}; failed to store analysis in cache: {err}"),
    );
}

/// Image quality cache load failure.
pub fn image_quality_cache_load_failed_audit(path: &Path, err: impl std::fmt::Display) {
    probe_quality_layer_audit(
        "image_quality_cache_load_failed",
        path,
        format!("failed to load cached quality analysis: {err}"),
    );
}

/// Image quality cache store failure.
pub fn image_quality_cache_store_failed_audit(
    path: &Path,
    phase: &str,
    err: impl std::fmt::Display,
) {
    probe_quality_layer_audit(
        "image_quality_cache_store_failed",
        path,
        format!("phase={phase}; failed to store quality analysis in cache: {err}"),
    );
}

/// Cache prune row count when DB returns valid usize.
#[must_use]
pub fn analysis_cache_prune_rows_optional(
    rows: Result<usize, impl std::fmt::Display>,
) -> Option<usize> {
    match rows {
        Ok(n) => Some(n),
        Err(e) => {
            delivery_substrate_batch_audit(
                "analysis_cache_prune_count_invalid",
                format!("failed to parse cache prune rowcount: {e}; refusing fabricated count 0"),
            );
            None
        }
    }
}

/// Legacy cache prune helper (`0` when parse fails; prefer
/// [`analysis_cache_prune_rows_optional`]).
// Keep the audited compatibility fallback visible instead of using `unwrap_or`.
#[allow(clippy::manual_unwrap_or, clippy::manual_unwrap_or_default)]
#[must_use]
pub fn analysis_cache_prune_rows_or_zero(rows: Result<usize, impl std::fmt::Display>) -> usize {
    match analysis_cache_prune_rows_optional(rows) {
        Some(n) => n,
        None => 0,
    }
}

/// BLAKE3 / fingerprint buffer slice with audited empty fallback.
#[must_use]
pub fn probe_hash_buffer_slice<'a>(buffer: &'a [u8], end: usize, context: &str) -> &'a [u8] {
    buffer.get(..end).unwrap_or_else(|| {
        delivery_substrate_batch_audit(
            "hash_buffer_slice_oob",
            format!(
                "{context}: slice end {end} exceeds buffer len {}",
                buffer.len()
            ),
        );
        &[]
    })
}

/// Quality / codec intelligence batch audit (no path).
pub fn probe_quality_batch_audit(branch: &'static str, detail: impl AsRef<str>) {
    probe_layer_batch_audit(branch, detail);
}

/// Quality / codec intelligence path-scoped audit.
pub fn probe_quality_layer_audit(branch: &'static str, path: &Path, detail: impl AsRef<str>) {
    probe_layer_audit(branch, path, detail);
}

/// Optional bool for HDR / feature flags (false when absent).
#[must_use]
pub fn probe_bool_or_false(value: Option<bool>, branch: &'static str, context: &str) -> bool {
    value.unwrap_or_else(|| {
        delivery_strict_batch_audit(
            branch,
            format!("{context}: boolean signal missing; treating as false"),
        );
        false
    })
}

/// WebP `VP8X` flags byte when header is truncated (`None`, not fake `0`).
#[must_use]
pub fn probe_webp_vp8x_flags_optional(flags: Option<u8>) -> Option<u8> {
    flags.or_else(|| {
        delivery_strict_batch_audit(
            "webp_vp8x_flags_missing",
            "VP8X header missing flags byte; refusing fabricated flags=0",
        );
        None
    })
}

/// Legacy VP8X flags helper (panics when absent; prefer
/// [`probe_webp_vp8x_flags_optional`]).
#[must_use]
pub fn probe_webp_vp8x_flags_or_zero(flags: Option<u8>) -> u8 {
    probe_webp_vp8x_flags_optional(flags).unwrap_or_else(|| {
        panic!("probe_webp_vp8x_flags_or_zero is legacy — use probe_webp_vp8x_flags_optional")
    })
}

/// ffprobe format.duration missing in container metadata.
pub fn probe_format_duration_missing_audit() {
    probe_layer_batch_audit(
        "ffprobe_format_duration_missing",
        crate::infra::static_logs::messages::MSG_PROBE_DURATION_MISSING,
    );
}

/// Stream `pix_fmt` label when missing (defaults to `"unknown"`).
#[must_use]
pub fn probe_pix_fmt_label(pix_fmt: Option<&str>, path: &Path, context: &str) -> String {
    pix_fmt
        .filter(|value| !value.trim().is_empty())
        .map_or_else(
            || {
                probe_layer_audit(
                    "ffprobe_pix_fmt_unknown",
                    path,
                    format!("{context}: pix_fmt missing; using \"unknown\""),
                );
                String::from("unknown")
            },
            ToString::to_string,
        )
}

/// Stream JSON `index` when absent (use enumeration fallback).
#[must_use]
pub fn probe_stream_index_or_fallback(
    json_index: Option<usize>,
    fallback: usize,
    path: Option<&Path>,
) -> usize {
    json_index.unwrap_or_else(|| {
        let detail =
            format!("stream index missing in ffprobe JSON; using enumeration index {fallback}");
        if let Some(path) = path {
            probe_layer_audit("ffprobe_stream_index_missing", path, detail);
        } else {
            delivery_strict_batch_audit("ffprobe_stream_index_missing", detail);
        }
        fallback
    })
}

/// `has_b_frames` clamped to `u8` with audit when out of range.
#[must_use]
pub fn probe_b_frames_u8_or_max(value: i64, path: &Path) -> u8 {
    u8::try_from(value.clamp(0, i64::from(u8::MAX))).unwrap_or_else(|_| {
        probe_layer_audit(
            "ffprobe_b_frames_clamped",
            path,
            format!(
                "B-frame count {value} out of u8 range; clamping to {}",
                u8::MAX
            ),
        );
        u8::MAX
    })
}

/// Side data `side_data_type` string (empty when missing).
#[must_use]
pub fn probe_side_data_type_label(sd_type: Option<&str>) -> String {
    sd_type
        .map_or_else(
            || {
                probe_layer_batch_audit(
                    "ffprobe_side_data_type_missing",
                    "side_data_type missing; using empty string",
                );
                String::new()
            },
            ToString::to_string,
        )
        .to_lowercase()
}

/// HDR side-data `u8` field when out of range (skip field).
pub fn probe_hdr_metadata_u8_or_skip(
    value: u64,
    branch: &'static str,
    detail: impl AsRef<str>,
) -> Option<u8> {
    match u8::try_from(value) {
        Ok(value) => Some(value),
        Err(e) => {
            probe_layer_batch_audit(branch, format!("{}: {e}", detail.as_ref()));
            None
        }
    }
}

/// ffprobe path-scoped probe failure.
pub fn probe_ffprobe_path_audit(branch: &'static str, path: &Path, detail: impl AsRef<str>) {
    probe_layer_audit(branch, path, detail);
}

/// ffprobe subprocess audit when only a display path string is available.
pub fn probe_ffprobe_input_audit(branch: &'static str, input: &str, detail: impl AsRef<str>) {
    probe_layer_batch_audit(branch, format!("{input}: {}", detail.as_ref()));
}

/// Video explore precheck / stream / SSIM path-scoped audit (strict-gated,
/// M96).
pub fn explore_precheck_audit(branch: &'static str, input: &Path, detail: impl AsRef<str>) {
    delivery_strict_path_audit(branch, input, detail);
}

/// Explore precheck batch audit (duration chain, mapping; strict-gated, M96).
pub fn explore_precheck_batch_audit(branch: &'static str, detail: impl AsRef<str>) {
    delivery_strict_batch_audit(branch, detail);
}

/// Misuse of contract `*_or_default` exports (logic error — production must use
/// `*_optional` / `?`).
#[cold]
#[track_caller]
fn gate_legacy_optional_misuse(branch: &'static str, detail: impl std::fmt::Display) -> ! {
    explore_precheck_batch_audit(branch, detail.to_string());
    panic!("{detail}");
}

/// Explore metric parse/seal reject: audit only under strict delivery (M75
/// anti-spam).
pub fn explore_metric_parse_reject_audit(metric: &str, detail: impl AsRef<str>) {
    explore_precheck_batch_audit(
        "explore_metric_seal",
        format!("{metric}: {}", detail.as_ref()),
    );
}

/// Explore precheck / `stream_analysis` operational failure (delegates to
/// [`explore_precheck_batch_audit`], M84/M96).
pub fn explore_precheck_degraded_audit(branch: &'static str, detail: impl AsRef<str>) {
    explore_precheck_batch_audit(branch, detail);
}

/// SSIM/CAMBI/MS-SSIM operational failure (probe/IO/parse): audit only under
/// strict (M79).
pub fn explore_ssim_metric_degraded_audit(branch: &'static str, detail: impl AsRef<str>) {
    explore_precheck_degraded_audit(branch, detail);
}

/// `nb_frames` absent in precheck JSON (`None`; audit only — not a measured
/// frame count).
#[must_use]
pub fn explore_precheck_nb_frames_optional(
    parsed: Option<u64>,
    input: &Path,
    context: &str,
) -> Option<u64> {
    match parsed {
        Some(v) => Some(v),
        None => {
            explore_precheck_degraded_audit(
                "explore_precheck_audit",
                format!(
                    "{context}: nb_frames absent or non-numeric for {}; refusing fabricated frame \
                     count 0",
                    input.display()
                ),
            );
            None
        }
    }
}

/// Legacy sentinel helper (deprecated; prefer
/// [`explore_precheck_nb_frames_optional`]).
#[must_use]
pub fn explore_precheck_nb_frames_or_zero(input: &Path, context: &str) -> u64 {
    explore_precheck_degraded_audit(
        "explore_precheck_audit",
        format!(
            "{context}: nb_frames absent or non-numeric for {}; refusing fabricated frame count 0",
            input.display()
        ),
    );
    panic!(
        "explore_precheck_nb_frames required for {} after gate audit",
        input.display()
    );
}

/// Resolved `nb_frames` from precheck JSON (`None` when absent; callers
/// re-derive from duration×fps).
#[must_use]
pub fn explore_precheck_nb_frames_resolved(
    parsed: Option<u64>,
    input: &Path,
    context: &str,
) -> Option<u64> {
    explore_precheck_nb_frames_optional(parsed, input, context)
}

/// Explore outcome / safety stop (highly compressed, iteration cap):
/// strict-gated (M84/M96).
pub fn explore_delivery_explore_outcome_audit(branch: &'static str, detail: impl AsRef<str>) {
    delivery_strict_batch_audit(branch, detail);
}

/// Explore progress time millis (`None` when u64→u32 cast would overflow).
#[must_use]
pub fn explore_progress_time_millis_optional(time_us: u64) -> Option<u32> {
    let millis = time_us / 1_000;
    crate::numeric_cast::u64_to_u32_strict(millis, "progress_time_millis").or_else(|| {
        explore_delivery_explore_outcome_audit(
            "explore_progress_time_overflow",
            "progress time u64→u32 overflow; refusing fabricated millis 0 for ETA display",
        );
        None
    })
}

/// Legacy progress millis helper (panics internally via `unwrap` on overflow
/// path; prefer optional).
#[must_use]
pub fn explore_progress_time_millis_or_zero(time_us: u64) -> u32 {
    explore_progress_time_millis_optional(time_us).unwrap_or_else(|| {
        panic!(
            "explore_progress_time_millis_or_zero is legacy — use \
             explore_progress_time_millis_optional (time_us={time_us})"
        );
    })
}

/// GPU coarse search / phase-3 explore fallback (path-scoped, strict-gated,
/// M96).
pub fn explore_gpu_coarse_audit(branch: &'static str, input: &Path, detail: impl AsRef<str>) {
    delivery_strict_path_audit(branch, input, detail);
}

/// GPU coarse search fallback when no stable input path is in scope
/// (strict-gated, M96).
pub fn explore_gpu_coarse_batch_audit(branch: &'static str, detail: impl AsRef<str>) {
    delivery_strict_batch_audit(branch, detail);
}

/// GPU coarse degrading / explore diagnostic batch audit (strict-gated,
/// M77/M83/M96 SSOT).
pub fn explore_gpu_coarse_fallback_audit(branch: &'static str, detail: impl AsRef<str>) {
    explore_gpu_coarse_batch_audit(branch, detail);
}

/// GPU coarse explore-phase diagnostic (alias of
/// [`explore_gpu_coarse_fallback_audit`], M83/M96).
pub fn explore_gpu_coarse_explore_audit(branch: &'static str, detail: impl AsRef<str>) {
    explore_gpu_coarse_fallback_audit(branch, detail);
}

/// Audio bitrate for GPU coarse encode strategy (`None` when probe omits or
/// zero `bit_rate`).
#[must_use]
pub fn explore_gpu_coarse_audio_bitrate_optional(bit_rate: Option<u64>) -> Option<u64> {
    match bit_rate.filter(|rate| *rate > 0) {
        Some(rate) => Some(rate),
        None => {
            explore_gpu_coarse_fallback_audit(
                "explore_gpu_size",
                "audio bit_rate absent or zero; refusing forged default kbps for strategy",
            );
            None
        }
    }
}

/// Legacy name (panics when absent; prefer
/// [`explore_gpu_coarse_audio_bitrate_optional`]).
#[must_use]
pub fn explore_gpu_coarse_audio_bitrate_or_default(bit_rate: Option<u64>) -> u64 {
    explore_gpu_coarse_audio_bitrate_optional(bit_rate).unwrap_or_else(|| {
        panic!("explore_gpu_coarse_audio_bitrate required after gate audit");
    })
}

/// HDR synthesis `intensity_target` fallback.
pub fn hdr_intensity_target_audit(branch: &'static str, detail: impl AsRef<str>) {
    delivery_substrate_batch_audit(branch, detail);
}

/// `FFmpeg` stdout drain / stderr reader failure during delivery encode.
pub fn delivery_ffmpeg_io_audit(branch: &'static str, detail: impl AsRef<str>) {
    delivery_substrate_batch_audit(branch, detail);
}

/// Process exit code when no file path is available (command-line context).
#[must_use]
pub fn process_exit_code_for_context(
    code: Option<i32>,
    tool: &str,
    context: impl AsRef<str>,
) -> i32 {
    let context = context.as_ref();
    code.unwrap_or_else(|| {
        delivery_strict_batch_audit(
            "process_exit_code",
            format!(
                "{tool} ({context}): exit code unavailable (subprocess reap/killing failure); \
                 reporting -1"
            ),
        );
        -1
    })
}

/// Recover a poisoned mutex guard (delivery tracking / reservations).
pub fn mutex_guard_or_recover<'a, T>(
    branch: &'static str,
    lock_result: Result<
        std::sync::MutexGuard<'a, T>,
        std::sync::PoisonError<std::sync::MutexGuard<'a, T>>,
    >,
) -> std::sync::MutexGuard<'a, T> {
    lock_result.unwrap_or_else(|err| {
        delivery_strict_batch_audit(branch, "mutex poisoned; recovering inner state");
        err.into_inner()
    })
}

/// Serialize stderr/progress terminal output with audited poison recovery
/// (M49/M169).
pub fn delivery_terminal_lock_guard<'a>(branch: &'static str) -> std::sync::MutexGuard<'a, ()> {
    mutex_guard_or_recover(branch, crate::ctrlc_guard::TERMINAL_LOCK.lock())
}

/// Recover a poisoned mutex at `into_inner` (batch handoff).
pub fn mutex_into_inner_or_recover<T>(branch: &'static str, lock: std::sync::Mutex<T>) -> T {
    lock.into_inner().unwrap_or_else(|err| {
        delivery_strict_batch_audit(
            branch,
            "mutex poisoned at into_inner; recovering inner state",
        );
        err.into_inner()
    })
}

/// Recover a poisoned `RwLock` read guard (tool cache / builder
/// infrastructure).
pub fn rwlock_read_guard_or_recover<'a, T>(
    branch: &'static str,
    lock_result: Result<
        std::sync::RwLockReadGuard<'a, T>,
        std::sync::PoisonError<std::sync::RwLockReadGuard<'a, T>>,
    >,
) -> std::sync::RwLockReadGuard<'a, T> {
    lock_result.unwrap_or_else(|err| {
        delivery_strict_batch_audit(branch, "rwlock read poisoned; recovering inner state");
        err.into_inner()
    })
}

/// Recover a poisoned `RwLock` write guard (tool cache / builder
/// infrastructure).
pub fn rwlock_write_guard_or_recover<'a, T>(
    branch: &'static str,
    lock_result: Result<
        std::sync::RwLockWriteGuard<'a, T>,
        std::sync::PoisonError<std::sync::RwLockWriteGuard<'a, T>>,
    >,
) -> std::sync::RwLockWriteGuard<'a, T> {
    lock_result.unwrap_or_else(|err| {
        delivery_strict_batch_audit(branch, "rwlock write poisoned; recovering inner state");
        err.into_inner()
    })
}

/// Batch byte totals when post-encode `output_size` is missing (strict-gated).
#[must_use]
pub fn delivery_batch_output_bytes_or_input(
    output_bytes: Option<u64>,
    input_bytes: u64,
    context: &str,
) -> u64 {
    output_bytes.unwrap_or_else(|| {
        delivery_strict_batch_audit(
            "batch_output_size_missing",
            format!(
                "{context}: output_size missing after reported success; using input_size \
                 {input_bytes} for batch totals"
            ),
        );
        input_bytes
    })
}

/// Recover a poisoned session-logging mutex and warn on stderr (M44).
pub fn logging_mutex_guard_or_recover<'a, T>(
    branch: &'static str,
    detail: &str,
    lock_result: Result<
        std::sync::MutexGuard<'a, T>,
        std::sync::PoisonError<std::sync::MutexGuard<'a, T>>,
    >,
) -> std::sync::MutexGuard<'a, T> {
    lock_result.unwrap_or_else(|err| {
        delivery_logging_path_audit(branch, Path::new("."), detail);
        crate::ui_stderr::line(
            crate::modern_ui::symbols::WARNING,
            crate::modern_ui::symbols::plain::WARNING,
            format!("[Logging] {detail}"),
        );
        err.into_inner()
    })
}

/// Tracing file appender file name (`app.log` when missing; M44 / U11).
#[must_use]
pub fn path_tracing_log_file_name_or_app_log(path: &Path, context: &str) -> std::ffi::OsString {
    path.file_name().filter(|n| !n.is_empty()).map_or_else(
        || {
            delivery_logging_path_audit(
                "path_file_name",
                path,
                format!("{context}: missing file_name; using app.log"),
            );
            std::ffi::OsStr::new("app.log").to_os_string()
        },
        std::ffi::OsStr::to_os_string,
    )
}

/// Result-box width when the content line set is empty (U11).
#[must_use]
pub fn ui_result_box_width_or_title_default(
    line_widths: impl Iterator<Item = usize>,
    title_width: usize,
) -> usize {
    line_widths.max().unwrap_or_else(|| {
        delivery_runtime_batch_audit(
            "ui_result_box",
            "empty content lines; using title width (min 40) for box layout",
        );
        title_width.max(40)
    })
}

/// ASCII temp suffix → UTF-8 (audited fallback on impossible failure).
#[must_use]
pub fn temp_output_suffix_utf8(suffix: &[u8]) -> String {
    String::from_utf8(suffix.to_vec()).unwrap_or_else(|_| {
        delivery_strict_batch_audit(
            "temp_suffix_utf8",
            "temp output suffix UTF-8 conversion failed; using \"0000000000\"",
        );
        String::from("0000000000")
    })
}

/// Human-readable size delta for conversion reports (`i64` overflow → audited
/// label).
#[must_use]
pub fn size_delta_report_label(diff_bytes: i128, path: &Path) -> String {
    i64::try_from(diff_bytes).map_or_else(
        |_| {
            delivery_strict_path_audit(
                "size_diff_overflow",
                path,
                "size delta i128→i64 overflow; using unavailable label",
            );
            String::from("size delta unavailable")
        },
        crate::modern_ui::format_size_diff,
    )
}

/// Exploration CRF anchor when warm-start cache is empty (audited; not a
/// measured encode result).
#[must_use]
pub fn warm_start_crf_or_predicted(
    warm_start: Option<f32>,
    predicted: f32,
    input: &Path,
    codec: &str,
) -> f32 {
    if let Some(crf) = warm_start {
        return crf;
    }

    // Fail-closed: when warm-start cache is empty, do not drive exploration from
    // the derived/predicted anchor. Use codec defaults as an audited
    // conservative baseline.
    let codec_default = match codec.to_ascii_lowercase().as_str() {
        "hevc" => crate::constants::CRF_HEVC_DEFAULT,
        "av1" => crate::constants::CRF_AV1_DEFAULT,
        "vp9" => crate::constants::CRF_VP9_DEFAULT,
        "x264" | "h264" => crate::constants::CRF_X264_DEFAULT,
        _ => crate::constants::EXPLORE_DEFAULT_INITIAL_CRF,
    };
    explore_gpu_coarse_fallback_audit(
        "warm_start_predicted_anchor",
        format!(
            "{}: no warm-start CRF for {codec}; using codec default search anchor \
             {codec_default:.2} (predicted {predicted:.2} ignored)",
            input.display()
        ),
    );
    codec_default
}

/// JPEG SOI magic (`FF D8`) for pre-process routing (invalid when unreadable).
#[must_use]
pub fn jpeg_magic_valid_for_delivery(input: &Path) -> bool {
    use std::io::Read;
    std::fs::File::open(input)
        .and_then(|mut file| {
            let mut buf = [0u8; 2];
            file.read_exact(&mut buf)?;
            Ok(buf == [0xFF, 0xD8])
        })
        .unwrap_or_else(|err| {
            delivery_strict_batch_audit(
                "jpeg_magic_probe",
                format!(
                    "{}: cannot read JPEG magic ({err}); treating header as invalid",
                    input.display()
                ),
            );
            false
        })
}

/// AVIF encoder quality when the caller omits an explicit value.
#[must_use]
pub fn avif_quality_or_fallback(quality: Option<u8>) -> u8 {
    quality.unwrap_or_else(|| {
        delivery_strict_batch_audit(
            "avif_quality",
            format!(
                "missing AVIF quality; using fallback {}",
                crate::constants::FALLBACK_QUALITY_AVIF
            ),
        );
        crate::constants::FALLBACK_QUALITY_AVIF
    })
}

/// Ingest CLI quality label when unset.
#[must_use]
pub fn ingest_quality_label_or_default(label: Option<&str>) -> String {
    label.map_or_else(
        || {
            delivery_strict_batch_audit(
                "ingest_quality_label",
                "quality label unset; using \"low\"",
            );
            String::from("low")
        },
        str::to_string,
    )
}

/// Parent directory for leaky temp outputs beside the final path.
#[must_use]
pub fn output_parent_or_dot(output: &Path) -> &Path {
    output.parent().unwrap_or_else(|| {
        delivery_strict_batch_audit(
            "output_parent",
            format!(
                "{}: missing parent directory; using \".\" for temp output",
                output.display()
            ),
        );
        Path::new(".")
    })
}

/// Stem segment for `temp_path_for_output` naming.
#[must_use]
pub fn temp_output_stem_lossy(output: &Path) -> String {
    output.file_stem().map_or_else(
        || {
            delivery_strict_batch_audit(
                "temp_output_stem",
                format!(
                    "{}: missing file_stem; using empty stem in temp name",
                    output.display()
                ),
            );
            String::new()
        },
        |stem| stem.to_string_lossy().into_owned(),
    )
}

/// Extension segment for `temp_path_for_output` naming.
#[must_use]
pub fn temp_output_extension_lossy(output: &Path) -> String {
    output.extension().map_or_else(
        || {
            delivery_strict_batch_audit(
                "temp_output_extension",
                format!(
                    "{}: missing extension; using empty ext in temp name",
                    output.display()
                ),
            );
            String::new()
        },
        |ext| ext.to_string_lossy().into_owned(),
    )
}

/// SSIM fragment for conversion success messages (empty when absent;
/// strict-gated audit, M113).
#[must_use]
pub fn conversion_ssim_message_token(ssim: Option<f64>) -> String {
    ssim.filter(|v| v.is_finite()).map_or_else(
        || {
            delivery_progress_batch_audit(
                "conversion_ssim",
                "conversion success message: SSIM unset or non-finite; omitting SSIM segment",
            );
            String::new()
        },
        |value| {
            crate::infra::static_logs::messages::MSG_CONVERSION_SSIM
                .replace("{ssim}", &format!("{value:.4}"))
        },
    )
}

/// `FFmpeg` / process exit-code suffix for user-facing errors (empty when
/// unknown + audit, M113).
#[must_use]
pub fn ui_exit_code_suffix_or_empty(exit_code: Option<i32>, context: &str) -> String {
    exit_code.map_or_else(
        || {
            delivery_runtime_batch_audit(
                "ui_exit_code",
                format!("{context}: exit code unknown; omitting exit code suffix"),
            );
            String::new()
        },
        |c| format!(" (exit code: {c})"),
    )
}

/// Optional metadata extension (empty when missing; audit only in strict
/// delivery).
#[must_use]
pub fn meta_extension_lowercase_or_empty(ext: Option<&str>, context: &str) -> String {
    ext.map_or_else(
        || {
            delivery_intent_batch_audit(
                "meta_extension",
                format!("{context}: missing source_extension; treating as empty"),
            );
            String::new()
        },
        str::to_lowercase,
    )
}

/// Optional metadata container label (empty when missing; audit only in strict
/// delivery).
#[must_use]
pub fn meta_container_lowercase_or_empty(container: Option<&str>, context: &str) -> String {
    container.map_or_else(
        || {
            delivery_intent_batch_audit(
                "meta_container",
                format!("{context}: missing container; treating as empty"),
            );
            String::new()
        },
        str::to_lowercase,
    )
}

/// Trace/display label with an expected default (no delivery audit).
#[must_use]
pub fn trace_label_or_default<'a>(label: Option<&'a str>, default: &'static str) -> &'a str {
    label.filter(|value| !value.is_empty()).unwrap_or(default)
}

/// First segment of a dotted filename stem (`IMG.1234.CR2` → `IMG`).
#[must_use]
pub fn path_stem_root_segment(stem: &str) -> &str {
    stem.split('.').next().unwrap_or(stem)
}

/// Parent directory for metadata/XMP sidecar resolution (audited `.` when at
/// filesystem root).
#[must_use]
pub fn path_parent_or_dot(path: &Path) -> &Path {
    path.parent().unwrap_or_else(|| {
        delivery_metadata_batch_audit(
            "path_parent",
            format!(
                "{}: missing parent directory; using \".\" for path resolution",
                path.display()
            ),
        );
        Path::new(".")
    })
}

/// Gate-owned placeholder for audit calls that have no stable asset path.
#[must_use]
pub fn delivery_dot_path() -> &'static Path {
    Path::new(".")
}

/// File stem for lightweight probes (empty when missing; no delivery audit).
#[must_use]
pub fn path_file_stem_lossy_or_empty(path: &Path) -> String {
    path.file_stem()
        .and_then(|s| s.to_str())
        .map_or_else(String::new, str::to_string)
}

/// UTF-8 file stem required for strict UTF-8 consumers (errors instead of
/// silent fallback).
///
/// # Errors
/// Returns a delivery error message when stem is missing or non-UTF-8.
pub fn path_file_stem_utf8_or_delivery_err<'a>(
    path: &'a Path,
    context: &str,
) -> Result<&'a str, String> {
    if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
        Ok(stem)
    } else {
        let detail = if path.file_stem().is_some() {
            format!("{context}: non-UTF-8 file_stem ({})", path.display())
        } else {
            format!("{context}: missing file_stem ({})", path.display())
        };
        delivery_io_path_audit("path_file_stem_utf8", path, &detail);
        Err(detail)
    }
}

/// File stem for output path joins (`OsStr`; errors only when stem segment is
/// missing).
///
/// # Errors
/// Returns a delivery error message when stem is missing (non-UTF-8 stems
/// remain joinable).
pub fn path_file_stem_os_or_delivery_err<'a>(
    path: &'a Path,
    context: &str,
) -> Result<&'a std::ffi::OsStr, String> {
    if let Some(stem) = path.file_stem() {
        Ok(stem)
    } else {
        let detail = format!("{context}: missing file_stem ({})", path.display());
        delivery_io_path_audit("path_file_stem_os", path, &detail);
        Err(detail)
    }
}

/// File stem for metadata/XMP routing (lossy when non-UTF-8; empty + audit only
/// when missing).
#[must_use]
pub fn path_file_stem_or_empty(path: &Path, context: &str) -> String {
    if let Some(stem) = path.file_stem() {
        stem.to_string_lossy().into_owned()
    } else {
        delivery_metadata_batch_audit(
            "path_file_stem",
            format!("{context}: missing file_stem; treating as empty"),
        );
        String::new()
    }
}

/// UTF-8 `file_name` when present (`None` + audit when missing or non-UTF-8).
#[must_use]
pub fn path_file_name_utf8_or_none<'a>(path: &'a Path, context: &str) -> Option<&'a str> {
    if let Some(name) = path.file_name().and_then(|name| name.to_str()) {
        Some(name)
    } else {
        let detail = if path.file_name().is_some() {
            format!("{context}: non-UTF-8 file_name; skipping entry")
        } else {
            format!("{context}: missing file_name; skipping entry")
        };
        delivery_io_path_audit("path_file_name_utf8", path, detail);
        None
    }
}

/// File name for XMP/metadata matching (empty when missing; audit only in
/// strict delivery).
#[must_use]
pub fn path_file_name_or_empty(path: &Path, context: &str) -> String {
    path.file_name().and_then(|n| n.to_str()).map_or_else(
        || {
            delivery_metadata_batch_audit(
                "path_file_name",
                format!("{context}: missing file_name; treating as empty"),
            );
            String::new()
        },
        str::to_string,
    )
}

/// Dolby Vision profile-8 compat id when bitstream omits it (audited default).
#[must_use]
pub fn dv_profile8_compat_id_or_default(compat_id: Option<u8>) -> u8 {
    compat_id.unwrap_or_else(|| {
        delivery_strict_batch_audit(
            "dv_compat_id",
            "HDR AUDIT: missing DV profile-8 compat_id; using default compat id",
        );
        crate::constants::DV_PROFILE8_DEFAULT_COMPAT_ID
    })
}

/// Byte at `index` for JPEG/MPF probes (audited sentinel when out of range).
#[must_use]
pub fn probe_jpeg_byte_at(buffer: &[u8], index: usize, sentinel: u8, label: &str) -> u8 {
    buffer.get(index).copied().unwrap_or_else(|| {
        probe_image_format_batch_audit(
            "probe_jpeg_byte",
            format!("{label}: missing byte at index {index}; using sentinel {sentinel:02X}"),
        );
        sentinel
    })
}

/// Sub-slice for JPEG/MPF parsing (audited empty slice when range is invalid).
#[must_use]
pub fn probe_jpeg_buffer_slice<'a>(
    buffer: &'a [u8],
    range: std::ops::Range<usize>,
    context: &str,
) -> &'a [u8] {
    let range_end = range.end;
    let range_start = range.start;
    buffer.get(range).unwrap_or_else(|| {
        probe_image_format_batch_audit(
            "probe_jpeg_slice",
            format!(
                "{context}: slice {range_start}..{range_end} exceeds buffer len {}",
                buffer.len()
            ),
        );
        &[]
    })
}

/// Primary decode error when secondary bitstream recovery also fails (audited
/// generic fallback).
#[must_use]
pub fn probe_image_decode_failure_or_unknown(
    read_error: Option<crate::unified_error::ImgQualityError>,
    context: &str,
) -> crate::unified_error::ImgQualityError {
    read_error.unwrap_or_else(|| {
        probe_layer_batch_audit(
            "probe_image_decode_failure",
            format!("{context}: primary decode error missing; using generic ImageReadError"),
        );
        crate::unified_error::ImgQualityError::ImageReadError("Unknown decode failure".to_string())
    })
}

/// Rational scoring signal for image probes (`None` when non-finite).
#[must_use]
pub fn probe_rational_from_f64_optional(signal: f64, context: &str) -> Option<crate::Rational> {
    crate::Rational::from_f64(signal).or_else(|| {
        probe_layer_batch_audit(
            "probe_rational_f64",
            format!("{context}: non-finite rational signal; refusing fabricated 0/1"),
        );
        None
    })
}

/// Legacy rational helper (`Rational(0)` when absent; prefer
/// [`probe_rational_from_f64_optional`]).
#[must_use]
pub fn probe_rational_from_f64_or_zero(signal: f64, context: &str) -> crate::Rational {
    probe_rational_from_f64_optional(signal, context).unwrap_or_else(|| crate::Rational::from(0_u8))
}

/// Weighted JPEG quality after strict u8 cast (audited fallback to luma
/// estimate).
#[must_use]
pub fn jpeg_weighted_quality_or_luma(cast: Option<u8>, luma_quality: u8) -> u8 {
    cast.unwrap_or_else(|| {
        delivery_numeric_fallback_audit(
            "jpeg_weighted_quality",
            "NUMERIC AUDIT: JPEG weighted quality u8 cast failed; using luma estimate",
        );
        luma_quality
    })
}

/// End index of a leading numeric metric token in ffmpeg/libvmaf stderr (not a
/// delivery fallback).
#[must_use]
pub fn explore_metric_numeric_end(value: &str, allow_sign: bool) -> usize {
    value
        .find(|c: char| {
            if allow_sign {
                !c.is_ascii_digit() && c != '.' && c != '-'
            } else {
                !c.is_ascii_digit() && c != '.'
            }
        })
        .unwrap_or(value.len())
}

/// Buffer prefix slice for HDR/ICC scans (audited empty when `end` exceeds
/// length).
#[must_use]
pub fn probe_buffer_prefix_or_empty<'a>(buffer: &'a [u8], end: usize, context: &str) -> &'a [u8] {
    buffer.get(..end).unwrap_or_else(|| {
        hdr_metadata_fallback_audit(
            "probe_buffer_prefix",
            Path::new("."),
            format!(
                "{context}: prefix end {end} exceeds buffer len {}",
                buffer.len()
            ),
        );
        &[]
    })
}

/// x265 params text segment when merging encoder flags (`None` → empty, no
/// audit).
// Keep conversion fallback shims explicit; do not collapse to `unwrap_or_default`.
#[allow(clippy::manual_unwrap_or_default)]
#[must_use]
pub fn x265_params_segment_or_empty(params: Option<&str>) -> &str {
    match params.filter(|s| !s.is_empty()) {
        Some(value) => value,
        None => "",
    }
}

/// Explore GPU coarse-search CRF anchor when `best_crf` is unset during
/// backtrack.
#[must_use]
pub fn explore_best_crf_or_backtrack_anchor(
    best_crf: Option<f32>,
    test_crf: f32,
    old_step: f32,
) -> f32 {
    best_crf.unwrap_or_else(|| {
        explore_gpu_coarse_fallback_audit(
            "best_crf",
            format!(
                "GPU explore backtrack: best_crf missing; using test_crf+step \
                 ({test_crf:.2}+{old_step:.2})"
            ),
        );
        test_crf + old_step
    })
}

/// Temp/backup file extension label (audited `tmp` when output has no
/// extension).
#[must_use]
pub fn backup_extension_label_or_tmp(output: &Path) -> String {
    output.extension().and_then(|ext| ext.to_str()).map_or_else(
        || {
            explore_gpu_coarse_fallback_audit(
                "backup_extension",
                format!(
                    "{}: missing output extension for backup path; using tmp",
                    output.display()
                ),
            );
            String::from("tmp")
        },
        str::to_string,
    )
}

/// First whitespace-delimited token from ffmpeg/statistics stdout (audited when
/// empty).
#[must_use]
pub fn probe_stdout_first_token<'a>(segment: &'a str, context: &str) -> &'a str {
    segment.split_whitespace().next().unwrap_or_else(|| {
        probe_layer_batch_audit(
            "stdout_token",
            format!("{context}: no whitespace token in probe output segment"),
        );
        ""
    })
}

/// Still-image `FFmpeg` pipe RGB `pix_fmt` routed through
/// [`crate::media_precision::ImagePrecisionProfile`] (M67).
#[must_use]
pub const fn precision_still_pipe_rgb_pix_fmt(
    profile: &crate::media_precision::ImagePrecisionProfile,
) -> crate::ffmpeg_builder::PixFmt {
    profile.still_pipe_rgb_pix_fmt()
}

/// PNG16 preservation decode RGB `pix_fmt` name (excludes float to misleading
/// 48-bit PNG pipe) (M67).
#[must_use]
pub const fn precision_png16_decode_rgb_pix_fmt(
    profile: &crate::media_precision::ImagePrecisionProfile,
) -> &'static str {
    profile.png16_decode_rgb_pix_fmt_name()
}

/// Animated GIF/WebP color richness in `[0, 1]` from palette size or entropy
/// fallback (M67).
#[must_use]
pub fn probe_animated_color_richness_unit_interval(
    palette_size: Option<u16>,
    reference_entropy: Option<f64>,
) -> f64 {
    let raw = match palette_size {
        Some(size) => f64::from(size) / 256.0,
        None => match reference_entropy.filter(|e| e.is_finite()) {
            Some(e) => e / 8.0,
            None => f64::NAN,
        },
    };
    if raw.is_finite() {
        raw.clamp(0.0, 1.0)
    } else {
        f64::NAN
    }
}

/// Optional finite probe feature: missing/non-finite → `NaN` (audit only; not a
/// measured value).
// Keep the audited absent marker explicit instead of using `unwrap_or`.
#[allow(clippy::manual_unwrap_or)]
#[must_use]
pub fn probe_optional_f64_or_zero(value: Option<f64>, context: &str) -> f64 {
    match value.filter(|v| v.is_finite()) {
        Some(v) => v,
        None => {
            probe_quality_batch_audit(
                "probe_feature_f64",
                format!("{context}: missing or non-finite feature; using NaN absent marker"),
            );
            f64::NAN
        }
    }
}

/// Optional quality-embedding scalar: missing/non-finite → `NaN` (never `0.0`
/// as fake measurement).
///
/// Gate audit helpers do **not** legitimize fabrication; callers must not treat
/// `NaN` as measured.
// Keep the absent marker explicit: quality embeddings must not use `unwrap_or(default)`.
#[allow(clippy::manual_unwrap_or)]
#[must_use]
pub fn quality_embedding_optional_f64_or_zero(value: Option<f64>) -> f64 {
    match value.filter(|v| v.is_finite()) {
        Some(v) => v,
        None => f64::NAN,
    }
}

/// Unit-interval embed slot from optional scalar (`NaN` when absent; no probe
/// audit).
#[must_use]
pub fn quality_embedding_optional_unit_interval_f32(value: Option<f64>) -> f32 {
    let scaled = quality_embedding_optional_f64_or_zero(value);
    if scaled.is_finite() {
        crate::numeric_cast::f64_to_f32_lossy(scaled.clamp(0.0, 1.0))
    } else {
        f32::NAN
    }
}

/// CPU calibration encode `pix_fmt` when ffprobe probe is present (`None` if
/// absent).
#[must_use]
pub fn explore_calibration_pix_fmt_optional(
    probe: Option<&crate::ffprobe::FFprobeResult>,
) -> Option<&'static str> {
    let p = probe?;
    Some(crate::hevc_yuv420_output_pix_fmt(p))
}

/// Legacy symbol (panics when probe absent; prefer
/// [`explore_calibration_pix_fmt_optional`]).
#[must_use]
pub fn explore_calibration_pix_fmt_or_default(
    probe: Option<&crate::ffprobe::FFprobeResult>,
) -> &'static str {
    explore_calibration_pix_fmt_optional(probe).unwrap_or_else(|| {
        gate_legacy_optional_misuse(
            "explore_calibration_pix_fmt",
            "ffprobe absent; use explore_calibration_pix_fmt_optional",
        );
    })
}

/// Measured ffprobe duration for calibration (`None` if absent; never
/// substitutes sample window).
#[must_use]
pub fn explore_calibration_duration_optional(probe_duration: Option<f64>) -> Option<f64> {
    match probe_duration.filter(|d| d.is_finite() && *d > 0.0_f64) {
        Some(d) => Some(d),
        None => {
            explore_precheck_batch_audit(
                "explore_calibration_duration",
                "ffprobe duration absent; refusing forged calibration window duration",
            );
            None
        }
    }
}

/// Legacy name (panics when duration absent; prefer
/// [`explore_calibration_duration_optional`]).
#[must_use]
pub fn explore_calibration_duration_or_sample(
    probe_duration: Option<f64>,
    sample_duration: f32,
) -> f64 {
    let _ = sample_duration;
    explore_calibration_duration_optional(probe_duration).unwrap_or_else(|| {
        gate_legacy_optional_misuse(
            "explore_calibration_duration",
            "ffprobe duration absent; use explore_calibration_duration_optional",
        );
    })
}

/// Dynamic GPU→CPU calibration failure (probe/encode/read): audit only under
/// strict (M81).
pub fn explore_calibration_degraded_audit(detail: impl AsRef<str>) {
    explore_precheck_batch_audit("explore_calibration_audit", detail);
}

/// Calibration temp encode output size when metadata read succeeds.
#[must_use]
pub fn explore_calibration_probe_size_optional(path: &Path, label: &str) -> Option<u64> {
    match std::fs::metadata(path) {
        Ok(metadata) => Some(metadata.len()),
        Err(err) => {
            explore_calibration_degraded_audit(format!(
                "Failed to read {label} (path={}): {err}",
                path.display()
            ));
            None
        }
    }
}

/// Legacy calibration size helper (panics when absent; prefer
/// [`explore_calibration_probe_size_optional`]).
#[must_use]
pub fn explore_calibration_probe_size_or_zero(path: &Path, label: &str) -> u64 {
    explore_calibration_probe_size_optional(path, label).unwrap_or_else(|| {
        panic!("explore calibration size required for {label} after gate audit");
    })
}

/// GPU coarse-search sample window from measured ffprobe duration (`None` if
/// absent).
#[must_use]
pub fn explore_gpu_sample_duration_optional(duration: Option<f64>, context: &str) -> Option<f32> {
    match duration.filter(|d| d.is_finite() && *d > 0.0_f64) {
        Some(d) => Some(crate::numeric_cast::f64_to_f32_lossy(d)),
        None => {
            explore_gpu_coarse_fallback_audit(
                "explore_gpu_sample_window",
                format!("{context}: ffprobe duration absent; refusing forged GPU sample window"),
            );
            None
        }
    }
}

/// Legacy symbol (panics when absent; prefer
/// [`explore_gpu_sample_duration_optional`]).
#[must_use]
pub fn explore_gpu_sample_duration_or_default(duration: Option<f64>) -> f32 {
    explore_gpu_sample_duration_optional(duration, "explore_gpu_sample_duration_or_default")
        .unwrap_or_else(|| {
            gate_legacy_optional_misuse(
                "explore_gpu_sample_window",
                "ffprobe duration absent; use explore_gpu_sample_duration_optional",
            );
        })
}

/// Adaptive VMAF-Y floor from search baseline (`None` when baseline absent).
#[must_use]
pub fn explore_adaptive_vmaf_y_floor_optional(search_baseline: Option<f64>) -> Option<f64> {
    let baseline = search_baseline?;
    Some(
        (baseline - crate::constants::EXPLORATION_VMAF_ALLOWED_DROP)
            .max(crate::constants::EXPLORATION_VMAF_Y_SANITY_FLOOR),
    )
}

/// Legacy symbol (panics when baseline absent; prefer
/// [`explore_adaptive_vmaf_y_floor_optional`]).
#[must_use]
pub fn explore_adaptive_vmaf_y_floor(search_baseline: Option<f64>) -> f64 {
    explore_adaptive_vmaf_y_floor_optional(search_baseline).unwrap_or_else(|| {
        gate_legacy_optional_misuse(
            "explore_adaptive_vmaf_y_floor",
            "search baseline absent; use explore_adaptive_vmaf_y_floor_optional",
        );
    })
}

/// Adaptive PSNR U/V floors from search baseline (`None` when baseline absent).
#[must_use]
pub fn explore_adaptive_psnr_uv_floor_optional(
    search_baseline: Option<(f64, f64)>,
) -> Option<(f64, f64)> {
    let (u, v) = search_baseline?;
    Some((
        (u - crate::constants::EXPLORATION_PSNR_ALLOWED_DROP)
            .max(crate::constants::EXPLORATION_PSNR_UV_SANITY_FLOOR),
        (v - crate::constants::EXPLORATION_PSNR_ALLOWED_DROP)
            .max(crate::constants::EXPLORATION_PSNR_UV_SANITY_FLOOR),
    ))
}

/// Legacy symbol (panics when baseline absent; prefer
/// [`explore_adaptive_psnr_uv_floor_optional`]).
#[must_use]
pub fn explore_adaptive_psnr_uv_floor(search_baseline: Option<(f64, f64)>) -> (f64, f64) {
    explore_adaptive_psnr_uv_floor_optional(search_baseline).unwrap_or_else(|| {
        gate_legacy_optional_misuse(
            "explore_adaptive_psnr_uv_floor",
            "search baseline absent; use explore_adaptive_psnr_uv_floor_optional",
        );
    })
}

/// Parse positive finite ffprobe duration text; return `None` on failure
/// (caller audits if needed).
#[must_use]
pub fn probe_ffprobe_duration_text_or_none(raw: &str, _context: &str) -> Option<f64> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    match trimmed.parse::<f64>() {
        Ok(v) if v.is_finite() && v > 0.0_f64 => Some(v),
        _ => None,
    }
}

/// Chroma subsampling factor for CRF heuristics when `pix_fmt` is known (`None`
/// if absent).
#[must_use]
pub fn probe_chroma_factor_optional(pix_fmt: Option<&str>, context: &str) -> Option<f64> {
    let Some(fmt) = pix_fmt else {
        probe_quality_batch_audit(
            "probe_chroma_factor",
            format!("{context}: missing pix_fmt; refusing forged chroma factor"),
        );
        return None;
    };
    let fmt_lower = fmt.to_lowercase();
    Some(if fmt_lower.contains("444") {
        crate::constants::CHROMA_FACTOR_YUV444
    } else if fmt_lower.contains("422") {
        crate::constants::CHROMA_FACTOR_YUV422
    } else if fmt_lower.contains("rgb") || fmt_lower.contains("gbr") {
        crate::constants::CHROMA_FACTOR_RGB
    } else {
        crate::constants::CHROMA_FACTOR_YUV420
    })
}

/// Legacy name (panics when `pix_fmt` absent; prefer
/// [`probe_chroma_factor_optional`]).
#[must_use]
pub fn probe_chroma_factor_or_default(pix_fmt: Option<&str>, context: &str) -> f64 {
    probe_chroma_factor_optional(pix_fmt, context).unwrap_or_else(|| {
        gate_legacy_optional_misuse(
            "probe_chroma_factor",
            format!("{context}: pix_fmt absent; use probe_chroma_factor_optional"),
        );
    })
}

/// Compression ratio for GPU quality scoring when `input_size` may be zero
/// (refuses fabricated 1.0).
#[must_use]
pub fn gpu_quality_compression_ratio_or_neutral(
    output_size: u64,
    input_size: u64,
    context: &str,
) -> f64 {
    if input_size == 0 {
        delivery_gpu_batch_audit(
            "gpu_quality_compression_ratio",
            format!(
                "{context}: input_size is 0 during quality score calculation; refusing fabricated \
                 compression ratio 1.0"
            ),
        );
        return f64::NAN;
    }
    crate::numeric_cast::u64_to_f64(output_size) / crate::numeric_cast::u64_to_f64(input_size)
}

/// GPU coarse boundary CRF: quality ceiling when detected, else search
/// `best_crf` when boundary found.
#[must_use]
pub fn explore_gpu_boundary_crf_from_search_optional(
    found_boundary: bool,
    best_crf: Option<f32>,
    quality_ceiling_crf: Option<f32>,
    context: &str,
) -> Option<f32> {
    if let Some(ceiling) = quality_ceiling_crf {
        return Some(ceiling);
    }
    if found_boundary {
        return best_crf;
    }
    delivery_gpu_batch_audit(
        "gpu_boundary_crf",
        format!("{context}: no quality ceiling and no search boundary; refusing forged CRF"),
    );
    None
}

/// Legacy symbol (panics when boundary absent; prefer
/// [`explore_gpu_boundary_crf_from_search_optional`]).
#[must_use]
pub fn explore_gpu_quality_ceiling_crf_or_last_tested(
    quality_ceiling_crf: Option<f32>,
    last_tested_crf: f32,
    context: &str,
) -> f32 {
    let _ = last_tested_crf;
    let resolved = explore_gpu_boundary_crf_from_search_optional(
        true,
        Some(last_tested_crf),
        quality_ceiling_crf,
        context,
    );
    if resolved.is_none() {
        delivery_gpu_batch_audit(
            "gpu_boundary_crf",
            format!("{context}: legacy ceiling CRF required but absent"),
        );
    }
    resolved.unwrap_or_else(|| {
        gate_legacy_optional_misuse(
            "gpu_boundary_crf",
            format!("{context}: GPU boundary CRF required"),
        );
    })
}

/// Phase-1A GPU search size when `best_size` is present.
#[must_use]
pub fn delivery_gpu_phase_best_size_optional(best_size: Option<u64>, context: &str) -> Option<u64> {
    if best_size.is_some() {
        best_size
    } else {
        delivery_gpu_batch_audit(
            "delivery_gpu",
            format!("{context}: best_size missing in Phase 1A; refusing fabricated size 0"),
        );
        None
    }
}

/// Phase-1A GPU search size when `best_size` must exist (logic-error path;
/// never fabricates `0`).
#[must_use]
pub fn delivery_gpu_phase_best_size_required(best_size: Option<u64>, context: &str) -> u64 {
    delivery_gpu_phase_best_size_optional(best_size, context).unwrap_or_else(|| {
        panic!("{context}: best_size required but absent after gate audit");
    })
}

/// Legacy Phase-1A size helper (delegates to
/// [`delivery_gpu_phase_best_size_required`]; no silent `0`).
#[must_use]
pub fn delivery_gpu_phase_best_size_or_zero(best_size: Option<u64>, context: &str) -> u64 {
    delivery_gpu_phase_best_size_required(best_size, context)
}

/// Binary-search CRF probe from `mid` (audited u16 fallback via `hi` when `mid`
/// overflows).
#[must_use]
pub fn delivery_gpu_binary_search_crf_from_mid(mid: i32, hi: i32) -> f32 {
    let mid_u16 = u16::try_from(mid.max(0_i32)).unwrap_or_else(|_| {
        delivery_gpu_batch_audit(
            "delivery_gpu",
            format!(
                "NUMERIC AUDIT: Binary search mid {mid} overflows u16 | Forensic: falling back to \
                 hi {hi} (Search integrity maintained)"
            ),
        );
        u16::try_from(hi.max(0_i32)).unwrap_or_else(|_| {
            delivery_gpu_batch_audit(
                "delivery_gpu",
                format!(
                    "NUMERIC AUDIT: Binary search hi {hi} also overflows u16 | FATAL: \
                     clamping to u16::MAX"
                ),
            );
            // Clamp to u16::MAX instead of silent forgery - this path should never
            // happen in real GPU CRF search (range 0-51), but explicit audit prevents
            // silent numeric corruption
            u16::MAX
        })
    });
    f32::from(mid_u16)
}

/// Legacy symbol (panics on calibration failure; call `quick_calibrate` and
/// propagate `Err`).
#[must_use]
pub fn explore_quick_calibrate_mapper_or_default(
    calibrated: Result<
        crate::video_explorer::dynamic_mapping::DynamicCrfMapper,
        impl std::fmt::Display,
    >,
    input: &Path,
    input_size: u64,
    context: &str,
) -> crate::video_explorer::dynamic_mapping::DynamicCrfMapper {
    let _ = (input, input_size, context);
    calibrated.unwrap_or_else(|err| {
        gate_legacy_optional_misuse(
            "explore_quick_calibrate",
            format!("quick_calibrate failed ({err}); propagate Err instead of mapper_or_default"),
        );
    })
}

/// Encode size improvement % when a prior best exists (`None` for first
/// sample).
#[must_use]
pub fn explore_encode_size_improvement_pct_optional(
    best_size: Option<u64>,
    new_size: u64,
    context: &str,
) -> Option<f64> {
    let best = best_size?;
    if best == 0 {
        delivery_gpu_batch_audit(
            "explore_encode_improvement",
            format!("{context}: prior best_size is 0; refusing forged improvement ratio"),
        );
        return None;
    }
    Some(
        (crate::numeric_cast::u64_to_f64(best) - crate::numeric_cast::u64_to_f64(new_size))
            / crate::numeric_cast::u64_to_f64(best)
            * 100.0_f64,
    )
}

/// Legacy name (panics when no prior best; prefer
/// [`explore_encode_size_improvement_pct_optional`]).
#[must_use]
pub fn explore_encode_size_improvement_pct(
    best_size: Option<u64>,
    new_size: u64,
    context: &str,
) -> f64 {
    explore_encode_size_improvement_pct_optional(best_size, new_size, context).unwrap_or_else(
        || {
            gate_legacy_optional_misuse(
                "explore_encode_improvement",
                format!(
                    "{context}: no prior best_size; use \
                     explore_encode_size_improvement_pct_optional"
                ),
            );
        },
    )
}

/// Ultimate-mode VMAF gate sample rate from measured duration hint (`None` if
/// absent).
#[must_use]
pub fn explore_ultimate_gate_sample_rate_optional(
    duration_hint: Option<f64>,
    _context: &str,
) -> Option<usize> {
    let duration_secs = duration_hint.filter(|d| d.is_finite() && *d > 0.0)?;
    let duration_min = duration_secs / 60.0_f64;
    Some(if duration_min <= 1.0 { 1 } else { 3 })
}

/// Legacy symbol (panics when duration hint absent; prefer
/// [`explore_ultimate_gate_sample_rate_optional`]).
#[must_use]
pub fn explore_ultimate_gate_sample_rate(duration_hint: Option<f64>, context: &str) -> usize {
    explore_ultimate_gate_sample_rate_optional(duration_hint, context).unwrap_or_else(|| {
        explore_gpu_coarse_batch_audit(
            "ultimate_gate_sample_rate",
            format!("{context}: missing duration hint; refusing forged full-frame sample rate 1"),
        );
        gate_legacy_optional_misuse(
            "ultimate_gate_sample_rate",
            format!("{context}: duration hint required"),
        );
    })
}

/// Latest encoded size from explore history (`None` when history empty).
#[must_use]
pub fn explore_latest_encoded_size_optional(last_size: Option<u64>, context: &str) -> Option<u64> {
    if last_size.is_some() {
        last_size
    } else {
        explore_precheck_batch_audit(
            "explore_latest_size",
            format!("{context}: empty size_history; refusing fabricated size 0"),
        );
        None
    }
}

/// Legacy latest-size helper (panics when absent; prefer
/// `explore_latest_encoded_size_optional`).
#[must_use]
pub fn explore_latest_encoded_size_or_zero(last_size: Option<u64>, context: &str) -> u64 {
    explore_latest_encoded_size_optional(last_size, context).unwrap_or_else(|| {
        panic!("{context}: encoded size required but history empty after gate audit");
    })
}

/// Explore audit elapsed seconds when start instant is missing (`None`, not
/// fake `0.0s`).
#[must_use]
pub fn explore_elapsed_secs_optional(elapsed: Option<Duration>, context: &str) -> Option<f64> {
    elapsed
        .map(|d| d.as_secs_f64())
        .filter(|secs| secs.is_finite())
        .or_else(|| {
            explore_precheck_batch_audit(
                "explore_elapsed",
                format!("{context}: missing start_time; refusing fabricated 0.0s elapsed"),
            );
            None
        })
}

/// Legacy explore elapsed helper (`NaN` when unset; prefer
/// `explore_elapsed_secs_optional` in UI).
// Keep the absent marker explicit: elapsed probes must not use `unwrap_or(default)`.
#[allow(clippy::manual_unwrap_or)]
#[must_use]
pub fn explore_elapsed_secs_or_zero(elapsed: Option<Duration>, context: &str) -> f64 {
    match explore_elapsed_secs_optional(elapsed, context) {
        Some(v) => v,
        None => f64::NAN,
    }
}

/// Dynamic GPU→CPU mapping offset from anchor size ratio (`NaN` when anchor
/// missing).
#[must_use]
pub fn explore_dynamic_mapping_offset_or_zero(
    anchor_size_ratio: Option<f64>,
    context: &str,
) -> f32 {
    anchor_size_ratio.filter(|r| r.is_finite()).map_or_else(
        || {
            explore_gpu_coarse_batch_audit(
                "dynamic_mapping_offset",
                format!("{context}: missing anchor size_ratio; refusing fabricated offset 0.0"),
            );
            f32::NAN
        },
        |ratio| {
            use crate::constants::{
                DYNAMIC_MAPPING_OFFSET_DEFAULT, DYNAMIC_MAPPING_OFFSET_TIER_1,
                DYNAMIC_MAPPING_OFFSET_TIER_2, DYNAMIC_MAPPING_OFFSET_TIER_3,
                DYNAMIC_MAPPING_RATIO_TIER_1, DYNAMIC_MAPPING_RATIO_TIER_2,
                DYNAMIC_MAPPING_RATIO_TIER_3,
            };
            if ratio >= 1.0 {
                0.0
            } else if ratio < DYNAMIC_MAPPING_RATIO_TIER_1 {
                DYNAMIC_MAPPING_OFFSET_TIER_1
            } else if ratio < DYNAMIC_MAPPING_RATIO_TIER_2 {
                DYNAMIC_MAPPING_OFFSET_TIER_2
            } else if ratio < DYNAMIC_MAPPING_RATIO_TIER_3 {
                DYNAMIC_MAPPING_OFFSET_TIER_3
            } else {
                DYNAMIC_MAPPING_OFFSET_DEFAULT
            }
        },
    )
}

/// JXL screening telemetry when a best candidate exists (`None` when absent).
#[must_use]
pub fn jxl_best_telemetry_optional(
    best_distance: Option<f32>,
    best_output_size: Option<u64>,
    context: &str,
) -> Option<(f32, u64)> {
    match (best_distance.filter(|d| d.is_finite()), best_output_size) {
        (Some(dist), Some(size)) => Some((dist, size)),
        _ => {
            delivery_jxl_batch_audit(
                "jxl_best_telemetry",
                format!(
                    "{context}: missing best candidate; refusing fabricated (0.0, 0) telemetry"
                ),
            );
            None
        }
    }
}

/// Legacy JXL telemetry helper (panics when absent; prefer
/// `jxl_best_telemetry_optional`).
#[must_use]
pub fn jxl_best_telemetry_or_zero(
    best_distance: Option<f32>,
    best_output_size: Option<u64>,
    context: &str,
) -> (f32, u64) {
    jxl_best_telemetry_optional(best_distance, best_output_size, context).unwrap_or_else(|| {
        panic!(
            "jxl_best_telemetry_or_zero is legacy — use jxl_best_telemetry_optional ({context})"
        );
    })
}

/// Runtime elapsed seconds when start instant was recorded (`None` when timer
/// never armed).
#[must_use]
pub fn runtime_elapsed_secs_optional(elapsed: Option<Duration>, context: &str) -> Option<u64> {
    elapsed.map(|d| d.as_secs()).or_else(|| {
        delivery_runtime_batch_audit(
            "runtime_elapsed",
            format!("{context}: start instant missing; refusing fabricated 0s elapsed"),
        );
        None
    })
}

/// Legacy runtime elapsed helper (`0` when unset; Ctrl-C watcher only — not a
/// quality metric).
#[must_use]
pub fn runtime_elapsed_secs_or_zero(elapsed: Option<Duration>, context: &str) -> u64 {
    runtime_elapsed_secs_optional(elapsed, context).unwrap_or_else(|| {
        delivery_runtime_batch_audit(
            "runtime_elapsed",
            format!(
                "{context}: start instant missing; using 0s (non-measurement interrupt path only)"
            ),
        );
        0
    })
}

/// GIF global color table byte size when palette entry count is known.
#[must_use]
pub fn gif_palette_byte_size_optional(palette_colors: Option<u32>, context: &str) -> Option<usize> {
    palette_colors
        .and_then(|colors| {
            let entries = crate::numeric_cast::u64_to_usize_strict(
                u64::from(colors),
                "gif_palette_color_count",
            )?;
            entries.checked_mul(3)
        })
        .or_else(|| {
            delivery_runtime_batch_audit(
                "gif_palette_size",
                format!("{context}: missing palette color count; refusing fabricated 0-byte GCT"),
            );
            None
        })
}

/// Legacy GIF GCT size helper (panics when absent; prefer
/// [`gif_palette_byte_size_optional`]).
#[must_use]
pub fn gif_palette_byte_size_or_zero(palette_colors: Option<u32>, context: &str) -> usize {
    gif_palette_byte_size_optional(palette_colors, context).unwrap_or_else(|| {
        panic!(
            "gif_palette_byte_size_or_zero is legacy — use gif_palette_byte_size_optional \
             ({context})"
        );
    })
}

/// GPU→CPU CRF center adjustment when compression potential is measured.
#[must_use]
pub fn gpu_compression_potential_adjustment_optional(
    compression_potential: Option<f64>,
    context: &str,
) -> Option<f32> {
    compression_potential
        .map(|potential| {
            if potential < 0.3_f64 {
                0.3_f32
            } else if potential > 0.7_f64 {
                -0.2_f32
            } else {
                0.0_f32
            }
        })
        .or_else(|| {
            delivery_gpu_batch_audit(
                "gpu_compression_potential",
                format!(
                    "{context}: missing compression potential; refusing fabricated adjustment 0.0"
                ),
            );
            None
        })
}

/// Legacy GPU compression-potential helper (panics when absent; prefer optional
/// + skip addend).
#[must_use]
pub fn gpu_compression_potential_adjustment_or_zero(
    compression_potential: Option<f64>,
    context: &str,
) -> f32 {
    gpu_compression_potential_adjustment_optional(compression_potential, context).unwrap_or_else(
        || {
            panic!(
                "gpu_compression_potential_adjustment_or_zero is legacy — use optional ({context})"
            );
        },
    )
}

/// Explore-progress optional `f32` when mutex is poisoned (treat as unset).
#[must_use]
pub fn progress_explore_optional_f32_or_none(
    lock: Result<
        std::sync::MutexGuard<'_, Option<f32>>,
        std::sync::PoisonError<std::sync::MutexGuard<'_, Option<f32>>>,
    >,
    context: &str,
) -> Option<f32> {
    lock.map_or_else(
        |_| {
            delivery_progress_batch_audit(
                "explore_progress_optional_f32_mutex",
                format!("{context}: optional f32 mutex poisoned; treating as unset"),
            );
            None
        },
        |guard| *guard,
    )
}

/// Explore-progress optional `u64` when mutex is poisoned (treat as unset).
#[must_use]
pub fn progress_explore_optional_u64_or_none(
    lock: Result<
        std::sync::MutexGuard<'_, Option<u64>>,
        std::sync::PoisonError<std::sync::MutexGuard<'_, Option<u64>>>,
    >,
    context: &str,
) -> Option<u64> {
    lock.map_or_else(
        |_| {
            delivery_progress_batch_audit(
                "explore_progress_optional_u64_mutex",
                format!("{context}: optional u64 mutex poisoned; treating as unset"),
            );
            None
        },
        |guard| *guard,
    )
}

/// Explore progress CRF (unset until first probe; poison → `None`, never fake
/// `0.0`).
#[must_use]
pub fn progress_explore_crf_or_zero(
    lock: Result<
        std::sync::MutexGuard<'_, Option<f32>>,
        std::sync::PoisonError<std::sync::MutexGuard<'_, Option<f32>>>,
    >,
    context: &str,
) -> Option<f32> {
    progress_explore_optional_f32_or_none(lock, context)
}

/// Explore progress encoded size (unset until first encode; poison → `None`).
#[must_use]
pub fn progress_explore_size_or_zero(
    lock: Result<
        std::sync::MutexGuard<'_, Option<u64>>,
        std::sync::PoisonError<std::sync::MutexGuard<'_, Option<u64>>>,
    >,
    context: &str,
) -> Option<u64> {
    progress_explore_optional_u64_or_none(lock, context)
}

/// Explore progress best SSIM (unset until first measurement; poison → `None`,
/// never fake `0.0`).
#[must_use]
pub fn progress_explore_ssim_or_zero(
    lock: Result<
        std::sync::MutexGuard<'_, Option<f64>>,
        std::sync::PoisonError<std::sync::MutexGuard<'_, Option<f64>>>,
    >,
    context: &str,
) -> Option<f64> {
    progress_explore_optional_f64_or_none(lock, context)
}

/// Explore-progress optional metric when mutex is poisoned (treat as unset;
/// M46).
#[must_use]
pub fn progress_explore_optional_f64_or_none(
    lock: Result<
        std::sync::MutexGuard<'_, Option<f64>>,
        std::sync::PoisonError<std::sync::MutexGuard<'_, Option<f64>>>,
    >,
    context: &str,
) -> Option<f64> {
    lock.map_or_else(
        |_| {
            delivery_progress_batch_audit(
                "explore_progress_optional_mutex",
                format!("{context}: optional metric mutex poisoned; treating as unset"),
            );
            None
        },
        |guard| *guard,
    )
}

/// Delivery log detail when the source path is missing (M46).
///
/// For global operations (e.g., database connections) without a specific file
/// context, returns the reason directly without warning - this is expected
/// behavior. For file-specific operations, includes the path in the output.
#[must_use]
pub fn delivery_log_detail_with_optional_path(path: Option<&Path>, reason: &str) -> String {
    path.map_or_else(
        || reason.to_string(), // Global operation without path - expected, no warning
        |p| format!("{} — {}", p.display(), reason),
    )
}

/// Inline SSIM label for progress bars (`N/A` when unset; audited in strict
/// delivery).
#[must_use]
pub fn ui_ssim_inline_or_na(value: Option<f64>) -> String {
    value.filter(|v| v.is_finite()).map_or_else(
        || {
            delivery_runtime_batch_audit(
                "ui_ssim_na",
                "explore progress SSIM unset or non-finite; displaying N/A",
            );
            String::from("N/A")
        },
        |s| format!("SSIM {s:.4}"),
    )
}

/// Finite f64 for status lines or a placeholder (audited in strict delivery;
/// M46).
#[must_use]
pub fn ui_f64_display_or_placeholder(
    value: Option<f64>,
    placeholder: &'static str,
    label: &'static str,
) -> String {
    ui_optional_f64_display_or_map(value, placeholder, label, |v| format!("{v:.4}"))
}

/// Optional f64 with custom finite formatting or a placeholder (audited in
/// strict delivery).
#[must_use]
pub fn ui_optional_f64_display_or_map(
    value: Option<f64>,
    placeholder: &'static str,
    label: &'static str,
    format_finite: impl FnOnce(f64) -> String,
) -> String {
    value.filter(|v| v.is_finite()).map_or_else(
        || {
            delivery_runtime_batch_audit(
                "ui_metric_placeholder",
                format!("{label}: using placeholder \"{placeholder}\""),
            );
            placeholder.to_string()
        },
        format_finite,
    )
}

/// Explore metric `N/A` display with configurable precision (M47).
#[must_use]
pub fn ui_f64_or_na(value: Option<f64>, label: &'static str, precision: u8) -> String {
    ui_optional_f64_display_or_map(value, "N/A", label, |v| {
        if precision == 1 {
            format!("{v:.1}")
        } else if precision == 2 {
            format!("{v:.2}")
        } else if precision == 3 {
            format!("{v:.3}")
        } else if precision == 6 {
            format!("{v:.6}")
        } else {
            format!("{v:.4}")
        }
    })
}

/// Optional integer metric as decimal string or `N/A` (M48).
#[must_use]
pub fn ui_optional_u32_or_na(value: Option<u32>, label: &'static str) -> String {
    value.map_or_else(
        || {
            delivery_runtime_batch_audit(
                "ui_metric_u32_na",
                format!("{label}: u32 unset; displaying N/A"),
            );
            String::from("N/A")
        },
        |v| v.to_string(),
    )
}

/// Optional u64 metric as decimal string or `N/A` (M48).
#[must_use]
pub fn ui_optional_u64_or_na(value: Option<u64>, label: &'static str) -> String {
    value.map_or_else(
        || {
            delivery_runtime_batch_audit(
                "ui_metric_u64_na",
                format!("{label}: u64 unset; displaying N/A"),
            );
            String::from("N/A")
        },
        |v| v.to_string(),
    )
}

/// Optional f64 percent display (`12.3%`) or `N/A` (M48).
#[must_use]
pub fn ui_f64_percent_or_na(value: Option<f64>, label: &'static str) -> String {
    ui_optional_f64_display_or_map(value, "N/A", label, |v| format!("{:.1}%", v * 100.0))
}

/// Duration delta label (`1.23s`) or `N/A` (M48).
#[must_use]
pub fn ui_duration_secs_label_or_na(value: Option<f64>, label: &'static str) -> String {
    ui_optional_f64_display_or_map(value, "N/A", label, |d| format!("{d:.2}s"))
}

/// Duration label for logs (`12.3s`) or audited `Unknown` when unset (U12 / DB
/// layer).
#[must_use]
pub fn ui_duration_secs_label_or_unknown(value: Option<f64>, label: &'static str) -> String {
    ui_optional_f64_display_or_map(value, "Unknown", label, |d| format!("{d:.1}s"))
}

/// Optional `u32` for penetration stderr (`42` / `unknown`) (M50 / U13).
#[must_use]
pub fn ui_optional_u32_display_or_unknown(value: Option<u32>) -> String {
    value.map_or_else(|| "unknown".to_string(), |v| v.to_string())
}

/// `ImageMagick` identify recovered animation duration (M50).
pub fn probe_imagemagick_animation_detected_audit(
    path: &Path,
    frame_count: u64,
    duration_secs: f64,
) {
    probe_layer_audit(
        "imagemagick_animation_duration",
        path,
        format!(
            "[Duration Fallback] ImageMagick animation detected: {frame_count} frames, \
             {duration_secs:.2}s ({})",
            path.display()
        ),
    );
}

/// Probe/info stderr with plain-aware stats prefix (M50).
pub fn ui_probe_stats_stderr(message: impl std::fmt::Display) {
    crate::progress_mode::emit_stderr(&format!(
        "{} {message}",
        crate::modern_ui::symbols::pick("📊", "[stats]")
    ));
}

/// Penetration-style warning on stderr (M50 / U13).
pub fn ui_penetration_warning_stderr(label: &str, message: impl std::fmt::Display) {
    crate::progress_mode::emit_stderr(&format!(
        "{}  [{label}] {message}",
        crate::modern_ui::symbols::styled_warning_icon()
    ));
}

/// User-facing error string with plain-aware prefix (M51 / M52 / L3 / D4).
#[must_use]
pub fn ui_user_facing_error(message: impl std::fmt::Display) -> String {
    format!(
        "{} {message}",
        crate::modern_ui::symbols::pick(
            crate::modern_ui::symbols::ERROR,
            crate::modern_ui::symbols::plain::ERROR,
        )
    )
}

/// Quality-intel alias for [`ui_user_facing_error`] (M51).
#[must_use]
pub fn ui_quality_user_error(message: impl std::fmt::Display) -> String {
    ui_user_facing_error(message)
}

/// User-facing warning string with plain-aware prefix (M53).
#[must_use]
pub fn ui_user_facing_warning(message: impl std::fmt::Display) -> String {
    format!(
        "{} {message}",
        crate::modern_ui::symbols::pick(
            crate::modern_ui::symbols::WARNING,
            crate::modern_ui::symbols::plain::WARNING,
        )
    )
}

/// `log_summary_header!` title with plain-aware icon prefix (U14 / L2).
#[must_use]
pub fn ui_log_summary_title_with_icon(
    emoji: &str,
    plain: &str,
    title: impl std::fmt::Display,
) -> String {
    format!("{} {title}", crate::modern_ui::symbols::pick(emoji, plain))
}

/// Visual artifact audit report title for run logs (U14 / L2).
#[must_use]
pub fn ui_visual_artifact_audit_title(path: &Path) -> String {
    ui_log_summary_title_with_icon(
        "🔍",
        "[AUDIT]",
        format!("Visual Artifact Audit: {}", path.display()),
    )
}

/// Plain-aware icon for stderr/detail lines (M54).
#[must_use]
pub fn ui_icon_pick(emoji: &str, plain: &str) -> String {
    crate::modern_ui::symbols::pick(emoji, plain).to_string()
}

#[must_use]
pub fn ui_safety_crit_mark() -> String {
    ui_icon_pick("🚨", "[CRIT]")
}

#[must_use]
pub fn ui_safety_err_mark() -> String {
    ui_icon_pick("❌", "[ERROR]")
}

#[must_use]
pub fn ui_safety_warn_mark() -> String {
    ui_icon_pick("⚠️", "[WARN]")
}

#[must_use]
pub fn ui_safety_hint_mark() -> String {
    ui_icon_pick("💡", "[HINT]")
}

/// Protected system directory safety block (M54).
#[must_use]
pub fn ui_safety_system_dir_blocked(path: &Path) -> String {
    format!(
        "{crit} DANGEROUS OPERATION BLOCKED!\n\
         ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n\
         {err} Target directory '{path}' is a protected system directory.\n\
         {err} Operating on this directory could cause IRREVERSIBLE DAMAGE to your system.\n\
         \n\
         {hint} Please specify a safe subdirectory instead.\n\
         ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━",
        path = path.display(),
        crit = ui_safety_crit_mark(),
        err = ui_safety_err_mark(),
        hint = ui_safety_hint_mark(),
    )
}

/// Home-root proximity safety block (M54).
#[must_use]
pub fn ui_safety_home_root_blocked(path: &Path) -> String {
    format!(
        "{crit} DANGEROUS OPERATION BLOCKED!\n\
         ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n\
         {err} Target '{path}' is too close to your home directory root.\n\
         {err} Operating here could affect ALL your personal files.\n\
         \n\
         {hint} Please specify a subdirectory like ~/Documents/photos instead.\n\
         ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━",
        path = path.display(),
        crit = ui_safety_crit_mark(),
        err = ui_safety_err_mark(),
        hint = ui_safety_hint_mark(),
    )
}

/// Apple Photos library safety block (M54).
#[must_use]
pub fn ui_safety_photos_library_blocked(path: &Path, library: &Path) -> String {
    format!(
        "{crit} APPLE PHOTOS LIBRARY DETECTED!\n\
         ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n\
         {err} Target path '{path}' is inside an Apple Photos library:\n\
         {err} '{library}'\n\
         \n\
         {warn} Direct manipulation of files inside Photos libraries can:\n\
         {warn} • Corrupt the Photos database\n\
         {warn} • Break photo organization and metadata\n\
         {warn} • Cause permanent data loss\n\
         \n\
         {hint} To process photos from your Photos library:\n\
         {hint} 1. Export photos from Photos.app to a separate folder\n\
         {hint} 2. Run this tool on the exported folder\n\
         {hint} 3. Import the converted photos back into Photos if needed\n\
         ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━",
        path = path.display(),
        library = library.display(),
        crit = ui_safety_crit_mark(),
        err = ui_safety_err_mark(),
        warn = ui_safety_warn_mark(),
        hint = ui_safety_hint_mark(),
    )
}

/// Explore CRF bisect: compresses-at mark (M54).
#[must_use]
pub fn ui_explore_crf_compress_ok_mark() -> String {
    ui_icon_pick("✅", "[OK]")
}

/// Explore CRF bisect: too-large mark (M54).
#[must_use]
pub fn ui_explore_crf_too_large_mark() -> String {
    ui_icon_pick("❌", "[ERROR]")
}

/// Explore CRF bisect: phase-2 target mark (M54).
#[must_use]
pub fn ui_explore_crf_target_mark() -> String {
    ui_icon_pick("📍", "[TARGET]")
}

/// Colored terminal label for [`crate::infra::static_logs::ErrorSeverity`] (M55
/// / L8).
#[must_use]
pub fn ui_error_severity_colored_label(
    severity: crate::infra::static_logs::ErrorSeverity,
) -> String {
    if crate::progress_mode::is_plain_mode() {
        return severity.label().to_string();
    }
    match severity {
        crate::infra::static_logs::ErrorSeverity::Critical => {
            format!("\x1b[31;1m{} CRITICAL\x1b[0m", ui_icon_pick("🛑", "[!]"))
        }
        crate::infra::static_logs::ErrorSeverity::Fatal => {
            format!("\x1b[31;1;7m{} FATAL\x1b[0m", ui_icon_pick("💀", "[!!!]"))
        }
        crate::infra::static_logs::ErrorSeverity::Rare => format!(
            "\x1b[1;33m{} RARE ERROR\x1b[0m",
            ui_icon_pick("☢️", "[RARE]")
        ),
        crate::infra::static_logs::ErrorSeverity::MetadataLoss => format!(
            "\x1b[1;35m{} METADATA LOSS\x1b[0m",
            ui_icon_pick("📋", "[META]")
        ),
        crate::infra::static_logs::ErrorSeverity::PipelineBroken => format!(
            "\x1b[1;36m{} PIPELINE BROKEN\x1b[0m",
            ui_icon_pick("🔧", "[PIPE]")
        ),
        crate::infra::static_logs::ErrorSeverity::UpstreamError => format!(
            "\x1b[33m{} UPSTREAM ERROR\x1b[0m",
            ui_icon_pick("⛔", "[UPSTREAM]")
        ),
        crate::infra::static_logs::ErrorSeverity::Standard => {
            format!("\x1b[31m{} ERROR\x1b[0m", ui_icon_pick("❌", "[ERROR]"))
        }
    }
}

/// Confidence percent whole number (`85%`) or `N/A` (M49).
#[must_use]
pub fn ui_confidence_pct_whole_or_na(value: Option<f64>, label: &'static str) -> String {
    ui_optional_f64_display_or_map(value, "N/A", label, |c| {
        format!("{:.0}%", c * crate::constants::PERCENTAGE_FACTOR)
    })
}

/// Confidence on 0–100 scale with one decimal (`92.3`) or `N/A` (M49).
#[must_use]
pub fn ui_confidence_scale100_one_decimal_or_na(value: Option<f64>, label: &'static str) -> String {
    ui_optional_f64_display_or_map(value, "N/A", label, |c| {
        format!("{:.1}", c * crate::constants::SCALE_100)
    })
}

/// Bit-depth display label when metadata is missing (M49).
#[must_use]
pub fn ui_bit_depth_format_label_or_na(
    bit_depth: Option<u8>,
    inferred_from_pix_fmt: bool,
    label: &'static str,
) -> String {
    match bit_depth {
        Some(depth) if inferred_from_pix_fmt => format!("{depth}-bit (pix_fmt-inferred)"),
        Some(depth) => format!("{depth}-bit"),
        None => {
            delivery_runtime_batch_audit(
                "ui_bit_depth_na",
                format!("{label}: bit depth missing; displaying N/A"),
            );
            String::from("N/A")
        }
    }
}

/// Static not-applicable label for recommendation structs (audited in strict
/// delivery).
#[must_use]
pub fn ui_metric_not_applicable_label(context: &'static str) -> String {
    delivery_runtime_batch_audit(
        "ui_metric_not_applicable",
        format!("{context}: field not applicable; displaying N/A"),
    );
    String::from("N/A")
}

/// Pair metric `u/v` display or `N/A` (M47).
#[must_use]
pub fn ui_f64_pair_slash_or_na(value: Option<(f64, f64)>, label: &'static str) -> String {
    value
        .filter(|(u, v)| u.is_finite() && v.is_finite())
        .map_or_else(
            || {
                delivery_runtime_batch_audit(
                    "ui_metric_pair_na",
                    format!("{label}: pair unset or non-finite; displaying N/A"),
                );
                String::from("N/A")
            },
            |(u, v)| format!("{u:.2}/{v:.2}"),
        )
}

/// Labeled pair metric or a static missing label (M47).
#[must_use]
pub fn ui_f64_pair_labeled_or_na(
    value: Option<(f64, f64)>,
    missing_label: &'static str,
    label: &'static str,
    format_finite: impl FnOnce((f64, f64)) -> String,
) -> String {
    value
        .filter(|(u, v)| u.is_finite() && v.is_finite())
        .map_or_else(
            || {
                delivery_runtime_batch_audit(
                    "ui_metric_pair_na",
                    format!("{label}: using missing label \"{missing_label}\""),
                );
                missing_label.to_string()
            },
            format_finite,
        )
}

/// Internal absent-measurement marker for legacy loop helpers (not a fabricated
/// score).
#[inline]
#[must_use]
const fn absent_measurement_f64() -> f64 {
    f64::NAN
}

#[inline]
#[must_use]
const fn optional_f64_or_absent(value: Option<f64>) -> f64 {
    match value {
        Some(v) => v,
        None => absent_measurement_f64(),
    }
}

/// Loop duration z-score neutral when metadata duration is missing (`NaN`, not
/// fake `0.0`).
#[must_use]
pub fn loop_missing_duration_z_neutral(context: &str) -> f64 {
    delivery_intent_batch_audit(
        "loop_duration_z",
        format!("{context}: missing duration_secs; refusing fabricated neutral z-score 0.0"),
    );
    f64::NAN
}

/// Extended-short tail headroom when duration is known.
#[must_use]
pub fn loop_extended_short_tail_headroom_optional(
    duration_secs: Option<f64>,
    short_clip_secs: f64,
    range: f64,
    context: &str,
) -> Option<f64> {
    duration_secs
        .map(|duration| 1.0_f64 - ((duration - short_clip_secs) / range).clamp(0.0, 1.0))
        .or_else(|| {
            delivery_intent_batch_audit(
                "loop_tail_headroom",
                format!("{context}: missing duration_secs; refusing fabricated tail headroom 0.0"),
            );
            None
        })
}

/// Legacy extended-short tail headroom (`NaN` when absent).
#[must_use]
pub fn loop_extended_short_tail_headroom_or_zero(
    duration_secs: Option<f64>,
    short_clip_secs: f64,
    range: f64,
    context: &str,
) -> f64 {
    optional_f64_or_absent(loop_extended_short_tail_headroom_optional(
        duration_secs,
        short_clip_secs,
        range,
        context,
    ))
}

/// Modern-bias overflow ramp when duration is known.
#[must_use]
pub fn loop_modern_bias_overflow_optional(
    duration_secs: Option<f64>,
    modern_bias_duration_secs: f64,
    context: &str,
) -> Option<f64> {
    duration_secs
        .map(|duration| {
            ((duration - modern_bias_duration_secs) / modern_bias_duration_secs.max(1.0))
                .clamp(0.0, 1.0)
        })
        .or_else(|| {
            delivery_intent_batch_audit(
                "loop_modern_bias_overflow",
                format!("{context}: missing duration_secs; refusing fabricated overflow 0.0"),
            );
            None
        })
}

/// Legacy modern-bias overflow (`NaN` when absent).
#[must_use]
pub fn loop_modern_bias_overflow_or_zero(
    duration_secs: Option<f64>,
    modern_bias_duration_secs: f64,
    context: &str,
) -> f64 {
    optional_f64_or_absent(loop_modern_bias_overflow_optional(
        duration_secs,
        modern_bias_duration_secs,
        context,
    ))
}

/// Short-side extreme proximity ramp when duration is known.
#[must_use]
pub fn loop_short_proximity_ramp_optional(
    duration_secs: Option<f64>,
    short_veto: f64,
    short_buf: f64,
    context: &str,
) -> Option<f64> {
    duration_secs
        .map(|dur| 1.0_f64 - (dur - short_veto) / short_buf)
        .or_else(|| {
            delivery_intent_batch_audit(
                "loop_short_proximity",
                format!(
                    "{context}: missing duration_secs; refusing fabricated short proximity 0.0"
                ),
            );
            None
        })
}

/// Legacy short proximity ramp (`NaN` when absent).
#[must_use]
pub fn loop_short_proximity_ramp_or_zero(
    duration_secs: Option<f64>,
    short_veto: f64,
    short_buf: f64,
    context: &str,
) -> f64 {
    optional_f64_or_absent(loop_short_proximity_ramp_optional(
        duration_secs,
        short_veto,
        short_buf,
        context,
    ))
}

/// Long-side extreme proximity ramp when duration is missing (1.0 + audit).
#[must_use]
pub fn loop_long_proximity_ramp_or_one(
    duration_secs: Option<f64>,
    long_ramp_bottom: f64,
    long_buf: f64,
    context: &str,
) -> f64 {
    duration_secs.map_or_else(
        || {
            delivery_intent_batch_audit(
                "loop_long_proximity",
                format!("{context}: missing duration_secs; using long proximity 1.0"),
            );
            1.0_f64
        },
        |dur| (dur - long_ramp_bottom) / long_buf,
    )
}

/// Bytes-per-frame when `frame_count` is known and non-zero.
#[must_use]
pub fn loop_bytes_per_frame_optional(
    file_size_bytes: u64,
    frame_count: Option<u64>,
    context: &str,
) -> Option<f64> {
    match frame_count {
        None => {
            delivery_intent_batch_audit(
                "loop_bytes_per_frame",
                format!("{context}: missing frame_count; refusing fabricated bytes_per_frame 0.0"),
            );
            None
        }
        Some(0) => {
            delivery_intent_batch_audit(
                "loop_bytes_per_frame",
                format!("{context}: frame_count is 0; refusing fabricated bytes_per_frame 0.0"),
            );
            None
        }
        Some(fc) => Some(
            crate::numeric_cast::u64_to_f64(file_size_bytes) / crate::numeric_cast::u64_to_f64(fc),
        ),
    }
}

/// Legacy bytes-per-frame helper (`NaN` when absent; prefer
/// [`loop_bytes_per_frame_optional`]).
#[must_use]
pub fn loop_bytes_per_frame_or_zero(
    file_size_bytes: u64,
    frame_count: Option<u64>,
    context: &str,
) -> f64 {
    optional_f64_or_absent(loop_bytes_per_frame_optional(
        file_size_bytes,
        frame_count,
        context,
    ))
}

/// Audible audio flag with fail-closed handling for unverified audio streams.
#[must_use]
pub fn loop_audible_audio_fail_closed(
    has_audio_stream: bool,
    audible_state: Option<bool>,
    context: &str,
) -> bool {
    if !has_audio_stream {
        return false;
    }
    audible_state.unwrap_or_else(|| {
        delivery_intent_batch_audit(
            "loop_audible_audio",
            format!("{context}: missing audio_is_silent flag; treating track as audible"),
        );
        true
    })
}

/// Scaled duration percentile: `Some(p) => p * scale`; missing primary uses
/// unscaled `fallback_secs` + audit.
#[must_use]
pub fn loop_scaled_duration_percentile_or_fallback(
    percentile: Option<f64>,
    fallback_secs: f64,
    scale: f64,
    field: &str,
    context: &str,
) -> f64 {
    loop_scaled_duration_percentile_or_fallback_policy(
        percentile,
        fallback_secs,
        scale,
        field,
        context,
        true,
    )
}

/// Total pixel count when width/height are valid.
#[must_use]
pub fn loop_total_pixels_optional(
    width: Option<u32>,
    height: Option<u32>,
    context: &str,
) -> Option<f64> {
    match (width, height) {
        (Some(w), Some(h)) if w > 0 && h > 0 => Some(f64::from(w) * f64::from(h)),
        _ => {
            delivery_intent_batch_audit(
                "loop_total_pixels",
                format!(
                    "{context}: missing or invalid width/height; refusing fabricated total_pixels \
                     0.0"
                ),
            );
            None
        }
    }
}

/// Legacy total-pixels helper (`NaN` when absent; prefer
/// [`loop_total_pixels_optional`]).
#[must_use]
pub fn loop_total_pixels_or_zero(width: Option<u32>, height: Option<u32>, context: &str) -> f64 {
    optional_f64_or_absent(loop_total_pixels_optional(width, height, context))
}

/// Duration z-score when metadata duration is missing (neutral 0.0 via
/// [`loop_missing_duration_z_neutral`]).
#[must_use]
pub fn loop_duration_z_or_neutral(
    duration_secs: Option<f64>,
    z_score: impl FnOnce(f64) -> f64,
    context: &str,
) -> f64 {
    duration_secs.map_or_else(|| loop_missing_duration_z_neutral(context), z_score)
}

/// Reference top keywords when DB profile is unavailable (empty slice + audit).
#[must_use]
pub fn loop_top_keywords_or_empty<'a>(
    keywords: Option<&'a [String]>,
    context: &str,
) -> &'a [String] {
    keywords.unwrap_or_else(|| {
        delivery_intent_batch_audit(
            "loop_top_keywords",
            format!("{context}: missing reference top_keywords; using empty keyword set"),
        );
        &[]
    })
}

/// Frame-count label for intent audits when metadata count is unknown
/// (`unknown` + audit).
#[must_use]
pub fn loop_frame_count_label_or_unknown(frame_count: Option<u64>, context: &str) -> String {
    frame_count.map_or_else(
        || {
            delivery_intent_batch_audit(
                "loop_frame_count_label",
                format!("{context}: missing frame_count in audit label; using \"unknown\""),
            );
            String::from("unknown")
        },
        |count| count.to_string(),
    )
}

/// Optional tree probability for diagnostics (`n/a` + audit when missing).
#[must_use]
pub fn loop_format_optional_probability_or_na(value: Option<f64>, context: &str) -> String {
    value.map_or_else(
        || {
            delivery_intent_batch_audit(
                "loop_format_probability",
                format!("{context}: missing tree_probability; displaying n/a"),
            );
            String::from("n/a")
        },
        |v| format!("{v:.2}"),
    )
}

/// Duration label for loop diagnostics (`None` + audit when missing).
#[must_use]
pub fn loop_format_duration_secs_label(duration_secs: Option<f64>, context: &str) -> String {
    duration_secs.map_or_else(
        || {
            delivery_intent_batch_audit(
                "loop_format_duration",
                format!("{context}: missing duration_secs; displaying None"),
            );
            String::from("None")
        },
        |d| format!("{d:.2}s"),
    )
}

/// Neighbor-count suffix for arbitration traces (empty + audit when missing).
#[must_use]
pub fn loop_neighbor_count_suffix_or_empty(neighbor_count: Option<usize>, context: &str) -> String {
    neighbor_count.map_or_else(
        || {
            delivery_intent_batch_audit(
                "loop_neighbor_count",
                format!("{context}: missing neighbor_count; omitting suffix"),
            );
            String::new()
        },
        |count| format!(", n={count}"),
    )
}

/// Layer tag extracted from a verdict reason (`Unknown` + audit when
/// unparsable).
#[must_use]
pub fn loop_layer_tag_from_reason_or_unknown(reason: &str, context: &str) -> String {
    reason.find(':').map_or_else(
        || {
            if reason.starts_with("Layer") {
                reason.split_once('→').map_or_else(
                    || reason.to_string(),
                    |(prefix, _)| prefix.trim().to_string(),
                )
            } else {
                delivery_intent_batch_audit(
                    "loop_layer_tag",
                    format!("{context}: unparsable verdict reason; using layer tag Unknown"),
                );
                String::from("Unknown")
            }
        },
        |colon_pos| reason[..colon_pos].trim().to_string(),
    )
}

/// FPS kinetic z-score weights when fps metadata is missing (zero contribution,
/// not measured fps=0).
#[must_use]
pub fn loop_fps_kinetic_weights_or_neutral(
    fps: Option<f64>,
    z_score: impl FnOnce(f64) -> f64,
    context: &str,
) -> (f64, f64) {
    fps.map_or_else(
        || {
            delivery_intent_batch_audit(
                "loop_fps_kinetic",
                format!("{context}: missing fps; zero kinetic contribution (not measured fps=0)"),
            );
            (0.0_f64, 0.0_f64)
        },
        |fps| {
            let z = z_score(fps);
            (z.max(0.0_f64), (-z).max(0.0_f64))
        },
    )
}

/// Sorted baseline median frame count when slice element exists.
#[must_use]
pub fn loop_baseline_median_frames_optional(
    median_frames: Option<u64>,
    context: &str,
) -> Option<f64> {
    median_frames
        .map(crate::numeric_cast::u64_to_f64)
        .or_else(|| {
            delivery_intent_batch_audit(
                "loop_baseline_median",
                format!("{context}: empty baseline slice; refusing fabricated median frames 0.0"),
            );
            None
        })
}

/// Legacy baseline median frames helper (`NaN` when absent).
#[must_use]
pub fn loop_baseline_median_frames_or_zero(median_frames: Option<u64>, context: &str) -> f64 {
    optional_f64_or_absent(loop_baseline_median_frames_optional(median_frames, context))
}

/// JXL oversize comparison sentinel when screened candidate index is invalid
/// (`u64::MAX` + audit).
#[must_use]
pub fn jxl_screened_output_size_or_max(output_size: Option<u64>, context: &str) -> u64 {
    output_size.unwrap_or_else(|| {
        delivery_jxl_batch_audit(
            "jxl_screened_size",
            format!("{context}: missing oversize candidate; using u64::MAX sentinel"),
        );
        u64::MAX
    })
}

/// Animated/static feature compression ratio when analysis omits it (audited
/// heuristic estimate).
#[must_use]
pub fn probe_compression_ratio_or_estimate(
    value: Option<f64>,
    estimate: f64,
    context: &str,
) -> f64 {
    value
        .filter(|v| v.is_finite() && *v > 0.0)
        .unwrap_or_else(|| {
            probe_quality_batch_audit(
                "probe_compression_ratio",
                format!("{context}: missing compression_ratio; using estimate {estimate}"),
            );
            estimate
        })
}

/// Animated-image frame delay variation when timing probes fail (audited
/// VFR/CFR default).
#[must_use]
pub fn animated_delay_variation_or_default(
    is_variable_frame_rate: bool,
    resolved: Option<f64>,
    context: &str,
) -> f64 {
    resolved.filter(|v| v.is_finite()).unwrap_or_else(|| {
        let default = if is_variable_frame_rate { 0.5 } else { 0.0 };
        probe_quality_batch_audit(
            "animated_delay_variation",
            format!("{context}: using default variation {default} (vfr={is_variable_frame_rate})"),
        );
        default
    })
}

/// UTF-8 prefix slice for display truncation (audited empty when `end` exceeds
/// length).
#[must_use]
pub fn utf8_prefix_or_empty<'a>(value: &'a str, end: usize, context: &str) -> &'a str {
    value.get(..end).unwrap_or_else(|| {
        delivery_strict_batch_audit(
            "utf8_prefix",
            format!("{context}: prefix end {end} exceeds len {}", value.len()),
        );
        ""
    })
}

/// UTF-8 suffix slice for stderr/token parsing (audited empty when `start`
/// exceeds length).
#[must_use]
pub fn utf8_suffix_or_empty<'a>(value: &'a str, start: usize, context: &str) -> &'a str {
    value.get(start..).unwrap_or_else(|| {
        delivery_strict_batch_audit(
            "utf8_suffix",
            format!(
                "{context}: suffix start {start} exceeds len {}",
                value.len()
            ),
        );
        ""
    })
}

/// Loop/training ingest physics vector when embedding slice is absent (strict
/// DB audit).
#[must_use]
pub fn db_physics_embedding_or_empty<'a>(physics: Option<&'a [f32]>, context: &str) -> &'a [f32] {
    physics.unwrap_or_else(|| {
        delivery_db_batch_audit(
            "db_physics_embedding",
            format!("{context}: missing physics_225 embedding; using empty slice"),
        );
        &[]
    })
}

/// DB ingest optional bool probe fields (false + strict audit when missing).
#[must_use]
pub fn db_optional_bool_or_false(value: Option<bool>, field: &str, context: &str) -> bool {
    value.unwrap_or_else(|| {
        delivery_db_batch_audit(
            "db_optional_bool",
            format!("{context}: missing optional bool '{field}'; treating as false"),
        );
        false
    })
}

/// DB metadata optional string column (empty + strict audit when missing).
#[must_use]
pub fn db_optional_string_or_empty(value: Option<String>, column: &str, context: &str) -> String {
    value.unwrap_or_else(|| {
        delivery_db_batch_audit(
            "db_optional_string",
            format!("{context}: missing optional string column '{column}'; using empty"),
        );
        String::new()
    })
}

/// KNN pgvector component when an optional sample feature is absent (L2 sparse
/// origin; **not** a measured metric).
#[must_use]
pub const fn knn_absent_feature_component() -> f64 {
    0.0_f64
}

/// Min/avg/max triple from DB samples; absent corpus → `None` (never forged
/// `(0,0,0)`).
#[must_use]
pub fn db_numeric_stats_triple_or_none(
    stats: Option<(f64, f64, f64)>,
    field: &str,
    context: &str,
) -> Option<(f64, f64, f64)> {
    if stats.is_none() {
        delivery_db_batch_audit(
            "db_numeric_stats",
            format!("{context}: empty sample set for '{field}'; refusing fabricated (0,0,0) stats"),
        );
    }
    stats
}

/// Legacy name; returns `NaN` triple when samples absent (callers must use
/// `db_numeric_stats_triple_or_none`).
// Keep the absent marker explicit: DB stats must not use `unwrap_or(default)`.
#[allow(clippy::manual_unwrap_or)]
#[must_use]
pub fn db_numeric_stats_triple_or_zero(
    stats: Option<(f64, f64, f64)>,
    field: &str,
    context: &str,
) -> (f64, f64, f64) {
    match db_numeric_stats_triple_or_none(stats, field, context) {
        Some(triple) => triple,
        None => (f64::NAN, f64::NAN, f64::NAN),
    }
}

/// Relative path parent for timestamp restore walks (audited when at relative
/// root).
#[must_use]
pub fn path_relative_parent_or_self(rel: &Path) -> &Path {
    if let Some(parent) = rel.parent().filter(|parent| !parent.as_os_str().is_empty()) {
        parent
    } else {
        delivery_metadata_batch_audit(
            "path_relative_parent",
            format!(
                "{}: no parent segment; using relative path as parent key",
                rel.display()
            ),
        );
        rel
    }
}

/// Spinner/progress glyph from a static table (audited when index math fails).
#[must_use]
pub fn ui_spinner_glyph_at<'a>(
    table: &'a [&'static str],
    index: usize,
    default: &'static str,
    branch: &'static str,
) -> &'a str {
    let len = table.len().max(1);
    table.get(index % len).copied().unwrap_or_else(|| {
        delivery_runtime_batch_audit(
            branch,
            format!("UI spinner: index {index} modulo {len} missing; using default glyph"),
        );
        default
    })
}

/// Optional explore-progress f64 suffix (empty when unset; not a delivery
/// fallback).
#[must_use]
pub fn ui_optional_f64_display_suffix(value: Option<f64>, label: &str) -> String {
    value
        .filter(|v| v.is_finite())
        .map_or_else(String::new, |v| format!(" {label} {v:.4}"))
}

/// Optional explore best-CRF suffix (empty when unset; not a delivery
/// fallback).
#[must_use]
pub fn ui_optional_crf_display_suffix(value: Option<f32>, label: &str) -> String {
    value
        .filter(|v| v.is_finite())
        .map_or_else(String::new, |v| format!(" {label} {v:.1}"))
}

/// Finite f32 for status lines or a placeholder (audited in strict delivery).
#[must_use]
pub fn ui_f32_display_or_placeholder(
    value: Option<f32>,
    placeholder: &'static str,
    label: &'static str,
) -> String {
    value.filter(|v| v.is_finite()).map_or_else(
        || {
            delivery_runtime_batch_audit(
                "ui_metric_f32_placeholder",
                format!("{label}: using placeholder \"{placeholder}\""),
            );
            placeholder.to_string()
        },
        |v| format!("{v:.1}"),
    )
}

/// Linux VAAPI render node when env vars are unset (audited default path).
#[must_use]
#[cfg(target_os = "linux")]
pub fn gpu_vaapi_device_path_or_default() -> String {
    std::env::var(crate::constants::ENV_VAAPI_DEVICE)
        .or_else(|_| std::env::var(crate::constants::ENV_VAAPI_DEVICE_FALLBACK))
        .unwrap_or_else(|_| {
            delivery_gpu_batch_audit(
                "vaapi_device",
                "GPU AUDIT: VAAPI device env unset; using /dev/dri/renderD128",
            );
            String::from("/dev/dri/renderD128")
        })
}

/// Run log directory base when `current_dir` is unavailable (audited `.`).
#[must_use]
pub fn delivery_run_logs_dir_or_dot(context: &str) -> PathBuf {
    std::env::current_dir().unwrap_or_else(|err| {
        delivery_runtime_batch_audit(
            "run_logs_dir",
            format!("{context}: cwd unavailable ({err}); using ."),
        );
        PathBuf::from(".")
    })
}

/// Safety relative-path base: `current_dir` when available, else audited `/`
/// (conservative lex normalization).
#[must_use]
pub fn delivery_safety_relative_base_or_root(context: &str) -> PathBuf {
    std::env::current_dir().unwrap_or_else(|err| {
        delivery_runtime_batch_audit(
            "safety_relative_base",
            format!("{context}: cwd unavailable ({err}); using / for lexical normalization"),
        );
        PathBuf::from("/")
    })
}

/// Current working directory when available (strict-gated audit on failure).
#[must_use]
pub fn delivery_cwd_or_audit(context: &str) -> Option<PathBuf> {
    match std::env::current_dir() {
        Ok(cwd) => Some(cwd),
        Err(err) => {
            delivery_strict_batch_audit(
                "cwd_unavailable",
                format!("{context}: current_dir unavailable ({err})"),
            );
            None
        }
    }
}

/// Join a relative path to cwd for validators (error when cwd is unavailable).
pub fn delivery_join_relative_to_cwd_or_err(path: &Path, context: &str) -> Result<PathBuf, String> {
    if path.is_absolute() {
        return Ok(path.to_path_buf());
    }
    let cwd = delivery_cwd_or_audit(context)
        .ok_or_else(|| format!("Failed to resolve current directory for {}", path.display()))?;
    Ok(cwd.join(path))
}

/// Absolute output path for tools that require it (`cwd` join, or audited `.`
/// fallback).
#[must_use]
pub fn delivery_absolute_output_path_or_dot(output: &Path, context: &str) -> PathBuf {
    if output.is_absolute() {
        return output.to_path_buf();
    }
    match std::env::current_dir() {
        Ok(cwd) => cwd.join(output),
        Err(err) => {
            delivery_api_batch_fallback_audit(
                "cwd",
                format!(
                    "{context}: current_dir unavailable ({err}); resolving output relative to \
                     \".\""
                ),
            );
            PathBuf::from(".").join(output)
        }
    }
}

/// Tracing registry filter: honor `RUST_LOG` when valid; audited fallback to
/// config level.
#[must_use]
pub fn tracing_registry_env_filter_or_config(
    program_name: &str,
    config_level: tracing::Level,
) -> tracing_subscriber::EnvFilter {
    use tracing_subscriber::EnvFilter;
    EnvFilter::try_from_default_env().unwrap_or_else(|err| {
        if std::env::var_os("RUST_LOG").is_some() {
            delivery_logging_path_audit(
                "rust_log_parse",
                Path::new("."),
                format!(
                    "RUST_LOG invalid ({err}); building filter from config level {config_level}"
                ),
            );
        }
        EnvFilter::new(format!(
            "{program_name}={config_level},foundation={config_level}"
        ))
    })
}

/// OS system temp directory — gate-internal SSOT for `std::env::temp_dir()`
/// (M169).
#[must_use]
fn delivery_system_temp_dir_ssot() -> PathBuf {
    std::env::temp_dir()
}

/// MFB state root under the gate-owned system temp SSOT.
#[must_use]
pub fn delivery_temp_mfb_root_ssot() -> PathBuf {
    delivery_system_temp_dir_ssot().join(crate::constants::MFB_DEFAULT_HOME_DIRNAME)
}

/// Scratch/temp directory for delivery artifacts (`get_mfb_tmp_dir` → audited
/// system temp).
#[must_use]
pub fn delivery_scratch_temp_dir_or_system_temp(context: &str) -> PathBuf {
    crate::process_lock::get_mfb_tmp_dir().unwrap_or_else(|err| {
        delivery_runtime_batch_audit(
            "scratch_temp_dir",
            format!("{context}: mfb tmp unavailable ({err}); using system temp_dir"),
        );
        delivery_system_temp_dir_ssot()
    })
}

/// RAII temp directory under MFB scratch (`get_mfb_tmp_dir` → audited system
/// temp).
pub fn delivery_temp_dir_in_scratch_or_err(
    context: &str,
    prefix: &str,
) -> std::io::Result<tempfile::TempDir> {
    let base = delivery_scratch_temp_dir_or_system_temp(context);
    tempfile::Builder::new().prefix(prefix).tempdir_in(&base)
}

/// Named temp file under MFB scratch (`get_mfb_tmp_dir` → audited system temp).
pub fn delivery_named_tempfile_in_scratch_or_err(
    context: &str,
    prefix: Option<&str>,
    suffix: Option<&str>,
) -> std::io::Result<tempfile::NamedTempFile> {
    let base = delivery_scratch_temp_dir_or_system_temp(context);
    let mut builder = tempfile::Builder::new();
    if let Some(prefix) = prefix {
        builder.prefix(prefix);
    } else {
        builder.prefix("mfb-");
    }
    if let Some(suffix) = suffix {
        builder.suffix(suffix);
    }
    builder.tempfile_in(&base)
}

/// Best-effort `create_dir_all` with path audit on failure (no silent `let _ =`
/// discard).
pub fn delivery_create_dir_all_or_audit(context: &str, dir: &Path) {
    if let Err(err) = std::fs::create_dir_all(dir) {
        delivery_runtime_path_audit(
            "create_dir_all",
            dir,
            format!("{context}: create_dir_all failed: {err}"),
        );
    }
}

/// Legacy fixed staging path helper (audited when destination has no
/// extension).
#[must_use]
pub fn path_robust_move_staging_path(dst: &Path, context: &str) -> PathBuf {
    if let Some(ext) = dst.extension().and_then(|e| e.to_str()) {
        dst.with_extension(format!("{ext}.mfb-tmp"))
    } else {
        delivery_io_batch_audit(
            "robust_move_staging",
            format!(
                "{context}: {} missing extension; staging as .mfb-tmp",
                dst.display()
            ),
        );
        dst.with_extension("mfb-tmp")
    }
}

/// Best-effort file removal with cleanup audit (no silent `let _ =
/// remove_file`).
pub fn delivery_remove_file_or_audit(context: &str, path: &Path) {
    match std::fs::remove_file(path) {
        Ok(()) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => delivery_cleanup_audit(path, context, err),
    }
}

/// Rename with path audit on failure.
#[must_use]
pub fn delivery_rename_or_audit(context: &str, from: &Path, to: &Path) -> bool {
    std::fs::rename(from, to).map_or_else(
        |err| {
            delivery_runtime_path_audit(
                "rename",
                from,
                format!("{context}: rename to {} failed: {err}", to.display()),
            );
            false
        },
        |()| true,
    )
}

/// Ensure an output path's parent exists; audit root outputs and mkdir
/// failures.
pub fn delivery_ensure_output_parent_or_audit(context: &str, output: &Path) {
    match output
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        Some(parent) if parent.is_dir() => {}
        Some(parent) if parent.exists() => delivery_runtime_path_audit(
            "output_parent_not_dir",
            parent,
            format!("{context}: output parent path exists but is not a directory"),
        ),
        Some(parent) => delivery_create_dir_all_or_audit(context, parent),
        None => delivery_runtime_path_audit(
            "output_parent",
            output,
            format!("{context}: output path has no parent directory"),
        ),
    }
}

/// Named temp file beside a final output path (atomic persist via `persist`);
/// audits missing parent.
pub fn delivery_named_tempfile_in_parent_or_err(
    context: &str,
    parent: &Path,
    prefix: &str,
    suffix: &str,
) -> std::io::Result<tempfile::NamedTempFile> {
    if !parent.exists() {
        delivery_runtime_path_audit(
            "tempfile_parent_missing",
            parent,
            format!("{context}: parent directory does not exist for output-adjacent temp file"),
        );
    } else if !parent.is_dir() {
        delivery_runtime_path_audit(
            "tempfile_parent_not_dir",
            parent,
            format!("{context}: parent path is not a directory for output-adjacent temp file"),
        );
    }
    tempfile::Builder::new()
        .prefix(prefix)
        .suffix(suffix)
        .tempfile_in(parent)
}

/// Session log directory from `MFB_LOG_DIR` or audited system temp (M45).
#[must_use]
pub fn delivery_log_dir_from_env_or_temp(context: &str) -> PathBuf {
    match std::env::var(crate::constants::ENV_MFB_LOG_DIR) {
        Ok(log_dir) => {
            let trimmed = log_dir.trim();
            if !trimmed.is_empty() {
                return PathBuf::from(trimmed);
            }
        }
        Err(std::env::VarError::NotPresent) => {}
        Err(e) => delivery_logging_path_audit(
            "log_dir_env",
            Path::new("."),
            format!("failed to read {}: {e}", crate::constants::ENV_MFB_LOG_DIR),
        ),
    }
    delivery_logging_path_audit(
        "log_dir_env",
        Path::new("."),
        format!(
            "{context}: {env} unset or empty; using system temp_dir",
            env = crate::constants::ENV_MFB_LOG_DIR
        ),
    );
    delivery_system_temp_dir_ssot()
}

/// Relativize an absolute path for `ImageMagick` when cwd is available (M45).
#[must_use]
pub fn path_magick_relativized_lossy(path: &Path) -> Cow<'_, str> {
    if path.is_relative() {
        return path.to_string_lossy();
    }
    let Some(cwd) = delivery_cwd_or_audit("path_magick_relativized") else {
        delivery_runtime_path_audit(
            "magick_cwd",
            path,
            "cwd unavailable; using absolute lossy path",
        );
        return path.to_string_lossy();
    };
    path.strip_prefix(&cwd).map_or_else(
        |_| {
            delivery_runtime_path_audit(
                "magick_strip_prefix",
                path,
                "path not under cwd; using absolute lossy path",
            );
            path.to_string_lossy()
        },
        |rel| Cow::Owned(rel.to_string_lossy().into_owned()),
    )
}

/// Isolated search temp stem when `file_stem` is missing (M45).
#[must_use]
pub fn path_search_temp_stem_or_output(path: &Path) -> Cow<'_, str> {
    path.file_stem().map_or_else(
        || {
            delivery_runtime_path_audit(
                "search_temp_stem",
                path,
                "missing file_stem; using \"output\"",
            );
            Cow::Borrowed("output")
        },
        |stem| stem.to_string_lossy(),
    )
}

/// Isolated search temp extension when missing (M45).
#[must_use]
pub fn path_search_temp_ext_or_tmp(path: &Path) -> Cow<'_, str> {
    path.extension().map_or_else(
        || {
            delivery_runtime_path_audit(
                "search_temp_ext",
                path,
                "missing extension; using \"tmp\"",
            );
            Cow::Borrowed("tmp")
        },
        |ext| ext.to_string_lossy(),
    )
}

/// Process cwd string for logging init (audited `<unknown>` when unavailable).
#[must_use]
pub fn delivery_cwd_display_or_unknown(context: &str) -> String {
    std::env::current_dir().map_or_else(
        |err| {
            delivery_logging_path_audit(
                "cwd_display",
                Path::new("."),
                format!("{context}: cwd unavailable ({err}); using <unknown>"),
            );
            String::from("<unknown>")
        },
        |path| path.display().to_string(),
    )
}

/// Disk-space probe path when CLI output dir is unset (defaults to input;
/// strict audit).
#[must_use]
pub fn delivery_disk_check_path_or_input<'a>(
    output: Option<&'a Path>,
    input: &'a Path,
    context: &str,
) -> &'a Path {
    output.unwrap_or_else(|| {
        delivery_pipeline_batch_audit(
            "disk_check_path",
            format!("{context}: output path unset; using input path for disk probe"),
        );
        input
    })
}

/// External tool executable when builder override is unset (audited default
/// binary name).
#[must_use]
pub fn delivery_tool_executable_or_default<'a>(
    configured: Option<&'a str>,
    default: &'static str,
    tool: &str,
    context: &str,
) -> &'a str {
    configured
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| {
            delivery_runtime_batch_audit(
                "tool_executable",
                format!("{context}: {tool} executable unset; using {default}"),
            );
            default
        })
}

/// First segment before a separator for metadata date strings (whole string
/// when split is empty).
#[must_use]
pub fn str_first_segment_or_whole<'a>(value: &'a str, separator: char, context: &str) -> &'a str {
    value.split(separator).next().unwrap_or_else(|| {
        probe_layer_batch_audit(
            "date_segment",
            format!("{context}: empty date string after split on '{separator}'"),
        );
        value
    })
}

/// Sorted distance sample at index with clamp to nearest endpoint; empty slice
/// → `None`.
#[must_use]
pub fn db_sorted_distance_at(distances: &[f64], index: usize, context: &str) -> Option<f64> {
    if distances.is_empty() {
        delivery_db_batch_audit(
            "db_distance_empty",
            format!("{context}: empty distance slice; refusing fabricated 0.0"),
        );
        return None;
    }
    if let Some(value) = distances.get(index).copied() {
        return Some(value);
    }
    let fallback = distances
        .first()
        .copied()
        .or_else(|| distances.last().copied())?;
    delivery_db_batch_audit(
        "db_distance_index",
        format!(
            "{context}: distance index {index} out of bounds (len={}); using endpoint {fallback}",
            distances.len()
        ),
    );
    Some(fallback)
}

/// Parent directory segment count for loop metadata (0 when ancestry is unknown
/// + audit).
#[must_use]
pub fn loop_parent_directory_depth(parent_directories: Option<&[String]>, context: &str) -> u32 {
    parent_directories.map_or_else(
        || {
            delivery_intent_batch_audit(
                "loop_parent_directory_depth",
                format!("{context}: missing parent_directories; using depth 0"),
            );
            0
        },
        |parts| crate::numeric_cast::usize_to_u32_sat(parts.len()),
    )
}

/// Optional loop duration percentile with an explicit baseline constant (strict
/// audit).
///
/// Baseline constants only — does not assume an empirical profile histogram.
#[must_use]
pub fn loop_optional_secs_or_baseline(
    value: Option<f64>,
    baseline: f64,
    field: &str,
    context: &str,
) -> f64 {
    loop_collection_secs_or_baseline_policy(value, baseline, field, context, false, true)
}

/// Collection duration field fallback; skip audit when profile lacks percentile
/// histogram (M122/M221).
#[must_use]
pub fn loop_collection_secs_or_baseline_policy(
    value: Option<f64>,
    baseline: f64,
    field: &str,
    context: &str,
    profile_percentiles_available: bool,
    collection_value_trusted: bool,
) -> f64 {
    if profile_percentiles_available || collection_value_trusted {
        return value.filter(|v| v.is_finite()).unwrap_or_else(|| {
            if profile_percentiles_available {
                delivery_db_batch_audit(
                    "loop_duration_baseline",
                    format!("{context}: missing {field}; using baseline {baseline}"),
                );
            }
            baseline
        });
    }
    if let Some(v) = value.filter(|v| v.is_finite()) {
        delivery_db_batch_audit(
            "collection_field_discarded",
            format!("{context}: ignoring untrusted {field}={v}; using baseline {baseline}"),
        );
    }
    baseline
}

/// ffprobe reported 0×0 (or invalid) dimensions; bitstream header supplied
/// canvas size (M122).
pub fn probe_bitstream_dimension_recovery_audit(
    path: &Path,
    probe_width: u32,
    probe_height: u32,
    recovered_width: u32,
    recovered_height: u32,
) {
    probe_layer_audit(
        "ffprobe_dimension_bitstream_recovery",
        path,
        format!(
            "ffprobe stream dimensions {probe_width}x{probe_height} invalid; recovered \
             {recovered_width}x{recovered_height} from container bitstream"
        ),
    );
}

/// `f64` sort comparison (NaN → `Equal` with numeric audit in strict delivery).
#[must_use]
pub fn f64_sort_cmp(a: f64, b: f64) -> Ordering {
    a.partial_cmp(&b).unwrap_or_else(|| {
        delivery_numeric_fallback_audit(
            "f64_sort_cmp",
            "NUMERIC AUDIT: NaN in f64 sort comparison; treating as Equal",
        );
        Ordering::Equal
    })
}

/// Inference log JSON field for optional finite `f64` (`null` when
/// missing/non-finite).
#[must_use]
pub fn json_finite_f64_or_null(
    value: Option<f64>,
    field: &str,
    context: &str,
) -> serde_json::Value {
    value.filter(|v| v.is_finite()).map_or_else(
        || {
            delivery_db_batch_audit(
                "json_finite_f64",
                format!("{context}: {field} missing or non-finite; using null"),
            );
            serde_json::Value::Null
        },
        serde_json::Value::from,
    )
}

/// Sparse inference telemetry: optional `f64` → JSON `null` without batch audit
/// (M117).
#[must_use]
pub fn json_inference_optional_f64_or_null(value: Option<f64>) -> serde_json::Value {
    value
        .filter(|v| v.is_finite())
        .map_or(serde_json::Value::Null, serde_json::Value::from)
}

/// Sparse inference telemetry: optional `i32` → JSON `null` without batch audit
/// (M117).
#[must_use]
pub fn json_inference_optional_i32_or_null(value: Option<i32>) -> serde_json::Value {
    value.map_or(serde_json::Value::Null, serde_json::Value::from)
}

/// Sparse inference telemetry: optional `bool` → JSON `null` without batch
/// audit (M117).
#[must_use]
pub fn json_inference_optional_bool_or_null(value: Option<bool>) -> serde_json::Value {
    value.map_or(serde_json::Value::Null, serde_json::Value::from)
}

/// Sparse inference telemetry: optional string → JSON `null` without batch
/// audit (M117).
#[must_use]
pub fn json_inference_optional_string_or_null(value: Option<&str>) -> serde_json::Value {
    value
        .filter(|s| !s.is_empty())
        .map_or(serde_json::Value::Null, |s| {
            serde_json::Value::String(s.to_string())
        })
}

/// Inference log JSON for a required `f64` scalar (`null` when non-finite).
#[must_use]
pub fn json_required_finite_f64_or_null(
    value: f64,
    field: &str,
    context: &str,
) -> serde_json::Value {
    if value.is_finite() {
        serde_json::Value::from(value)
    } else {
        delivery_db_batch_audit(
            "json_finite_f64",
            format!("{context}: {field} non-finite; using null"),
        );
        serde_json::Value::Null
    }
}

/// Inference log JSON for optional `i32` (`null` when missing).
#[must_use]
pub fn json_optional_i32_or_null(
    value: Option<i32>,
    field: &str,
    context: &str,
) -> serde_json::Value {
    value.map_or_else(
        || {
            delivery_db_batch_audit(
                "json_optional_i32",
                format!("{context}: {field} missing; using null"),
            );
            serde_json::Value::Null
        },
        serde_json::Value::from,
    )
}

/// Inference log JSON for optional `bool` (`null` when missing).
#[must_use]
pub fn json_optional_bool_or_null(
    value: Option<bool>,
    field: &str,
    context: &str,
) -> serde_json::Value {
    value.map_or_else(
        || {
            delivery_db_batch_audit(
                "json_optional_bool",
                format!("{context}: {field} missing; using null"),
            );
            serde_json::Value::Null
        },
        serde_json::Value::from,
    )
}

/// Inference log JSON for optional string (`null` when missing/empty).
#[must_use]
pub fn json_optional_string_or_null(
    value: Option<&str>,
    field: &str,
    context: &str,
) -> serde_json::Value {
    value.filter(|s| !s.is_empty()).map_or_else(
        || {
            delivery_db_batch_audit(
                "json_optional_string",
                format!("{context}: {field} missing or empty; using null"),
            );
            serde_json::Value::Null
        },
        |s| serde_json::Value::String(s.to_string()),
    )
}

/// Loop duration fallback between two resolved percentile values (strict audit
/// when primary missing).
///
/// Prefer [`loop_duration_or_fallback_policy`] in production so empirical
/// histogram provenance is explicit.
#[must_use]
pub fn loop_duration_or_fallback(
    primary: Option<f64>,
    fallback: f64,
    field: &str,
    context: &str,
) -> f64 {
    loop_duration_or_fallback_policy(primary, fallback, field, context, false)
}

/// Collection `duration_p90` for threshold math: sample-derived P90 only, or
/// audited feature-map fallback (M218/M219/M224).
#[must_use]
pub fn loop_collection_duration_p90_or_baseline(
    collection_p90: Option<f64>,
    baseline: f64,
    field: &str,
    context: &str,
    profile_percentiles_available: bool,
    collection_p90_from_samples: bool,
) -> f64 {
    if collection_p90_from_samples {
        return loop_collection_secs_or_baseline_policy(
            collection_p90,
            baseline,
            field,
            context,
            profile_percentiles_available,
            true,
        );
    }
    if profile_percentiles_available {
        if let Some(v) = collection_p90.filter(|v| v.is_finite()) {
            delivery_db_batch_audit(
                "collection_duration_p90_discarded",
                format!(
                    "{context}: ignoring non-sample {field}={v} despite profile histogram; using \
                     baseline {baseline}"
                ),
            );
        }
        return baseline;
    }
    if let Some(v) = collection_p90.filter(|v| v.is_finite()) {
        delivery_db_batch_audit(
            "collection_duration_p90_discarded",
            format!(
                "{context}: ignoring {field}={v} from feature_stats without sample or histogram \
                 provenance; using baseline {baseline}"
            ),
        );
    }
    baseline
}

/// Median duration for short-clip thresholding; absent without empirical
/// profile histogram (M219).
#[must_use]
pub fn loop_duration_p50_or_capped_p75_policy(
    p50: Option<f64>,
    p75: Option<f64>,
    profile_percentiles_available: bool,
) -> Option<f64> {
    if profile_percentiles_available {
        loop_duration_p50_or_capped_p75(p50, p75)
    } else {
        None
    }
}

/// Loop duration fallback; skip audit when reference profile has no percentile
/// histogram (M117).
#[must_use]
pub fn loop_duration_or_fallback_policy(
    primary: Option<f64>,
    fallback: f64,
    field: &str,
    context: &str,
    profile_percentiles_available: bool,
) -> f64 {
    if profile_percentiles_available {
        if let Some(value) = primary.filter(|v| v.is_finite()) {
            return value;
        }
        delivery_db_batch_audit(
            "loop_duration_fallback",
            format!("{context}: {field} missing; using fallback {fallback}"),
        );
    }
    fallback
}

/// Scaled loop duration percentile; skip audit when profile has no percentile
/// histogram (M117).
#[must_use]
pub fn loop_scaled_duration_percentile_or_fallback_policy(
    percentile: Option<f64>,
    fallback_secs: f64,
    scale: f64,
    field: &str,
    context: &str,
    profile_percentiles_available: bool,
) -> f64 {
    if !profile_percentiles_available {
        return fallback_secs;
    }
    if let Some(value) = percentile.filter(|v| v.is_finite()) {
        value * scale
    } else {
        delivery_intent_batch_audit(
            "loop_duration_percentile",
            format!("{context}: missing {field}; using unscaled fallback duration {fallback_secs}"),
        );
        fallback_secs
    }
}

/// Algorithm env flag (`1` = enabled); unset uses `default_enabled` without
/// audit.
#[must_use]
pub fn algorithm_env_flag_enabled_or_default(env_key: &str, default_enabled: bool) -> bool {
    match std::env::var(env_key) {
        Ok(value) => value == "1",
        Err(std::env::VarError::NotPresent) => default_enabled,
        Err(e) => {
            delivery_strict_batch_audit(
                "algorithm_env_flag",
                format!("failed to read env {env_key}: {e}; using default {default_enabled}"),
            );
            default_enabled
        }
    }
}

/// Algorithm env `usize` parse with documented default (strict audit when
/// unset/invalid).
#[must_use]
pub fn algorithm_env_usize_or_default(env_key: &str, default: usize, context: &str) -> usize {
    match std::env::var(env_key) {
        Ok(raw) => match raw.trim().parse::<usize>() {
            Ok(n) if n > 0 => n,
            Ok(_) => {
                delivery_strict_batch_audit(
                    "algorithm_env_usize",
                    format!(
                        "{context}: env {env_key} must be greater than zero; using default \
                         {default}"
                    ),
                );
                default
            }
            Err(e) => {
                delivery_strict_batch_audit(
                    "algorithm_env_usize",
                    format!(
                        "{context}: failed to parse env {env_key}='{raw}': {e}; using default \
                         {default}"
                    ),
                );
                default
            }
        },
        Err(std::env::VarError::NotPresent) => {
            delivery_strict_batch_audit(
                "algorithm_env_usize",
                format!("{context}: env {env_key} unset; using default {default}"),
            );
            default
        }
        Err(e) => {
            delivery_strict_batch_audit(
                "algorithm_env_usize",
                format!("{context}: failed to read env {env_key}: {e}; using default {default}"),
            );
            default
        }
    }
}

/// Metadata timestamp restore error when the first failure reason was not
/// captured.
#[must_use]
pub fn io_error_or_metadata_label(
    err: Option<std::io::Error>,
    label: &'static str,
) -> std::io::Error {
    err.unwrap_or_else(|| {
        delivery_metadata_batch_audit(
            "io_error_label",
            format!("Metadata I/O: missing captured error; using labeled default ({label})"),
        );
        std::io::Error::other(label)
    })
}

/// KNN feature weight from stats map (`None` when weight absent — not a forged
/// default).
#[must_use]
pub fn db_feature_weight_optional(
    weight: Option<f64>,
    feature: &str,
    context: &str,
) -> Option<f64> {
    let Some(value) = weight else {
        delivery_db_batch_audit(
            "db_feature_weight",
            format!("{context}: missing weight for feature '{feature}'; refusing forged default"),
        );
        return None;
    };
    Some(value.max(crate::constants::KNN_VECTOR_MIN_WEIGHT))
}

/// Legacy name; panics after audit when weight missing (prefer
/// [`db_feature_weight_optional`]).
#[must_use]
pub fn db_feature_weight_or_default(weight: Option<f64>, feature: &str, context: &str) -> f64 {
    db_feature_weight_optional(weight, feature, context).unwrap_or_else(|| {
        panic!("db feature weight required for '{feature}' after gate audit ({context})");
    })
}

/// Lossy path extension string for depth-map outputs (empty when missing;
/// strict audit).
#[must_use]
pub fn path_extension_lossy_or_empty(path: &Path, context: &str) -> String {
    path.extension().map_or_else(
        || {
            probe_layer_batch_audit(
                "path_extension_lossy",
                format!("{context}: missing extension; treating as empty"),
            );
            String::new()
        },
        |ext| ext.to_string_lossy().into_owned(),
    )
}

/// `FFprobe` optional string field (empty when missing; strict audit).
#[must_use]
pub fn probe_ffprobe_optional_string(value: Option<&str>, field: &str, context: &str) -> String {
    value.map_or_else(
        || {
            probe_layer_batch_audit(
                "probe_ffprobe_field",
                format!("{context}: missing ffprobe field '{field}'; using empty string"),
            );
            String::new()
        },
        str::to_string,
    )
}

/// `FFprobe` JSON value → `f64` extraction: try string parse first, then native
/// `as_f64` (M204).
///
/// Centralises the two-step parse previously inlined as `.or_else(||
/// raw.as_f64())`.
#[must_use]
pub fn probe_ffprobe_json_value_as_f64(raw: &serde_json::Value) -> Option<f64> {
    if let Some(s) = raw.as_str() {
        return match s.parse::<f64>() {
            Ok(value) => Some(value),
            Err(e) => {
                probe_layer_batch_audit(
                    "ffprobe_json_parse",
                    format!("failed to parse ffprobe JSON string value '{s}' into finite f64: {e}"),
                );
                None
            }
        };
    }
    raw.as_f64()
}

#[must_use]
pub fn probe_ffprobe_codec_name_lowercase(codec: Option<&str>, context: &str) -> String {
    codec.map_or_else(
        || {
            probe_layer_batch_audit(
                "probe_codec_name",
                format!("{context}: missing codec_name in ffprobe stream"),
            );
            String::new()
        },
        str::to_ascii_lowercase,
    )
}

/// Last non-empty stderr line for encode/tool failures (audited when stderr is
/// empty).
#[must_use]
pub fn encode_stderr_last_line_or_unknown(
    stderr: &str,
    branch: &'static str,
    context: &str,
) -> String {
    stderr.lines().rfind(|line| !line.is_empty()).map_or_else(
        || {
            delivery_encode_batch_audit(
                branch,
                format!("{context}: tool stderr empty; using Unknown error"),
            );
            String::from("Unknown error")
        },
        str::to_string,
    )
}

/// Preserve full path when `strip_prefix` fails during delivery copy layout.
#[must_use]
pub fn strip_prefix_or_self<'a>(path: &'a Path, base: &'a Path, branch: &'static str) -> &'a Path {
    path.strip_prefix(base).unwrap_or_else(|_| {
        delivery_path_layout_fallback_audit(
            branch,
            format!(
                "{} not under {}; preserving full path in output layout",
                path.display(),
                base.display()
            ),
        );
        path
    })
}

/// Uppercase extension label for quality UI (audited `unknown` when missing).
#[must_use]
pub fn path_extension_uppercase_or_unknown(path: &Path, context: &str) -> String {
    path.extension().map_or_else(
        || {
            probe_quality_layer_audit(
                "path_extension_uppercase",
                path,
                format!("{context}: missing extension; using unknown format label"),
            );
            String::from("unknown")
        },
        |ext| ext.to_string_lossy().to_uppercase(),
    )
}

/// Lowercase extension for bulk scans/filters (missing extension is normal; no
/// audit).
#[must_use]
pub fn path_extension_lowercase_or_empty_unchecked(path: &Path) -> String {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map_or_else(String::new, str::to_lowercase)
}

/// Lowercase extension for delivery routing (empty when missing, with audit).
#[must_use]
pub fn path_extension_lowercase_or_empty(path: &Path, context: &str) -> String {
    path.extension().and_then(|ext| ext.to_str()).map_or_else(
        || {
            delivery_strict_batch_audit(
                "path_extension",
                format!("{context}: no file extension; treating as empty"),
            );
            String::new()
        },
        str::to_ascii_lowercase,
    )
}

/// Outcome of the static-image conversion preflight gate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StaticConversionVerdict {
    Proceed,
    /// Outside `img` static domain — do not write outputs; count as ignored.
    Ignore {
        reason: String,
        class: &'static str,
    },
}

impl StaticConversionVerdict {
    #[must_use]
    pub fn ignore_reason(&self) -> Option<&str> {
        match self {
            Self::Proceed => None,
            Self::Ignore { reason, .. } => Some(reason),
        }
    }

    #[must_use]
    pub const fn ignore_class(&self) -> Option<&'static str> {
        match self {
            Self::Proceed => None,
            Self::Ignore { class, .. } => Some(*class),
        }
    }
}

/// When strict media conversion is on, CLI/API size-growth allowance is
/// suppressed.
#[must_use]
pub fn effective_allow_size_tolerance(requested: bool) -> bool {
    if crate::algorithm_runtime::strict_media_conversion_delivery_enabled() {
        false
    } else {
        requested
    }
}

#[must_use]
pub fn exploration_confidence_met(result: &ExploreResult) -> bool {
    if !crate::algorithm_runtime::strict_media_conversion_delivery_enabled() {
        return true;
    }
    if result.ultimate_mode
        && result.ultimate_quality_passed.is_passed()
        && result.has_complete_ultimate_quality_metrics()
    {
        return true;
    }
    result
        .confidence
        .is_some_and(|c| c.is_finite() && c >= crate::constants::MIN_EXPLORATION_CONFIDENCE)
}

#[must_use]
fn explore_quality_delivery_met(result: &ExploreResult, strict: bool) -> bool {
    if !result.quality_passed.is_passed() {
        return false;
    }
    if result.perceptual_quality_failed() {
        return false;
    }
    if strict && result.used_fallback {
        return false;
    }
    if result.uses_ultimate_quality_contract() {
        if strict {
            return result.has_complete_ultimate_quality_metrics()
                && result.ultimate_quality_passed.is_passed()
                && precision::ultimate_metrics_meet_exploration_sanity(
                    result.vmaf_y_score,
                    result.cambi_score,
                    result.psnr_uv_score,
                );
        }
        return result.ultimate_quality_passed.is_passed()
            || result.ultimate_quality_passed.is_skipped();
    }
    if strict {
        return result.ssim.is_some_and(f64::is_finite) && result.perceptual_quality_met();
    }
    true
}

#[must_use]
fn explore_size_delivery_met(result: &ExploreResult, strict: bool) -> bool {
    if strict {
        result.size_target_met.is_passed()
    } else {
        result.size_target_met.is_passed() || result.size_compression_met()
    }
}

/// JXL ultimate exploration delivery (image path).
#[must_use]
pub fn jxl_explore_delivery_acceptable(result: &JxlExploreResult) -> bool {
    result.delivery_acceptable()
}

/// Central explore delivery gate (single source for
/// `ExploreResult::pipeline_acceptable`).
#[must_use]
pub fn video_explore_pipeline_acceptable(
    result: &ExploreResult,
    match_quality: bool,
    explore_smaller: bool,
) -> bool {
    if !exploration_confidence_met(result) {
        return false;
    }
    let strict = crate::algorithm_runtime::strict_media_conversion_delivery_enabled();
    if match_quality || !explore_smaller {
        explore_quality_delivery_met(result, strict)
    } else {
        explore_size_delivery_met(result, strict)
    }
}

fn analysis_format_can_be_animated(format: &str) -> bool {
    matches!(
        format.to_ascii_uppercase().as_str(),
        "GIF" | "WEBP" | "PNG" | "APNG" | "AVIF" | "JXL" | "HEIC" | "HEIF"
    )
}

/// Align [`ImageAnalysis::is_animated`] with the same byte/ffprobe logic as
/// [`animation_reject_outcome`].
///
/// Format **family** comes from content sniffing (`analysis.format`), not from
/// trusting the path extension alone. This prevents legacy analyzer heuristics
/// (e.g. GIF duration/GCE hints) from disagreeing with the img static gate on
/// true single-frame assets.
pub fn reconcile_analysis_animation_flag(path: &Path, analysis: &mut ImageAnalysis) {
    if !analysis_format_can_be_animated(&analysis.format) {
        return;
    }
    let Some(detected_format) = detected_format_for_animation(&analysis.format) else {
        return;
    };

    match crate::image_detection::detect_animation(path, &detected_format) {
        Ok((detected_animated, frame_count, _)) => {
            let confirmed_static =
                match crate::image_detection::animatable_format_confirmed_static_only(
                    path,
                    &detected_format,
                    false,
                    frame_count,
                ) {
                    Ok(value) => value,
                    Err(err) => {
                        probe_layer_audit(
                            "analysis_animation_static_confirm",
                            path,
                            format!(
                                "Failed to confirm static-only animated-capable format; \
                                 preserving detected animation flag: {err}"
                            ),
                        );
                        false
                    }
                };
            if !detected_animated && confirmed_static {
                analysis.is_animated = false;
            } else {
                analysis.is_animated = detected_animated;
            }
        }
        Err(err) => {
            probe_layer_audit(
                "analysis_animation_detect",
                path,
                format!("detect_animation failed; fail-closed is_animated=false: {err}"),
            );
            analysis.is_animated = false;
        }
    }
}

fn detected_format_for_animation(format: &str) -> Option<crate::image_detection::DetectedFormat> {
    use crate::image_detection::DetectedFormat;

    match format.to_ascii_uppercase().as_str() {
        "GIF" => Some(DetectedFormat::GIF),
        "WEBP" => Some(DetectedFormat::WebP),
        "PNG" | "APNG" => Some(DetectedFormat::PNG),
        "AVIF" => Some(DetectedFormat::AVIF),
        "JXL" => Some(DetectedFormat::JXL),
        "HEIC" => Some(DetectedFormat::HEIC),
        "HEIF" => Some(DetectedFormat::HEIF),
        _ => None,
    }
}

/// Whether static conversion may proceed without recorded analysis uncertainty.
#[must_use]
pub const fn analysis_trusted_for_static_conversion(analysis: &ImageAnalysis) -> bool {
    analysis.analysis_error.is_none()
}

fn analysis_uncertainty_ignore_reason(analysis: &ImageAnalysis) -> Option<String> {
    analysis.analysis_error.as_ref().map(|err| {
        format!(
            "Analysis uncertainty for {} ({err}) - refusing static conversion",
            analysis.format
        )
    })
}

fn strict_entropy_trust_reason(analysis: &ImageAnalysis) -> Option<String> {
    if !crate::algorithm_runtime::strict_media_conversion_delivery_enabled() {
        return None;
    }
    if analysis.features.entropy.is_some_and(f64::is_finite) {
        return None;
    }
    Some(format!(
        "missing or non-finite entropy for {} - refusing static conversion",
        analysis.format
    ))
}

/// Strict static isolation + analysis-trust gate (fail-closed).
///
/// Animation ambiguity and [`ImageAnalysis::analysis_error`] always yield
/// [`StaticConversionVerdict::Ignore`]. Entropy trust and exploration
/// relaxations honour `strict_media_conversion_delivery_enabled` (see
/// `algorithm_runtime`).
#[must_use]
pub fn static_image_conversion_verdict(
    input: &Path,
    analysis: &ImageAnalysis,
) -> StaticConversionVerdict {
    if let Some(reason) = analysis_uncertainty_ignore_reason(analysis) {
        return StaticConversionVerdict::Ignore {
            reason,
            class: crate::infra::static_logs::audit_ignore_class::IMG_ANALYSIS_UNCERTAINTY,
        };
    }

    if let Some(outcome) = animation_reject_outcome(input, analysis) {
        return StaticConversionVerdict::Ignore {
            reason: outcome.reason,
            class: outcome.class,
        };
    }

    if let Some(reason) = strict_entropy_trust_reason(analysis) {
        return StaticConversionVerdict::Ignore {
            reason,
            class: crate::infra::static_logs::audit_ignore_class::IMG_STRICT_ENTROPY,
        };
    }

    StaticConversionVerdict::Proceed
}

/// Structured img ignore for animation detection (fast preflight + full
/// analysis).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImgIgnoreOutcome {
    pub reason: String,
    pub class: &'static str,
}

#[must_use]
pub fn animation_reject_reason(input: &Path, analysis: &ImageAnalysis) -> Option<String> {
    animation_reject_outcome(input, analysis).map(|o| o.reason)
}

const IMG_STATIC_ONLY: &str = "img is static-only; file ignored (not processed by img)";

#[must_use]
pub fn animation_reject_outcome(
    input: &Path,
    analysis: &ImageAnalysis,
) -> Option<ImgIgnoreOutcome> {
    use crate::infra::static_logs::audit_ignore_class::{
        IMG_ANIMATED_HANDOFF, IMG_ANIMATION_AMBIGUITY,
    };

    if !analysis_format_can_be_animated(&analysis.format) {
        return None;
    }

    let detected_format = detected_format_for_animation(&analysis.format)?;

    match crate::image_detection::detect_animation(input, &detected_format) {
        Ok((true, frame_count, _)) => Some(ImgIgnoreOutcome {
            reason: format!(
                "Animated {} verified (frame_count={}) — {IMG_STATIC_ONLY}",
                analysis.format,
                delivery_frame_count_label(frame_count, &analysis.format)
            ),
            class: IMG_ANIMATED_HANDOFF,
        }),
        Ok((false, frame_count, _)) => {
            match crate::image_detection::animatable_format_confirmed_static_only(
                input,
                &detected_format,
                false,
                frame_count,
            ) {
                Ok(true) => {
                    if analysis.is_animated {
                        probe_detection_recovery_audit(
                            "img_static_override_analyzer_animated",
                            format!(
                                "{}: multifaceted verification confirmed true \
                                 single-frame/static; img proceeds",
                                input.display()
                            ),
                        );
                    }
                    None
                }
                Ok(false) => Some(ImgIgnoreOutcome {
                    reason: format!(
                        "Cannot confirm static-only for {} (multi-stream/cover ambiguity or \
                         latent animation) — {IMG_STATIC_ONLY}",
                        analysis.format
                    ),
                    class: IMG_ANIMATION_AMBIGUITY,
                }),
                Err(err) => Some(ImgIgnoreOutcome {
                    reason: format!(
                        "Animation verification failed for {} ({err}) — {IMG_STATIC_ONLY}",
                        analysis.format
                    ),
                    class: IMG_ANIMATION_AMBIGUITY,
                }),
            }
        }
        Err(err) => {
            if analysis.is_animated {
                return Some(ImgIgnoreOutcome {
                    reason: format!(
                        "Animated {} indicated ({err}) — {IMG_STATIC_ONLY}",
                        analysis.format
                    ),
                    class: IMG_ANIMATED_HANDOFF,
                });
            }
            Some(ImgIgnoreOutcome {
                reason: format!(
                    "Animation verification failed for {} ({err}) — {IMG_STATIC_ONLY}",
                    analysis.format
                ),
                class: IMG_ANIMATION_AMBIGUITY,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::CheckResult;
    use serial_test::serial;

    #[test]
    fn analysis_error_yields_ignore_verdict() {
        let analysis = ImageAnalysis {
            format: "PNG".to_string(),
            analysis_error: Some("entropy missing".to_string()),
            ..ImageAnalysis::default()
        };
        assert!(!analysis_trusted_for_static_conversion(&analysis));
        let verdict = static_image_conversion_verdict(Path::new("/tmp/x.png"), &analysis);
        assert!(matches!(
            verdict,
            StaticConversionVerdict::Ignore {
                class: crate::infra::static_logs::audit_ignore_class::IMG_ANALYSIS_UNCERTAINTY,
                ..
            }
        ));
    }

    #[test]
    fn warm_start_crf_or_predicted_uses_codec_default_when_warm_start_missing() {
        let input = Path::new("/tmp/input.png");
        let predicted = 42.0_f32;

        let hevc = warm_start_crf_or_predicted(None, predicted, input, "hevc");
        assert!(
            (hevc - crate::constants::CRF_HEVC_DEFAULT).abs() < f32::EPSILON,
            "missing warm-start must fall back to HEVC codec default"
        );

        let unknown = warm_start_crf_or_predicted(None, predicted, input, "unknown_codec");
        assert!(
            (unknown - crate::constants::EXPLORE_DEFAULT_INITIAL_CRF).abs() < f32::EPSILON,
            "unknown codec must fall back to explore default"
        );
    }

    #[test]
    fn loop_audible_audio_unknown_stream_state_fails_closed() {
        assert!(loop_audible_audio_fail_closed(
            true,
            None,
            "loop-audio-test"
        ));
        assert!(!loop_audible_audio_fail_closed(
            false,
            None,
            "loop-audio-test"
        ));
        assert!(!loop_audible_audio_fail_closed(
            true,
            Some(false),
            "loop-audio-test"
        ));
        assert!(loop_audible_audio_fail_closed(
            true,
            Some(true),
            "loop-audio-test"
        ));
    }

    #[test]
    fn animation_reject_outcome_tags_animated_webp() {
        use std::io::Write;
        use tempfile::NamedTempFile;

        let webp = crate::image_formats::webp::synthetic_two_frame_animated_webp_for_test();
        let mut file = NamedTempFile::with_suffix(".webp").expect("temp webp");
        file.write_all(&webp).expect("write webp");
        let path = file.path();

        let analysis = ImageAnalysis {
            format: "WEBP".to_string(),
            ..ImageAnalysis::default()
        };
        let outcome = animation_reject_outcome(path, &analysis).expect("animated webp");
        assert_eq!(
            outcome.class,
            crate::infra::static_logs::audit_ignore_class::IMG_ANIMATED_HANDOFF
        );
    }

    #[test]
    fn animation_reject_outcome_tags_animated_apng_png() {
        use std::io::Write;
        use tempfile::NamedTempFile;

        let apng = crate::image_detection::synthetic_two_frame_apng_for_test();
        let mut file = NamedTempFile::with_suffix(".png").expect("temp png");
        file.write_all(&apng).expect("write apng");
        let path = file.path();

        let analysis = ImageAnalysis {
            format: "PNG".to_string(),
            is_animated: false,
            ..ImageAnalysis::default()
        };
        let outcome = animation_reject_outcome(path, &analysis).expect("animated apng");
        assert_eq!(
            outcome.class,
            crate::infra::static_logs::audit_ignore_class::IMG_ANIMATED_HANDOFF
        );
        assert!(
            !outcome.reason.contains("vid run"),
            "ignore reason must not instruct automatic vid invocation from img"
        );
    }

    #[test]
    fn animation_reject_outcome_tags_animated_handoff() {
        let analysis = ImageAnalysis {
            format: "WEBP".to_string(),
            is_animated: true,
            ..ImageAnalysis::default()
        };
        let outcome =
            animation_reject_outcome(Path::new("/tmp/x.webp"), &analysis).expect("animated webp");
        assert_eq!(
            outcome.class,
            crate::infra::static_logs::audit_ignore_class::IMG_ANIMATED_HANDOFF
        );
    }

    /// Valid single-frame GIF (decoder + ffprobe agree on one raster).
    fn synthetic_true_single_frame_gif87a() -> Vec<u8> {
        let mut gif_data = Vec::new();
        {
            let mut encoder = ::gif::Encoder::new(&mut gif_data, 1, 1, &[0, 0, 0, 255, 255, 255])
                .expect("encoder");
            let pixel = [0u8];
            encoder
                .write_frame(&::gif::Frame {
                    delay: 0,
                    width: 1,
                    height: 1,
                    buffer: std::borrow::Cow::Borrowed(&pixel),
                    ..Default::default()
                })
                .expect("frame");
        }
        gif_data
    }

    #[test]
    fn reconcile_clears_false_positive_animated_on_true_single_frame_gif() {
        use std::io::Write;
        use tempfile::NamedTempFile;

        let gif_data = synthetic_true_single_frame_gif87a();
        let mut file = NamedTempFile::with_suffix(".gif").expect("temp gif");
        file.write_all(&gif_data).expect("write gif");
        let path = file.path();

        let mut analysis = ImageAnalysis {
            format: "GIF".to_string(),
            is_animated: true,
            ..ImageAnalysis::default()
        };
        reconcile_analysis_animation_flag(path, &mut analysis);
        assert!(
            !analysis.is_animated,
            "legacy analyzer flag must align with detect_animation + static proof"
        );
    }

    #[test]
    fn true_single_frame_gif_proceeds_on_img_gate() {
        use std::io::Write;
        use tempfile::NamedTempFile;

        let gif_data = synthetic_true_single_frame_gif87a();
        let mut file = NamedTempFile::with_suffix(".gif").expect("temp gif");
        file.write_all(&gif_data).expect("write gif");
        let path = file.path();

        let analysis = ImageAnalysis {
            format: "GIF".to_string(),
            is_animated: true,
            ..ImageAnalysis::default()
        };
        assert!(
            crate::image_detection::animatable_format_confirmed_static_only(
                path,
                &crate::image_detection::DetectedFormat::GIF,
                false,
                Some(1),
            )
            .expect("gif static check")
        );
        assert!(
            animation_reject_outcome(path, &analysis).is_none(),
            "verified single-frame GIF must stay on img (override analyzer is_animated)"
        );
    }

    /// Single-frame `GIF89a` with GCE (timing extension only) must not be
    /// rejected for GCE alone.
    #[test]
    fn true_single_frame_gif89a_with_gce_proceeds_on_img_gate() {
        use std::io::Write;
        use tempfile::NamedTempFile;

        let mut gif_data = Vec::new();
        {
            let mut encoder = ::gif::Encoder::new(&mut gif_data, 1, 1, &[0, 0, 0, 255, 255, 255])
                .expect("encoder");
            let pixel = [0u8];
            encoder
                .write_frame(&::gif::Frame {
                    delay: 10,
                    width: 1,
                    height: 1,
                    buffer: std::borrow::Cow::Borrowed(&pixel),
                    ..Default::default()
                })
                .expect("frame");
        }

        let mut file = NamedTempFile::with_suffix(".gif").expect("temp gif");
        file.write_all(&gif_data).expect("write gif");
        let path = file.path();

        let analysis = ImageAnalysis {
            format: "GIF".to_string(),
            is_animated: true,
            ..ImageAnalysis::default()
        };
        assert!(
            !crate::image_detection::detect_animation(
                path,
                &crate::image_detection::DetectedFormat::GIF
            )
            .expect("detect")
            .0
        );
        assert!(
            animation_reject_outcome(path, &analysis).is_none(),
            "single-frame GIF89a with GCE is still a static still for img"
        );
    }

    #[test]
    fn analysis_error_ignored_for_non_animatable_format() {
        let analysis = ImageAnalysis {
            format: "JPEG".to_string(),
            analysis_error: Some("probe failed".to_string()),
            ..ImageAnalysis::default()
        };
        let verdict = static_image_conversion_verdict(Path::new("/tmp/x.jpg"), &analysis);
        assert!(matches!(
            verdict,
            StaticConversionVerdict::Ignore {
                reason: _,
                class: _
            }
        ));
    }

    #[test]
    fn trusted_static_analysis_proceeds_without_animation() {
        let analysis = ImageAnalysis {
            format: "JPEG".to_string(),
            features: crate::image_analyzer::ImageFeatures {
                entropy: Some(7.5),
                ..Default::default()
            },
            ..ImageAnalysis::default()
        };
        assert!(analysis_trusted_for_static_conversion(&analysis));
        let verdict = static_image_conversion_verdict(Path::new("/tmp/x.jpg"), &analysis);
        assert_eq!(verdict, StaticConversionVerdict::Proceed);
    }

    #[test]
    #[serial]
    fn strict_mode_suppresses_size_tolerance_even_when_requested() {
        let _strict = crate::common_utils::EnvGuard::set(
            crate::constants::ENV_DISABLE_STRICT_MEDIA_CONVERSION,
            "0",
        );
        assert!(!effective_allow_size_tolerance(true));
        assert!(!effective_allow_size_tolerance(false));

        let _relax = crate::common_utils::EnvGuard::set(
            crate::constants::ENV_DISABLE_STRICT_MEDIA_CONVERSION,
            "1",
        );
        assert!(effective_allow_size_tolerance(true));
        assert!(!effective_allow_size_tolerance(false));
    }

    #[test]
    #[serial]
    fn strict_explore_requires_confidence_and_size_target() {
        let _guard = crate::common_utils::EnvGuard::set(
            crate::constants::ENV_DISABLE_STRICT_MEDIA_CONVERSION,
            "0",
        );
        let weak = ExploreResult {
            size_target_met: CheckResult::Passed,
            quality_passed: CheckResult::NotChecked,
            size_change_pct: -8.0,
            confidence: Some(0.1),
            ..ExploreResult::default()
        };
        assert!(!video_explore_pipeline_acceptable(&weak, false, true));

        let ok = ExploreResult {
            size_target_met: CheckResult::Passed,
            quality_passed: CheckResult::NotChecked,
            confidence: Some(crate::constants::MIN_EXPLORATION_CONFIDENCE + 0.1),
            ..ExploreResult::default()
        };
        assert!(video_explore_pipeline_acceptable(&ok, false, true));
    }

    #[test]
    #[serial]
    fn strict_explore_rejects_psnr_derived_ssim_fallback() {
        let _guard = crate::common_utils::EnvGuard::set(
            crate::constants::ENV_DISABLE_STRICT_MEDIA_CONVERSION,
            "0",
        );
        let result = ExploreResult {
            quality_passed: CheckResult::Passed,
            ssim: Some(0.96),
            used_fallback: true,
            confidence: Some(crate::constants::MIN_EXPLORATION_CONFIDENCE + 0.1),
            ..ExploreResult::default()
        };
        assert!(!video_explore_pipeline_acceptable(&result, true, false));
    }

    #[test]
    #[serial]
    fn strict_explore_rejects_ms_ssim_failed_while_quality_passed() {
        let _guard = crate::common_utils::EnvGuard::set(
            crate::constants::ENV_DISABLE_STRICT_MEDIA_CONVERSION,
            "0",
        );
        let result = ExploreResult {
            quality_passed: CheckResult::Passed,
            ms_ssim_passed: CheckResult::Failed("SSIM below target".into()),
            ssim: Some(0.99),
            confidence: Some(crate::constants::MIN_EXPLORATION_CONFIDENCE + 0.1),
            ..ExploreResult::default()
        };
        assert!(!video_explore_pipeline_acceptable(&result, true, false));
    }

    #[test]
    #[serial]
    fn non_strict_explore_rejects_ultimate_quality_failed() {
        let _guard = crate::common_utils::EnvGuard::set(
            crate::constants::ENV_DISABLE_STRICT_MEDIA_CONVERSION,
            "1",
        );
        let result = ExploreResult {
            quality_passed: CheckResult::Passed,
            ultimate_quality_passed: CheckResult::Failed("3D quality gate failed".into()),
            ultimate_mode: true,
            vmaf_y_score: Some(90.0),
            confidence: Some(crate::constants::MIN_EXPLORATION_CONFIDENCE + 0.1),
            ..ExploreResult::default()
        };
        assert!(!video_explore_pipeline_acceptable(&result, true, false));
    }

    #[test]
    #[serial]
    fn strict_explore_ultimate_accepts_vmaf_without_ssim() {
        let _guard = crate::common_utils::EnvGuard::set(
            crate::constants::ENV_DISABLE_STRICT_MEDIA_CONVERSION,
            "0",
        );
        let result = ExploreResult {
            quality_passed: CheckResult::Passed,
            ultimate_quality_passed: CheckResult::Passed,
            ultimate_mode: true,
            vmaf_y_score: Some(97.0),
            cambi_score: Some(5.0),
            psnr_uv_score: Some((50.0, 49.0)),
            confidence: Some(crate::constants::MIN_EXPLORATION_CONFIDENCE + 0.1),
            ssim: None,
            ..ExploreResult::default()
        };
        assert!(video_explore_pipeline_acceptable(&result, true, false));
    }

    #[test]
    #[serial]
    fn strict_explore_ultimate_bypasses_confidence_when_3d_passed() {
        let _guard = crate::common_utils::EnvGuard::set(
            crate::constants::ENV_DISABLE_STRICT_MEDIA_CONVERSION,
            "0",
        );
        let result = ExploreResult {
            quality_passed: CheckResult::Passed,
            ultimate_quality_passed: CheckResult::Passed,
            ultimate_mode: true,
            vmaf_y_score: Some(97.0),
            cambi_score: Some(5.0),
            psnr_uv_score: Some((50.0, 49.0)),
            confidence: Some(0.05),
            ssim: None,
            ..ExploreResult::default()
        };
        assert!(video_explore_pipeline_acceptable(&result, true, false));
    }

    #[test]
    #[serial]
    fn strict_explore_ultimate_rejects_cambi_above_sanity_ceiling() {
        let _guard = crate::common_utils::EnvGuard::set(
            crate::constants::ENV_DISABLE_STRICT_MEDIA_CONVERSION,
            "0",
        );
        let high_cambi = ExploreResult {
            quality_passed: CheckResult::Passed,
            ultimate_quality_passed: CheckResult::Passed,
            ultimate_mode: true,
            vmaf_y_score: Some(97.0),
            cambi_score: Some(10.0),
            psnr_uv_score: Some((50.0, 49.0)),
            ..ExploreResult::default()
        }
        .sealed();
        assert!(!high_cambi.quality_passed.is_passed());
        assert!(!video_explore_pipeline_acceptable(&high_cambi, true, false));
    }

    #[test]
    #[serial]
    fn strict_explore_ultimate_rejects_sanity_floor_vmaf() {
        let _guard = crate::common_utils::EnvGuard::set(
            crate::constants::ENV_DISABLE_STRICT_MEDIA_CONVERSION,
            "0",
        );
        let weak_vmaf = ExploreResult {
            quality_passed: CheckResult::Passed,
            ultimate_quality_passed: CheckResult::Passed,
            ultimate_mode: true,
            vmaf_y_score: Some(80.0),
            cambi_score: Some(5.0),
            psnr_uv_score: Some((40.0, 40.0)),
            confidence: Some(crate::constants::MIN_EXPLORATION_CONFIDENCE + 0.1),
            ..ExploreResult::default()
        }
        .sealed();
        assert!(!weak_vmaf.quality_passed.is_passed());
        assert!(!video_explore_pipeline_acceptable(&weak_vmaf, true, false));
    }

    #[test]
    #[serial]
    fn strict_explore_ultimate_rejects_passed_without_full_3d_metrics() {
        let _guard = crate::common_utils::EnvGuard::set(
            crate::constants::ENV_DISABLE_STRICT_MEDIA_CONVERSION,
            "0",
        );
        let incomplete = ExploreResult {
            quality_passed: CheckResult::Passed,
            ultimate_quality_passed: CheckResult::Passed,
            ultimate_mode: true,
            vmaf_y_score: Some(97.0),
            confidence: Some(crate::constants::MIN_EXPLORATION_CONFIDENCE + 0.1),
            ..ExploreResult::default()
        }
        .sealed();
        assert!(!incomplete.quality_passed.is_passed());
        assert!(!video_explore_pipeline_acceptable(&incomplete, true, false));
    }

    #[test]
    #[serial]
    fn strict_explore_ultimate_requires_ultimate_mode_flag_not_metrics_alone() {
        let _guard = crate::common_utils::EnvGuard::set(
            crate::constants::ENV_DISABLE_STRICT_MEDIA_CONVERSION,
            "0",
        );
        let with_metrics_only = ExploreResult {
            quality_passed: CheckResult::Passed,
            ultimate_quality_passed: CheckResult::Passed,
            ultimate_mode: false,
            vmaf_y_score: Some(97.0),
            cambi_score: Some(5.0),
            psnr_uv_score: Some((50.0, 49.0)),
            ms_ssim_passed: CheckResult::Failed("SSIM below target".into()),
            confidence: Some(0.05),
            ..ExploreResult::default()
        };
        assert!(!with_metrics_only.uses_ultimate_quality_contract());
        assert!(!video_explore_pipeline_acceptable(
            &with_metrics_only,
            true,
            false
        ));
    }

    #[test]
    fn gif_encode_fps_prefers_extracted_frame_duration() {
        let fps =
            gif_encode_fps_from_probe(Path::new("/tmp/anim.gif"), 2.0, 60, Some(24.0), Some(30.0));
        let fps = fps.expect("extracted frame duration should yield fps");
        assert!(crate::float_compare::approx_eq_f64(fps, 30.0));
    }

    #[test]
    fn color_depth_optional_refuses_forged_eight() {
        assert_eq!(color_depth_optional(None, "unit_test"), None);
        assert_eq!(color_depth_optional(Some(10), "unit_test"), Some(10));
    }

    #[test]
    fn explore_boundary_crf_err_when_no_refine() {
        let err = explore_boundary_crf_optional(None, 23.5, Path::new("/tmp/in.mp4"))
            .expect_err("missing refinement must not substitute boundary_crf");
        assert!(
            err.to_string().contains("no refinement"),
            "unexpected: {err}"
        );
    }

    #[test]
    fn recovery_channel_type_defaults_unknown() {
        let label = recovery_channel_type_label(None, Path::new("/tmp/x.bin"));
        assert_eq!(label, "unknown");
    }

    #[test]
    fn gif_encode_fps_falls_back_to_avg_with_audit_path() {
        let fps = gif_encode_fps_from_probe(Path::new("/tmp/anim.gif"), 0.0, 0, Some(25.0), None);
        let fps = fps.expect("average frame duration should yield fps");
        assert!(crate::float_compare::approx_eq_f64(fps, 25.0));
    }

    #[test]
    fn delivery_db_diag_cell_or_unknown_matches_map_or_semantics() {
        assert_eq!(delivery_db_diag_cell_or_unknown(Some("layer7")), "layer7");
        assert_eq!(delivery_db_diag_cell_or_unknown(Some("")), "");
        assert_eq!(delivery_db_diag_cell_or_unknown(None), "?");
    }

    #[test]
    fn ui_ssim_inline_or_empty_omits_segment_when_missing() {
        assert!(ui_ssim_inline_or_empty(None, "unit_test").is_empty());
        let present = ui_ssim_inline_or_empty(Some(0.95), "unit_test");
        assert!(present.starts_with("SSIM "));
    }

    #[test]
    fn ui_ssim_inline_when_unmeasured_omits_without_audit_path() {
        assert!(ui_ssim_inline_when_unmeasured(None).is_empty());
        assert!(explore_progress_ssim_token_pending(None).is_empty());
        assert_eq!(ui_ssim_inline_when_unmeasured(Some(0.95)), "SSIM 0.9500");
    }

    #[test]
    fn loop_collection_secs_or_baseline_policy_skips_audit_without_percentiles() {
        let value = loop_collection_secs_or_baseline_policy(
            None,
            15.0,
            "collection.duration_p90",
            "unit_test",
            false,
            false,
        );
        assert!((value - 15.0).abs() < f64::EPSILON);
    }

    #[test]
    fn jxl_previous_candidate_size_audits_baseline_fallback() {
        let size = jxl_previous_candidate_size_or_fallback(None, 42, "unit_test");
        assert_eq!(size, 42);
        let size = jxl_previous_candidate_size_or_fallback(Some(99), 42, "unit_test");
        assert_eq!(size, 99);
    }

    #[test]
    fn algorithm_feature_distribution_required_rejects_absent_key() {
        let err = algorithm_feature_distribution_required(None, "duration")
            .expect_err("missing key must fail closed");
        assert!(err.to_string().contains("duration"), "unexpected: {err}");
    }

    #[test]
    fn loop_collection_secs_or_baseline_policy_discards_untrusted_some() {
        let value = loop_collection_secs_or_baseline_policy(
            Some(99.0),
            15.0,
            "collection.duration_avg",
            "unit_test",
            false,
            false,
        );
        assert!(
            (value - 15.0).abs() < f64::EPSILON,
            "untrusted collection field must not drive thresholds without histogram or provenance"
        );
    }

    #[test]
    fn loop_duration_or_fallback_policy_skips_audit_without_percentiles() {
        let value =
            loop_duration_or_fallback_policy(None, 12.0, "duration.p25", "unit_test", false);
        assert!((value - 12.0).abs() < f64::EPSILON);
        let ignores_synthetic =
            loop_duration_or_fallback_policy(Some(99.0), 12.0, "duration.p25", "unit_test", false);
        assert!(
            (ignores_synthetic - 12.0).abs() < f64::EPSILON,
            "collection-derived percentiles must not affect thresholds when histogram absent"
        );
    }

    #[test]
    fn loop_scaled_duration_percentile_policy_ignores_synthetic_without_histogram() {
        let value = loop_scaled_duration_percentile_or_fallback_policy(
            Some(20.0),
            8.0,
            1.5,
            "duration.p50",
            "unit_test",
            false,
        );
        assert!(
            (value - 8.0).abs() < f64::EPSILON,
            "scaled percentile must not apply when profile lacks empirical histogram"
        );
    }

    #[test]
    fn loop_collection_duration_p90_discards_feature_stats_without_samples() {
        let value = loop_collection_duration_p90_or_baseline(
            Some(42.0),
            15.0,
            "collection.duration_p90",
            "unit_test",
            false,
            false,
        );
        assert!(
            (value - 15.0).abs() < f64::EPSILON,
            "feature-stats P90 must not drive thresholds without sample provenance"
        );
    }

    #[test]
    fn loop_collection_duration_p90_accepts_sample_provenance_without_histogram() {
        let value = loop_collection_duration_p90_or_baseline(
            Some(42.0),
            15.0,
            "collection.duration_p90",
            "unit_test",
            false,
            true,
        );
        assert!(
            (value - 42.0).abs() < f64::EPSILON,
            "sample-derived collection P90 must remain usable without profile histogram"
        );
    }

    #[test]
    fn loop_collection_duration_p90_discards_non_sample_when_profile_has_histogram() {
        let value = loop_collection_duration_p90_or_baseline(
            Some(42.0),
            15.0,
            "collection.duration_p90",
            "unit_test",
            true,
            false,
        );
        assert!(
            (value - 15.0).abs() < f64::EPSILON,
            "profile histogram must not legitimize non-sample collection P90"
        );
    }

    #[test]
    fn json_inference_optional_f64_or_null_is_silent_null() {
        assert!(json_inference_optional_f64_or_null(None).is_null());
        assert_eq!(
            json_inference_optional_f64_or_null(Some(1.5)),
            serde_json::json!(1.5)
        );
    }

    #[test]
    fn ui_exit_code_suffix_or_empty_omits_when_unknown() {
        assert!(ui_exit_code_suffix_or_empty(None, "unit_test").is_empty());
        assert_eq!(
            ui_exit_code_suffix_or_empty(Some(1), "unit_test"),
            " (exit code: 1)"
        );
    }

    #[test]
    fn processed_path_key_skips_audit_for_nonexistent_path() {
        let missing = Path::new("/nonexistent/mfb-processed-path-key-test-xyzzy");
        assert!(!missing.exists());
        let key = processed_path_key(missing);
        assert_eq!(key, missing.display().to_string());
    }

    #[cfg(unix)]
    #[test]
    fn delivery_remove_file_or_audit_removes_broken_symlink() -> std::io::Result<()> {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir()?;
        let link = dir.path().join("broken-link");
        symlink(dir.path().join("missing-target"), &link)?;

        assert!(!link.exists());
        assert!(std::fs::symlink_metadata(&link).is_ok());

        delivery_remove_file_or_audit("unit_test_broken_symlink", &link);

        assert!(std::fs::symlink_metadata(&link).is_err());

        Ok(())
    }

    #[test]
    fn canonicalize_for_checkpoint_matches_tool_input_on_existing_path() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("probe.bin");
        std::fs::write(&file, b"x").expect("write fixture");
        let tool = canonicalize_for_tool_input(&file);
        let checkpoint = canonicalize_for_checkpoint_path(&file);
        assert_eq!(tool, checkpoint);
    }

    #[test]
    fn loop_scaled_duration_percentile_uses_unscaled_fallback_when_missing() {
        let scale = crate::constants::LOOP_INTENT_MOTION_MEDIAN_SCALE;
        let fallback = 4.0_f64;
        let missing = loop_scaled_duration_percentile_or_fallback(
            None,
            fallback,
            scale,
            "duration.p50",
            "unit_test",
        );
        assert!((missing - fallback).abs() < f64::EPSILON);
        let present = loop_scaled_duration_percentile_or_fallback(
            Some(10.0),
            fallback,
            scale,
            "duration.p50",
            "unit_test",
        );
        let expected = 10.0_f64 * scale;
        assert!(
            (present - expected).abs() < f64::EPSILON,
            "present={present} expected={expected} scale={scale}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn path_file_stem_helpers_preserve_non_utf8_for_delivery_join() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;

        let non_utf8 = OsStr::from_bytes(b"\xff\xfe");
        let path = Path::new("/tmp").join(non_utf8).with_extension("jpg");

        let os_stem = path_file_stem_os_or_delivery_err(&path, "unit_test")
            .expect("non-UTF-8 stem must remain joinable");
        assert_eq!(os_stem, non_utf8);

        let lossy = path_file_stem_or_empty(&path, "unit_test");
        assert!(
            !lossy.is_empty(),
            "metadata stem must not be empty for non-UTF-8"
        );

        assert!(
            path_file_stem_utf8_or_delivery_err(&path, "unit_test").is_err(),
            "UTF-8-only helper must reject non-UTF-8 stems"
        );
    }

    #[test]
    fn algorithm_feature_distribution_required_rejects_missing_slot() {
        let err = algorithm_feature_distribution_required(None, "duration")
            .expect_err("missing feature_stats slot must fail closed");
        assert!(
            err.to_string().contains("duration"),
            "unexpected error: {err}"
        );
    }
}
