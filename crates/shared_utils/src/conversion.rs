//! Conversion Utilities Module
//!
//! Provides common conversion functionality shared across all tools:
//! - `ConversionResult`: Unified result structure
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

use crate::constants::MIN_OUTPUT_SIZE_BEFORE_DELETE_IMAGE;
use crate::conversion_types::SelectedCodec;
use crate::modern_ui::{colors, symbols};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicU64, Ordering},
    LazyLock, Mutex,
};
use std::time::{SystemTime, UNIX_EPOCH};

static PROCESSED_FILES: LazyLock<Mutex<HashSet<String>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));
static RESERVED_OUTPUT_PATHS: LazyLock<Mutex<HashMap<String, String>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
static TEMP_OUTPUT_COUNTER: AtomicU64 = AtomicU64::new(0);

#[cfg(test)]
static TEST_RESERVATION_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

pub fn next_temp_output_suffix() -> String {
    const ALPHABET: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let pid = u128::from(std::process::id());
    let counter = u128::from(TEMP_OUTPUT_COUNTER.fetch_add(1, Ordering::Relaxed));
    let mut value = timestamp ^ (pid << 32) ^ counter;
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

fn stable_path_key(path: &Path) -> String {
    path.canonicalize()
        .unwrap_or_else(|_| path.to_path_buf())
        .display()
        .to_string()
}

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

fn reserve_unique_output_path(input: &Path, candidate: PathBuf) -> PathBuf {
    let input_key = stable_path_key(input);
    let mut reservations = RESERVED_OUTPUT_PATHS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);

    let mut resolved = candidate;
    let mut collision_index = 1usize;

    loop {
        let output_key = stable_path_key(&resolved);
        match reservations.get(&output_key) {
            Some(owner) if owner != &input_key => {
                resolved = path_with_collision_suffix(&resolved, collision_index);
                collision_index += 1;
            }
            _ => {
                reservations.insert(output_key, input_key);
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

    let mut processed = PROCESSED_FILES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    processed.extend(loaded);

    Ok(())
}

/// Save the processed files list.
///
/// # Errors
///
/// Returns an error if the file cannot be written or serialized.
pub fn save_processed_list(list_path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let processed = PROCESSED_FILES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut file = fs::File::create(list_path)?;
    #[cfg(unix)]
    flock_exclusive(&file)?;
    #[cfg(unix)]
    let _flock_guard = ProcessedListLockGuard(std::os::unix::io::AsRawFd::as_raw_fd(&file));

    for path in processed.iter() {
        writeln!(file, "{path}")?;
    }
    file.flush()?;

    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversionResult {
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
pub enum ConversionOutcome {
    Converted,
    Skipped,
    FallbackPreserved,
    Ignored,
    Failed,
}

/// Metrics for a video exploration outcome, used to populate `ConversionResult`.
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
    #[must_use]
    pub fn format_message(&self, reduction_pct: f64) -> String {
        let reduction = reduction_pct / 100.0;
        let size_tag = if reduction >= 0.0 {
            format!("\x1b[1;32m-{reduction_pct:.1}%\x1b[0m")
        } else {
            let diff_bytes = i128::from(self.output_size) - i128::from(self.input_size);
            let diff_bytes_i64 = i64::try_from(diff_bytes).unwrap_or(i64::MAX);
            let size_diff = crate::modern_ui::format_size_diff(diff_bytes_i64);
            format!("\x1b[1;33m{size_diff}\x1b[0m")
        };

        let crf_display = if self.is_lossless {
            format!("{:.2} (Lossless)", self.crf)
        } else {
            format!("{:.2}", self.crf)
        };

        let explored_msg = match self.explored_from_crf {
            Some(from) if (self.crf - from).abs() > 0.1 => {
                format!(" (explored from CRF {from:.1})")
            }
            _ => String::new(),
        };

        let ssim_msg = self
            .ssim
            .map(|s| format!(", SSIM: {s:.4}"))
            .unwrap_or_default();

        let core_msg = format!(
            "{} (CRF {}{}, {} iter{}): {}",
            self.codec_name.to_uppercase(),
            crf_display,
            explored_msg,
            self.iterations,
            ssim_msg,
            size_tag
        );

        if let Some(q) = self.quality_label {
            if q.is_empty() {
                format!("✅ {core_msg}")
            } else {
                format!("✅ {q} | {core_msg}")
            }
        } else {
            format!("✅ {core_msg}")
        }
    }
}

impl ConversionResult {
    fn copy_original_for_fallback(
        input: &Path,
        options: &ConvertOptions,
        phase: &str,
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
            tracing::warn!(
                input = %input.display(),
                phase,
                "Apple-compat fallback: not copying incompatible original"
            );
            if options.verbose() {
                eprintln!("   ⚠️  Apple compatibility mode: not copying incompatible original");
            }
            None
        }
    }

    #[must_use]
    pub const fn outcome(&self) -> ConversionOutcome {
        if self.ignored {
            ConversionOutcome::Ignored
        } else if self.skipped {
            if self.success {
                ConversionOutcome::Skipped
            } else {
                ConversionOutcome::FallbackPreserved
            }
        } else if self.success {
            ConversionOutcome::Converted
        } else {
            ConversionOutcome::Failed
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
    pub fn skipped_duplicate(input: &Path) -> Self {
        Self {
            success: true,
            input_path: input.display().to_string(),
            output_path: None,
            input_size: fs::metadata(input).map_or(0, |m| m.len()),
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
    pub fn skipped_exists(input: &Path, output: &Path) -> Self {
        let input_size = fs::metadata(input).map_or(0, |m| m.len());
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
        let diff_bytes_i64 = i64::try_from(diff_bytes).unwrap_or(i64::MAX);
        let size_diff = crate::modern_ui::format_size_diff(diff_bytes_i64);
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
        let input_size = fs::metadata(input).map_or(0, |m| m.len());
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
        let input_size = fs::metadata(input).map_or(0, |m| m.len());
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
            0.0
        } else {
            (1.0 - (crate::numeric_cast::u64_to_f64(output_size) / crate::numeric_cast::u64_to_f64(input_size))) * 100.0
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
            0.0
        } else {
            1.0 - (crate::numeric_cast::u64_to_f64(output_size) / crate::numeric_cast::u64_to_f64(input_size))
        };
        let reduction_pct = reduction * 100.0;

        // Build size-change suffix: "-14.5%" (saved) or "+2.1MB" (grew) with ANSI colors
        let size_tag = if reduction >= 0.0 {
            format!("\x1b[1;32m-{reduction_pct:.1}%\x1b[0m")
        } else {
            let diff_bytes = i128::from(output_size) - i128::from(input_size);
            let diff_bytes_i64 = i64::try_from(diff_bytes).unwrap_or(i64::MAX);
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
        let core_msg = match extra_info {
            Some(info) => format!("{format_name} {action} ({info}): {size_tag}"),
            None => format!("{format_name} {action}: {size_tag}"),
        };

        let message = if let Some(q) = quality_label {
            if q.is_empty() {
                format!("✅ {core_msg}")
            } else {
                format!("✅ {q} | {core_msg}")
            }
        } else {
            format!("✅ {core_msg}")
        };

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
            0.0
        } else {
            (1.0 - (crate::numeric_cast::u64_to_f64(metrics.output_size) / crate::numeric_cast::u64_to_f64(metrics.input_size))) * 100.0
        };

        let message = metrics.format_message(reduction_pct);

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
    pub const fn force(&self) -> bool { self.flags.contains(ConvertFlags::FORCE) }
    #[must_use]
    pub const fn delete_original(&self) -> bool { self.flags.contains(ConvertFlags::DELETE_ORIGINAL) }
    #[must_use]
    pub const fn in_place(&self) -> bool { self.flags.contains(ConvertFlags::IN_PLACE) }
    #[must_use]
    pub const fn explore(&self) -> bool { self.flags.contains(ConvertFlags::EXPLORE) }
    #[must_use]
    pub const fn match_quality(&self) -> bool { self.flags.contains(ConvertFlags::MATCH_QUALITY) }
    #[must_use]
    pub const fn apple_compat(&self) -> bool { self.flags.contains(ConvertFlags::APPLE_COMPAT) }
    #[must_use]
    pub const fn compress(&self) -> bool { self.flags.contains(ConvertFlags::COMPRESS) }
    #[must_use]
    pub const fn use_gpu(&self) -> bool { self.flags.contains(ConvertFlags::USE_GPU) }
    #[must_use]
    pub const fn ultimate(&self) -> bool { self.flags.contains(ConvertFlags::ULTIMATE) }
    #[must_use]
    pub const fn allow_size_tolerance(&self) -> bool { self.flags.contains(ConvertFlags::ALLOW_SIZE_TOLERANCE) }
    #[must_use]
    pub const fn verbose(&self) -> bool { self.flags.contains(ConvertFlags::VERBOSE) }
}

impl Default for ConvertOptions {
    fn default() -> Self {
        Self {
            flags: ConvertFlags::empty(),
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
        crate::is_apple_native_format(&input_ext)
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
                explore: self.explore(),
                match_quality: self.match_quality(),
                compress: self.compress(),
                ultimate: self.ultimate(),
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
                .unwrap_or(Path::new(""));

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

#[must_use]
pub fn format_size_change(input_size: u64, output_size: u64) -> String {
    let reduction = if input_size == 0 {
        0.0
    } else {
        1.0 - (crate::numeric_cast::u64_to_f64(output_size)
            / crate::numeric_cast::u64_to_f64(input_size))
    };
    let reduction_pct = reduction * 100.0;

    if reduction >= 0.0 {
        format!("size reduced {reduction_pct:.1}%")
    } else {
        let diff_bytes = crate::numeric_cast::u64_to_i64_sat(output_size)
            .saturating_sub(crate::numeric_cast::u64_to_i64_sat(input_size));
        let size_diff = crate::modern_ui::format_size_diff(diff_bytes);
        format!("size increased {:.1}% ({})", -reduction_pct, size_diff)
    }
}

#[must_use]
pub fn calculate_size_reduction(input_size: u64, output_size: u64) -> f64 {
    if input_size == 0 {
        return 0.0;
    }
    (1.0 - (crate::numeric_cast::u64_to_f64(output_size) / crate::numeric_cast::u64_to_f64(input_size))) * 100.0
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
) -> Option<ConversionResult> {
    if !options.force() && is_already_processed(input) {
        return Some(ConversionResult::skipped_duplicate(input));
    }

    if output.exists() && !options.force() {
        return Some(ConversionResult::skipped_exists(input, output));
    }

    None
}

/// Finalize the conversion process.
///
/// # Errors
/// Returns an `io::Result` if finalization fails.
pub fn finalize_conversion(
    input: &Path,
    output: &Path,
    input_size: u64,
    format_name: &str,
    extra_info: Option<&str>,
    options: &ConvertOptions,
) -> std::io::Result<ConversionResult> {
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

    Ok(ConversionResult::success(
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
    if let Err(e) = crate::preserve_metadata(input, output) {
        eprintln!("⚠️ Failed to preserve metadata: {e}");
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
        if let Err(e) = crate::metadata::preserve_metadata(src, output) {
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
            if ext == "jxl" || ext == "mov" || ext == "mp4" || ext == "heic" || ext == "avif" {
                if let Err(e) = crate::metadata::append_mfb_branding(output) {
                    tracing::debug!("Failed to append MFB branding to Finder comment: {}", e);
                }
            }
        }

        // Step 3: Apply timestamps AFTER all file modifications
        // This is critical because ExifTool and other tools reset creation time to current time
        // We must reapply timestamps as the final step to preserve original creation time
        crate::metadata::apply_file_timestamps(src, output);
    }

    Ok(true)
}

/// Get image/video dimensions using ffprobe → image crate → `ImageMagick` fallback chain.
///
/// Returns (width, height) or an error if all methods fail.
/// Get dimensions of an input video file using ffprobe.
///
/// # Errors
/// Returns an error message if ffprobe fails.
pub fn get_input_dimensions(input: &Path) -> Result<(u32, u32), String> {
    // Method 1: ffprobe
    if let Ok(probe) = crate::probe_video(input) {
        if probe.width > 0 && probe.height > 0 {
            return Ok((probe.width, probe.height));
        }
    }

    // Method 2: image crate
    if let Ok((w, h)) = image::image_dimensions(input) {
        return Ok((w, h));
    }

    // Method 3: ImageMagick identify
    {
        let output = crate::image_builders::IdentifyBuilder::new()
            .use_magick(true)
            .format("%w %h\n")
            .input(input)
            .build()
            .output()
            .or_else(|_| {
                crate::image_builders::IdentifyBuilder::new()
                    .use_magick(false)
                    .format("%w %h\n")
                    .input(input)
                    .build()
                    .output()
            });
        if let Ok(out) = output {
            if out.status.success() {
                let s = String::from_utf8_lossy(&out.stdout);
                if let Some(line) = s.lines().next() {
                    let parts: Vec<&str> = line.split_whitespace().collect();
                    if parts.len() >= 2 {
                        if let (Some(p0), Some(p1)) = (parts.first(), parts.get(1)) {
                            if let (Ok(w), Ok(h)) = (p0.parse::<u32>(), p1.parse::<u32>()) {
                                if w > 0 && h > 0 {
                                    return Ok((w, h));
                                }
                            }
                        }
                    }
                }
            }
        }
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
/// Returns `Some(ConversionResult)` if the output should be rejected (caller should return it),
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
            increase_kb: increase_bytes_f64 / 1024.0,
            increase_mb: increase_bytes_f64 / (1024.0 * 1024.0),
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
            if self.options.allow_size_tolerance() && delta.increase_bytes < Self::tolerance_bytes() {
                return None;
            }
            return Some(SizeGuardFailure::CompressionGoalMissed);
        }

        None
    }

    fn handle_failure(&self, failure: SizeGuardFailure) -> ConversionResult {
        match failure {
            SizeGuardFailure::ToleranceExceeded => self.reject_tolerance_exceeded(),
            SizeGuardFailure::CompressionGoalMissed => self.reject_compression_goal(),
        }
    }

    fn reject_tolerance_exceeded(&self) -> ConversionResult {
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

        ConversionResult::skipped_size_increase(self.input, self.input_size, self.output_size)
    }

    fn reject_compression_goal(&self) -> ConversionResult {
        let delta = self.delta();

        if delta.change_pct.abs() < 0.01 {
            crate::log_eprintln!(
                "   🗑️  {} output deleted: {}",
                self.format_label,
                "\x1b[1;33msize unchanged (compression goal not achieved)\x1b[0m"
            );
            crate::log_eprintln!(
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

        ConversionResult::skipped_size_unchanged(self.input, self.input_size, self.format_label)
    }

    fn log_discard(&self, delta: SizeDeltaSummary, mode: Option<&str>) {
        if delta.uses_mb() {
            if let Some(mode_label) = mode {
                crate::log_eprintln!(
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
                crate::log_eprintln!(
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
            crate::log_eprintln!(
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
            crate::log_eprintln!(
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
            crate::log_eprintln!(
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
        crate::log_eprintln!(
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
                    crate::log_eprintln!("   {} Cleanup failed: {}", symbols::WARNING, err);
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
        match crate::copy_on_skip_or_fail(
            self.input,
            self.options.output_dir.as_deref(),
            self.options.base_dir.as_deref(),
            false,
        ) {
            Ok(Some(dest)) => match failure {
                SizeGuardFailure::ToleranceExceeded => {
                    crate::log_eprintln!(
                        "   {} Original preserved: {}",
                        symbols::SHIELD,
                        format!("{}{}{}", colors::DIM, dest.display(), colors::RESET)
                    );
                }
                SizeGuardFailure::CompressionGoalMissed => {
                    crate::log_eprintln!(
                        "   📋 Original copied to: {}",
                        format!("\x1b[2m{}\x1b[0m", dest.display())
                    );
                }
            },
            Ok(None) => {}
            Err(err) => match failure {
                SizeGuardFailure::ToleranceExceeded => {
                    eprintln!("   ⚠️  Failed to copy original: {err}");
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
) -> Option<ConversionResult> {
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
            if let Some(output_dir) = output.parent() {
                if let Some(filename) = aae.file_name() {
                    let target_aae = output_dir.join(filename);
                    if let Err(e) = fs::copy(&aae, &target_aae) {
                        eprintln!("⚠️  Failed to migrate AAE file: {e}");
                    }
                }
            }
        } else {
            // Delete orphaned AAE file
            if let Err(e) = fs::remove_file(&aae) {
                eprintln!("⚠️  Failed to delete orphaned AAE file: {e}");
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::{tempdir_in, NamedTempFile};

    #[test]
    fn test_strict_size_reduction_formula() {
        let test_cases = [
            (1000u64, 500u64, 50.0f64),
            (1000, 250, 75.0),
            (1000, 100, 90.0),
            (1000, 900, 10.0),
            (1000, 1000, 0.0),
            (1000, 2000, -100.0),
            (1000, 1500, -50.0),
        ];

        for (input, output, expected) in test_cases {
            let result = calculate_size_reduction(input, output);
            let expected_calc = (1.0 - (crate::numeric_cast::u64_to_f64(output) / crate::numeric_cast::u64_to_f64(input))) * 100.0;

            assert!(
                (result - expected).abs() < 0.001,
                "STRICT: {input}->{output}  expected {expected}, got {result}"
            );
            assert!(
                (result - expected_calc).abs() < 0.0001,
                "STRICT: Formula mismatch for {input}->{output}"
            );
        }
    }

    #[test]
    fn test_strict_large_file_sizes() {
        let reduction = calculate_size_reduction(10_000_000_000, 5_000_000_000);
        assert!(
            (reduction - 50.0).abs() < 0.001,
            "STRICT: 10GB->5GB should be exactly 50%, got {reduction}"
        );

        let reduction = calculate_size_reduction(100_000_000_000, 25_000_000_000);
        assert!(
            (reduction - 75.0).abs() < 0.001,
            "STRICT: 100GB->25GB should be exactly 75%, got {reduction}"
        );
    }

    #[test]
    fn test_strict_small_file_sizes() {
        let reduction = calculate_size_reduction(100, 50);
        assert!(
            (reduction - 50.0).abs() < 0.001,
            "STRICT: 100->50 bytes should be exactly 50%, got {reduction}"
        );
    }

    #[test]
    fn test_format_size_change_reduction() {
        let msg = format_size_change(1000, 500);
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
            .expect_err("removed API should return an error instead of panicking");

        assert_eq!(err.kind(), std::io::ErrorKind::Unsupported);
        assert!(
            err.to_string()
                .contains("commit_temp_to_output has been removed"),
            "unexpected error message: {err}"
        );
    }

    #[test]
    fn test_commit_temp_to_output_with_metadata_accepts_in_place_output() {
        let temp_dir = tempdir_in("/tmp").expect("create temp dir");
        let output = temp_dir.path().join("already-final.jxl");
        std::fs::write(&output, b"jxl").expect("write output");

        let committed = commit_temp_to_output_with_metadata(&output, &output, false, None)
            .expect("in-place commit should succeed");

        assert!(committed);
        assert_eq!(
            std::fs::read(&output).expect("read output"),
            b"jxl",
            "in-place commit must not remove the synthesized file"
        );
    }

    #[test]
    fn test_load_processed_list_is_atomic_on_invalid_utf8() {
        clear_processed_list();

        let tracked = std::env::temp_dir().join("mfb-processed-track.mp4");
        let tracked_canonical = tracked.display().to_string();
        let mut list = NamedTempFile::new().expect("failed to create processed list");
        list.write_all(tracked_canonical.as_bytes())
            .expect("failed to write valid entry");
        list.write_all(b"\n\xff\n")
            .expect("failed to write invalid utf8");

        let err = load_processed_list(list.path())
            .expect_err("invalid utf8 should fail instead of partially loading state");
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
            .expect_err("newline path should be rejected before filesystem access");
        assert!(err.contains("PATH SECURITY ERROR"));
    }

    #[cfg(unix)]
    #[test]
    fn test_validate_output_path_allows_symlink_parent_when_parent_resolves() {
        use std::os::unix::fs::symlink;
        let temp = tempfile::TempDir::new().expect("temp dir");
        let real_dir = temp.path().join("real");
        fs::create_dir_all(&real_dir).expect("real dir");
        let link_dir = temp.path().join("link");
        symlink(&real_dir, &link_dir).expect("symlink");

        validate_output_path(&link_dir.join("out.jxl"), None)
            .expect("symlinked parent directory should resolve safely");
    }

    #[cfg(unix)]
    #[test]
    fn test_validate_output_path_rejects_symlink_leaf() {
        use std::os::unix::fs::symlink;
        let temp = tempfile::TempDir::new().expect("temp dir");
        let real_dir = temp.path().join("real");
        fs::create_dir_all(&real_dir).expect("real dir");
        let output = temp.path().join("out.jxl");
        let target = real_dir.join("target.jxl");
        std::fs::write(&target, b"stub").expect("target");
        symlink(&target, &output).expect("symlink leaf");

        let err = validate_output_path(&output, None)
            .expect_err("symlink output leaf should still be rejected");
        assert!(err.contains("symbolic link"));
    }

    #[test]
    fn test_format_size_change_increase() {
        let msg = format_size_change(500, 1000);
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
        let msg = format_size_change(1000, 1000);
        assert!(msg.contains("reduced"), "Same size shows as 0% reduced");
        assert!(msg.contains("0.0%"), "Should show 0.0% for same size");
    }

    #[test]
    fn test_determine_output_path() {
        let _lock = TEST_RESERVATION_LOCK.lock().unwrap();
        clear_reserved_output_paths();
        let temp = tempdir_in(std::env::current_dir().unwrap()).unwrap();
        let input = temp.path().join("nested/image.png");
        let output = determine_output_path(&input, "jxl", &None).unwrap();
        assert_eq!(output, temp.path().join("nested/image.JXL"));
    }

    #[test]
    fn test_determine_output_path_with_dir() {
        let _lock = TEST_RESERVATION_LOCK.lock().unwrap();
        clear_reserved_output_paths();
        let temp = tempdir_in(std::env::current_dir().unwrap()).unwrap();
        let input = temp.path().join("nested/image.png");
        let output_dir = Some(temp.path().join("output"));
        let output = determine_output_path(&input, "avif", &output_dir).unwrap();
        assert_eq!(output, temp.path().join("output/image.AVIF"));
    }

    #[test]
    fn test_determine_output_path_various_extensions() {
        let _lock = TEST_RESERVATION_LOCK.lock().unwrap();
        clear_reserved_output_paths();
        let temp = tempdir_in(std::env::current_dir().unwrap()).unwrap();
        let input = temp.path().join("nested/video.mp4");

        let webm = determine_output_path(&input, "webm", &None).unwrap();
        assert_eq!(webm, temp.path().join("nested/video.WEBM"));

        let mkv = determine_output_path(&input, "mkv", &None).unwrap();
        assert_eq!(mkv, temp.path().join("nested/video.MKV"));
    }

    #[test]
    fn test_determine_output_path_disambiguates_batch_collisions() {
        let _lock = TEST_RESERVATION_LOCK.lock().unwrap();
        clear_reserved_output_paths();
        let temp = tempdir_in(std::env::current_dir().unwrap()).unwrap();
        let output_dir = Some(temp.path().join("output"));
        let first = temp.path().join("set_a/clip.mp4");
        let second = temp.path().join("set_b/clip.mp4");

        let first_output = determine_output_path(&first, "gif", &output_dir).unwrap();
        let second_output = determine_output_path(&second, "gif", &output_dir).unwrap();

        assert_eq!(first_output, temp.path().join("output/clip.GIF"));
        assert_eq!(second_output, temp.path().join("output/clip (1).GIF"));
    }

    #[test]
    fn test_determine_output_path_keeps_same_reservation_for_same_input() {
        let _lock = TEST_RESERVATION_LOCK.lock().unwrap();
        clear_reserved_output_paths();
        let temp = tempdir_in(std::env::current_dir().unwrap()).unwrap();
        let output_dir = Some(temp.path().join("output"));
        let input = temp.path().join("nested/clip.mp4");

        let first_output = determine_output_path(&input, "gif", &output_dir).unwrap();
        let second_output = determine_output_path(&input, "gif", &output_dir).unwrap();

        assert_eq!(first_output, second_output);
        assert_eq!(first_output, temp.path().join("output/clip.GIF"));
    }

    #[test]
    fn test_conversion_result_success() {
        let input = Path::new("/test/input.png");
        let output = Path::new("/test/output.avif");

        let result = ConversionResult::success(input, output, 1000, 500, "AVIF", None, None);

        assert!(result.success);
        assert!(!result.skipped);
        assert_eq!(result.input_size, 1000);
        assert_eq!(result.output_size, Some(500));
        assert!((result.size_reduction.unwrap() - 50.0).abs() < 0.1);
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
        assert_eq!(result.outcome(), ConversionOutcome::Converted);
    }

    #[test]
    fn test_conversion_result_size_increase() {
        let input = Path::new("/test/input.png");

        let result = ConversionResult::skipped_size_increase(input, 500, 1000);

        assert!(result.success);
        assert!(result.skipped);
        assert_eq!(result.skip_reason, Some("size_increase".to_string()));
        assert!(result.message.contains("larger"));
        assert_eq!(result.outcome(), ConversionOutcome::Skipped);
    }

    #[test]
    fn test_conversion_result_size_unchanged() {
        let input = Path::new("/test/input.png");

        let result = ConversionResult::skipped_size_unchanged(input, 1000, "JXL");

        assert!(result.success);
        assert!(result.skipped);
        assert_eq!(result.skip_reason, Some("size_unchanged".to_string()));
        assert!(result.message.contains("unchanged"));
        assert!(result.message.contains("compression goal not achieved"));
        assert_eq!(result.outcome(), ConversionOutcome::Skipped);
    }

    #[test]
    fn test_conversion_result_outcome_fallback_preserved() {
        let input = Path::new("input.webp");
        let options = ConvertOptions::default();
        let result = ConversionResult::failed_with_fallback(
            input,
            &options,
            "fallback preserved",
            "encode_failed",
        );

        assert_eq!(result.outcome(), ConversionOutcome::FallbackPreserved);
    }

    #[test]
    fn test_conversion_result_converted_with_message() {
        let input = Path::new("/test/input.mov");
        let output = Path::new("/test/output.mp4");
        let result = ConversionResult::converted_with_message(
            input,
            output,
            2_000,
            1_000,
            "HEVC conversion successful: -50.0%",
        );

        assert!(result.success);
        assert!(!result.skipped);
        assert_eq!(result.output_path.as_deref(), Some("/test/output.mp4"));
        assert_eq!(result.size_reduction, Some(50.0));
        assert_eq!(result.outcome(), ConversionOutcome::Converted);
    }

    #[test]
    fn test_convert_options_default() {
        let opts = ConvertOptions::default();

        assert!(!opts.force());
        assert!(opts.output_dir.is_none());
        assert!(!opts.delete_original());
        assert!(!opts.in_place());
        assert!(!opts.should_delete_original());
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

        let mode = opts.flag_mode().unwrap();
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

        let mode = opts.flag_mode().unwrap();
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
            gpu_config.flag_mode().unwrap(),
            cpu_config.flag_mode().unwrap()
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

        let mode = opts.flag_mode().unwrap();
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
            ssim: Some(0.9985),
            explored_from_crf: Some(21.0),
            quality_label: Some("Medium"),
        };
        let result = ConversionResult::success_video_explored(
            input_path,
            output_path,
            &metrics,
        );

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
}
