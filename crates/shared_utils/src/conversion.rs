//! Conversion Utilities Module
//!
//! Provides common conversion functionality shared across all tools:
//! - `TaskResult`: Unified result structure
//! - `ConvertOptions`: Common conversion options
//! - Anti-duplicate mechanism: Track processed files
//! - Result builders: Reduce boilerplate code
//! - Size formatting: Unified message formatting
//!
//! ## Atomic output (TOCTOU)
//! All conversion paths **must** write to a temp path via `temp_path_for_output()` then
//! call `commit_temp_to_output_with_metadata(temp, output, force, original)`.
//! Do not write directly to the final output.
//!
//! ## Compress mode (authoritative)
//! When `options.compress` is true: **only** `output_size < input_size` is accepted.
//! **Any** `output_size >= input_size` (including equal) is rejected — goal not achieved.
//! All size checks use `>=` for this; do not change to `>`.
//!
//! ## `allow_size_tolerance` (default true)
//! When true: "oversized" threshold is `output size increase < 1_048_576 bytes` (accept). Video path may treat
//! `video_compression_ratio < 1.01` as acceptable when `require_compression` is checked.
//! Does **not** mean "accept up to `1_048_576` bytes larger as success" for compress goal — compress still requires output < input.

#![cfg_attr(test, allow(clippy::field_reassign_with_default))]

use crate::Rational;
use crate::builder_base::ToolBuilder;
use crate::constants::MIN_OUTPUT_SIZE_BEFORE_DELETE_IMAGE;
use crate::conversion_types::SelectedCodec;
use crate::ffprobe::probe_video;
use crate::metadata::preserve;
use crate::modern_ui::{colors, symbols};
use crate::quality_matcher::is_apple_native_format;
use crate::smart_file_copier::copy_on_skip_or_fail;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::{
    LazyLock, Mutex,
    atomic::{AtomicU64, Ordering},
};
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::warn;

static PROCESSED_FILES: LazyLock<Mutex<HashSet<String>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));
static RESERVED_OUTPUT_PATHS: LazyLock<Mutex<HashMap<String, String>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
static TEMP_OUTPUT_COUNTER: AtomicU64 = AtomicU64::new(0);

#[cfg(test)]
static TEST_RESERVATION_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

pub fn next_temp_output_suffix() -> String {
    const ALPHABET: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let timestamp = match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(d) => d.as_nanos(),
        Err(e) => {
            crate::log_anomaly!(
                crate::static_logs::messages::LABEL_SYSTEM,
                &format!("System time before Unix Epoch: {e}")
            );
            0
        }
    };
    let pid = u128::from(std::process::id());
    let counter = u128::from(TEMP_OUTPUT_COUNTER.fetch_add(1, Ordering::Relaxed));
    let mut value = timestamp ^ (pid << crate::constants::PID_SHIFT_FOR_HASH) ^ counter;
    let mut suffix = [b'0'; 10];

    for slot in suffix.iter_mut().rev() {
        let idx = (value % ALPHABET.len() as u128) as usize;
        *slot = *ALPHABET.get(idx).unwrap_or(&b'0');
        value /= ALPHABET.len() as u128;
    }

    String::from_utf8_lossy(&suffix).into_owned()
}

pub fn is_already_processed(path: &Path) -> bool {
    let canonical = path
        .canonicalize()
        .ok()
        .and_then(|p| p.to_str().map(String::from))
        .unwrap_or_else(|| path.display().to_string());

    let processed = PROCESSED_FILES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    processed.contains(&canonical)
}

pub fn mark_as_processed(path: &Path) {
    let canonical = path
        .canonicalize()
        .ok()
        .and_then(|p| p.to_str().map(String::from))
        .unwrap_or_else(|| path.display().to_string());

    let mut processed = PROCESSED_FILES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    processed.insert(canonical);
}

pub fn clear_processed_list() {
    let mut processed = PROCESSED_FILES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    processed.clear();
}

/// Generates a stable key for a path by canonicalizing it.
///
/// If canonicalization fails, falls back to the original path.
///
/// # Arguments
/// * `path` - The path to generate a key for
///
/// # Returns
/// Stable string representation of the path
fn stable_path_key(path: &Path) -> String {
    path.canonicalize()
        .unwrap_or_else(|_| path.to_path_buf())
        .display()
        .to_string()
}

/// Creates a path with a collision suffix to avoid filename conflicts.
///
/// # Arguments
/// * `path` - The original path
/// * `collision_index` - The collision index to append
///
/// # Returns
/// New path with collision suffix
fn path_with_collision_suffix(path: &Path, collision_index: usize) -> PathBuf {
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("output");
    let file_name = match path.extension().and_then(|e| e.to_str()) {
        Some(ext) if !ext.is_empty() => format!("{stem} ({collision_index}).{ext}"),
        _ => format!("{stem} ({collision_index})"),
    };

    path.parent()
        .unwrap_or_else(|| Path::new(""))
        .join(file_name)
}

/// Reserves a unique output path to avoid conflicts.
///
/// Uses a reservation system to ensure no two inputs map to the same output.
///
/// # Arguments
/// * `input` - The input file path
/// * `candidate` - The desired output path
///
/// # Returns
/// Unique output path that doesn't conflict
fn reserve_unique_output_path(input: &Path, candidate: PathBuf) -> PathBuf {
    let input_key = stable_path_key(input);
    let mut reservations = RESERVED_OUTPUT_PATHS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);

    let mut resolved = candidate;
    let mut collision_index = crate::constants::COLLISION_INDEX_START;

    loop {
        let output_key = stable_path_key(&resolved);
        match reservations.get(&output_key) {
            Some(owner) if owner != &input_key => {
                resolved = path_with_collision_suffix(&resolved, collision_index);
                collision_index += 1;
            }
            _ => {
                reservations.insert(output_key, input_key);
                drop(reservations);
                return resolved;
            }
        }
    }
}

#[must_use]
pub fn reserve_output_path(input: &Path, candidate: &Path) -> PathBuf {
    reserve_unique_output_path(input, candidate.to_path_buf())
}

#[cfg(test)]
fn clear_reserved_output_paths() {
    let mut reservations = RESERVED_OUTPUT_PATHS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    reservations.clear();
}

pub use crate::checkpoint::{safe_delete_original, verify_output_integrity};

/// Acquires an exclusive file lock using Unix flock.
///
/// # Arguments
/// * `file` - The file to lock
///
/// # Returns
/// Ok(()) if lock acquired, or IO error if failed
#[cfg(unix)]
fn flock_exclusive(file: &fs::File) -> std::io::Result<()> {
    use std::os::unix::io::AsRawFd;
    let ret = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) };
    if ret == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(unix)]
struct ProcessedListLockGuard(std::os::unix::io::RawFd);

#[cfg(unix)]
impl Drop for ProcessedListLockGuard {
    fn drop(&mut self) {
        let _ = unsafe { libc::flock(self.0, libc::LOCK_UN) };
    }
}

/// Load the processed files list.
///
/// # Errors
///
/// Returns an error if the file cannot be read or deserialized.
pub fn load_processed_list(list_path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    if !list_path.exists() {
        return Ok(());
    }

    let file = fs::File::open(list_path)?;
    #[cfg(unix)]
    flock_exclusive(&file)?;
    #[cfg(unix)]
    let _flock_guard = ProcessedListLockGuard(std::os::unix::io::AsRawFd::as_raw_fd(&file));
    let reader = BufReader::new(&file);
    let mut loaded = HashSet::new();

    let mut read_error = None;
    for line in reader.lines() {
        match line {
            Ok(path) => {
                loaded.insert(path);
            }
            Err(err) => {
                if read_error.is_none() {
                    read_error = Some(err);
                }
            }
        }
    }

    if let Some(err) = read_error {
        return Err(Box::new(err));
    }

    PROCESSED_FILES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .extend(loaded);

    Ok(())
}

/// Save the processed files list.
///
/// # Errors
///
/// Returns an error if the file cannot be written or serialized.
pub fn save_processed_list(list_path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let processed_paths: Vec<String> = PROCESSED_FILES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .iter()
        .cloned()
        .collect();
    let mut file = fs::File::create(list_path)?;
    #[cfg(unix)]
    flock_exclusive(&file)?;
    #[cfg(unix)]
    let _flock_guard = ProcessedListLockGuard(std::os::unix::io::AsRawFd::as_raw_fd(&file));

    for path in processed_paths {
        writeln!(file, "{path}")?;
    }
    file.flush()?;

    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskResult {
    pub success: bool,
    pub input_path: String,
    pub output_path: Option<String>,
    pub input_size: u64,
    pub output_size: Option<u64>,
    pub size_reduction: Option<f64>,
    pub message: String,
    pub skipped: bool,
    pub ignored: bool,
    pub skip_reason: Option<String>,
    pub blake3: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Outcome {
    Converted,
    Skipped,
    FallbackPreserved,
    Ignored,
    Failed,
}

/// Metrics for a video exploration outcome, used to populate `TaskResult`.
#[derive(Debug, Clone, Default)]
pub struct VideoExplorationMetrics<'a> {
    pub input_size: u64,
    pub output_size: u64,
    pub codec_name: &'a str,
    pub crf: f32,
    pub is_lossless: bool,
    pub iterations: u32,
    pub ssim: Option<f64>,
    pub explored_from_crf: Option<f32>,
    pub quality_label: Option<&'a str>,
}

impl VideoExplorationMetrics<'_> {
    /// # Errors
    /// Returns an error if the size difference is out of i64 range.
    pub fn format_message(&self, reduction_pct: f64) -> anyhow::Result<String> {
        let reduction = reduction_pct / crate::constants::PERCENTAGE_FACTOR;
        let size_tag = if reduction >= 0.0_f64 {
            format!("\x1b[1;32m-{reduction_pct:.1}%\x1b[0m")
        } else {
            let diff_bytes = i128::from(self.output_size) - i128::from(self.input_size);
            let diff_bytes_i64 = crate::numeric_cast::i128_to_i64_strict(diff_bytes, "diff_bytes")
                .ok_or_else(|| anyhow::anyhow!("Value out of i64 range for diff_bytes"))?;
            let size_diff = crate::modern_ui::format_size_diff(diff_bytes_i64);
            format!("\x1b[1;33m{size_diff}\x1b[0m")
        };

        let crf_display = if self.is_lossless {
            format!("{:.2} (Lossless)", self.crf)
        } else {
            format!("{:.2}", self.crf)
        };

        let explored_msg = match self.explored_from_crf {
            Some(from) if (self.crf - from).abs() > crate::constants::CRF_COMPARISON_EPSILON => {
                format!(" (explored from CRF {from:.1})")
            }
            _ => String::new(),
        };

        let ssim_msg = self
            .ssim
            .map(|s| format!(", SSIM: {s:.4}"))
            .unwrap_or_default();

        let core_msg = format!(
            "{codec} (CRF {crf_display}{explored_msg}, {iterations} iter{ssim_msg}): {size_tag}",
            codec = self.codec_name.to_uppercase(),
            iterations = self.iterations,
        );

        let formatted = self.quality_label.filter(|q| !q.is_empty()).map_or_else(
            || format!("✅ {core_msg}"),
            |q| format!("✅ {q} | {core_msg}"),
        );
        Ok(formatted)
    }
}

impl TaskResult {
    fn copy_original_for_fallback(
        input: &Path,
        options: &ConvertOptions,
        _phase: &str,
    ) -> Option<PathBuf> {
        if options.should_copy_original_on_skip(input) {
            crate::smart_file_copier::copy_on_skip_or_fail(
                input,
                options.output_dir.as_deref(),
                options.base_dir.as_deref(),
                options.verbose(),
            )
            .ok()
            .flatten()
        } else {
            crate::log_anomaly!(
                crate::static_logs::messages::LABEL_CONVERSION,
                &format!(
                    "{}: (input={input_display})",
                    crate::static_logs::messages::APPLE_COMPAT_NOT_COPYING,
                    input_display = input.display()
                )
            );
            if options.verbose() {
                crate::log_hint!(
                    crate::static_logs::messages::LABEL_CONVERSION,
                    crate::static_logs::messages::APPLE_COMPAT_NOT_COPYING_DETAILED
                );
            }
            None
        }
    }

    #[must_use]
    pub const fn outcome(&self) -> Outcome {
        if self.ignored {
            Outcome::Ignored
        } else if self.skipped {
            if self.success {
                Outcome::Skipped
            } else {
                Outcome::FallbackPreserved
            }
        } else if self.success {
            Outcome::Converted
        } else {
            Outcome::Failed
        }
    }

    #[must_use]
    pub fn with_blake3(mut self, hash: String) -> Self {
        self.blake3 = Some(hash);
        self
    }
    #[must_use]
    pub fn is_jpeg_transcode(&self) -> bool {
        // After terminology fix, "transcoding" is only used for JPEG bitstream reconstruction (lossless JXL)
        self.message.contains("transcoding") || self.message.contains("JPEG lossless")
    }

    #[must_use]
    /// # Panics
    /// Panics if file metadata cannot be accessed.
    pub fn skipped_duplicate(input: &Path) -> Self {
        let input_size = fs::metadata(input)
            .unwrap_or_else(|e| panic!("FATAL: Metadata unreachable during duplicate check: {e}"))
            .len();
        Self {
            success: true,
            input_path: input.display().to_string(),
            output_path: None,
            input_size,
            output_size: None,
            size_reduction: None,
            message: "Skipped: Already processed".to_string(),
            skipped: true,
            ignored: false,
            skip_reason: Some("duplicate".to_string()),
            blake3: None,
        }
    }

    #[must_use]
    /// # Panics
    /// Panics if file metadata cannot be accessed.
    pub fn skipped_exists(input: &Path, output: &Path) -> Self {
        let input_size = fs::metadata(input)
            .unwrap_or_else(|e| panic!("FATAL: Metadata unreachable during exist check: {e}"))
            .len();
        Self {
            success: true,
            input_path: input.display().to_string(),
            output_path: Some(output.display().to_string()),
            input_size,
            output_size: fs::metadata(output).map(|m| m.len()).ok(),
            size_reduction: None,
            message: "Skipped: Output file exists".to_string(),
            skipped: true,
            ignored: false,
            skip_reason: Some("exists".to_string()),
            blake3: None,
        }
    }

    #[must_use]
    pub fn skipped_custom(input: &Path, input_size: u64, reason: &str, skip_reason: &str) -> Self {
        Self {
            success: true,
            input_path: input.display().to_string(),
            output_path: None,
            input_size,
            output_size: None,
            size_reduction: None,
            message: reason.to_string(),
            skipped: true,
            ignored: false,
            skip_reason: Some(skip_reason.to_string()),
            blake3: None,
        }
    }

    #[must_use]
    pub fn skipped_size_increase(input: &Path, input_size: u64, output_size: u64) -> Self {
        let diff_bytes = i128::from(output_size) - i128::from(input_size);
        let size_diff = crate::numeric_cast::i128_to_i64_strict(diff_bytes, "size_diff")
            .map_or_else(
                || "> i64::MAX".to_string(),
                crate::modern_ui::format_size_diff,
            );
        Self {
            success: true,
            input_path: input.display().to_string(),
            output_path: None,
            input_size,
            output_size: None,
            size_reduction: None,
            message: format!("Skipped: Output would be larger ({size_diff})"),
            skipped: true,
            ignored: false,
            skip_reason: Some("size_increase".to_string()),
            blake3: None,
        }
    }

    /// Used when compress mode is on and output size equals input (goal: must be strictly smaller).
    #[must_use]
    pub fn skipped_size_unchanged(input: &Path, input_size: u64, format_label: &str) -> Self {
        Self {
            success: true,
            input_path: input.display().to_string(),
            output_path: None,
            input_size,
            output_size: None,
            size_reduction: None,
            message: format!(
                "Skipped: {format_label} output size unchanged (compression goal not achieved)"
            ),
            skipped: true,
            ignored: false,
            skip_reason: Some("size_unchanged".to_string()),
            blake3: None,
        }
    }

    #[must_use]
    pub fn skipped_with_fallback(
        input: &Path,
        options: &ConvertOptions,
        reason: &str,
        skip_reason_id: &str,
    ) -> Self {
        Self::skipped_with_fallback_owned(
            input,
            options,
            reason.to_string(),
            skip_reason_id.to_string(),
        )
    }

    #[must_use]
    pub fn skipped_with_fallback_owned(
        input: &Path,
        options: &ConvertOptions,
        reason: String,
        skip_reason_id: String,
    ) -> Self {
        let input_size = fs::metadata(input).map_or_else(
            |e| {
                crate::log_anomaly!(
                    crate::static_logs::messages::LABEL_CONVERSION,
                    &format!(
                        "Failed to read metadata for {}; defaulting to size 0. Error: {e}",
                        input.display()
                    )
                );
                0
            },
            |m| m.len(),
        );
        let copied_dest = Self::copy_original_for_fallback(input, options, "skip");
        crate::conversion::mark_as_processed(input);

        Self {
            success: true,
            input_path: input.display().to_string(),
            output_path: copied_dest
                .as_ref()
                .map(|p| p.display().to_string())
                .or_else(|| Some(input.display().to_string())),
            input_size,
            output_size: copied_dest
                .as_ref()
                .and_then(|p| fs::metadata(p).ok())
                .map(|m| m.len()),
            size_reduction: None,
            message: reason,
            skipped: true,
            ignored: false,
            skip_reason: Some(skip_reason_id),
            blake3: None,
        }
    }

    #[must_use]
    pub fn failed_with_fallback(
        input: &Path,
        options: &ConvertOptions,
        reason: &str,
        skip_reason_id: &str,
    ) -> Self {
        Self::failed_with_fallback_owned(
            input,
            options,
            reason.to_string(),
            skip_reason_id.to_string(),
        )
    }

    #[must_use]
    pub fn failed_with_fallback_owned(
        input: &Path,
        options: &ConvertOptions,
        reason: String,
        skip_reason_id: String,
    ) -> Self {
        let input_size = fs::metadata(input).map_or_else(
            |e| {
                crate::log_anomaly!(
                    crate::static_logs::messages::LABEL_CONVERSION,
                    &format!(
                        "Failed to read metadata for {}; defaulting to size 0. Error: {e}",
                        input.display()
                    )
                );
                0
            },
            |m| m.len(),
        );
        let copied_dest = Self::copy_original_for_fallback(input, options, "failure");
        crate::conversion::mark_as_processed(input);

        Self {
            success: false,
            input_path: input.display().to_string(),
            output_path: copied_dest
                .as_ref()
                .map(|p| p.display().to_string())
                .or_else(|| Some(input.display().to_string())),
            input_size,
            output_size: copied_dest
                .as_ref()
                .and_then(|p| fs::metadata(p).ok())
                .map(|m| m.len()),
            size_reduction: None,
            message: reason,
            skipped: true,
            ignored: false,
            skip_reason: Some(skip_reason_id),
            blake3: None,
        }
    }

    #[must_use]
    pub fn converted_with_message(
        input: &Path,
        output: &Path,
        input_size: u64,
        output_size: u64,
        message: &str,
    ) -> Self {
        Self::converted_with_message_owned(
            input,
            output,
            input_size,
            output_size,
            message.to_string(),
        )
    }

    #[must_use]
    pub fn converted_with_message_owned(
        input: &Path,
        output: &Path,
        input_size: u64,
        output_size: u64,
        message: String,
    ) -> Self {
        let size_reduction = if input_size == 0 {
            0.0_f64
        } else {
            (1.0_f64
                - (crate::numeric_cast::u64_to_f64(output_size)
                    / crate::numeric_cast::u64_to_f64(input_size)))
                * crate::constants::PERCENTAGE_FACTOR
        };

        Self {
            success: true,
            input_path: input.display().to_string(),
            output_path: Some(output.display().to_string()),
            input_size,
            output_size: Some(output_size),
            size_reduction: Some(size_reduction),
            message,
            skipped: false,
            ignored: false,
            skip_reason: None,
            blake3: None,
        }
    }

    #[must_use]
    pub fn success(
        input: &Path,
        output: &Path,
        input_size: u64,
        output_size: u64,
        format_name: &str,
        extra_info: Option<&str>,
        quality_label: Option<&str>,
    ) -> Self {
        let reduction = if input_size == 0 {
            0.0_f64
        } else {
            1.0_f64
                - (crate::numeric_cast::u64_to_f64(output_size)
                    / crate::numeric_cast::u64_to_f64(input_size))
        };
        let reduction_pct = reduction * crate::constants::PERCENTAGE_FACTOR;

        // Build size-change suffix: "-14.5%" (saved) or "+2.1MB" (grew) with ANSI colors
        let size_tag = if reduction >= 0.0_f64 {
            format!("\x1b[1;32m-{reduction_pct:.1}%\x1b[0m")
        } else {
            let diff_bytes = i128::from(output_size) - i128::from(input_size);
            let diff_bytes_i64 = crate::numeric_cast::i128_to_i64_strict(diff_bytes, "size_diff")
                .unwrap_or(i64::MAX);
            let size_diff = crate::modern_ui::format_size_diff(diff_bytes_i64);
            format!("\x1b[1;33m{size_diff}\x1b[0m")
        };

        // Determine technically accurate verb:
        // - "transcoding" specifically for bitstream reconstruction (JPEG -> JXL)
        // - "encoding" for all other conversions from source pixels
        let is_jpeg = input.extension().is_some_and(|e| {
            let ext = e.to_string_lossy().to_lowercase();
            matches!(
                ext.as_str(),
                "jpg" | "jpeg" | "jpe" | "jif" | "jfif" | "jfi" | "jxr"
            )
        }) || extra_info.is_some_and(|i| i.to_lowercase().contains("jpeg"));

        let action = if is_jpeg && format_name.eq_ignore_ascii_case("JXL") {
            "transcoding"
        } else {
            "encoding"
        };

        // Message body (no ✅ here — caller already emits it).
        // Format: "✅ <FormatName> <Action>: -14.5%"
        let core_msg = extra_info.map_or_else(
            || format!("{format_name} {action}: {size_tag}"),
            |info| format!("{format_name} {action} ({info}): {size_tag}"),
        );

        let message = quality_label.filter(|q| !q.is_empty()).map_or_else(
            || format!("✅ {core_msg}"),
            |q| format!("✅ {q} | {core_msg}"),
        );

        Self {
            success: true,
            input_path: input.display().to_string(),
            output_path: Some(output.display().to_string()),
            input_size,
            output_size: Some(output_size),
            size_reduction: Some(reduction_pct),
            message,
            skipped: false,
            ignored: false,
            skip_reason: None,
            blake3: None,
        }
    }

    #[must_use]
    pub fn success_video_explored(
        input: &Path,
        output: &Path,
        metrics: &VideoExplorationMetrics<'_>,
    ) -> Self {
        let reduction_pct = if metrics.input_size == 0 {
            0.0_f64
        } else {
            (1.0_f64
                - (crate::numeric_cast::u64_to_f64(metrics.output_size)
                    / crate::numeric_cast::u64_to_f64(metrics.input_size)))
                * crate::constants::PERCENTAGE_FACTOR
        };

        let message = match metrics.format_message(reduction_pct) {
            Ok(m) => m,
            Err(e) => {
                warn!("Failed to format video exploration message: {}", e);
                String::from("(formatting error)")
            }
        };

        Self {
            success: true,
            input_path: input.display().to_string(),
            output_path: Some(output.display().to_string()),
            input_size: metrics.input_size,
            output_size: Some(metrics.output_size),
            size_reduction: Some(reduction_pct),
            message,
            skipped: false,
            ignored: false,
            skip_reason: None,
            blake3: None,
        }
    }
}

bitflags::bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
    pub struct ConvertFlags: u32 {
        const FORCE = 1 << 0;
        const DELETE_ORIGINAL = 1 << 1;
        const IN_PLACE = 1 << 2;
        const EXPLORE = 1 << 3;
        const MATCH_QUALITY = 1 << 4;
        const APPLE_COMPAT = 1 << 5;
        const COMPRESS = 1 << 6;
        const USE_GPU = 1 << 7;
        const ULTIMATE = 1 << 8;
        const ALLOW_SIZE_TOLERANCE = 1 << 9;
        const VERBOSE = 1 << 10;
    }
}

#[derive(Debug, Clone)]
pub struct ConvertOptions {
    pub flags: ConvertFlags,
    pub output_dir: Option<PathBuf>,
    pub base_dir: Option<PathBuf>,
    pub codec: SelectedCodec,
    pub child_threads: usize,
    pub input_format: Option<String>,
    pub quality_label: Option<String>,
}

impl ConvertOptions {
    #[must_use]
    pub const fn force(&self) -> bool {
        self.flags.contains(ConvertFlags::FORCE)
    }
    #[must_use]
    pub const fn delete_original(&self) -> bool {
        self.flags.contains(ConvertFlags::DELETE_ORIGINAL)
    }
    #[must_use]
    pub const fn in_place(&self) -> bool {
        self.flags.contains(ConvertFlags::IN_PLACE)
    }
    #[must_use]
    pub const fn explore(&self) -> bool {
        self.flags.contains(ConvertFlags::EXPLORE)
    }
    #[must_use]
    pub const fn match_quality(&self) -> bool {
        self.flags.contains(ConvertFlags::MATCH_QUALITY)
    }
    #[must_use]
    pub const fn apple_compat(&self) -> bool {
        self.flags.contains(ConvertFlags::APPLE_COMPAT)
    }
    #[must_use]
    pub const fn compress(&self) -> bool {
        self.flags.contains(ConvertFlags::COMPRESS)
    }
    #[must_use]
    pub const fn use_gpu(&self) -> bool {
        self.flags.contains(ConvertFlags::USE_GPU)
    }
    #[must_use]
    pub const fn ultimate(&self) -> bool {
        self.flags.contains(ConvertFlags::ULTIMATE)
    }
    #[must_use]
    pub const fn allow_size_tolerance(&self) -> bool {
        self.flags.contains(ConvertFlags::ALLOW_SIZE_TOLERANCE)
    }
    #[must_use]
    pub const fn verbose(&self) -> bool {
        self.flags.contains(ConvertFlags::VERBOSE)
    }
}

impl Default for ConvertOptions {
    fn default() -> Self {
        Self {
            flags: ConvertFlags::USE_GPU | ConvertFlags::ALLOW_SIZE_TOLERANCE,
            output_dir: None,
            base_dir: None,
            codec: SelectedCodec::Hevc,
            child_threads: 0,
            input_format: None,
            quality_label: None,
        }
    }
}

impl ConvertOptions {
    #[must_use]
    pub fn should_copy_original_on_skip(&self, input: &Path) -> bool {
        if !self.apple_compat() {
            return true;
        }
        let input_ext = input
            .extension()
            .and_then(|ext| ext.to_str())
            .map(str::to_lowercase)
            .unwrap_or_default();
        is_apple_native_format(&input_ext)
    }

    #[must_use]
    pub const fn should_delete_original(&self) -> bool {
        self.delete_original() || self.in_place()
    }

    /// Determine the flag mode from options.
    ///
    /// # Errors
    /// Returns an error message if flag combination is invalid.
    pub fn flag_mode(&self) -> Result<crate::flag_validator::FlagMode, String> {
        crate::flag_validator::validate_flags_result_with_ultimate(
            crate::flag_validator::FlagRequest {
                base: crate::flag_validator::FlagBase {
                    explore: self.explore(),
                    match_quality: self.match_quality(),
                    compress: self.compress(),
                },
                tier: crate::flag_validator::FlagTier {
                    ultimate: self.ultimate(),
                },
            },
        )
    }

    #[must_use]
    pub const fn explore_mode(&self) -> crate::video_explorer::ExploreMode {
        // flag_mode() result is irrelevant — always use PreciseQualityMatchWithCompression
        crate::video_explorer::ExploreMode::PreciseQualityMatchWithCompression
    }
}

/// Determine the output path for a file.
///
/// # Errors
/// Returns an error message if determination fails.
pub fn determine_output_path(
    input: &Path,
    extension: &str,
    output_dir: &Option<PathBuf>,
) -> Result<PathBuf, String> {
    let stem = input
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("output");

    let up_ext = extension.to_uppercase();
    let output = match output_dir {
        Some(dir) => {
            fs::create_dir_all(dir).map_err(|e| {
                format!("Failed to create output directory {}: {}", dir.display(), e)
            })?;
            dir.join(format!("{stem}.{up_ext}"))
        }
        None => input.with_extension(up_ext),
    };

    let input_canonical = input.canonicalize().unwrap_or_else(|_| input.to_path_buf());
    let output_canonical = if output.exists() {
        output.canonicalize().unwrap_or_else(|_| output.clone())
    } else {
        output.clone()
    };

    if input_canonical == output_canonical || input == output {
        return Err(format!(
            "Input and output paths are identical: {}\n\
             Tip: use --output/-o for a different output dir, or --in-place to replace in place (deletes original)",
            input.display()
        ));
    }

    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent).map_err(|e| {
            format!(
                "Failed to create output parent directory {}: {}",
                parent.display(),
                e
            )
        })?;
    }

    validate_output_path(&output, None)?;

    Ok(reserve_unique_output_path(input, output))
}

/// Determine the output path with a base directory.
///
/// # Errors
/// Returns an error message if determination fails.
pub fn determine_output_path_with_base(
    input: &Path,
    base_dir: &Path,
    extension: &str,
    output_dir: &Option<PathBuf>,
) -> Result<PathBuf, String> {
    let stem = input
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("output");

    let up_ext = extension.to_uppercase();
    let output = match output_dir {
        Some(dir) => {
            let rel_path = input
                .strip_prefix(base_dir)
                .unwrap_or(input)
                .parent()
                .unwrap_or_else(|| Path::new(""));

            let out_subdir = dir.join(rel_path);
            fs::create_dir_all(&out_subdir).map_err(|e| {
                format!(
                    "Failed to create output directory {}: {}",
                    out_subdir.display(),
                    e
                )
            })?;

            out_subdir.join(format!("{stem}.{up_ext}"))
        }
        None => input.with_extension(up_ext),
    };

    let input_canonical = input.canonicalize().unwrap_or_else(|_| input.to_path_buf());
    let output_canonical = if output.exists() {
        output.canonicalize().unwrap_or_else(|_| output.clone())
    } else {
        output.clone()
    };

    if input_canonical == output_canonical || input == output {
        return Err(format!(
            "Input and output paths are identical: {}\n\
             Tip: use --output/-o for a different output dir, or --in-place to replace in place (deletes original)",
            input.display()
        ));
    }

    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent).map_err(|e| {
            format!(
                "Failed to create output parent directory {}: {}",
                parent.display(),
                e
            )
        })?;
    }

    validate_output_path(&output, Some(base_dir))?;

    Ok(reserve_unique_output_path(input, output))
}

/// # Errors
/// Returns an error if the size difference calculation overflows i64.
pub fn format_size_change(
    input_size: u64,
    output_size: u64,
) -> crate::unified_error::Result<String> {
    let reduction = if input_size == 0 {
        0.0_f64
    } else {
        (Rational::from(1) - (Rational::from(output_size) / Rational::from(input_size))).to_f64()
    };
    let reduction_pct = reduction * crate::constants::PERCENTAGE_FACTOR;

    if reduction >= 0.0 {
        Ok(format!("size reduced {reduction_pct:.1}%"))
    } else {
        let diff_bytes = crate::numeric_cast::u64_to_i64_strict(output_size, "output_size")
            .ok_or_else(|| {
                crate::unified_error::ImgQualityError::NumericError(
                    "output_size cast to i64 failed".into(),
                )
            })?
            .saturating_sub(crate::numeric_cast::u64_to_i64_sat(input_size));
        let size_diff = crate::modern_ui::format_size_diff(diff_bytes);
        Ok(format!(
            "size increased {:.1}% ({})",
            -reduction_pct, size_diff
        ))
    }
}

#[must_use]
pub fn calculate_size_reduction(input_size: u64, output_size: u64) -> f64 {
    if input_size == 0 {
        return 0.0;
    }
    ((Rational::from(1) - (Rational::from(output_size) / Rational::from(input_size)))
        * Rational::from(100))
    .to_f64()
}

/// Pre-conversion check: tests duplicate and output-exists skip conditions.
///
/// **TOCTOU note**: The `output.exists()` check here is advisory only.
/// Callers MUST use `temp_path_for_output()` + `commit_temp_to_output()`
/// to write atomically; do NOT rely on this check as a write guard.
#[must_use]
pub fn pre_conversion_check(
    input: &Path,
    output: &Path,
    options: &ConvertOptions,
) -> Option<TaskResult> {
    if !options.force() && is_already_processed(input) {
        return Some(TaskResult::skipped_duplicate(input));
    }

    if output.exists() && !options.force() {
        return Some(TaskResult::skipped_exists(input, output));
    }

    None
}

/// Finalize the conversion process.
///
/// # Errors
/// Returns an `io::Result` if finalization fails.
pub fn finalize_task(
    input: &Path,
    output: &Path,
    input_size: u64,
    format_name: &str,
    extra_info: Option<&str>,
    options: &ConvertOptions,
) -> std::io::Result<TaskResult> {
    let output_size = std::fs::metadata(output)?.len();

    // Metadata already preserved by commit_temp_to_output_with_metadata
    // (includes EXIF, XMP, xattrs, permissions, and timestamps)

    mark_as_processed(input);

    if format_name.eq_ignore_ascii_case("JXL") {
        crate::progress_mode::jxl_success();
    }

    if options.should_delete_original() {
        safe_delete_original(input, output, MIN_OUTPUT_SIZE_BEFORE_DELETE_IMAGE)?;
    }

    Ok(TaskResult::success(
        input,
        output,
        input_size,
        output_size,
        format_name,
        extra_info,
        options.quality_label.as_deref(),
    ))
}

/// Perform post-conversion actions.
///
/// # Errors
/// Returns an `io::Result` if actions fail.
pub fn post_conversion_actions(
    input: &Path,
    output: &Path,
    options: &ConvertOptions,
) -> std::io::Result<()> {
    if let Err(e) = preserve(input, output) {
        crate::log_anomaly!(
            crate::static_logs::messages::LABEL_METADATA,
            &format!("Failed to preserve metadata: {e}")
        );
    }

    mark_as_processed(input);

    if options.should_delete_original() {
        safe_delete_original(input, output, MIN_OUTPUT_SIZE_BEFORE_DELETE_IMAGE)?;
    }

    Ok(())
}

// --- Atomic output (TOCTOU mitigation) ---

/// Guard that removes the temp file on drop if it still exists (e.g. conversion failed before commit).
///
/// Hold this for the lifetime of conversion; after successful `commit_temp_to_output` the file is gone so drop is a no-op.
pub struct TempOutputGuard(PathBuf);

impl TempOutputGuard {
    #[must_use]
    pub const fn new(path: PathBuf) -> Self {
        Self(path)
    }
}

impl Drop for TempOutputGuard {
    fn drop(&mut self) {
        if self.0.exists() {
            let _ = crate::io_utils::safe_remove_file(&self.0);
        }
    }
}

/// **LEAKY**: Returns a path for temporary output in the same directory as `output`.
///
/// \[WARNING\] This function pollutes the user's folder with intermediate files.
/// For Ghost Mode (Zero Pollution), use `shared_utils::path_safety::isolated_temp_path_for_search` instead.
///
/// Ensures `fs::rename(temp, output)` is atomic on the same filesystem. Use with `commit_temp_to_output`.
/// Uses stem + ".tmp." + extension (e.g. file.mov → file.tmp.mov) so `FFmpeg` and other
/// tools that infer format from extension still see the correct extension (mov, mp4, mkv, etc.).
#[must_use]
pub fn temp_path_for_output(output: &Path) -> PathBuf {
    let stem = output
        .file_stem()
        .map(|s| s.to_string_lossy())
        .unwrap_or_default();
    let ext = output
        .extension()
        .map(|e| e.to_string_lossy())
        .unwrap_or_default();
    let parent = output.parent().unwrap_or_else(|| Path::new("."));

    // Use a timestamp/pid/counter suffix so temp naming stays branch-agnostic across rand APIs.
    let random_id = next_temp_output_suffix();

    parent.join(format!("{stem}.tmp.{random_id}.{ext}"))
}

/// **DEPRECATED AND REMOVED**: This function has been removed.
///
/// All conversions MUST preserve metadata. Use `commit_temp_to_output_with_metadata` instead.
///
/// This function previously did NOT preserve metadata (timestamps, EXIF, XMP, xattrs, permissions),
/// which violated the program's core requirement of comprehensive metadata preservation.
#[deprecated(
    since = "0.10.71",
    note = "Removed. Use commit_temp_to_output_with_metadata instead."
)]
/// Commit a temporary file to the final output location.
///
/// # Errors
/// Returns an `io::Result` if commit fails.
pub fn commit_temp_to_output(_temp: &Path, _output: &Path, _force: bool) -> std::io::Result<bool> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "commit_temp_to_output has been removed; use commit_temp_to_output_with_metadata instead",
    ))
}

/// Commits a temp file with complete metadata preservation from the original file.
///
/// Preserves: timestamps (atime, mtime, btime), xattrs, permissions, EXIF data, XMP sidecars.
/// Commit a temporary file to the final output location with metadata preservation.
///
/// # Errors
/// Returns an `io::Result` if commit fails.
pub fn commit_temp_to_output_with_metadata(
    temp: &Path,
    output: &Path,
    force: bool,
    original: Option<&Path>,
) -> std::io::Result<bool> {
    validate_output_path(output, None).map_err(std::io::Error::other)?;

    if temp.exists() {
        let size = fs::metadata(temp)?.len();
        if size == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "Refusing to commit empty output (temp file size 0)",
            ));
        }
    }
    let in_place_commit = temp == output;

    if !in_place_commit && !force && output.exists() {
        let _ = crate::io_utils::safe_remove_file(temp);
        return Ok(false);
    }

    if !in_place_commit {
        crate::io_utils::robust_move(temp, output)?;
    }

    // Preserve complete metadata from original file if provided
    if let Some(src) = original {
        // Step 1: Preserve metadata (EXIF, XMP, xattrs, permissions)
        // This may modify the file (e.g., ExifTool writes EXIF/XMP), which changes timestamps
        if let Err(e) = crate::metadata::preserve(src, output) {
            crate::log_upstream_error!(
                "Metadata preservation",
                "Failed to preserve metadata for {}: {}",
                output.display(),
                e
            );
        }
        crate::metadata::merge_xmp_sidecar_into_dest(src, output);

        // Step 2: Finder comment branding — only on the committed conversion output
        #[cfg(target_os = "macos")]
        {
            let ext = output
                .extension()
                .and_then(|e| e.to_str())
                .map(str::to_lowercase)
                .unwrap_or_default();
            if (ext == "jxl" || ext == "mov" || ext == "mp4" || ext == "heic" || ext == "avif")
                && let Err(e) = crate::metadata::append_mfb_branding(output)
            {
                crate::log_info!(
                    crate::static_logs::messages::LABEL_METADATA,
                    &format!("Failed to append MFB branding to Finder comment: {e}")
                );
            }
        }

        // Step 3: Apply timestamps AFTER all file modifications
        // This is critical because ExifTool and other tools reset creation time to current time
        // We must reapply timestamps as the final step to preserve original creation time
        crate::metadata::apply_file_timestamps(src, output);
    }

    Ok(true)
}

/// Read image dimensions directly from the file header without external dependencies.
///
/// Supports the hot-path image formats (GIF/PNG/JPEG/WebP/BMP). Much faster and more
/// reliable than subprocess fallbacks — works regardless of ffprobe/ImageMagick availability
/// and handles filenames with non-ASCII characters uniformly.
///
/// Returns `None` if the format is unsupported or the header is malformed.
#[must_use]
pub fn dimensions_from_header(input: &Path) -> Option<(u32, u32)> {
    use std::io::Read;

    const PNG_MAGIC: [u8; 8] = [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];

    let mut file = std::fs::File::open(input).ok()?;
    // Large enough to cover all supported header layouts (JPEG SOF can appear a few KB in).
    let mut head = [0u8; 4096];
    let n = file.read(&mut head).ok()?;
    let head = head.get(..n)?;

    // GIF: magic "GIF87a"/"GIF89a", logical screen width/height at bytes 6..10 as little-endian u16.
    if head.len() >= 10 && (head.starts_with(b"GIF87a") || head.starts_with(b"GIF89a")) {
        let w = u16::from_le_bytes([head[6], head[7]]);
        let h = u16::from_le_bytes([head[8], head[9]]);
        if w > 0 && h > 0 {
            return Some((u32::from(w), u32::from(h)));
        }
    }

    // PNG: magic 89 50 4E 47 0D 0A 1A 0A then IHDR chunk at offset 8 (4 len + "IHDR" + width/height BE u32).
    if head.len() >= 24 && head.starts_with(&PNG_MAGIC) && &head[12..16] == b"IHDR" {
        let w = u32::from_be_bytes([head[16], head[17], head[18], head[19]]);
        let h = u32::from_be_bytes([head[20], head[21], head[22], head[23]]);
        if w > 0 && h > 0 {
            return Some((w, h));
        }
    }

    // BMP: magic "BM", DIB header width/height at offsets 18..22 and 22..26 (little-endian i32; height can be negative).
    if head.len() >= 26 && head.starts_with(b"BM") {
        let w = i32::from_le_bytes([head[18], head[19], head[20], head[21]]);
        let h = i32::from_le_bytes([head[22], head[23], head[24], head[25]]);
        if w > 0 && h != 0 {
            return Some((w.unsigned_abs(), h.unsigned_abs()));
        }
    }

    // WebP: "RIFF" <size> "WEBP" then VP8/VP8L/VP8X chunk.
    if head.len() >= 30 && head.starts_with(b"RIFF") && &head[8..12] == b"WEBP" {
        let chunk = &head[12..16];
        if chunk == b"VP8 " && head.len() >= 30 {
            // Lossy VP8 bitstream: width/height at chunk offset 14 (14-bit values after 3-byte frame tag).
            let w = u16::from_le_bytes([head[26], head[27]]) & 0x3FFF;
            let h = u16::from_le_bytes([head[28], head[29]]) & 0x3FFF;
            if w > 0 && h > 0 {
                return Some((u32::from(w), u32::from(h)));
            }
        } else if chunk == b"VP8L" && head.len() >= 25 {
            // Lossless VP8L: signature 0x2F then 28 bits = (width-1)<<0 | (height-1)<<14.
            let sig = head[20];
            if sig == 0x2F {
                let b1 = u32::from(head[21]);
                let b2 = u32::from(head[22]);
                let b3 = u32::from(head[23]);
                let b4 = u32::from(head[24]);
                let w = (b1 | ((b2 & 0x3F) << 8)) + 1;
                let h = (((b2 & 0xC0) >> 6) | (b3 << 2) | ((b4 & 0x0F) << 10)) + 1;
                if w > 0 && h > 0 {
                    return Some((w, h));
                }
            }
        } else if chunk == b"VP8X" && head.len() >= 30 {
            // Extended: canvas width-1 at offset 24..27, canvas height-1 at 27..30 (24-bit LE).
            let w =
                (u32::from(head[24]) | (u32::from(head[25]) << 8) | (u32::from(head[26]) << 16))
                    + 1;
            let h =
                (u32::from(head[27]) | (u32::from(head[28]) << 8) | (u32::from(head[29]) << 16))
                    + 1;
            if w > 0 && h > 0 {
                return Some((w, h));
            }
        }
    }

    // JPEG: FF D8 start of image.
    if head.starts_with(&[0xFF, 0xD8]) {
        return scan_jpeg_dimensions(&mut file, head);
    }

    // ISOBMFF (HEIC/HEIF/AVIF/MP4): ftyp box.
    if head.len() >= 16 && &head[4..8] == b"ftyp" {
        return scan_isobmff_dimensions(&mut file, head);
    }

    // JPEG XL container: magic bytes.
    if head.len() >= 12 && &head[0..12] == b"\x00\x00\x00\x0CJXL \x0D\x0A\x87\x0A" {
        return scan_jxl_container_dimensions(&mut file, head);
    }

    None
}

fn scan_jpeg_dimensions(file: &mut std::fs::File, head: &[u8]) -> Option<(u32, u32)> {
    use std::io::{Read, Seek, SeekFrom};
    // We may need more than the initial 4KB; widen the buffer progressively.
    let mut buf: Vec<u8> = head.to_vec();
    // Seek restart for sequential scan.
    if file.seek(SeekFrom::Start(buf.len() as u64)).is_ok() {
        let mut more = [0u8; 8192];
        while let Ok(read_n) = file.read(&mut more) {
            if read_n == 0 {
                break;
            }
            buf.extend_from_slice(&more[..read_n]);
            if buf.len() >= 2_097_152 {
                break; // Hard cap at 2 MiB; avoid OOM on pathological files.
            }
        }
    }

    let mut i = 2_usize;
    while i + 8 < buf.len() {
        if buf[i] != 0xFF {
            return None;
        }
        // Skip padding FF bytes.
        while i + 1 < buf.len() && buf[i + 1] == 0xFF {
            i += 1;
        }
        if i + 1 >= buf.len() {
            return None;
        }
        let marker = buf[i + 1];
        i += 2;
        // Standalone markers without payload.
        if marker == 0xD8 || marker == 0xD9 || (0xD0..=0xD7).contains(&marker) {
            continue;
        }
        if i + 2 > buf.len() {
            return None;
        }
        let seg_len = u16::from_be_bytes([buf[i], buf[i + 1]]) as usize;
        // SOFn (Start of Frame): 0xC0, 0xC1, 0xC2, 0xC3, 0xC5-0xC7, 0xC9-0xCB, 0xCD-0xCF.
        let is_sof = (0xC0..=0xCF).contains(&marker)
            && marker != 0xC4 // DHT
            && marker != 0xC8 // JPG
            && marker != 0xCC; // DAC
        if is_sof && i + 7 <= buf.len() {
            let h = u16::from_be_bytes([buf[i + 3], buf[i + 4]]);
            let w = u16::from_be_bytes([buf[i + 5], buf[i + 6]]);
            if w > 0 && h > 0 {
                return Some((u32::from(w), u32::from(h)));
            }
            return None;
        }
        if seg_len < 2 {
            return None;
        }
        i += seg_len;
    }
    None
}

fn scan_isobmff_dimensions(file: &mut std::fs::File, head: &[u8]) -> Option<(u32, u32)> {
    use std::io::{Read, Seek, SeekFrom};
    // Fully buffer up to 2 MiB — "meta" box can appear after "mdat" for HEIF.
    let mut buf: Vec<u8> = head.to_vec();
    if file.seek(SeekFrom::Start(buf.len() as u64)).is_ok() {
        let mut more = [0u8; 16_384];
        while let Ok(read_n) = file.read(&mut more) {
            if read_n == 0 {
                break;
            }
            buf.extend_from_slice(&more[..read_n]);
            if buf.len() >= 2_097_152 {
                break;
            }
        }
    }
    scan_isobmff_ispe(&buf)
}

fn scan_jxl_container_dimensions(file: &mut std::fs::File, head: &[u8]) -> Option<(u32, u32)> {
    use std::io::{Read, Seek, SeekFrom};
    // ISOBMFF-style JXL container: same scan_isobmff_ispe works since JXL uses image item props too.
    let mut buf: Vec<u8> = head.to_vec();
    if file.seek(SeekFrom::Start(buf.len() as u64)).is_ok() {
        let mut more = [0u8; 16_384];
        while let Ok(read_n) = file.read(&mut more) {
            if read_n == 0 {
                break;
            }
            buf.extend_from_slice(&more[..read_n]);
            if buf.len() >= 1_048_576 {
                break;
            }
        }
    }
    scan_isobmff_ispe(&buf)
}

/// Scan ISOBMFF byte slice for the first `ispe` (Image Spatial Extents) box and return (width, height).
///
/// ISOBMFF layout: box = [4-byte BE length][4-byte type]{payload...}
/// For `ispe` v0: payload = [1-byte version=0][3-byte flags=0][4-byte width BE][4-byte height BE].
/// We scan recursively through container boxes (`meta`, `iprp`, `ipco`) to find `ispe`.
fn scan_isobmff_ispe(buf: &[u8]) -> Option<(u32, u32)> {
    // Container boxes whose children we must recurse into.
    const CONTAINERS: &[&[u8; 4]] = &[b"meta", b"iprp", b"ipco", b"moov", b"trak", b"mdia"];

    fn recurse(data: &[u8], depth: u32) -> Option<(u32, u32)> {
        if depth > 8 {
            return None; // Defend against pathological nesting.
        }
        let mut i: usize = 0;
        while i + 8 <= data.len() {
            let size =
                u32::from_be_bytes([data[i], data[i + 1], data[i + 2], data[i + 3]]) as usize;
            let box_type = &data[i + 4..i + 8];

            // Header + payload offset. size==1 means 64-bit extended size follows.
            let (hdr_len, total_len) = if size == 1 {
                if i + 16 > data.len() {
                    return None;
                }
                let ext = u64::from_be_bytes([
                    data[i + 8],
                    data[i + 9],
                    data[i + 10],
                    data[i + 11],
                    data[i + 12],
                    data[i + 13],
                    data[i + 14],
                    data[i + 15],
                ]);
                (16_usize, usize::try_from(ext).unwrap_or(usize::MAX))
            } else if size == 0 {
                // Extends to end of stream.
                (8usize, data.len() - i)
            } else {
                (8usize, size)
            };

            if total_len < hdr_len || i + total_len > data.len() {
                // Box extends past buffer; try partial payload for containers.
                if CONTAINERS.iter().any(|&t| t == box_type)
                    && let Some(dims) = recurse(&data[i + hdr_len..], depth + 1)
                {
                    return Some(dims);
                }
                return None;
            }

            let payload = &data[i + hdr_len..i + total_len];

            if box_type == b"ispe" && payload.len() >= 12 {
                // v0 ispe: 4 bytes (version+flags) + 4 bytes width + 4 bytes height.
                let w = u32::from_be_bytes([payload[4], payload[5], payload[6], payload[7]]);
                let h = u32::from_be_bytes([payload[8], payload[9], payload[10], payload[11]]);
                if w > 0 && h > 0 {
                    return Some((w, h));
                }
            }

            // "meta" is a FullBox: skip 4 bytes of version+flags before its children.
            let recurse_payload = if box_type == b"meta" && payload.len() >= 4 {
                &payload[4..]
            } else {
                payload
            };

            if CONTAINERS.iter().any(|&t| t == box_type)
                && let Some(dims) = recurse(recurse_payload, depth + 1)
            {
                return Some(dims);
            }

            i += total_len;
        }
        None
    }

    recurse(buf, 0)
}

/// Media info fallback chain that does NOT invoke ffprobe.
///
/// Tries the `image` crate first, then `ImageMagick identify` with extended format strings
/// to extract REAL metadata (width, height, `channel_type`, depth) directly from the bitstream.
/// This fulfills the "Zero-Forgery" mandate by using actual measured data.
#[must_use]
pub fn media_info_without_ffprobe(input: &Path) -> Option<(u32, u32, String, u8)> {
    // Stage 0: Native header parse — fastest path, no subprocess, no external deps.
    // Covers GIF/PNG/JPEG/WebP/BMP which are the vast majority of files.
    // We only have dimensions here; channel/depth extraction still requires a bitstream analyzer.
    if let Some((w, h)) = dimensions_from_header(input) {
        return Some((w, h, "unknown".to_string(), 8));
    }

    // Stage 1: Fast in-process image crate (handles more formats including some TIFF/ICO variants).
    if let Ok(img) =
        image::ImageReader::open(input).and_then(image::ImageReader::with_guessed_format)
        && let Ok(dims) = img.into_dimensions()
    {
        let (w, h) = dims;
        if w > 0 && h > 0 {
            return Some((w, h, "unknown".to_string(), 8));
        }
    }

    // Stage 2: ImageMagick identify (covers JXL, HEIC, AVIF and provides bit-depth/channels).
    // Format: %w (width) %h (height) %[channels] (type string, e.g. 'srgba') %z (depth)
    let output = crate::image_builders::IdentifyBuilder::new()
        .use_magick(true)
        .format("%w %h %[channels] %z\n")
        .input(input)
        .build()
        .output()
        .or_else(|_| {
            crate::image_builders::IdentifyBuilder::new()
                .use_magick(false)
                .format("%w %h %[channels] %z\n")
                .input(input)
                .build()
                .output()
        });

    if let Ok(out) = output
        && out.status.success()
    {
        let s = String::from_utf8_lossy(&out.stdout);
        if let Some(line) = s.lines().next() {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 4 {
                let w = parts[0].parse::<u32>().ok()?;
                let h = parts[1].parse::<u32>().ok()?;
                let channel_type = parts[2].to_lowercase();
                let depth = parts[3].parse::<u8>().ok()?;

                if w > 0 && h > 0 {
                    return Some((w, h, channel_type, depth));
                }
            }
        }
    }

    None
}

/// Dimension fallback chain that does NOT invoke ffprobe.
/// (Maintained for backward compatibility, redirects to `media_info_without_ffprobe`)
#[must_use]
pub fn dimensions_without_ffprobe(input: &Path) -> Option<(u32, u32)> {
    media_info_without_ffprobe(input).map(|(w, h, _, _)| (w, h))
}

/// Get image/video dimensions using ffprobe → `image` crate → `ImageMagick` fallback chain.
///
/// # Errors
/// Returns an error message if every method fails.
pub fn get_input_dimensions(input: &Path) -> Result<(u32, u32), String> {
    if let Ok(probe) = probe_video(input)
        && probe.width > 0
        && probe.height > 0
    {
        return Ok((probe.width, probe.height));
    }

    if let Some((w, h)) = dimensions_without_ffprobe(input) {
        return Ok((w, h));
    }

    Err(format!(
        "Could not get file dimensions: {}\n\
         ffprobe, image crate, and ImageMagick identify all failed; check file integrity or install ffmpeg/ImageMagick",
        input.display(),
    ))
}

/// Check if output exceeds size tolerance and clean up if so.
///
/// **Two independent but coordinated flags:**
/// - `allow_size_tolerance`: when true, allows size increase < `1_048_576` bytes; when false, requires `output <= input`.
///   This absolute byte tolerance is fairer to all file sizes than percentage-based.
/// - `compress`: when true, **goal is to make output smaller than input**.
///   **BUT: respects `allow_size_tolerance` when enabled** - if increase < `1_048_576` bytes, still accepts.
///   Only when increase ≥ `1_048_576` bytes (or tolerance disabled), compress mode rejects the output.
///
/// **Logic flow:**
/// 1. Check oversized threshold (tolerance-based): if increase ≥ `1_048_576` bytes → reject
/// 2. Check compress goal: if compress=true AND increase ≥ tolerance → reject
/// 3. Otherwise: accept
///
/// Returns `Some(TaskResult)` if the output should be rejected (caller should return it),
/// or `None` if the output passes the size check.
#[derive(Debug, Clone, Copy)]
struct SizeDeltaSummary {
    increase_bytes: u64,
    increase_kb: f64,
    increase_mb: f64,
    change_pct: f64,
}

impl SizeDeltaSummary {
    fn from_sizes(input_size: u64, output_size: u64) -> Self {
        let increase_bytes = output_size.saturating_sub(input_size);
        let increase_bytes_f64 = crate::numeric_cast::u64_to_f64(increase_bytes);
        let input_size_f64 = crate::numeric_cast::u64_to_f64(input_size);
        let output_size_f64 = crate::numeric_cast::u64_to_f64(output_size);

        Self {
            increase_bytes,
            increase_kb: increase_bytes_f64 / crate::constants::KB_DIVISOR,
            increase_mb: increase_bytes_f64 / crate::constants::MB_DIVISOR,
            change_pct: if input_size == 0 {
                0.0
            } else {
                ((output_size_f64 / input_size_f64) - 1.0) * 100.0
            },
        }
    }

    fn uses_mb(self) -> bool {
        self.increase_mb >= 1.0
    }

    fn ratio_pct(self) -> f64 {
        100.0 + self.change_pct
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SizeGuardFailure {
    ToleranceExceeded,
    CompressionGoalMissed,
}

struct SizeToleranceCheck<'a> {
    input: &'a Path,
    output: &'a Path,
    input_size: u64,
    output_size: u64,
    options: &'a ConvertOptions,
    format_label: &'a str,
}

impl SizeToleranceCheck<'_> {
    fn delta(&self) -> SizeDeltaSummary {
        SizeDeltaSummary::from_sizes(self.input_size, self.output_size)
    }

    pub const fn tolerance_bytes() -> u64 {
        crate::constants::DEFAULT_SIZE_TOLERANCE_BYTES
    }

    fn is_guard_active(&self) -> bool {
        let input_ext = self
            .input
            .extension()
            .and_then(|ext| ext.to_str())
            .unwrap_or("");
        crate::quality_matcher::is_size_guard_active(input_ext, self.options.apple_compat())
    }

    fn max_allowed_size(&self) -> u64 {
        if self.options.allow_size_tolerance() && self.is_guard_active() {
            self.input_size.saturating_add(Self::tolerance_bytes())
        } else if self.is_guard_active() {
            self.input_size
        } else {
            u64::MAX
        }
    }

    fn evaluate(&self) -> Option<SizeGuardFailure> {
        if self.output_size >= self.max_allowed_size() {
            return Some(SizeGuardFailure::ToleranceExceeded);
        }

        if self.options.compress() && self.output_size >= self.input_size {
            let delta = self.delta();
            if self.options.allow_size_tolerance() && delta.increase_bytes < Self::tolerance_bytes()
            {
                return None;
            }
            return Some(SizeGuardFailure::CompressionGoalMissed);
        }

        None
    }

    fn handle_failure(&self, failure: SizeGuardFailure) -> TaskResult {
        match failure {
            SizeGuardFailure::ToleranceExceeded => self.reject_tolerance_exceeded(),
            SizeGuardFailure::CompressionGoalMissed => self.reject_compression_goal(),
        }
    }

    fn reject_tolerance_exceeded(&self) -> TaskResult {
        let delta = self.delta();
        let mode = if self.options.allow_size_tolerance() {
            "tolerance: absolute (< 1_048_576 bytes increase)"
        } else {
            "strict mode: no tolerance"
        };

        self.log_discard(delta, Some(mode));
        self.cleanup_output(SizeGuardFailure::ToleranceExceeded);
        self.preserve_original(SizeGuardFailure::ToleranceExceeded);
        mark_as_processed(self.input);

        TaskResult::skipped_size_increase(self.input, self.input_size, self.output_size)
    }

    fn reject_compression_goal(&self) -> TaskResult {
        let delta = self.delta();

        if delta.change_pct.abs() < 0.01_f64 {
            crate::log_detail!(
                "   🗑️  {} output deleted: {}",
                self.format_label,
                "\x1b[1;33msize unchanged (compression goal not achieved)\x1b[0m"
            );
            crate::log_detail!(
                "   📊 Size: {} → {} bytes",
                format!("\x1b[2m{}\x1b[0m", self.input_size),
                format!("\x1b[2m{}\x1b[0m", self.output_size)
            );
        } else {
            self.log_discard(delta, None);
        }

        self.cleanup_output(SizeGuardFailure::CompressionGoalMissed);
        self.preserve_original(SizeGuardFailure::CompressionGoalMissed);
        mark_as_processed(self.input);

        TaskResult::skipped_size_unchanged(self.input, self.input_size, self.format_label)
    }

    fn log_discard(&self, delta: SizeDeltaSummary, mode: Option<&str>) {
        if delta.uses_mb() {
            if let Some(mode_label) = mode {
                crate::log_detail!(
                    "   {} {} output discarded │ {}ratio: {:.1}%{} │ {}increase: +{:.2}MB{} │ {}",
                    symbols::CROSS,
                    self.format_label,
                    colors::BOLD,
                    delta.ratio_pct(),
                    colors::RESET,
                    colors::MFB_ORANGE,
                    delta.increase_mb,
                    colors::RESET,
                    mode_label
                );
            } else {
                crate::log_detail!(
                    "   {} {} output discarded │ {}ratio: {:.1}%{} │ {}increase: +{:.2}MB{}",
                    symbols::CROSS,
                    self.format_label,
                    colors::BOLD,
                    delta.ratio_pct(),
                    colors::RESET,
                    colors::MFB_ORANGE,
                    delta.increase_mb,
                    colors::RESET
                );
            }
            crate::log_detail!(
                "   {} Size: {} → {} (Δ +{:.2}MB)",
                symbols::CHART,
                format!("{}{}{} bytes", colors::DIM, self.input_size, colors::RESET),
                format!(
                    "{}{}{} bytes",
                    colors::MFB_RED,
                    self.output_size,
                    colors::RESET
                ),
                delta.increase_mb
            );
            return;
        }

        if let Some(mode_label) = mode {
            crate::log_detail!(
                "   {} {} output discarded │ {}ratio: {:.1}%{} │ {}increase: +{:.1}KB{} │ {}",
                symbols::CROSS,
                self.format_label,
                colors::BOLD,
                delta.ratio_pct(),
                colors::RESET,
                colors::MFB_ORANGE,
                delta.increase_kb,
                colors::RESET,
                mode_label
            );
        } else {
            crate::log_detail!(
                "   {} {} output discarded │ {}ratio: {:.1}%{} │ {}increase: +{:.1}KB{}",
                symbols::CROSS,
                self.format_label,
                colors::BOLD,
                delta.ratio_pct(),
                colors::RESET,
                colors::MFB_ORANGE,
                delta.increase_kb,
                colors::RESET
            );
        }
        crate::log_detail!(
            "   {} Size: {} → {} (Δ +{:.1}KB)",
            symbols::CHART,
            format!("{}{}{} bytes", colors::DIM, self.input_size, colors::RESET),
            format!(
                "{}{}{} bytes",
                colors::MFB_RED,
                self.output_size,
                colors::RESET
            ),
            delta.increase_kb
        );
    }

    fn cleanup_output(&self, failure: SizeGuardFailure) {
        if let Err(err) = fs::remove_file(self.output) {
            match failure {
                SizeGuardFailure::ToleranceExceeded => {
                    crate::log_detail!("   {} Cleanup failed: {}", symbols::WARNING, err);
                }
                SizeGuardFailure::CompressionGoalMissed => {
                    crate::log_upstream_error!(
                        "File cleanup",
                        "Failed to remove output file: {}",
                        err
                    );
                }
            }
        }
    }

    fn preserve_original(&self, failure: SizeGuardFailure) {
        match copy_on_skip_or_fail(
            self.input,
            self.options.output_dir.as_deref(),
            self.options.base_dir.as_deref(),
            false,
        ) {
            Ok(Some(dest)) => match failure {
                SizeGuardFailure::ToleranceExceeded => {
                    crate::log_detail!(
                        "   {} Original preserved: {}",
                        symbols::SHIELD,
                        format!("{}{}{}", colors::DIM, dest.display(), colors::RESET)
                    );
                }
                SizeGuardFailure::CompressionGoalMissed => {
                    crate::log_detail!(
                        "   📋 Original copied to: {}",
                        format!("\x1b[2m{}\x1b[0m", dest.display())
                    );
                }
            },
            Ok(None) => {}
            Err(err) => match failure {
                SizeGuardFailure::ToleranceExceeded => {
                    crate::log_anomaly!(
                        crate::static_logs::messages::LABEL_COPY,
                        &format!("Failed to copy original: {err}")
                    );
                }
                SizeGuardFailure::CompressionGoalMissed => {
                    crate::log_upstream_error!(
                        "File copy",
                        "Failed to copy original to output dir: {}",
                        err
                    );
                }
            },
        }
    }
}

#[must_use]
pub fn check_size_tolerance(
    input: &Path,
    output: &Path,
    input_size: u64,
    output_size: u64,
    options: &ConvertOptions,
    format_label: &str,
) -> Option<TaskResult> {
    let check = SizeToleranceCheck {
        input,
        output,
        input_size,
        output_size,
        options,
        format_label,
    };

    check
        .evaluate()
        .map(|failure| check.handle_failure(failure))
}

/// Validate input file for conversion.
/// Checks:
/// - File exists and is a regular file (not directory or special file)
/// - File is not a symbolic link (security risk)
/// - File is readable
///
/// Returns Ok(()) if valid, Err with descriptive message otherwise.
/// Validate an input file before conversion.
///
/// # Errors
/// Returns an error message if validation fails.
pub fn validate_input_file(input: &Path) -> Result<(), String> {
    crate::path_validator::validate_path(input).map_err(|e| e.to_string())?;

    if input.to_str().is_none() {
        return Err(format!(
            "Input path contains non-UTF-8 bytes and cannot be passed safely to external tools: {}",
            input.display()
        ));
    }

    // Check if path exists
    if !input.exists() {
        return Err(format!("Input file does not exist: {}", input.display()));
    }

    // Check if it's a symbolic link (security risk)
    if input.is_symlink() {
        return Err(format!(
            "Symbolic links are not supported for security reasons: {}",
            input.display()
        ));
    }

    // Check if it's a regular file
    if !input.is_file() {
        return Err(format!(
            "Input is not a regular file (may be a directory or special file): {}",
            input.display()
        ));
    }

    // Check if file is readable by attempting to open it
    if let Err(e) = fs::File::open(input) {
        return Err(format!("Cannot read input file {}: {}", input.display(), e));
    }

    Ok(())
}

/// Validate output path for conversion.
/// Checks:
/// - Output is not a symbolic link (security risk)
///
/// Returns Ok(()) if valid, Err with descriptive message otherwise.
///
/// Note: Path traversal check removed - output paths are generated programmatically
/// and may intentionally be in adjacent directories (e.g., _optimized suffix mode).
/// Validate an output path before conversion.
///
/// # Errors
/// Returns an error message if validation fails.
pub fn validate_output_path(output: &Path, _base_dir: Option<&Path>) -> Result<(), String> {
    crate::path_validator::validate_path(output).map_err(|e| e.to_string())?;

    if output.to_str().is_none() {
        return Err(format!(
            "Output path contains non-UTF-8 bytes and cannot be passed safely to external tools: {}",
            output.display()
        ));
    }

    ensure_output_parent_resolves(output)?;

    // Check if output is a symbolic link
    if output.exists() && output.is_symlink() {
        return Err(format!(
            "Output path is a symbolic link, refusing to overwrite: {}",
            output.display()
        ));
    }

    Ok(())
}

/// Ensures that the parent directory of an output path can be resolved.
///
/// Validates that the output path's parent directory exists and is accessible.
/// Converts relative paths to absolute paths for validation.
///
/// # Arguments
/// * `path` - The output file path to validate
///
/// # Returns
/// Ok(()) if parent directory is accessible, or error message string
fn ensure_output_parent_resolves(path: &Path) -> Result<(), String> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|e| {
                format!(
                    "Failed to resolve current directory for {}: {}",
                    path.display(),
                    e
                )
            })?
            .join(path)
    };

    let mut existing = if absolute.exists() {
        absolute.parent().ok_or_else(|| {
            format!(
                "Failed to resolve parent directory for output path: {}",
                path.display()
            )
        })?
    } else {
        absolute.as_path()
    };
    while !existing.exists() {
        existing = existing.parent().ok_or_else(|| {
            format!(
                "Failed to resolve an existing parent directory for output path: {}",
                path.display()
            )
        })?;
    }

    let resolved = existing.canonicalize().map_err(|e| {
        format!(
            "Failed to resolve output path parent {}: {}",
            existing.display(),
            e
        )
    })?;

    if !resolved.is_dir() {
        return Err(format!(
            "Resolved output parent is not a directory: {}",
            resolved.display()
        ));
    }

    Ok(())
}

/// Handle Apple AAE (Apple Adjustment Envelope) files.
/// AAE files store photo editing metadata from iPhone/Photos.app.
/// When the source image is converted, the AAE becomes orphaned.
///
/// - In `apple_compat` mode: migrate AAE to output directory
/// - Otherwise: delete orphaned AAE file
pub fn handle_aae_file(input: &Path, output: &Path, apple_compat: bool) {
    let aae_path = input.with_extension("AAE");
    let aae_path_lower = input.with_extension("aae");

    let existing_aae = if aae_path.exists() {
        Some(aae_path)
    } else if aae_path_lower.exists() {
        Some(aae_path_lower)
    } else {
        None
    };

    if let Some(aae) = existing_aae {
        if apple_compat {
            // Migrate AAE to output directory
            if let Some(output_dir) = output.parent()
                && let Some(filename) = aae.file_name()
            {
                let target_aae = output_dir.join(filename);
                if let Err(e) = fs::copy(&aae, &target_aae) {
                    crate::log_anomaly!(
                        crate::static_logs::messages::LABEL_XMP,
                        &format!("Failed to migrate AAE file: {e}")
                    );
                }
            }
        } else {
            // Delete orphaned AAE file
            if let Err(e) = fs::remove_file(&aae) {
                crate::log_anomaly!(
                    crate::static_logs::messages::LABEL_CLEANUP,
                    &format!("Failed to delete orphaned AAE file: {e}")
                );
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::{NamedTempFile, tempdir_in};

    #[test]
    fn test_strict_size_reduction_formula() {
        let test_cases = [
            (1000u64, 500u64, 50.0f64),
            (1000, 250, 75.0_f64),
            (1000, 100, 90.0_f64),
            (1000, 900, 10.0_f64),
            (1000, 1000, 0.0_f64),
            (1000, 2000, -100.0_f64),
            (1000, 1500, -50.0_f64),
        ];

        for (input, output, expected) in test_cases {
            let result = calculate_size_reduction(input, output);
            let expected_calc = (1.0_f64
                - (crate::numeric_cast::u64_to_f64(output)
                    / crate::numeric_cast::u64_to_f64(input)))
                * crate::constants::PERCENTAGE_FACTOR;

            assert!(
                (result - expected).abs() < 0.001_f64,
                "STRICT: {input}->{output}  expected {expected}, got {result}"
            );
            assert!(
                (result - expected_calc).abs() < 0.000_1_f64,
                "STRICT: Formula mismatch for {input}->{output}"
            );
        }
    }

    #[test]
    fn test_strict_large_file_sizes() {
        let reduction = calculate_size_reduction(10_000_000_000, 5_000_000_000);
        assert!(
            (reduction - 50.0).abs() < 0.001_f64,
            "STRICT: 10GB->5GB should be exactly 50%, got {reduction}"
        );

        let reduction = calculate_size_reduction(100_000_000_000, 25_000_000_000);
        assert!(
            (reduction - 75.0).abs() < 0.001_f64,
            "STRICT: 100GB->25GB should be exactly 75%, got {reduction}"
        );
    }

    #[test]
    fn test_strict_small_file_sizes() {
        let reduction = calculate_size_reduction(100, 50);
        assert!(
            (reduction - 50.0).abs() < 0.001_f64,
            "STRICT: 100->50 bytes should be exactly 50%, got {reduction}"
        );
    }

    #[test]
    fn test_format_size_change_reduction() {
        let msg = format_size_change(1000, 500).unwrap();
        assert!(
            msg.contains("reduced"),
            "Should say 'reduced' for smaller output"
        );
        assert!(msg.contains("50.0%"), "Should show 50.0% for half size");
    }

    #[test]
    fn test_temp_path_for_output_keeps_extension() {
        // Temp path must end with same extension as output so FFmpeg/muxers see correct format.
        let path1 = temp_path_for_output(Path::new("/dir/file.mov"))
            .to_string_lossy()
            .to_string();
        assert!(path1.starts_with("/dir/file.tmp."));
        assert!(path1.to_lowercase().ends_with(".mov"));

        let path2 = temp_path_for_output(Path::new("out.mp4"))
            .to_string_lossy()
            .to_string();
        assert!(path2.starts_with("out.tmp."));
        assert!(path2.to_lowercase().ends_with(".mp4"));

        let path3 = temp_path_for_output(Path::new("a/b/c.mkv"))
            .to_string_lossy()
            .to_string();
        assert!(path3.starts_with("a/b/c.tmp."));
        assert!(path3.to_lowercase().ends_with(".mkv"));

        let path4 = temp_path_for_output(Path::new("name.with.dots.mov"))
            .to_string_lossy()
            .to_string();
        assert!(path4.starts_with("name.with.dots.tmp."));
        assert!(path4.to_lowercase().ends_with(".mov"));
    }

    #[test]
    fn test_removed_commit_temp_to_output_returns_error() {
        #[expect(deprecated, reason = "regression test for removed compatibility shim")]
        let err = commit_temp_to_output(Path::new("temp.tmp"), Path::new("out.mp4"), false)
            .err()
            .unwrap_or_else(|| panic!("removed API should return an error instead of panicking"));

        assert_eq!(err.kind(), std::io::ErrorKind::Unsupported);
        assert!(
            err.to_string()
                .contains("commit_temp_to_output has been removed"),
            "unexpected error message: {err}"
        );
    }

    #[test]
    fn test_commit_temp_to_output_with_metadata_accepts_in_place_output() {
        let temp_dir = tempdir_in("/tmp").unwrap_or_else(|e| panic!("create temp dir: {e:?}"));
        let output = temp_dir.path().join("already-final.jxl");
        std::fs::write(&output, b"jxl").unwrap_or_else(|e| panic!("write output: {e:?}"));

        let committed = commit_temp_to_output_with_metadata(&output, &output, false, None)
            .unwrap_or_else(|e| panic!("in-place commit should succeed: {e:?}"));

        assert!(committed);
        assert_eq!(
            std::fs::read(&output).unwrap_or_else(|e| panic!("read output: {e:?}")),
            b"jxl",
            "in-place commit must not remove the synthesized file"
        );
    }

    #[test]
    fn test_load_processed_list_is_atomic_on_invalid_utf8() {
        clear_processed_list();

        let tracked = std::env::temp_dir().join("mfb-processed-track.mp4");
        let tracked_canonical = tracked.display().to_string();
        let mut list = NamedTempFile::new()
            .unwrap_or_else(|e| panic!("failed to create processed list: {e:?}"));
        list.write_all(tracked_canonical.as_bytes())
            .unwrap_or_else(|e| panic!("failed to write valid entry: {e:?}"));
        list.write_all(b"\n\xff\n")
            .unwrap_or_else(|e| panic!("failed to write invalid utf8: {e:?}"));

        let err = load_processed_list(list.path()).err().unwrap_or_else(|| {
            panic!("invalid utf8 should fail instead of partially loading state")
        });
        assert!(
            !is_already_processed(&tracked),
            "processed list should not be partially updated on read failure"
        );
        assert!(
            err.to_string()
                .contains("stream did not contain valid UTF-8"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn test_validate_input_file_rejects_newlines() {
        let err = validate_input_file(Path::new("bad\nname.png"))
            .err()
            .unwrap_or_else(|| panic!("newline path should be rejected before filesystem access"));
        assert!(err.contains("PATH SECURITY ERROR"));
    }

    #[cfg(unix)]
    #[test]
    fn test_validate_output_path_allows_symlink_parent_when_parent_resolves() {
        use std::os::unix::fs::symlink;
        let temp = tempfile::TempDir::new().unwrap_or_else(|e| panic!("temp dir: {e:?}"));
        let real_dir = temp.path().join("real");
        fs::create_dir_all(&real_dir).unwrap_or_else(|e| panic!("real dir: {e:?}"));
        let link_dir = temp.path().join("link");
        symlink(&real_dir, &link_dir).unwrap_or_else(|e| panic!("symlink: {e:?}"));

        validate_output_path(&link_dir.join("out.jxl"), None)
            .unwrap_or_else(|e| panic!("symlinked parent directory should resolve safely: {e:?}"));
    }

    #[cfg(unix)]
    #[test]
    fn test_validate_output_path_rejects_symlink_leaf() {
        use std::os::unix::fs::symlink;
        let temp = tempfile::TempDir::new().unwrap_or_else(|e| panic!("temp dir: {e:?}"));
        let real_dir = temp.path().join("real");
        fs::create_dir_all(&real_dir).unwrap_or_else(|e| panic!("real dir: {e:?}"));
        let output = temp.path().join("out.jxl");
        let target = real_dir.join("target.jxl");
        std::fs::write(&target, b"stub").unwrap_or_else(|e| panic!("target: {e:?}"));
        symlink(&target, &output).unwrap_or_else(|e| panic!("symlink leaf: {e:?}"));

        let err = validate_output_path(&output, None)
            .err()
            .unwrap_or_else(|| panic!("symlink output leaf should still be rejected"));
        assert!(err.contains("symbolic link"));
    }

    #[test]
    fn test_format_size_change_increase() {
        let msg = format_size_change(500, 1000).unwrap();
        assert!(
            msg.contains("increased"),
            "Should say 'increased' for larger output"
        );
        assert!(
            msg.contains("100.0%"),
            "Should show 100.0% for doubled size"
        );
    }

    #[test]
    fn test_format_size_change_no_change() {
        let msg = format_size_change(1000, 1000).unwrap();
        assert!(msg.contains("reduced"), "Same size shows as 0% reduced");
        assert!(msg.contains("0.0%"), "Should show 0.0% for same size");
    }

    #[test]
    fn test_determine_output_path() {
        let _lock = TEST_RESERVATION_LOCK
            .lock()
            .unwrap_or_else(|e| panic!("{e:?}"));
        clear_reserved_output_paths();
        let temp = tempdir_in(std::env::current_dir().unwrap_or_else(|e| panic!("{e:?}")))
            .unwrap_or_else(|e| panic!("{e:?}"));
        let input = temp.path().join("nested/image.png");
        let output =
            determine_output_path(&input, "jxl", &None).unwrap_or_else(|e| panic!("{e:?}"));
        assert_eq!(output, temp.path().join("nested/image.JXL"));
    }

    #[test]
    fn test_determine_output_path_with_dir() {
        let _lock = TEST_RESERVATION_LOCK
            .lock()
            .unwrap_or_else(|e| panic!("{e:?}"));
        clear_reserved_output_paths();
        let temp = tempdir_in(std::env::current_dir().unwrap_or_else(|e| panic!("{e:?}")))
            .unwrap_or_else(|e| panic!("{e:?}"));
        let input = temp.path().join("nested/image.png");
        let output_dir = Some(temp.path().join("output"));
        let output =
            determine_output_path(&input, "avif", &output_dir).unwrap_or_else(|e| panic!("{e:?}"));
        assert_eq!(output, temp.path().join("output/image.AVIF"));
    }

    #[test]
    fn test_determine_output_path_various_extensions() {
        let _lock = TEST_RESERVATION_LOCK
            .lock()
            .unwrap_or_else(|e| panic!("{e:?}"));
        clear_reserved_output_paths();
        let temp = tempdir_in(std::env::current_dir().unwrap_or_else(|e| panic!("{e:?}")))
            .unwrap_or_else(|e| panic!("{e:?}"));
        let input = temp.path().join("nested/video.mp4");

        let webm = determine_output_path(&input, "webm", &None).unwrap_or_else(|e| panic!("{e:?}"));
        assert_eq!(webm, temp.path().join("nested/video.WEBM"));

        let mkv = determine_output_path(&input, "mkv", &None).unwrap_or_else(|e| panic!("{e:?}"));
        assert_eq!(mkv, temp.path().join("nested/video.MKV"));
    }

    #[test]
    fn test_determine_output_path_disambiguates_batch_collisions() {
        let _lock = TEST_RESERVATION_LOCK
            .lock()
            .unwrap_or_else(|e| panic!("{e:?}"));
        clear_reserved_output_paths();
        let temp = tempdir_in(std::env::current_dir().unwrap_or_else(|e| panic!("{e:?}")))
            .unwrap_or_else(|e| panic!("{e:?}"));
        let output_dir = Some(temp.path().join("output"));
        let first = temp.path().join("set_a/clip.mp4");
        let second = temp.path().join("set_b/clip.mp4");

        let first_output =
            determine_output_path(&first, "gif", &output_dir).unwrap_or_else(|e| panic!("{e:?}"));
        let second_output =
            determine_output_path(&second, "gif", &output_dir).unwrap_or_else(|e| panic!("{e:?}"));

        assert_eq!(first_output, temp.path().join("output/clip.GIF"));
        assert_eq!(second_output, temp.path().join("output/clip (1).GIF"));
    }

    #[test]
    fn test_determine_output_path_keeps_same_reservation_for_same_input() {
        let _lock = TEST_RESERVATION_LOCK
            .lock()
            .unwrap_or_else(|e| panic!("{e:?}"));
        clear_reserved_output_paths();
        let temp = tempdir_in(std::env::current_dir().unwrap_or_else(|e| panic!("{e:?}")))
            .unwrap_or_else(|e| panic!("{e:?}"));
        let output_dir = Some(temp.path().join("output"));
        let input = temp.path().join("nested/clip.mp4");

        let first_output =
            determine_output_path(&input, "gif", &output_dir).unwrap_or_else(|e| panic!("{e:?}"));
        let second_output =
            determine_output_path(&input, "gif", &output_dir).unwrap_or_else(|e| panic!("{e:?}"));

        assert_eq!(first_output, second_output);
        assert_eq!(first_output, temp.path().join("output/clip.GIF"));
    }

    #[test]
    fn test_conversion_result_success() {
        let input = Path::new("/test/input.png");
        let output = Path::new("/test/output.avif");

        let result = TaskResult::success(input, output, 1000, 500, "AVIF", None, None);

        assert!(result.success);
        assert!(!result.skipped);
        assert_eq!(result.input_size, 1000);
        assert_eq!(result.output_size, Some(500));
        assert!(
            (result
                .size_reduction
                .unwrap_or_else(|| panic!("missing size reduction"))
                - 50.0)
                .abs()
                < 0.1_f64
        );
        assert!(
            result.message.contains("encoding"),
            "expected 'encoding' in: {}",
            result.message
        );
        assert!(
            result.message.contains("-50.0%"),
            "expected '-50.0%' in: {}",
            result.message
        );
        assert_eq!(result.outcome(), Outcome::Converted);
    }

    #[test]
    fn test_conversion_result_size_increase() {
        let input = Path::new("/test/input.png");

        let result = TaskResult::skipped_size_increase(input, 500, 1000);

        assert!(result.success);
        assert!(result.skipped);
        assert_eq!(result.skip_reason, Some("size_increase".to_string()));
        assert!(result.message.contains("larger"));
        assert_eq!(result.outcome(), Outcome::Skipped);
    }

    #[test]
    fn test_conversion_result_size_unchanged() {
        let input = Path::new("/test/input.png");

        let result = TaskResult::skipped_size_unchanged(input, 1000, "JXL");

        assert!(result.success);
        assert!(result.skipped);
        assert_eq!(result.skip_reason, Some("size_unchanged".to_string()));
        assert!(result.message.contains("unchanged"));
        assert!(result.message.contains("compression goal not achieved"));
        assert_eq!(result.outcome(), Outcome::Skipped);
    }

    #[test]
    fn test_conversion_result_outcome_fallback_preserved() {
        let input = Path::new("input.webp");
        let options = ConvertOptions::default();
        let result = TaskResult::failed_with_fallback(
            input,
            &options,
            "fallback preserved",
            "encode_failed",
        );

        assert_eq!(result.outcome(), Outcome::FallbackPreserved);
    }

    #[test]
    fn test_conversion_result_converted_with_message() {
        let input = Path::new("/test/input.mov");
        let output = Path::new("/test/output.mp4");
        let result = TaskResult::converted_with_message(
            input,
            output,
            2_000,
            1_000,
            "HEVC conversion successful: -50.0%",
        );

        assert!(result.success);
        assert!(!result.skipped);
        assert_eq!(result.output_path.as_deref(), Some("/test/output.mp4"));
        assert_eq!(result.size_reduction, Some(50.0_f64));
        assert_eq!(result.outcome(), Outcome::Converted);
    }

    #[test]
    fn test_convert_options_default() {
        let opts = ConvertOptions::default();

        assert!(!opts.force());
        assert!(opts.output_dir.is_none());
        assert!(!opts.delete_original());
        assert!(!opts.in_place());
        assert!(!opts.should_delete_original());
        assert!(opts.use_gpu());
        assert!(opts.allow_size_tolerance());
    }

    #[test]
    fn test_convert_options_delete_original() {
        let mut opts = ConvertOptions::default();
        opts.flags.set(ConvertFlags::DELETE_ORIGINAL, true);

        assert!(opts.should_delete_original());
    }

    #[test]
    fn test_convert_options_in_place() {
        let mut opts = ConvertOptions::default();
        opts.flags.set(ConvertFlags::IN_PLACE, true);

        assert!(opts.should_delete_original());
    }

    #[test]
    fn test_flag_mode_with_gpu() {
        let mut opts = ConvertOptions::default();
        opts.flags.set(ConvertFlags::EXPLORE, true);
        opts.flags.set(ConvertFlags::MATCH_QUALITY, true);
        opts.flags.set(ConvertFlags::COMPRESS, true);
        opts.flags.set(ConvertFlags::USE_GPU, true);

        let mode = opts.flag_mode().unwrap_or_else(|e| panic!("{e:?}"));
        assert_eq!(
            mode,
            crate::flag_validator::FlagMode::PreciseQualityWithCompress
        );
        assert!(opts.use_gpu(), "GPU should remain enabled");
    }

    #[test]
    fn test_flag_mode_with_cpu() {
        let mut opts = ConvertOptions::default();
        opts.flags.set(ConvertFlags::EXPLORE, true);
        opts.flags.set(ConvertFlags::MATCH_QUALITY, true);
        opts.flags.set(ConvertFlags::COMPRESS, true);
        opts.flags.set(ConvertFlags::USE_GPU, false);

        let mode = opts.flag_mode().unwrap_or_else(|e| panic!("{e:?}"));
        assert_eq!(
            mode,
            crate::flag_validator::FlagMode::PreciseQualityWithCompress
        );
        assert!(!opts.use_gpu(), "CPU mode should remain disabled");
    }

    #[test]
    fn test_only_recommended_flags_valid_with_gpu_cpu() {
        let mut gpu_config = ConvertOptions::default();
        gpu_config.flags.set(ConvertFlags::EXPLORE, true);
        gpu_config.flags.set(ConvertFlags::MATCH_QUALITY, true);
        gpu_config.flags.set(ConvertFlags::COMPRESS, true);
        gpu_config.flags.set(ConvertFlags::USE_GPU, true);
        assert!(gpu_config.flag_mode().is_ok());

        let mut cpu_config = ConvertOptions::default();
        cpu_config.flags.set(ConvertFlags::EXPLORE, true);
        cpu_config.flags.set(ConvertFlags::MATCH_QUALITY, true);
        cpu_config.flags.set(ConvertFlags::COMPRESS, true);
        cpu_config.flags.set(ConvertFlags::USE_GPU, false);
        assert!(cpu_config.flag_mode().is_ok());

        assert_eq!(
            gpu_config.flag_mode().unwrap_or_else(|e| panic!("{e:?}")),
            cpu_config.flag_mode().unwrap_or_else(|e| panic!("{e:?}"))
        );
    }

    #[test]
    fn test_invalid_flag_combinations_rejected() {
        let invalid_combos = [
            (false, false, false),
            (false, false, true),
            (false, true, false),
            (true, false, false),
        ];

        for (explore, match_quality, compress) in invalid_combos {
            let mut opts = ConvertOptions::default();
            opts.flags.set(ConvertFlags::EXPLORE, explore);
            opts.flags.set(ConvertFlags::MATCH_QUALITY, match_quality);
            opts.flags.set(ConvertFlags::COMPRESS, compress);
            assert!(
                opts.flag_mode().is_err(),
                "({explore}, {match_quality}, {compress}) should be invalid"
            );
        }
    }

    #[test]
    fn test_convert_options_all_flags_enabled() {
        let mut opts = ConvertOptions::default();
        opts.flags.set(ConvertFlags::FORCE, true);
        opts.flags.set(ConvertFlags::DELETE_ORIGINAL, true);
        opts.flags.set(ConvertFlags::IN_PLACE, true);
        opts.flags.set(ConvertFlags::EXPLORE, true);
        opts.flags.set(ConvertFlags::MATCH_QUALITY, true);
        opts.flags.set(ConvertFlags::COMPRESS, true);
        opts.flags.set(ConvertFlags::APPLE_COMPAT, true);
        opts.flags.set(ConvertFlags::USE_GPU, false);

        assert!(opts.force());
        assert!(opts.should_delete_original());
        assert!(opts.apple_compat());
        assert!(!opts.use_gpu());

        let mode = opts.flag_mode().unwrap_or_else(|e| panic!("{e:?}"));
        assert_eq!(
            mode,
            crate::flag_validator::FlagMode::PreciseQualityWithCompress
        );
    }

    #[test]
    fn test_convert_options_invalid_flag_combination() {
        let mut opts = ConvertOptions::default();
        opts.flags.set(ConvertFlags::EXPLORE, true);
        opts.flags.set(ConvertFlags::MATCH_QUALITY, false);
        opts.flags.set(ConvertFlags::COMPRESS, true);

        let result = opts.flag_mode();
        assert!(
            result.is_err(),
            "explore + compress without match_quality should be invalid"
        );
    }

    #[test]
    fn test_explore_mode_returns_precise_quality_with_compression() {
        let mut opts = ConvertOptions::default();
        opts.flags.set(ConvertFlags::EXPLORE, true);
        opts.flags.set(ConvertFlags::MATCH_QUALITY, true);
        opts.flags.set(ConvertFlags::COMPRESS, true);

        assert_eq!(
            opts.explore_mode(),
            crate::video_explorer::ExploreMode::PreciseQualityMatchWithCompression,
        );
    }

    #[test]
    fn test_success_video_explored_formatting() {
        let input_path = Path::new("input.mov");
        let output_path = Path::new("output.mp4");
        let metrics = VideoExplorationMetrics {
            input_size: 1000,
            output_size: 800,
            codec_name: "HEVC",
            crf: 23.5,
            is_lossless: false,
            iterations: 3,
            ssim: Some(0.998_5_f64),
            explored_from_crf: Some(21.0),
            quality_label: Some("Medium"),
        };
        let result = TaskResult::success_video_explored(input_path, output_path, &metrics);

        assert!(result.success);
        assert!(result.message.contains("HEVC"));
        assert!(result.message.contains("CRF 23.50"));
        assert!(result.message.contains("explored from CRF 21.0"));
        assert!(result.message.contains("3 iter"));
        assert!(result.message.contains("SSIM: 0.9985"));
        assert!(result.message.contains("-20.0%"));
        assert!(result.message.contains("Medium"));
        // Colors are present
        assert!(result.message.contains("\x1b[1;32m"));
    }

    #[test]
    fn test_dimensions_from_header_gif87a() {
        // GIF87a, 160x120 (0xA0 0x00, 0x78 0x00)
        let bytes = [
            b'G', b'I', b'F', b'8', b'7', b'a', 0xA0, 0x00, 0x78, 0x00, 0x00, 0x00,
        ];
        let tmp = NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), bytes).unwrap();
        assert_eq!(dimensions_from_header(tmp.path()), Some((160, 120)));
    }

    #[test]
    fn test_dimensions_from_header_gif89a() {
        let bytes = [
            b'G', b'I', b'F', b'8', b'9', b'a', 0x01, 0x02, 0x03, 0x04, 0x00, 0x00,
        ];
        let tmp = NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), bytes).unwrap();
        assert_eq!(dimensions_from_header(tmp.path()), Some((0x0201, 0x0403)));
    }

    #[test]
    fn test_dimensions_from_header_png() {
        // Minimal PNG: 8-byte magic + 4-byte IHDR length + "IHDR" + 4-byte width BE + 4-byte height BE
        let bytes = [
            0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, // magic
            0x00, 0x00, 0x00, 0x0D, // IHDR length = 13
            b'I', b'H', b'D', b'R', // IHDR
            0x00, 0x00, 0x02, 0x80, // width = 640
            0x00, 0x00, 0x01, 0xE0, // height = 480
            0x08, 0x02, 0x00, 0x00,
            0x00, // bit depth, color type, compression, filter, interlace
        ];
        let tmp = NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), bytes).unwrap();
        assert_eq!(dimensions_from_header(tmp.path()), Some((640, 480)));
    }

    #[test]
    fn test_dimensions_from_header_jpeg_sof0() {
        // Minimal JPEG: SOI + SOF0 marker + length(17) + precision + height BE + width BE + components
        let bytes = [
            0xFF, 0xD8, // SOI
            0xFF, 0xC0, // SOF0
            0x00, 0x11, // length
            0x08, // precision
            0x01, 0xE0, // height = 480
            0x02, 0x80, // width = 640
            0x03, 0x01, 0x22, 0x00, 0x02, 0x11, 0x01, 0x03, 0x11, 0x01,
        ];
        let tmp = NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), bytes).unwrap();
        assert_eq!(dimensions_from_header(tmp.path()), Some((640, 480)));
    }

    #[test]
    fn test_dimensions_from_header_jpeg_with_app_segments() {
        // JPEG with APP0 (JFIF) + APP1 (EXIF-ish padding) before SOF0 — tests scan across markers.
        let mut bytes = vec![0xFF, 0xD8]; // SOI
        // APP0 segment: FF E0, length 0x10, "JFIF\0", version 1.1, density units etc. (16 bytes total with length)
        bytes.extend_from_slice(&[
            0xFF, 0xE0, 0x00, 0x10, b'J', b'F', b'I', b'F', 0x00, 0x01, 0x01, 0x00, 0x00, 0x48,
            0x00, 0x48, 0x00, 0x00,
        ]);
        // APP1 segment: 32-byte dummy payload (length 0x0020 includes the 2 length bytes)
        bytes.extend_from_slice(&[0xFF, 0xE1, 0x00, 0x20]);
        bytes.extend(std::iter::repeat_n(0x00, 30));
        // SOF0 marker: FF C0, length 0x0011, precision 8, height 0x0300=768, width 0x0400=1024, 3 components
        bytes.extend_from_slice(&[
            0xFF, 0xC0, 0x00, 0x11, 0x08, 0x03, 0x00, 0x04, 0x00, 0x03, 0x01, 0x22, 0x00, 0x02,
            0x11, 0x01, 0x03, 0x11, 0x01,
        ]);
        let tmp = NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), &bytes).unwrap();
        assert_eq!(dimensions_from_header(tmp.path()), Some((1024, 768)));
    }

    #[test]
    fn test_dimensions_from_header_webp_vp8() {
        // WebP VP8 (lossy): RIFF + size + WEBP + VP8 + chunk length + 3-byte frame tag + start code + 14-bit w/h
        let mut bytes = b"RIFF".to_vec();
        bytes.extend_from_slice(&[0x20, 0x00, 0x00, 0x00]); // size (ignored by our reader)
        bytes.extend_from_slice(b"WEBP");
        bytes.extend_from_slice(b"VP8 ");
        bytes.extend_from_slice(&[0x10, 0x00, 0x00, 0x00]); // chunk length
        bytes.extend_from_slice(&[0x00, 0x00, 0x00]); // 3-byte frame tag (key frame)
        bytes.extend_from_slice(&[0x9D, 0x01, 0x2A]); // start code
        // width = 640 (low 14 bits), height = 480
        bytes.extend_from_slice(&[0x80, 0x02, 0xE0, 0x01]);
        let tmp = NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), &bytes).unwrap();
        assert_eq!(dimensions_from_header(tmp.path()), Some((640, 480)));
    }

    #[test]
    fn test_dimensions_from_header_bmp() {
        // BMP: "BM" + size(dummy) + reserved(4) + offset(4) + DIB header size(4) + width LE i32 + height LE i32 + ...
        let bytes = [
            b'B', b'M', 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x36, 0x00, 0x00, 0x00, 0xA0, 0x00,
            0x00, 0x00, 0x78, 0x00, 0x00, 0x00, 0x01, 0x00, 0x18, 0x00,
        ];
        let tmp = NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), bytes).unwrap();
        assert_eq!(dimensions_from_header(tmp.path()), Some((160, 120)));
    }

    #[test]
    fn test_dimensions_from_header_rejects_unknown() {
        let bytes = b"this is not any recognised image format at all whatsoever";
        let tmp = NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), bytes).unwrap();
        assert_eq!(dimensions_from_header(tmp.path()), None);
    }

    #[test]
    fn test_dimensions_from_header_rejects_truncated_gif() {
        // GIF magic but truncated before width/height
        let bytes = b"GIF89a";
        let tmp = NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), bytes).unwrap();
        assert_eq!(dimensions_from_header(tmp.path()), None);
    }

    #[test]
    fn test_dimensions_from_header_rejects_zero_dims() {
        // PNG with width=0
        let bytes = [
            0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, b'I', b'H',
            b'D', b'R', 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0xE0, 0x08, 0x02, 0x00, 0x00,
            0x00,
        ];
        let tmp = NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), bytes).unwrap();
        assert_eq!(dimensions_from_header(tmp.path()), None);
    }
}
