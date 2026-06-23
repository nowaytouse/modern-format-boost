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
//! All conversion paths **must** write to a temp path via `temp_path_for_output()` or
//! `foundation::path_safety::isolated_temp_path_for_search()` then
//! call `commit_temp_to_output_with_metadata(temp, output, force, original)`.
//! Do not write directly to the final output.
//!
//! ## Compress mode (authoritative)
//! When `options.compress` is true: **only** `output_size < input_size` is accepted.
//! **Any** `output_size >= input_size` (including equal) is rejected — goal not achieved.
//! All size checks use `>=` for this; do not change to `>`.
//!
//! ## `allow_size_tolerance` (default false)
//! When true: "oversized" threshold is `output size increase < 1_048_576 bytes` (accept). Video path may treat
//! `video_compression_ratio < 1.01` as acceptable when `require_compression` is checked.
//! Does **not** mean "accept up to `1_048_576` bytes larger as success" for compress goal — compress still requires output < input.

#![cfg_attr(test, allow(clippy::field_reassign_with_default))]

use crate::Rational;
use crate::constants::MIN_OUTPUT_SIZE_BEFORE_DELETE_IMAGE;
use crate::conversion_types::SelectedCodec;
use crate::ffprobe::probe_video;
use crate::metadata::preserve;
use crate::modern_ui::{colors, symbols};
use crate::quality_matcher::is_apple_native_format;
use crate::smart_file_copier::copy_on_skip_or_fail;
use anyhow::Context;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{
    LazyLock, Mutex,
    atomic::{AtomicU64, Ordering},
};
use std::time::{SystemTime, UNIX_EPOCH};

static PROCESSED_FILES: LazyLock<Mutex<HashSet<String>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));
static RESERVED_OUTPUT_PATHS: LazyLock<Mutex<HashMap<String, String>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
static TEMP_OUTPUT_COUNTER: AtomicU64 = AtomicU64::new(0);

#[cfg(test)]
static TEST_RESERVATION_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

const TOKEN_REDUCTION_PCT: &str = "{reduction_pct}";
const TOKEN_SIZE_DIFF: &str = "{size_diff}";
const TOKEN_CRF: &str = "{crf}";
const TOKEN_FROM: &str = "{from}";
const TOKEN_CODEC: &str = "{codec}";
const TOKEN_CRF_DISPLAY: &str = "{crf_display}";
const TOKEN_EXPLORED_MSG: &str = "{explored_msg}";
const TOKEN_ITERATIONS: &str = "{iterations}";
const TOKEN_SSIM_MSG: &str = "{ssim_msg}";
const TOKEN_SIZE_TAG: &str = "{size_tag}";

/// Generates a unique suffix for temporary output files.
///
/// Uses timestamp, PID, and atomic counter to ensure uniqueness across concurrent processes.
/// Falls back to counter-only mode if system time is unavailable (e.g., clock skew).
///
/// # Returns
/// A 10-character alphanumeric suffix string.
///
/// # Panics
/// Panics if UTF-8 conversion fails (impossible since ALPHABET is ASCII-only).
pub fn next_temp_output_suffix() -> String {
    const ALPHABET: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    const ALPHABET_LEN: u128 = 36;

    let timestamp = match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(d) => d.as_nanos(),
        Err(e) => crate::media_conversion_gate::delivery_temp_suffix_epoch_nanos(e),
    };

    let pid = u128::from(std::process::id());
    let counter = u128::from(TEMP_OUTPUT_COUNTER.fetch_add(1, Ordering::Relaxed));
    let mut value = timestamp ^ (pid << crate::constants::PID_SHIFT_FOR_HASH) ^ counter;
    let mut suffix = [b'0'; 10];

    for slot in suffix.iter_mut().rev() {
        let rem = value % ALPHABET_LEN;
        let idx = match u64::try_from(rem) {
            Ok(n) => {
                match crate::numeric_cast::u64_to_usize_strict(n, "temp_suffix_alphabet_idx") {
                    Some(i) if i < ALPHABET.len() => i,
                    _ => {
                        crate::media_conversion_gate::delivery_numeric_fallback_audit(
                            "delivery_numeric",
                            "temp suffix alphabet index invalid; using slot 0",
                        );
                        0
                    }
                }
            }
            Err(e) => {
                crate::media_conversion_gate::delivery_numeric_fallback_audit(
                    "delivery_numeric",
                    format!("temp suffix remainder {rem} does not fit u64: {e}; using slot 0"),
                );
                0
            }
        };
        // SAFETY: idx is guaranteed to be < 36 by modulo operation
        *slot = ALPHABET[idx];
        value /= ALPHABET_LEN;
    }

    crate::media_conversion_gate::temp_output_suffix_utf8(&suffix)
}

/// Checks if a file has already been processed.
///
/// Uses canonical path for reliable duplicate detection across symlinks and relative paths.
/// Falls back to display path if canonicalization fails (e.g., file doesn't exist yet).
///
/// # Arguments
/// * `path` - The file path to check
///
/// # Returns
/// `true` if the file has been processed, `false` otherwise.
pub fn is_already_processed(path: &Path) -> bool {
    let canonical = crate::media_conversion_gate::processed_path_key(path);

    let processed = crate::media_conversion_gate::mutex_guard_or_recover(
        "processed_files_lock",
        PROCESSED_FILES.lock(),
    );
    processed.contains(&canonical)
}

/// Marks a file as processed to prevent duplicate processing.
///
/// Uses canonical path for reliable tracking across symlinks and relative paths.
/// Falls back to display path if canonicalization fails.
///
/// # Arguments
/// * `path` - The file path to mark as processed
pub fn mark_as_processed(path: &Path) {
    let canonical = crate::media_conversion_gate::processed_path_key(path);

    let mut processed = crate::media_conversion_gate::mutex_guard_or_recover(
        "processed_files_lock",
        PROCESSED_FILES.lock(),
    );
    processed.insert(canonical);
}

/// Clears the processed files list.
///
/// Used for testing or when starting a fresh processing session.
pub fn clear_processed_list() {
    let mut processed = crate::media_conversion_gate::mutex_guard_or_recover(
        "processed_files_lock",
        PROCESSED_FILES.lock(),
    );
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
    crate::media_conversion_gate::processed_path_key(path)
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
    let stem = crate::media_conversion_gate::output_stem_for_delivery(path);
    let file_name = match path.extension().and_then(|e| e.to_str()) {
        Some(ext) if !ext.is_empty() => format!("{stem} ({collision_index}).{ext}"),
        _ => format!("{stem} ({collision_index})"),
    };

    crate::media_conversion_gate::path_parent_or_dot(path).join(file_name)
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
/// Reserves a unique output path to avoid conflicts between concurrent conversions.
///
/// If the candidate path is already reserved by another input, appends a collision suffix
/// (e.g., "file (2).jxl") and retries until a unique path is found.
///
/// # Arguments
/// * `input` - The input file path (used as reservation owner)
/// * `candidate` - The desired output path
///
/// # Returns
/// A unique output path that doesn't conflict with other reservations.
fn reserve_unique_output_path(input: &Path, candidate: PathBuf) -> PathBuf {
    let input_key = stable_path_key(input);
    let mut reservations = crate::media_conversion_gate::mutex_guard_or_recover(
        "reserved_output_paths_lock",
        RESERVED_OUTPUT_PATHS.lock(),
    );

    let mut resolved = candidate.clone();
    let mut collision_index = crate::constants::COLLISION_INDEX_START;

    loop {
        let output_key = stable_path_key(&resolved);
        let exists_on_disk = resolved.exists();

        let should_collide = match reservations.get(&output_key) {
            Some(owner) if owner == &input_key => false, // Already reserved by me
            Some(_) => true,                             // Reserved by someone else
            None if exists_on_disk => true,              // Naming collision with old file
            None => false,                               // Completely available
        };

        if should_collide {
            resolved = path_with_collision_suffix(&candidate, collision_index);
            collision_index += 1;

            if collision_index > 10_000 {
                crate::media_conversion_gate::delivery_path_layout_fallback_audit(
                    "output_path_collision",
                    format!(
                        "collision index exceeded 10,000 for output reservation (input: {}, candidate: {}); using timestamp suffix",
                        input.display(),
                        resolved.display()
                    ),
                );
                let suffix_addend = match crate::media_conversion_gate::unix_epoch_secs_optional() {
                    Some(timestamp) => match usize::try_from(timestamp) {
                        Ok(epoch_index) => epoch_index,
                        Err(e) => {
                            crate::media_conversion_gate::delivery_numeric_fallback_audit(
                                "delivery_numeric",
                                format!(
                                    "epoch timestamp {timestamp} does not fit usize for collision suffix: {e}; using collision index"
                                ),
                            );
                            collision_index
                        }
                    },
                    None => collision_index,
                };
                resolved = path_with_collision_suffix(&candidate, collision_index + suffix_addend);
                break;
            }
        } else {
            reservations.insert(output_key, input_key);
            break;
        }
    }

    drop(reservations);
    resolved
}

#[must_use]
pub fn reserve_output_path(input: &Path, candidate: &Path) -> PathBuf {
    reserve_unique_output_path(input, candidate.to_path_buf())
}

/// Clears all output path reservations.
///
/// Used for testing to reset reservation state between test runs.
#[cfg(test)]
fn clear_reserved_output_paths() {
    let mut reservations = crate::media_conversion_gate::mutex_guard_or_recover(
        "reserved_output_paths_lock",
        RESERVED_OUTPUT_PATHS.lock(),
    );
    reservations.clear();
}

pub use crate::checkpoint::{safe_delete_original, verify_output_integrity};

const PROCESSED_LIST_BLOB_SCHEMA: i32 = 1;
const PROCESSED_SESSION_KEY_MAX_LEN: usize = 128;

#[derive(Debug, Serialize, Deserialize)]
struct ProcessedListBlob {
    paths: Vec<String>,
}

fn validate_processed_session_key(session_key: &str) -> std::io::Result<()> {
    if session_key.is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "processed list session_key must not be empty",
        ));
    }
    if session_key.len() > PROCESSED_SESSION_KEY_MAX_LEN {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("processed list session_key exceeds {PROCESSED_SESSION_KEY_MAX_LEN} bytes"),
        ));
    }
    Ok(())
}

/// Load the in-memory processed set from `mfb_store.sqlite` (`blob_store.processed`).
///
/// **Atomic behavior**: decode failure leaves the in-memory set unchanged.
///
/// # Errors
/// Propagates store I/O or JSON decode errors.
pub fn load_processed_list(session_key: &str) -> std::io::Result<()> {
    validate_processed_session_key(session_key)?;
    let Some(bytes) = crate::mfb_sqlite_store::blob_get(
        crate::mfb_sqlite_store::NS_PROCESSED,
        session_key,
        PROCESSED_LIST_BLOB_SCHEMA,
    )
    .map_err(std::io::Error::other)?
    else {
        return Ok(());
    };

    let blob: ProcessedListBlob = serde_json::from_slice(&bytes).map_err(|err| {
        crate::media_conversion_gate::delivery_api_batch_fallback_audit(
            "processed_list_load_failed",
            format!("processed list blob decode failed for session_key={session_key}: {err}"),
        );
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("processed list blob decode failed: {err}"),
        )
    })?;

    crate::media_conversion_gate::mutex_guard_or_recover(
        "processed_files_lock",
        PROCESSED_FILES.lock(),
    )
    .extend(blob.paths);

    Ok(())
}

/// Persist the in-memory processed set to `mfb_store.sqlite`.
///
/// # Errors
/// Propagates store I/O or serialization errors.
pub fn save_processed_list(session_key: &str) -> std::io::Result<()> {
    validate_processed_session_key(session_key)?;
    let paths: Vec<String> = crate::media_conversion_gate::mutex_guard_or_recover(
        "processed_files_lock",
        PROCESSED_FILES.lock(),
    )
    .iter()
    .cloned()
    .collect();

    let bytes = serde_json::to_vec(&ProcessedListBlob { paths }).map_err(|err| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("processed list serialize failed: {err}"),
        )
    })?;

    crate::mfb_sqlite_store::blob_put(
        crate::mfb_sqlite_store::NS_PROCESSED,
        session_key,
        PROCESSED_LIST_BLOB_SCHEMA,
        None,
        &bytes,
    )
    .map_err(std::io::Error::other)
}

/// Clear in-memory state and delete the persisted processed blob for `session_key`.
///
/// # Errors
/// Propagates store delete errors.
pub fn clear_processed_list_for_session(session_key: &str) -> std::io::Result<()> {
    validate_processed_session_key(session_key)?;
    clear_processed_list();
    crate::mfb_sqlite_store::blob_delete(crate::mfb_sqlite_store::NS_PROCESSED, session_key)
        .map_err(std::io::Error::other)?;
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
    /// Set when the task completed a video CRF exploration (`success_video_explored`).
    pub explore_final_crf: Option<f32>,
    pub explore_iterations: Option<u32>,
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
    pub fn format_message(&self, reduction_pct: Option<f64>) -> anyhow::Result<String> {
        let size_tag = if let Some(reduction_pct) = reduction_pct {
            let reduction = reduction_pct / crate::constants::PERCENTAGE_FACTOR;
            if reduction >= 0.0_f64 {
                crate::infra::static_logs::messages::MSG_CONVERSION_SIZE_TAG_NEG
                    .replace(TOKEN_REDUCTION_PCT, &format!("{reduction_pct:.1}"))
            } else {
                let diff_bytes = i128::from(self.output_size) - i128::from(self.input_size);
                let diff_bytes_i64 =
                    crate::numeric_cast::i128_to_i64_strict(diff_bytes, "diff_bytes")
                        .ok_or_else(|| anyhow::anyhow!("Value out of i64 range for diff_bytes"))?;
                let size_diff = crate::modern_ui::format_size_diff(diff_bytes_i64);
                crate::infra::static_logs::messages::MSG_CONVERSION_SIZE_TAG_POS
                    .replace(TOKEN_SIZE_DIFF, &size_diff)
            }
        } else {
            crate::infra::static_logs::messages::MSG_CONVERSION_SIZE_TAG_NEG
                .replace(TOKEN_REDUCTION_PCT, "N/A")
        };

        let crf_display = if self.is_lossless {
            crate::infra::static_logs::messages::MSG_CONVERSION_CRF_LOSSLESS
                .replace(TOKEN_CRF, &format!("{:.2}", self.crf))
        } else {
            crate::infra::static_logs::messages::MSG_CONVERSION_CRF_NORMAL
                .replace(TOKEN_CRF, &format!("{:.2}", self.crf))
        };

        let explored_msg = match self.explored_from_crf {
            Some(from) if (self.crf - from).abs() > crate::constants::CRF_COMPARISON_EPSILON => {
                crate::infra::static_logs::messages::MSG_CONVERSION_EXPLORED_FROM
                    .replace(TOKEN_FROM, &format!("{from:.2}"))
            }
            _ => String::new(),
        };

        let ssim_msg = crate::media_conversion_gate::conversion_ssim_message_token(self.ssim);

        let core_msg = crate::infra::static_logs::messages::MSG_CONVERSION_CORE_MSG
            .replacen(TOKEN_CODEC, &self.codec_name.to_uppercase(), 1)
            .replacen(TOKEN_CRF_DISPLAY, &crf_display, 1)
            .replacen(TOKEN_EXPLORED_MSG, &explored_msg, 1)
            .replacen(TOKEN_ITERATIONS, &self.iterations.to_string(), 1)
            .replacen(TOKEN_SSIM_MSG, &ssim_msg, 1)
            .replacen(TOKEN_SIZE_TAG, &size_tag, 1);

        let formatted = crate::media_conversion_gate::conversion_message_with_quality_label(
            &core_msg,
            self.quality_label,
        );
        Ok(formatted)
    }
}

impl TaskResult {
    fn copy_original_for_fallback(
        input: &Path,
        options: &ConvertOptions,
        _phase: &str,
    ) -> anyhow::Result<Option<PathBuf>> {
        if options.should_copy_original_on_skip(input) {
            crate::smart_file_copier::copy_on_skip_or_fail(
                input,
                options.output_dir.as_deref(),
                options.base_dir.as_deref(),
                options.verbose(),
            )
            .with_context(|| {
                crate::infra::static_logs::messages::MSG_CONVERSION_FALLBACK_FAIL
                    .replace("{}", &input.display().to_string())
            })
        } else {
            crate::media_conversion_gate::apple_compat_fallback_audit(
                "apple_compat_no_copy_on_skip",
                input,
                "not copying original on skip; using slower re-encode",
            );
            if options.verbose() {
                crate::log_hint!(
                    crate::infra::static_logs::messages::LABEL_CONVERSION,
                    crate::infra::static_logs::messages::APPLE_COMPAT_NOT_COPYING_DETAILED
                );
            }
            Ok(None)
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

    /// # Errors
    /// Returns an error if input metadata cannot be read.
    pub fn skipped_duplicate(input: &Path) -> anyhow::Result<Self> {
        let input_size = fs::metadata(input)
            .with_context(|| {
                crate::infra::static_logs::messages::MSG_CONVERSION_METADATA_FAIL
                    .replace("{}", &input.display().to_string())
            })?
            .len();
        Ok(Self {
            success: true,
            input_path: input.display().to_string(),
            output_path: None,
            input_size,
            output_size: None,
            size_reduction: None,
            message: crate::infra::static_logs::messages::MSG_CONVERSION_DUPLICATE.to_string(),
            skipped: true,
            ignored: false,
            skip_reason: Some("duplicate".to_string()),
            blake3: None,
            explore_final_crf: None,
            explore_iterations: None,
        })
    }

    /// # Errors
    /// Returns an error if input metadata cannot be read.
    pub fn skipped_exists(input: &Path, output: &Path) -> anyhow::Result<Self> {
        let input_size = fs::metadata(input)
            .with_context(|| {
                crate::infra::static_logs::messages::MSG_CONVERSION_METADATA_FAIL
                    .replace("{}", &input.display().to_string())
            })?
            .len();
        let output_size = fs::metadata(output)
            .with_context(|| {
                crate::infra::static_logs::messages::MSG_CONVERSION_METADATA_FAIL
                    .replace("{}", &output.display().to_string())
            })?
            .len();
        Ok(Self {
            success: true,
            input_path: input.display().to_string(),
            output_path: Some(output.display().to_string()),
            input_size,
            output_size: Some(output_size),
            size_reduction: None,
            message: crate::infra::static_logs::messages::MSG_CONVERSION_EXISTS.to_string(),
            skipped: true,
            ignored: false,
            skip_reason: Some("exists".to_string()),
            blake3: None,
            explore_final_crf: None,
            explore_iterations: None,
        })
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
            explore_final_crf: None,
            explore_iterations: None,
        }
    }

    #[must_use]
    pub fn ignored_custom(input: &Path, input_size: u64, reason: &str, reason_id: &str) -> Self {
        Self {
            success: false,
            input_path: input.display().to_string(),
            output_path: None,
            input_size,
            output_size: None,
            size_reduction: None,
            message: reason.to_string(),
            skipped: false,
            ignored: true,
            skip_reason: Some(reason_id.to_string()),
            blake3: None,
            explore_final_crf: None,
            explore_iterations: None,
        }
    }

    #[must_use]
    pub fn skipped_size_increase(input: &Path, input_size: u64, output_size: u64) -> Self {
        let diff_bytes = i128::from(output_size) - i128::from(input_size);
        let size_diff = crate::media_conversion_gate::conversion_size_increase_diff_tag(diff_bytes);
        Self {
            success: true,
            input_path: input.display().to_string(),
            output_path: None,
            input_size,
            output_size: None,
            size_reduction: None,
            message: crate::infra::static_logs::messages::MSG_CONVERSION_SIZE_INCREASE
                .replace("{}", &size_diff),
            skipped: true,
            ignored: false,
            skip_reason: Some("size_increase".to_string()),
            blake3: None,
            explore_final_crf: None,
            explore_iterations: None,
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
                "Conversion Audit ({format_label}): Output size unchanged; compression goal not achieved"
            ),
            skipped: true,
            ignored: false,
            skip_reason: Some("size_unchanged".to_string()),
            blake3: None,
            explore_final_crf: None,
            explore_iterations: None,
        }
    }

    /// # Errors
    /// Returns an error if input metadata cannot be read or the original cannot be copied.
    pub fn skipped_with_fallback(
        input: &Path,
        options: &ConvertOptions,
        reason: &str,
        skip_reason_id: &str,
    ) -> anyhow::Result<Self> {
        Self::skipped_with_fallback_owned(
            input,
            options,
            reason.to_string(),
            skip_reason_id.to_string(),
        )
    }

    /// # Errors
    /// Returns an error if input metadata cannot be read or the original cannot be copied.
    pub fn skipped_with_fallback_owned(
        input: &Path,
        options: &ConvertOptions,
        reason: String,
        skip_reason_id: String,
    ) -> anyhow::Result<Self> {
        let input_size = fs::metadata(input)
            .with_context(|| {
                crate::infra::static_logs::messages::MSG_CONVERSION_METADATA_FAIL
                    .replace("{}", &input.display().to_string())
            })?
            .len();
        let copied_dest = Self::copy_original_for_fallback(input, options, "skip")?;
        let copied_size = copied_dest
            .as_ref()
            .map(|p| {
                fs::metadata(p)
                    .with_context(|| {
                        crate::infra::static_logs::messages::MSG_CONVERSION_COPIED_METADATA_FAIL
                            .replace("{}", &p.display().to_string())
                    })
                    .map(|m| m.len())
            })
            .transpose()?;
        crate::conversion::mark_as_processed(input);

        Ok(Self {
            success: true,
            input_path: input.display().to_string(),
            output_path: Some(
                crate::media_conversion_gate::conversion_fallback_output_path_display(
                    copied_dest.as_deref(),
                    input,
                ),
            ),
            input_size,
            output_size: copied_size,
            size_reduction: None,
            message: reason,
            skipped: true,
            ignored: false,
            skip_reason: Some(skip_reason_id),
            blake3: None,
            explore_final_crf: None,
            explore_iterations: None,
        })
    }

    /// # Errors
    /// Returns an error if input metadata cannot be read or the original cannot be copied.
    pub fn failed_with_fallback(
        input: &Path,
        options: &ConvertOptions,
        reason: &str,
        skip_reason_id: &str,
    ) -> anyhow::Result<Self> {
        Self::failed_with_fallback_owned(
            input,
            options,
            reason.to_string(),
            skip_reason_id.to_string(),
        )
    }

    /// # Errors
    /// Returns an error if input metadata cannot be read or the original cannot be copied.
    pub fn failed_with_fallback_owned(
        input: &Path,
        options: &ConvertOptions,
        reason: String,
        skip_reason_id: String,
    ) -> anyhow::Result<Self> {
        let input_size = fs::metadata(input)
            .with_context(|| {
                crate::infra::static_logs::messages::MSG_CONVERSION_FALLBACK_FAILURE_READ
                    .replace("{}", &input.display().to_string())
            })?
            .len();
        let copied_dest = Self::copy_original_for_fallback(input, options, "failure")?;
        let copied_size = copied_dest
            .as_ref()
            .map(|p| {
                fs::metadata(p)
                    .with_context(|| {
                        crate::infra::static_logs::messages::MSG_CONVERSION_FALLBACK_COPIED_READ
                            .replace("{}", &p.display().to_string())
                    })
                    .map(|m| m.len())
            })
            .transpose()?;
        crate::conversion::mark_as_processed(input);

        Ok(Self {
            success: false,
            input_path: input.display().to_string(),
            output_path: Some(
                crate::media_conversion_gate::conversion_fallback_output_path_display(
                    copied_dest.as_deref(),
                    input,
                ),
            ),
            input_size,
            output_size: copied_size,
            size_reduction: None,
            message: reason,
            skipped: true,
            ignored: false,
            skip_reason: Some(skip_reason_id),
            blake3: None,
            explore_final_crf: None,
            explore_iterations: None,
        })
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
            None
        } else {
            Some(
                (1.0_f64
                    - (crate::numeric_cast::u64_to_f64(output_size)
                        / crate::numeric_cast::u64_to_f64(input_size)))
                    * crate::constants::PERCENTAGE_FACTOR,
            )
        };

        Self {
            success: true,
            input_path: input.display().to_string(),
            output_path: Some(output.display().to_string()),
            input_size,
            output_size: Some(output_size),
            size_reduction,
            message,
            skipped: false,
            ignored: false,
            skip_reason: None,
            blake3: None,
            explore_final_crf: None,
            explore_iterations: None,
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
            None
        } else {
            Some(
                1.0_f64
                    - (crate::numeric_cast::u64_to_f64(output_size)
                        / crate::numeric_cast::u64_to_f64(input_size)),
            )
        };
        let reduction_pct = reduction.map(|v| v * crate::constants::PERCENTAGE_FACTOR);

        // Build size-change suffix: "-14.5%" (saved) or "+2.1MB" (grew) with ANSI colors
        let size_tag = if let Some(reduction_pct) = reduction_pct {
            let reduction = reduction_pct / crate::constants::PERCENTAGE_FACTOR;
            if reduction >= 0.0_f64 {
                crate::infra::static_logs::messages::MSG_CONVERSION_SIZE_TAG_NEG
                    .replace(TOKEN_REDUCTION_PCT, &format!("{reduction_pct:.1}"))
            } else {
                let diff_bytes = i128::from(output_size) - i128::from(input_size);
                let size_diff =
                    crate::media_conversion_gate::size_delta_report_label(diff_bytes, input);
                crate::infra::static_logs::messages::MSG_CONVERSION_SIZE_TAG_POS
                    .replace(TOKEN_SIZE_DIFF, &size_diff)
            }
        } else {
            let diff_bytes = i128::from(output_size) - i128::from(input_size);
            let size_diff =
                crate::media_conversion_gate::size_delta_report_label(diff_bytes, input);
            format!("{size_diff} (ratio N/A)")
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
        let core_msg = crate::media_conversion_gate::conversion_result_core_msg(
            format_name,
            action,
            &size_tag,
            extra_info,
        );

        let message = crate::media_conversion_gate::conversion_message_with_quality_label(
            &core_msg,
            quality_label,
        );

        Self {
            success: true,
            input_path: input.display().to_string(),
            output_path: Some(output.display().to_string()),
            input_size,
            output_size: Some(output_size),
            size_reduction: reduction_pct,
            message,
            skipped: false,
            ignored: false,
            skip_reason: None,
            blake3: None,
            explore_final_crf: None,
            explore_iterations: None,
        }
    }

    #[must_use]
    pub fn success_video_explored(
        input: &Path,
        output: &Path,
        metrics: &VideoExplorationMetrics<'_>,
    ) -> Self {
        let reduction_pct = if metrics.input_size == 0 {
            None
        } else {
            Some(
                (1.0_f64
                    - (crate::numeric_cast::u64_to_f64(metrics.output_size)
                        / crate::numeric_cast::u64_to_f64(metrics.input_size)))
                    * crate::constants::PERCENTAGE_FACTOR,
            )
        };

        let message = match metrics.format_message(reduction_pct) {
            Ok(m) => m,
            Err(e) => {
                crate::media_conversion_gate::delivery_api_batch_fallback_audit(
                    "explore_message_format_failed",
                    format!("failed to format exploration message: {e}"),
                );
                String::from("(formatting error)")
            }
        };

        Self {
            success: true,
            input_path: input.display().to_string(),
            output_path: Some(output.display().to_string()),
            input_size: metrics.input_size,
            output_size: Some(metrics.output_size),
            size_reduction: reduction_pct,
            message,
            skipped: false,
            ignored: false,
            skip_reason: None,
            blake3: None,
            explore_final_crf: Some(metrics.crf),
            explore_iterations: Some(metrics.iterations),
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
        const REQUIRE_JPEG_RECONSTRUCTION = 1 << 11;
        const REQUIRE_OUTPUT_DELIVERY = 1 << 12;
        const ALLOW_JPEG_PIXEL_REENCODE_FALLBACK = 1 << 13;
        const ALLOW_EXPERT_OPTIONS = 1 << 14;
        const ARCHIVE = 1 << 15;
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
    pub const fn archive(&self) -> bool {
        self.flags.contains(ConvertFlags::ARCHIVE)
    }
    #[must_use]
    pub const fn allow_size_tolerance(&self) -> bool {
        self.flags.contains(ConvertFlags::ALLOW_SIZE_TOLERANCE)
    }

    /// Honours [`crate::media_conversion_gate::effective_allow_size_tolerance`] (strict layer may veto).
    #[must_use]
    pub fn effective_allow_size_tolerance(&self) -> bool {
        crate::media_conversion_gate::effective_allow_size_tolerance(self.allow_size_tolerance())
    }
    #[must_use]
    pub const fn verbose(&self) -> bool {
        self.flags.contains(ConvertFlags::VERBOSE)
    }
    #[must_use]
    pub const fn require_jpeg_reconstruction(&self) -> bool {
        self.flags
            .contains(ConvertFlags::REQUIRE_JPEG_RECONSTRUCTION)
    }
    #[must_use]
    pub const fn require_output_delivery(&self) -> bool {
        self.flags.contains(ConvertFlags::REQUIRE_OUTPUT_DELIVERY)
    }
    #[must_use]
    pub const fn allow_jpeg_pixel_reencode_fallback(&self) -> bool {
        self.flags
            .contains(ConvertFlags::ALLOW_JPEG_PIXEL_REENCODE_FALLBACK)
    }
    #[must_use]
    pub const fn allow_expert_options(&self) -> bool {
        self.flags.contains(ConvertFlags::ALLOW_EXPERT_OPTIONS)
    }
}

impl Default for ConvertOptions {
    fn default() -> Self {
        Self {
            flags: ConvertFlags::USE_GPU,
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
        let input_ext = crate::media_conversion_gate::path_extension_lowercase_or_empty(
            input,
            "should_copy_original_on_skip",
        );
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

fn output_stem_or_audited(input: &Path) -> String {
    crate::media_conversion_gate::output_stem_for_delivery(input)
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
    let stem = output_stem_or_audited(input);

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

    let input_canonical = crate::media_conversion_gate::canonicalize_for_tool_input(input);
    let output_canonical = if output.exists() {
        crate::media_conversion_gate::canonicalize_for_tool_input(&output)
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
    let stem = output_stem_or_audited(input);

    let up_ext = extension.to_uppercase();
    let output = match output_dir {
        Some(dir) => {
            let rel_path = crate::media_conversion_gate::strip_prefix_or_self(
                input,
                base_dir,
                "strip_base_dir",
            );
            let parent = crate::media_conversion_gate::path_parent_or_dot(rel_path);

            let out_subdir = dir.join(parent);
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

    let input_canonical = crate::media_conversion_gate::canonicalize_for_tool_input(input);
    let output_canonical = if output.exists() {
        crate::media_conversion_gate::canonicalize_for_tool_input(&output)
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
    if input_size == 0 {
        return Ok("size change N/A (zero input size)".to_string());
    }
    let reduction =
        (Rational::from(1) - (Rational::from(output_size) / Rational::from(input_size))).to_f64();
    let reduction_pct = reduction * crate::constants::PERCENTAGE_FACTOR;

    if reduction >= 0.0 {
        Ok(format!("size reduced {reduction_pct:.1}%"))
    } else {
        let diff_bytes_i128 = i128::from(output_size) - i128::from(input_size);
        let diff_bytes = crate::numeric_cast::i128_to_i64_strict(diff_bytes_i128, "size_diff")
            .ok_or_else(|| {
                crate::unified_error::ImgQualityError::NumericError(
                    "size difference cast to i64 failed".into(),
                )
            })?;
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
        // Unknown denominator: do not fabricate a neutral "0.0% reduction".
        return f64::NAN;
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
#[must_use = "Result must be checked"]
/// # Errors
/// Returns an error if auxiliary metadata or filesystem operations fail during checks.
pub fn pre_conversion_check(
    input: &Path,
    output: &Path,
    options: &ConvertOptions,
) -> anyhow::Result<Option<TaskResult>> {
    if !options.force() && is_already_processed(input) {
        return Ok(Some(TaskResult::skipped_duplicate(input)?));
    }

    if output.exists() && !options.force() {
        return Ok(Some(TaskResult::skipped_exists(input, output)?));
    }

    Ok(None)
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
    preserve(input, output)?;

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
        if self.0.exists()
            && let Err(e) = crate::io_utils::safe_remove_file(&self.0)
        {
            crate::media_conversion_gate::delivery_cleanup_audit(&self.0, "temp_output_drop", e);
        }
    }
}

/// **LEAKY**: Returns a path for temporary output in the same directory as `output`.
///
/// \[WARNING\] This function pollutes the user's folder with intermediate files.
/// For Ghost Mode (Zero Pollution), use `foundation::path_safety::isolated_temp_path_for_search` instead.
///
/// Ensures `fs::rename(temp, output)` is atomic on the same filesystem. Use with `commit_temp_to_output`.
/// Uses stem + ".tmp." + extension (e.g. file.mov → file.tmp.mov) so `FFmpeg` and other
/// tools that infer format from extension still see the correct extension (mov, mp4, mkv, etc.).
#[must_use]
pub fn temp_path_for_output(output: &Path) -> PathBuf {
    let stem = crate::media_conversion_gate::temp_output_stem_lossy(output);
    let ext = crate::media_conversion_gate::temp_output_extension_lossy(output);
    let parent = crate::media_conversion_gate::output_parent_or_dot(output);

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
    commit_temp_to_output_with_metadata_inner(temp, output, force, original, false)
}

/// Same as [`commit_temp_to_output_with_metadata`] but skips the pixel-diff orientation audit.
///
/// Use this when the caller has already verified pixel correctness via an
/// equivalent proof (e.g. `verify_jxl_pixel_equivalence_integrity`) before commit,
/// to avoid running `djxl` twice on the same file.
///
/// # Errors
/// Returns an `io::Result` if commit fails.
pub fn commit_temp_to_output_with_metadata_pixel_already_verified(
    temp: &Path,
    output: &Path,
    force: bool,
    original: Option<&Path>,
) -> std::io::Result<bool> {
    commit_temp_to_output_with_metadata_inner(temp, output, force, original, true)
}

fn commit_temp_to_output_with_metadata_inner(
    temp: &Path,
    output: &Path,
    force: bool,
    original: Option<&Path>,
    pixel_audit_already_done: bool,
) -> std::io::Result<bool> {
    validate_temp_output_commit_paths(temp, output)?;

    // Temporary output may be generated in an isolated cache directory rather than
    // beside the final output (e.g. ghost-mode / isolated search temp dirs).
    // `robust_move()` handles cross-mount moves safely, so the validation here
    // focuses on file legitimacy, resolved temp parent, and output parent safety.
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
        crate::media_conversion_gate::delivery_remove_file_or_audit(
            "commit temp cleanup when output exists",
            temp,
        );
        return Ok(false);
    }

    if !in_place_commit {
        crate::io_utils::robust_move(temp, output)?;
    }

    let mut repaired_jxl_exif_after_commit = false;

    // Preserve complete metadata from original file if provided
    if let Some(src) = original {
        // Missing source metadata must not block delivery commit (M23).
        // We still want to fail hard on *partial* tool audits (PartialAudit),
        // but we must not treat "source missing" as a partial failure.
        // EACCES/EIO are NOT "missing" — log them so a skipped Spotlight xattr
        // reapply is distinguishable from a genuinely absent source.
        let src_exists = match std::fs::metadata(src) {
            Ok(_) => true,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
            Err(e) => {
                crate::log_upstream_error!(
                    "Metadata preservation",
                    "Source metadata probe failed for {} ({e}); treating as missing (M23)",
                    src.display()
                );
                false
            }
        };

        // Step 1: Preserve metadata (EXIF, XMP, xattrs, permissions)
        // This may modify the file (e.g., ExifTool writes EXIF/XMP), which changes timestamps
        match crate::metadata::preserve_for_delivery(src, output) {
            Ok(report)
                if matches!(
                    report.exif,
                    crate::metadata::MetadataLayerOutcome::PartialAudit
                ) || matches!(
                    report.xattr,
                    crate::metadata::MetadataLayerOutcome::PartialAudit
                ) =>
            {
                crate::log_upstream_error!(
                    "Metadata preservation",
                    "Metadata preservation PartialAudit (exif={:?}, xattr={:?}) for {}",
                    report.exif,
                    report.xattr,
                    output.display()
                );
                return Err(std::io::Error::other(format!(
                    "Metadata preservation partial failure for {} (exif={:?}, xattr={:?})",
                    output.display(),
                    report.exif,
                    report.xattr
                )));
            }
            Ok(report) if report.any_partial_or_skipped() => {
                crate::log_info!(
                    crate::infra::static_logs::messages::LABEL_METADATA,
                    &format!(
                        "Metadata delivery best-effort for {} (exif={:?}, xattr={:?}, timestamps={:?})",
                        output.display(),
                        report.exif,
                        report.xattr,
                        report.timestamps,
                    ),
                );
            }
            Ok(_) => {}
            Err(e) => {
                crate::log_upstream_error!(
                    "Metadata preservation",
                    "Failed to access output for metadata preservation on {}: {}",
                    output.display(),
                    e
                );
                return Err(std::io::Error::other(format!(
                    "Failed to preserve metadata for {}: {}",
                    output.display(),
                    e
                )));
            }
        }
        crate::metadata::merge_xmp_sidecar_into_dest(src, output)?;
        if pixel_audit_already_done {
            tracing::debug!(
                target: "orientation_pixel_diff",
                source = %src.display(),
                output = %output.display(),
                "delivery orientation pixel audit skipped (pixel equivalence already verified by caller)"
            );
        } else {
            audit_orientation_pixel_verification_for_delivery(src, output)?;
        }
        strip_residual_orientation_tag_for_delivery(output)?;
        repair_corrupt_jxl_brotli_exif_for_delivery(output, Some(src))?;
        repaired_jxl_exif_after_commit = true;

        // Step 2: Finder comment branding — only on the committed conversion output
        #[cfg(target_os = "macos")]
        {
            let ext = crate::media_conversion_gate::path_extension_lowercase_or_empty(
                output,
                "commit_temp_mfb_branding",
            );
            if (ext == "jxl" || ext == "mov" || ext == "mp4" || ext == "heic" || ext == "avif")
                && let Err(e) = crate::metadata::append_mfb_branding(output)
            {
                crate::log_info!(
                    crate::infra::static_logs::messages::LABEL_METADATA,
                    &format!("Failed to append MFB branding to Finder comment: {e}")
                );
            }
        }

        #[cfg(target_os = "macos")]
        if src_exists {
            let mut xattr_report = crate::metadata::MetadataDeliveryReport::default();
            crate::metadata::reapply_macos_exact_copy_xattrs_for_delivery(
                src,
                output,
                &mut xattr_report,
            );
            if matches!(
                xattr_report.xattr,
                crate::metadata::MetadataLayerOutcome::PartialAudit
            ) {
                crate::log_upstream_error!(
                    "Metadata preservation",
                    "macOS exact-copy xattr preservation PartialAudit for {}",
                    output.display()
                );
                return Err(std::io::Error::other(format!(
                    "Failed to preserve macOS exact-copy xattrs for {} (xattr={:?})",
                    output.display(),
                    xattr_report.xattr
                )));
            }
        }

        // Step 3: Apply timestamps AFTER all file modifications
        // This is critical because ExifTool and other tools reset creation time to current time
        // We must reapply timestamps as the final step to preserve original creation time
        // Step 3: Apply timestamps AFTER all file modifications
        // Only attempt when source metadata is available; otherwise keep commit non-blocking.
        if src_exists {
            let mut ts_report = crate::metadata::MetadataDeliveryReport::default();
            crate::metadata::apply_file_timestamps_for_delivery(src, output, &mut ts_report)?;
            if matches!(
                ts_report.timestamps,
                crate::metadata::MetadataLayerOutcome::PartialAudit
            ) {
                crate::log_upstream_error!(
                    "Metadata timestamps",
                    "Timestamp preservation PartialAudit for {}",
                    output.display()
                );
                return Err(std::io::Error::other(format!(
                    "Failed to preserve timestamps for {} (timestamps={:?})",
                    output.display(),
                    ts_report.timestamps
                )));
            }
        }
    }

    if !repaired_jxl_exif_after_commit {
        repair_corrupt_jxl_brotli_exif_for_delivery(output, None)?;
    }

    Ok(true)
}

fn repair_corrupt_jxl_brotli_exif_for_delivery(
    output: &Path,
    source: Option<&Path>,
) -> std::io::Result<()> {
    use crate::builder_base::ToolBuilder;
    use crate::image::format_detect::{FormatKind, detect_true_format};

    let format = detect_true_format(output).map_err(|err| {
        std::io::Error::other(format!(
            "Failed to detect output format for JXL EXIF repair on {}: {err}",
            output.display()
        ))
    })?;
    if !matches!(format, FormatKind::Jxl) {
        return Ok(());
    }
    if !crate::ExiftoolBuilder::check_available() {
        crate::media_conversion_gate::delivery_metadata_path_audit(
            "delivery_metadata_jxl_exif_repair",
            output,
            "ExifTool unavailable; cannot probe JXL EXIF metadata for Photos import compatibility",
        );
        return Ok(());
    }

    let initial = jxl_exiftool_validate_output(output)?;
    if !jxl_validate_reports_corrupt_brotli_exif(&initial) {
        return Ok(());
    }

    crate::media_conversion_gate::delivery_metadata_path_audit(
        "delivery_metadata_jxl_exif_repair",
        output,
        "Corrupted Brotli 'Exif' data detected in JXL metadata; stripping EXIF metadata box to preserve Photos import compatibility",
    );
    let repair = crate::ExiftoolBuilder::new()
        .overwrite_original()
        .arg("-Exif:all=")
        .input(output)
        .build()
        .output()
        .map_err(|err| {
            std::io::Error::other(format!(
                "Failed to launch JXL EXIF metadata repair for {}: {err}",
                output.display()
            ))
        })?;
    if !repair.status.success() {
        return Err(std::io::Error::other(format!(
            "JXL EXIF metadata repair failed for {}: stdout={} stderr={}",
            output.display(),
            String::from_utf8_lossy(&repair.stdout).trim(),
            String::from_utf8_lossy(&repair.stderr).trim()
        )));
    }

    if let Some(src) = source {
        if std::fs::metadata(src).is_ok() {
            crate::metadata::rehydrate_jxl_internal_metadata_without_orientation(src, output)?;
        } else {
            crate::media_conversion_gate::delivery_metadata_path_audit(
                "delivery_metadata_jxl_exif_repair",
                output,
                format!(
                    "JXL EXIF metadata repair stripped corrupt EXIF; source unavailable for non-Orientation metadata rehydration ({})",
                    src.display()
                ),
            );
        }
    }

    let repaired = jxl_exiftool_validate_output(output)?;
    if jxl_validate_reports_corrupt_brotli_exif(&repaired) {
        return Err(std::io::Error::other(format!(
            "JXL EXIF metadata repair did not clear corrupt Brotli EXIF for {}",
            output.display()
        )));
    }
    Ok(())
}

fn jxl_exiftool_validate_output(output: &Path) -> std::io::Result<std::process::Output> {
    use crate::builder_base::ToolBuilder;

    let validate = crate::ExiftoolBuilder::new()
        .arg("-validate")
        .arg("-warning")
        .arg("-error")
        .input(output)
        .build()
        .output()
        .map_err(|err| {
            std::io::Error::other(format!(
                "Failed to launch JXL metadata validation for {}: {err}",
                output.display()
            ))
        })?;
    if !validate.status.success() {
        return Err(std::io::Error::other(format!(
            "JXL metadata validation failed for {}: stdout={} stderr={}",
            output.display(),
            String::from_utf8_lossy(&validate.stdout).trim(),
            String::from_utf8_lossy(&validate.stderr).trim()
        )));
    }
    Ok(validate)
}

fn jxl_validate_reports_corrupt_brotli_exif(output: &std::process::Output) -> bool {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    stdout.contains("Corrupted Brotli 'Exif' data")
        || stderr.contains("Corrupted Brotli 'Exif' data")
}

fn strip_residual_orientation_tag_for_delivery(output: &Path) -> std::io::Result<()> {
    use crate::image::format_detect::{FormatKind, detect_true_format};
    use crate::image::orientation::strip_residual_orientation_tag;

    let format = detect_true_format(output).map_err(|err| {
        std::io::Error::other(format!(
            "Failed to detect output format for orientation cleanup on {}: {err}",
            output.display()
        ))
    })?;
    if !matches!(
        format,
        FormatKind::Jpeg | FormatKind::Avif | FormatKind::Heif | FormatKind::WebP
    ) && !video_extension_needs_orientation_cleanup(output)
    {
        return Ok(());
    }

    strip_residual_orientation_tag(output).map_err(|err| {
        std::io::Error::other(format!(
            "Failed to strip residual Orientation tag from {}: {err}",
            output.display()
        ))
    })
}

fn audit_orientation_pixel_verification_for_delivery(
    source: &Path,
    output: &Path,
) -> std::io::Result<()> {
    let Some((format, tolerance)) = delivery_orientation_diff_policy_for_output(output)? else {
        return Ok(());
    };
    match crate::image::orientation::verify_orientation_pixel_diff(
        source, output, format, tolerance,
    ) {
        Ok(crate::image::orientation::PixelDiffResult::Match) => {
            tracing::info!(
                target: "orientation_pixel_diff",
                source = %source.display(),
                output = %output.display(),
                format = ?format,
                "delivery orientation pixel verification matched"
            );
        }
        Ok(crate::image::orientation::PixelDiffResult::SkippedToolAbsent { tool }) => {
            tracing::warn!(
                target: "orientation_pixel_diff",
                source = %source.display(),
                output = %output.display(),
                tool = %tool,
                "delivery orientation pixel verification skipped because decoder tool is absent"
            );
        }
        Ok(crate::image::orientation::PixelDiffResult::Mismatch { max_delta, channel }) => {
            tracing::warn!(
                target: "orientation_pixel_diff",
                source = %source.display(),
                output = %output.display(),
                format = ?format,
                max_delta,
                channel,
                "delivery orientation pixel verification mismatch; non-destructive conversion output preserved"
            );
        }
        Err(err) => {
            tracing::warn!(
                target: "orientation_pixel_diff",
                source = %source.display(),
                output = %output.display(),
                error = %err,
                "delivery orientation pixel verification errored; non-destructive conversion output preserved"
            );
        }
    }
    Ok(())
}

#[cfg(test)]
fn delivery_orientation_diff_tolerance_for_output(
    output: &Path,
) -> std::io::Result<Option<crate::image::orientation::DiffTolerance>> {
    Ok(delivery_orientation_diff_policy_for_output(output)?.map(|(_format, tolerance)| tolerance))
}

fn delivery_orientation_diff_policy_for_output(
    output: &Path,
) -> std::io::Result<
    Option<(
        crate::image::format_detect::FormatKind,
        crate::image::orientation::DiffTolerance,
    )>,
> {
    let format = crate::image::format_detect::detect_true_format(output).map_err(|err| {
        std::io::Error::other(format!(
            "Failed to detect output format for orientation pixel verification on {}: {err}",
            output.display()
        ))
    })?;
    Ok(
        crate::image::orientation::orientation_diff_tolerance_for_format(format)
            .map(|tolerance| (format, tolerance)),
    )
}

fn video_extension_needs_orientation_cleanup(output: &Path) -> bool {
    output
        .extension()
        .and_then(std::ffi::OsStr::to_str)
        .is_some_and(|extension| {
            matches!(
                extension.to_ascii_lowercase().as_str(),
                "mp4" | "m4v" | "mov" | "mkv" | "webm"
            )
        })
}

fn validate_temp_output_commit_paths(temp: &Path, output: &Path) -> std::io::Result<()> {
    validate_output_path(output, None).map_err(std::io::Error::other)?;
    validate_temp_output_path(temp)
}

fn validate_temp_output_path(temp: &Path) -> std::io::Result<()> {
    if temp.to_str().is_none() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("Temp output path is not valid UTF-8: {}", temp.display()),
        ));
    }

    let temp_meta = fs::symlink_metadata(temp).map_err(|err| {
        std::io::Error::new(
            err.kind(),
            format!("Failed to inspect temp output {}: {err}", temp.display()),
        )
    })?;
    if temp_meta.file_type().is_symlink() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!(
                "Refusing to commit temp output through symbolic link: {}",
                temp.display()
            ),
        ));
    }
    if !temp_meta.file_type().is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!(
                "Refusing to commit non-regular temp output: {}",
                temp.display()
            ),
        ));
    }

    let temp_parent = temp.parent().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("Temp output has no parent directory: {}", temp.display()),
        )
    })?;
    let temp_parent_canonical = temp_parent.canonicalize().map_err(|err| {
        std::io::Error::new(
            err.kind(),
            format!(
                "Failed to resolve temp output parent {}: {err}",
                temp_parent.display()
            ),
        )
    })?;
    if !temp_parent_canonical.is_dir() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!(
                "Temp output parent is not a directory: {}",
                temp_parent_canonical.display()
            ),
        ));
    }

    Ok(())
}

/// Read image dimensions directly from the file header without external dependencies.
///
/// Supports the hot-path image formats (GIF/PNG/JPEG/WebP/BMP). Much faster and more
/// reliable than subprocess fallbacks — works regardless of ffprobe/ImageMagick availability
/// and handles filenames with non-ASCII characters uniformly.
///
/// Returns `None` if the format is unsupported or the header is malformed.
#[allow(clippy::indexing_slicing)] // hot-path header sniff: lengths checked before indexed reads
pub fn dimensions_from_header(input: &Path) -> std::io::Result<Option<(u32, u32)>> {
    use std::io::Read;

    const PNG_MAGIC: [u8; 8] = [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];

    let mut file = std::fs::File::open(input).map_err(|err| {
        crate::media_conversion_gate::probe_layer_audit(
            "header_dimension_open_failed",
            input,
            format!("failed to open media header for dimension probe: {err}"),
        );
        err
    })?;
    // Large enough to cover all supported header layouts (JPEG SOF can appear a few KB in).
    let mut head = [0u8; 4096];
    let n = file.read(&mut head).map_err(|err| {
        crate::media_conversion_gate::probe_layer_audit(
            "header_dimension_read_failed",
            input,
            format!("failed to read media header for dimension probe: {err}"),
        );
        err
    })?;
    let Some(head) = head.get(..n) else {
        return Ok(None);
    };

    // GIF: magic "GIF87a"/"GIF89a", logical screen width/height at bytes 6..10 as little-endian u16.
    if head.len() >= 10 && (head.starts_with(b"GIF87a") || head.starts_with(b"GIF89a")) {
        let w = u16::from_le_bytes([head[6], head[7]]);
        let h = u16::from_le_bytes([head[8], head[9]]);
        if w > 0 && h > 0 {
            return Ok(Some((u32::from(w), u32::from(h))));
        }
    }

    // PNG: magic 89 50 4E 47 0D 0A 1A 0A then IHDR chunk at offset 8 (4 len + "IHDR" + width/height BE u32).
    if head.len() >= 24 && head.starts_with(&PNG_MAGIC) && &head[12..16] == b"IHDR" {
        let w = u32::from_be_bytes([head[16], head[17], head[18], head[19]]);
        let h = u32::from_be_bytes([head[20], head[21], head[22], head[23]]);
        if w > 0 && h > 0 {
            return Ok(Some((w, h)));
        }
    }

    // BMP: magic "BM", DIB header width/height at offsets 18..22 and 22..26 (little-endian i32; height can be negative).
    if head.len() >= 26 && head.starts_with(b"BM") {
        let w = i32::from_le_bytes([head[18], head[19], head[20], head[21]]);
        let h = i32::from_le_bytes([head[22], head[23], head[24], head[25]]);
        if w > 0 && h != 0 {
            return Ok(Some((w.unsigned_abs(), h.unsigned_abs())));
        }
    }

    // WebP: delegate to shared RIFF chunk parser (VP8 / VP8L / VP8X / ANMF canvas).
    if head.len() >= 30 && head.starts_with(b"RIFF") && &head[8..12] == b"WEBP" {
        if let Some(dims) = crate::image_formats::webp::dimensions_from_bytes(head) {
            return Ok(Some(dims));
        }
        return crate::image_formats::webp::canvas_dimensions_from_path(input);
    }

    // JPEG: FF D8 start of image.
    if head.starts_with(&[0xFF, 0xD8]) {
        return Ok(scan_jpeg_sof(&mut file, head)?.map(|info| (info.width, info.height)));
    }

    // ISOBMFF (HEIC/HEIF/AVIF/MP4): ftyp box.
    if head.len() >= 16 && &head[4..8] == b"ftyp" {
        return scan_isobmff_dimensions(&mut file, head);
    }

    // JPEG XL container: magic bytes.
    if head.len() >= 12 && &head[0..12] == b"\x00\x00\x00\x0CJXL \x0D\x0A\x87\x0A" {
        return scan_jxl_container_dimensions(&mut file, head);
    }

    Ok(None)
}

pub fn jpeg_precision_from_header(input: &Path) -> std::io::Result<Option<u8>> {
    use std::io::Read;

    let mut file = std::fs::File::open(input).map_err(|err| {
        crate::media_conversion_gate::probe_layer_audit(
            "jpeg_precision_open_failed",
            input,
            format!("failed to open JPEG for precision probe: {err}"),
        );
        err
    })?;
    let mut head = [0u8; 4096];
    let n = file.read(&mut head).map_err(|err| {
        crate::media_conversion_gate::probe_layer_audit(
            "jpeg_precision_read_failed",
            input,
            format!("failed to read JPEG header for precision probe: {err}"),
        );
        err
    })?;
    let Some(head) = head.get(..n) else {
        return Ok(None);
    };

    if !head.starts_with(&[0xFF, 0xD8]) {
        return Ok(None);
    }

    Ok(scan_jpeg_sof(&mut file, head)?.map(|info| info.precision))
}

struct JpegSofInfo {
    width: u32,
    height: u32,
    precision: u8,
}

/// # Errors
///
/// Returns `None` on malformed or truncated JPEG structure (no panic on hostile input).
fn scan_jpeg_sof(file: &mut std::fs::File, head: &[u8]) -> std::io::Result<Option<JpegSofInfo>> {
    use std::io::{Read, Seek, SeekFrom};
    // We may need more than the initial 4KB; widen the buffer progressively.
    let mut buf: Vec<u8> = head.to_vec();
    // Seek restart for sequential scan.
    file.seek(SeekFrom::Start(crate::numeric_cast::usize_to_u64(
        buf.len(),
    )))?;
    let mut more = [0u8; 8192];
    loop {
        let read_n = file.read(&mut more)?;
        if read_n == 0 {
            break;
        }
        buf.extend_from_slice(&more[..read_n]);
        if buf.len() >= 2_097_152 {
            break; // Hard cap at 2 MiB; avoid OOM on pathological files.
        }
    }

    let mut i = 2_usize;
    while i + 8 < buf.len() {
        if buf[i] != 0xFF {
            return Ok(None);
        }
        // Skip padding FF bytes.
        while i + 1 < buf.len() && buf[i + 1] == 0xFF {
            i += 1;
        }
        if i + 1 >= buf.len() {
            return Ok(None);
        }
        let marker = buf[i + 1];
        i += 2;
        // Standalone markers without payload.
        if marker == 0xD8 || marker == 0xD9 || (0xD0..=0xD7).contains(&marker) {
            continue;
        }
        if i + 2 > buf.len() {
            return Ok(None);
        }
        let Some(seg_len) = crate::numeric_cast::u64_to_usize_strict(
            u64::from(u16::from_be_bytes([buf[i], buf[i + 1]])),
            "jpeg_segment_length",
        ) else {
            return Ok(None);
        };
        if seg_len < 2 {
            return Ok(None);
        }
        // SOFn (Start of Frame): 0xC0, 0xC1, 0xC2, 0xC3, 0xC5-0xC7, 0xC9-0xCB, 0xCD-0xCF.
        let is_sof = (0xC0..=0xCF).contains(&marker)
            && marker != 0xC4 // DHT
            && marker != 0xC8 // JPG
            && marker != 0xCC; // DAC
        if is_sof && i + 7 <= buf.len() {
            let precision = buf[i + 2];
            let h = u16::from_be_bytes([buf[i + 3], buf[i + 4]]);
            let w = u16::from_be_bytes([buf[i + 5], buf[i + 6]]);
            if w > 0 && h > 0 {
                return Ok(Some(JpegSofInfo {
                    width: u32::from(w),
                    height: u32::from(h),
                    precision,
                }));
            }
            return Ok(None);
        }
        i += seg_len;
    }
    Ok(None)
}

fn scan_isobmff_dimensions(
    file: &mut std::fs::File,
    head: &[u8],
) -> std::io::Result<Option<(u32, u32)>> {
    use std::io::{Read, Seek, SeekFrom};
    // Fully buffer up to 2 MiB — "meta" box can appear after "mdat" for HEIF.
    let mut buf: Vec<u8> = head.to_vec();
    file.seek(SeekFrom::Start(crate::numeric_cast::usize_to_u64(
        buf.len(),
    )))?;
    let mut more = [0u8; 16_384];
    loop {
        let read_n = file.read(&mut more)?;
        if read_n == 0 {
            break;
        }
        buf.extend_from_slice(&more[..read_n]);
        if buf.len() >= 2_097_152 {
            break;
        }
    }
    Ok(scan_isobmff_ispe(&buf))
}

fn scan_jxl_container_dimensions(
    file: &mut std::fs::File,
    head: &[u8],
) -> std::io::Result<Option<(u32, u32)>> {
    use std::io::{Read, Seek, SeekFrom};
    // ISOBMFF-style JXL container: same scan_isobmff_ispe works since JXL uses image item props too.
    let mut buf: Vec<u8> = head.to_vec();
    file.seek(SeekFrom::Start(crate::numeric_cast::usize_to_u64(
        buf.len(),
    )))?;
    let mut more = [0u8; 16_384];
    loop {
        let read_n = file.read(&mut more)?;
        if read_n == 0 {
            break;
        }
        buf.extend_from_slice(&more[..read_n]);
        if buf.len() >= 1_048_576 {
            break;
        }
    }
    Ok(scan_isobmff_ispe(&buf))
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
            let size_u32 = u32::from_be_bytes([
                *data.get(i)?,
                *data.get(i + 1)?,
                *data.get(i + 2)?,
                *data.get(i + 3)?,
            ]);
            let size =
                crate::numeric_cast::u64_to_usize_strict(u64::from(size_u32), "isobmff_box_size")?;
            let box_type = data.get(i + 4..i + 8)?;

            // Header + payload offset. size==1 means 64-bit extended size follows.
            let (hdr_len, total_len) = if size == 1 {
                if i + 16 > data.len() {
                    return None;
                }
                let ext = u64::from_be_bytes([
                    *data.get(i + 8)?,
                    *data.get(i + 9)?,
                    *data.get(i + 10)?,
                    *data.get(i + 11)?,
                    *data.get(i + 12)?,
                    *data.get(i + 13)?,
                    *data.get(i + 14)?,
                    *data.get(i + 15)?,
                ]);
                let Ok(ext_len) = usize::try_from(ext) else {
                    return None;
                };
                (16_usize, ext_len)
            } else if size == 0 {
                // Extends to end of stream.
                (8_usize, data.len() - i)
            } else {
                (8_usize, size)
            };

            if total_len < hdr_len || i + total_len > data.len() {
                // Box extends past buffer; try partial payload for containers.
                if CONTAINERS.iter().any(|&t| t == box_type)
                    && let Some(dims) = recurse(data.get(i + hdr_len..)?, depth + 1)
                {
                    return Some(dims);
                }
                return None;
            }

            let payload = data.get(i + hdr_len..i + total_len)?;

            if box_type == b"ispe" && payload.len() >= 12 {
                // v0 ispe: 4 bytes (version+flags) + 4 bytes width + 4 bytes height.
                let w = u32::from_be_bytes([
                    *payload.get(4)?,
                    *payload.get(5)?,
                    *payload.get(6)?,
                    *payload.get(7)?,
                ]);
                let h = u32::from_be_bytes([
                    *payload.get(8)?,
                    *payload.get(9)?,
                    *payload.get(10)?,
                    *payload.get(11)?,
                ]);
                if w > 0 && h > 0 {
                    return Some((w, h));
                }
            }

            // "meta" is a FullBox: skip 4 bytes of version+flags before its children.
            let recurse_payload = if box_type == b"meta" && payload.len() >= 4 {
                payload.get(4..)?
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BitstreamMediaInfo {
    pub width: u32,
    pub height: u32,
    pub channel_type: Option<String>,
    pub bit_depth: Option<u8>,
}

/// Tries header parsing, then the `image` crate, then `ImageMagick identify`.
///
/// Width/height may be available before channel/depth. Optional fields remain
/// unknown unless a bitstream analyzer measured them directly.
pub fn media_info_without_ffprobe(input: &Path) -> anyhow::Result<Option<BitstreamMediaInfo>> {
    let mut fallback_dims = dimensions_from_header(input).with_context(|| {
        format!(
            "failed to read media header while probing {}",
            input.display()
        )
    })?;

    // Stage 0: Native header parse — fastest path, no subprocess, no external deps.
    // Covers GIF/PNG/JPEG/WebP/BMP which are the vast majority of files.
    // We keep the dimensions as a fallback, but continue so later stages can
    // fill in real channel/depth metadata when available.
    // Stage 1: Fast in-process image crate (handles more formats including some TIFF/ICO variants).
    match image::ImageReader::open(input) {
        Ok(mut reader) => {
            use image::GenericImageView;
            use image::Limits;
            let mut limits = Limits::default();
            limits.max_alloc = Some(crate::constants::MAX_IMAGE_DECODE_ALLOC_BYTES);
            let _ = reader.set_limits(limits);

            match reader.decode() {
                Ok((img, _)) => {
                    let (w, h) = img.dimensions();
                    if w > 0 && h > 0 {
                        fallback_dims = Some((w, h));
                    }
                }
                Err(err) => {
                    crate::media_conversion_gate::probe_layer_audit(
                        "image_crate_dimension_probe_failed",
                        input,
                        format!("image crate dimension probe failed: {err}"),
                    );
                }
            }
        }
        Err(err) => {
            crate::media_conversion_gate::probe_layer_audit(
                "image_crate_open_probe_failed",
                input,
                format!("image crate open/format probe failed: {err}"),
            );
        }
    }

    // Stage 2: ImageMagick identify (covers JXL, HEIC, AVIF and provides bit-depth/channels).
    // Format: %w (width) %h (height) %[channels] (type string, e.g. 'srgba') %z (depth)
    let output = crate::media_conversion_gate::probe_identify_output_magick_then_system(
        input,
        "%w %h %[channels] %z\n",
    );

    match output {
        Ok(out) if out.status.success() => {
            let s = String::from_utf8_lossy(&out.stdout);
            if let Some(line) = s.lines().next() {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() >= 4 {
                    let parsed = parts[0]
                        .parse::<u32>()
                        .and_then(|w| parts[1].parse::<u32>().map(|h| (w, h)))
                        .and_then(|(w, h)| parts[3].parse::<u8>().map(|depth| (w, h, depth)));
                    match parsed {
                        Ok((w, h, depth)) if w > 0 && h > 0 => {
                            return Ok(Some(BitstreamMediaInfo {
                                width: w,
                                height: h,
                                channel_type: Some(parts[2].to_lowercase()),
                                bit_depth: Some(depth),
                            }));
                        }
                        Ok(_) => {}
                        Err(err) => {
                            crate::media_conversion_gate::probe_layer_audit(
                                "identify_media_info_parse_failed",
                                input,
                                format!(
                                    "ImageMagick media-info probe parse failed for line {line:?}: {err}"
                                ),
                            );
                        }
                    }
                } else {
                    crate::media_conversion_gate::probe_layer_audit(
                        "identify_media_info_shape_failed",
                        input,
                        format!("ImageMagick media-info probe returned incomplete line: {line:?}"),
                    );
                }
            }
        }
        Ok(out) => {
            crate::media_conversion_gate::probe_layer_audit(
                "identify_media_info_probe_failed",
                input,
                format!(
                    "ImageMagick media-info probe exited with status {}: {}",
                    out.status,
                    String::from_utf8_lossy(&out.stderr).trim()
                ),
            );
        }
        Err(err) => {
            crate::media_conversion_gate::probe_layer_audit(
                "identify_media_info_launch_failed",
                input,
                format!("ImageMagick media-info probe failed to launch: {err}"),
            );
        }
    }

    Ok(fallback_dims.map(|(width, height)| BitstreamMediaInfo {
        width,
        height,
        channel_type: None,
        bit_depth: None,
    }))
}

/// Dimension fallback chain that does NOT invoke ffprobe.
/// (Maintained for convenience, redirects to `media_info_without_ffprobe`)
pub fn dimensions_without_ffprobe(input: &Path) -> anyhow::Result<Option<(u32, u32)>> {
    Ok(media_info_without_ffprobe(input)?.map(|info| (info.width, info.height)))
}

/// Get image/video dimensions using ffprobe → `image` crate → `ImageMagick` fallback chain.
///
/// # Errors
/// Returns an error message if every method fails.
pub fn get_input_dimensions(input: &Path) -> Result<(u32, u32), String> {
    match probe_video(input) {
        Ok(probe) if probe.width > 0 && probe.height > 0 => {
            return Ok((probe.width, probe.height));
        }
        Ok(probe) => {
            crate::media_conversion_gate::probe_layer_audit(
                "ffprobe_dimension_zero",
                input,
                format!(
                    "ffprobe returned invalid dimensions {}x{}",
                    probe.width, probe.height
                ),
            );
        }
        Err(err) => {
            crate::media_conversion_gate::probe_layer_audit(
                "ffprobe_dimension_failed",
                input,
                format!("ffprobe dimension probe failed before image fallback: {err}"),
            );
        }
    }

    if let Some((w, h)) = dimensions_without_ffprobe(input).map_err(|err| err.to_string())? {
        return Ok((w, h));
    }

    Err(crate::infra::static_logs::messages::MSG_CONVERSION_DIM_FAIL
        .replace("{}", &input.display().to_string()))
}

/// Check if output exceeds allowed size growth and clean up if so.
///
/// **Two independent but coordinated flags:**
/// - `allow_size_tolerance`: when true, allows bounded byte growth; when false, requires `output <= input`.
///   This absolute byte allowance is fairer to all file sizes than percentage-based.
/// - `compress`: when true, **goal is to make output smaller than input**.
///   **BUT: respects `allow_size_tolerance` when enabled** - if increase is below the byte allowance, still accepts.
///   Only when increase reaches the byte allowance (or the allowance is disabled), compress mode rejects the output.
///
/// **Logic flow:**
/// 1. Check oversized threshold: if increase reaches the byte allowance → reject
/// 2. Check compress goal: if compress=true AND increase reaches the allowance → reject
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
                f64::NAN
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

    const fn allowed_growth_bytes() -> u64 {
        crate::constants::DEFAULT_SIZE_TOLERANCE_BYTES
    }

    fn is_guard_active(&self) -> bool {
        let input_ext = crate::media_conversion_gate::path_extension_lowercase_or_empty(
            self.input,
            "size_guard_active",
        );
        crate::quality_matcher::is_size_guard_active(&input_ext, self.options.apple_compat())
    }

    fn max_allowed_size(&self) -> u64 {
        if self.options.effective_allow_size_tolerance() && self.is_guard_active() {
            self.input_size.saturating_add(Self::allowed_growth_bytes())
        } else if self.is_guard_active() {
            self.input_size
        } else {
            u64::MAX
        }
    }

    fn evaluate(&self) -> Option<SizeGuardFailure> {
        if self.options.require_output_delivery() {
            return None;
        }

        if self.output_size >= self.max_allowed_size() {
            return Some(SizeGuardFailure::ToleranceExceeded);
        }

        if self.options.compress() && self.output_size >= self.input_size {
            let delta = self.delta();
            if self.options.effective_allow_size_tolerance()
                && delta.increase_bytes < Self::allowed_growth_bytes()
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
        let mode = if self.options.effective_allow_size_tolerance() {
            "allowed growth: absolute byte allowance"
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
            crate::log_detail!(format!(
                "Conversion Audit ({format_label}): Discarding candidate (Size unchanged | Mode: UNCHANGED)",
                format_label = self.format_label,
            ));
            crate::log_detail!(format!(
                "Conversion Audit: Discarding candidate (Size unchanged) original=\x1b[2m{input_size}\x1b[0m candidate=\x1b[2m{output_size}\x1b[0m",
                input_size = self.input_size,
                output_size = self.output_size,
            ));
        } else {
            self.log_discard(delta, None);
        }

        self.cleanup_output(SizeGuardFailure::CompressionGoalMissed);
        self.preserve_original(SizeGuardFailure::CompressionGoalMissed);
        mark_as_processed(self.input);

        TaskResult::skipped_size_unchanged(self.input, self.input_size, self.format_label)
    }

    fn log_discard(&self, delta: SizeDeltaSummary, mode: Option<&str>) {
        let cross = symbols::CROSS;
        let chart = symbols::CHART;
        let bold = colors::BOLD;
        let reset = colors::RESET;
        let orange = colors::MFB_ORANGE;
        let dim = colors::DIM;
        let red = colors::MFB_RED;
        let format_label = self.format_label;
        let ratio_pct = delta.ratio_pct();
        let input_size = self.input_size;
        let output_size = self.output_size;

        if delta.uses_mb() {
            let delta_mb = delta.increase_mb;
            if let Some(mode_label) = mode {
                crate::log_detail!(format!(
                    "{cross} Conversion Audit ({format_label}): Discarding candidate ({bold}{ratio_pct:.1}% over budget{reset}, {orange}+{delta_mb:.2} MB{reset} | Mode: {mode_label})"
                ));
            } else {
                crate::log_detail!(format!(
                    "{cross} Conversion Audit ({format_label}): Discarding candidate ({bold}{ratio_pct:.1}% over budget{reset}, {orange}+{delta_mb:.2} MB{reset})"
                ));
            }
            crate::log_detail!(format!(
                "{chart} Conversion Audit: original {dim}{input_size}{reset} bytes, candidate {red}{output_size}{reset} bytes (+{delta_mb:.2} MB)"
            ));
            return;
        }

        let delta_kb = delta.increase_kb;
        if let Some(mode_label) = mode {
            crate::log_detail!(format!(
                "{cross} Conversion Audit ({format_label}): Discarding candidate ({bold}{ratio_pct:.1}% over budget{reset}, {orange}+{delta_kb:.1} KB{reset} | Mode: {mode_label})"
            ));
        } else {
            crate::log_detail!(format!(
                "{cross} Conversion Audit ({format_label}): Discarding candidate ({bold}{ratio_pct:.1}% over budget{reset}, {orange}+{delta_kb:.1} KB{reset})"
            ));
        }
        crate::log_detail!(format!(
            "{chart} Conversion Audit: original {dim}{input_size}{reset} bytes, candidate {red}{output_size}{reset} bytes (+{delta_kb:.1} KB)"
        ));
    }

    fn cleanup_output(&self, failure: SizeGuardFailure) {
        if let Err(err) = fs::remove_file(self.output) {
            match failure {
                SizeGuardFailure::ToleranceExceeded => {
                    crate::log_detail!(format!(
                        "{warn} Conversion Audit: Failed to cleanup temporary bitstream artifacts: {err}",
                        warn = symbols::WARNING,
                    ));
                }
                SizeGuardFailure::CompressionGoalMissed => {
                    crate::log_upstream_error!(
                        "File cleanup",
                        format!("Conversion Audit: Upstream cleanup failed: {err}")
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
                    crate::log_detail!(format!(
                        "{shield} Conversion Audit: Preserving original file due to safety margin veto -> {dim}{dest}{reset}",
                        shield = symbols::SHIELD,
                        dim = colors::DIM,
                        reset = colors::RESET,
                        dest = dest.display(),
                    ));
                }
                SizeGuardFailure::CompressionGoalMissed => {
                    crate::log_detail!(format!(
                        "Conversion Audit: Copying original bitstream (Passthrough Mode) -> \x1b[2m{dest}\x1b[0m",
                        dest = dest.display(),
                    ));
                }
            },
            Ok(None) => {}
            Err(err) => match failure {
                SizeGuardFailure::ToleranceExceeded => {
                    crate::media_conversion_gate::delivery_api_path_fallback_audit(
                        "size_guard_bitstream_copy_failed",
                        self.input,
                        format!("failed to copy original bitstream: {err}"),
                    );
                }
                SizeGuardFailure::CompressionGoalMissed => {
                    crate::log_upstream_error!(
                        "File copy",
                        format!("Conversion Audit: Upstream copy failed: {err}")
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
        return Err(
            crate::infra::static_logs::messages::MSG_CONVERSION_VALIDATE_UTF8
                .replace("{}", &input.display().to_string()),
        );
    }

    // Check if path exists
    if !input.exists() {
        return Err(
            crate::infra::static_logs::messages::MSG_CONVERSION_VALIDATE_EXIST
                .replace("{}", &input.display().to_string()),
        );
    }

    // Check if it's a symbolic link (security risk)
    if input.is_symlink() {
        return Err(
            crate::infra::static_logs::messages::MSG_CONVERSION_VALIDATE_SYMLINK
                .replace("{}", &input.display().to_string()),
        );
    }

    // Check if it's a regular file
    if !input.is_file() {
        return Err(
            crate::infra::static_logs::messages::MSG_CONVERSION_VALIDATE_REGULAR
                .replace("{}", &input.display().to_string()),
        );
    }

    // Check if file is readable by attempting to open it
    if let Err(e) = fs::File::open(input) {
        return Err(
            crate::infra::static_logs::messages::MSG_CONVERSION_VALIDATE_READ
                .replacen("{}", &input.display().to_string(), 1)
                .replacen("{}", &e.to_string(), 1),
        );
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
        return Err(
            crate::infra::static_logs::messages::MSG_CONVERSION_VALIDATE_OUTPUT_UTF8
                .replace("{}", &output.display().to_string()),
        );
    }

    ensure_output_parent_resolves(output)?;

    match fs::symlink_metadata(output) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() {
                return Err(format!(
                    "Output path is a symbolic link, refusing to overwrite: {}",
                    output.display()
                ));
            }
            if !metadata.file_type().is_file() {
                return Err(format!(
                    "Output path exists but is not a regular file: {}",
                    output.display()
                ));
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => {
            return Err(format!(
                "Failed to inspect output path {} before overwrite: {e}",
                output.display()
            ));
        }
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
    let absolute = crate::media_conversion_gate::delivery_join_relative_to_cwd_or_err(
        path,
        "conversion ensure_output_parent_resolves",
    )?;

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
/// - In `apple_compat` mode: migrate AAE beside the output stem
/// - Otherwise: delete orphaned AAE file
///
/// # Errors
/// Returns an error when sidecar discovery, migration, metadata reapplication,
/// or deletion fails.
pub fn handle_aae_file(
    input: &Path,
    output: &Path,
    apple_compat: bool,
) -> std::io::Result<crate::metadata::AaeSidecarAction> {
    crate::metadata::handle_aae_sidecar(input, output, apple_compat)
}
#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use super::*;
    use std::process::Command;
    use tempfile::{NamedTempFile, tempdir_in};

    fn generated_jxl_toolchain_available_or_skip(contract_label: &str) -> bool {
        crate::test_ci_contract::require_imagemagick_in_ci(contract_label);
        crate::test_ci_contract::require_tool_on_path(crate::constants::TOOL_CJXL, contract_label);
        crate::test_ci_contract::require_tool_on_path(crate::constants::TOOL_DJXL, contract_label);
        if !crate::test_ci_contract::exiftool_available_or_ci_panic() {
            return false;
        }
        crate::MagickBuilder::check_available()
            && which::which(crate::constants::TOOL_CJXL).is_ok()
            && which::which(crate::constants::TOOL_DJXL).is_ok()
    }

    fn command_status_success(command: &mut Command, label: &str) {
        let output = command
            .output()
            .unwrap_or_else(|err| panic!("{label} failed to launch: {err}"));
        assert!(
            output.status.success(),
            "{label} failed: stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn exiftool_validate_text(path: &Path) -> String {
        let output = Command::new(crate::constants::TOOL_EXIFTOOL)
            .args(["-validate", "-warning", "-error"])
            .arg(path)
            .output()
            .unwrap_or_else(|err| panic!("exiftool validate failed to launch: {err}"));
        assert!(
            output.status.success(),
            "exiftool validate failed: stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    }

    fn exiftool_tag_value(path: &Path, tag: &str) -> String {
        let output = Command::new(crate::constants::TOOL_EXIFTOOL)
            .args(["-s3", tag])
            .arg(path)
            .output()
            .unwrap_or_else(|err| panic!("exiftool {tag} failed to launch: {err}"));
        assert!(
            output.status.success(),
            "exiftool {tag} failed: stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    fn assert_djxl_decodes(path: &Path, output_png: &Path) {
        command_status_success(
            Command::new(crate::constants::TOOL_DJXL)
                .arg(path)
                .arg(output_png),
            "djxl decode",
        );
    }

    fn replace_jxl_exif_brob_with_corrupt_payload(path: &Path) {
        let mut bytes = std::fs::read(path).unwrap_or_else(|err| panic!("read JXL fixture: {err}"));
        let mut offset = 0usize;
        let corrupt_box = [
            0x00, 0x00, 0x00, 0x0D, b'b', b'r', b'o', b'b', b'E', b'x', b'i', b'f', 0x06,
        ];
        while offset.saturating_add(8) <= bytes.len() {
            let mut size_bytes = [0u8; 4];
            size_bytes.copy_from_slice(&bytes[offset..offset + 4]);
            let box_size_u32 = u32::from_be_bytes(size_bytes);
            let mut header_len = 8usize;
            let box_size = if box_size_u32 == 1 {
                let mut large_size_bytes = [0u8; 8];
                large_size_bytes.copy_from_slice(&bytes[offset + 8..offset + 16]);
                header_len = 16;
                usize::try_from(u64::from_be_bytes(large_size_bytes))
                    .expect("JXL large box size must fit usize in test")
            } else if box_size_u32 == 0 {
                bytes.len() - offset
            } else {
                usize::try_from(box_size_u32).expect("JXL box size must fit usize in test")
            };
            assert!(
                box_size >= header_len && offset.saturating_add(box_size) <= bytes.len(),
                "malformed JXL box while preparing corrupt EXIF fixture"
            );
            if &bytes[offset + 4..offset + 8] == b"brob"
                && &bytes[offset + header_len..offset + header_len + 4] == b"Exif"
            {
                bytes.splice(offset..offset + box_size, corrupt_box);
                std::fs::write(path, bytes)
                    .unwrap_or_else(|err| panic!("write corrupt JXL fixture: {err}"));
                return;
            }
            offset += box_size;
        }
        panic!("test JXL fixture did not contain a brob Exif box");
    }

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
    fn test_converted_with_message_owned_input_size_zero_has_no_size_reduction() {
        let input = NamedTempFile::new().unwrap();
        let output = NamedTempFile::new().unwrap();

        let result = TaskResult::converted_with_message_owned(
            input.path(),
            output.path(),
            0,
            123,
            "test message".to_string(),
        );

        assert!(
            result.size_reduction.is_none(),
            "input_size==0 must not fabricate size_reduction numeric value"
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
    fn test_format_size_change_input_size_zero_does_not_fabricate_reduction() {
        let msg = format_size_change(0, 500).unwrap();
        assert!(
            msg.contains("N/A"),
            "zero input_size must not fabricate a numeric reduction message: {msg}"
        );
    }

    #[test]
    fn test_calculate_size_reduction_input_size_zero_returns_nan() {
        let reduction = calculate_size_reduction(0, 500);
        assert!(
            reduction.is_nan(),
            "zero input_size must return NaN, not fabricated 0.0: {reduction}"
        );
    }

    #[test]
    fn test_size_delta_summary_zero_input_change_pct_is_nan() {
        let delta = SizeDeltaSummary::from_sizes(0, 500);
        assert!(
            delta.change_pct.is_nan(),
            "zero input_size must not fabricate 0.0 change_pct"
        );
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
    fn test_commit_temp_to_output_with_metadata_accepts_temp_in_different_parent() {
        let temp_dir = tempdir_in("/tmp").unwrap_or_else(|e| panic!("create temp dir: {e:?}"));
        let temp_parent = temp_dir.path().join("temp_parent");
        let output_parent = temp_dir.path().join("output_parent");
        std::fs::create_dir_all(&temp_parent).unwrap();
        std::fs::create_dir_all(&output_parent).unwrap();

        let temp = temp_parent.join("temp.jxl");
        let output = output_parent.join("final.jxl");
        std::fs::write(&temp, b"jxl").unwrap_or_else(|e| panic!("write temp: {e:?}"));

        let committed = commit_temp_to_output_with_metadata(&temp, &output, true, None)
            .unwrap_or_else(|e| panic!("cross-parent commit should succeed: {e:?}"));

        assert!(committed);
        assert!(output.exists(), "output file should exist after commit");
        assert_eq!(std::fs::read(&output).unwrap(), b"jxl");
        assert!(!temp.exists(), "temp file should be removed after commit");
    }

    #[test]
    fn test_commit_temp_to_output_with_metadata_accepts_isolated_temp_root() {
        let temp_dir = tempdir_in("/tmp").unwrap_or_else(|e| panic!("create temp dir: {e:?}"));
        let temp_root = temp_dir.path().join("isolated_cache");
        let output_parent = temp_dir.path().join("output_parent");
        std::fs::create_dir_all(&temp_root).unwrap();
        std::fs::create_dir_all(&output_parent).unwrap();

        let temp = temp_root.join("search-output.search.123456.jxl");
        let output = output_parent.join("final.jxl");
        std::fs::write(&temp, b"jxl").unwrap_or_else(|e| panic!("write temp: {e:?}"));

        let committed = commit_temp_to_output_with_metadata(&temp, &output, true, None)
            .unwrap_or_else(|e| panic!("isolated temp root commit should succeed: {e:?}"));

        assert!(committed);
        assert!(output.exists(), "output file should exist after commit");
        assert_eq!(std::fs::read(&output).unwrap(), b"jxl");
        assert!(!temp.exists(), "temp file should be removed after commit");
    }

    #[test]
    fn test_commit_temp_to_output_missing_original_still_delivers_output() {
        let temp_dir = tempdir_in("/tmp").unwrap_or_else(|e| panic!("create temp dir: {e:?}"));
        let temp = temp_dir.path().join("temp.jxl");
        let output = temp_dir.path().join("final.jxl");
        let missing_original = temp_dir.path().join("missing-source.jpg");
        std::fs::write(&temp, b"jxl").unwrap_or_else(|e| panic!("write temp: {e:?}"));

        let committed =
            commit_temp_to_output_with_metadata(&temp, &output, false, Some(&missing_original))
                .unwrap_or_else(|e| {
                    panic!("missing source metadata must not block delivery commit: {e:?}")
                });
        assert!(committed, "commit should succeed");
        assert!(
            output.exists(),
            "output must remain after best-effort metadata skip"
        );
        assert_eq!(std::fs::read(&output).unwrap(), b"jxl");
    }

    #[test]
    fn commit_temp_to_output_repairs_corrupt_jxl_brotli_exif_box() {
        let contract_label = "JXL corrupt Brotli EXIF delivery repair";
        if !generated_jxl_toolchain_available_or_skip(contract_label) {
            return;
        }

        let temp_dir = tempdir_in("/tmp").unwrap_or_else(|e| panic!("create temp dir: {e:?}"));
        let source = temp_dir.path().join("source.jpg");
        let temp = temp_dir.path().join("temp.JXL");
        let output = temp_dir.path().join("final.JXL");

        command_status_success(
            Command::new(crate::media_conversion_gate::delivery_imagemagick_cli_path_or_default())
                .arg("-size")
                .arg("2x2")
                .arg("canvas:red")
                .arg(&source),
            "create source JPEG",
        );
        command_status_success(
            Command::new(crate::constants::TOOL_EXIFTOOL)
                .arg("-overwrite_original")
                .arg("-Orientation=Rotate 90 CW")
                .arg("-Make=MFBTestMake")
                .arg("-Model=MFBTestModel")
                .arg("-EXIF:DateTimeOriginal=2020:01:02 03:04:05")
                .arg(&source),
            "write source EXIF",
        );
        command_status_success(
            Command::new(crate::constants::TOOL_CJXL)
                .arg(&source)
                .arg(&temp)
                .arg("--lossless_jpeg=1")
                .arg("--effort=7"),
            "encode source JXL",
        );
        command_status_success(
            Command::new(crate::constants::TOOL_EXIFTOOL)
                .arg("-overwrite_original")
                .arg("-tagsFromFile")
                .arg(&source)
                .arg("-all:all")
                .arg("-unsafe")
                .arg(&temp),
            "copy source metadata to JXL",
        );
        replace_jxl_exif_brob_with_corrupt_payload(&temp);
        let before = exiftool_validate_text(&temp);
        assert!(
            before.contains("Corrupted Brotli 'Exif' data"),
            "fixture must reproduce Photos-incompatible JXL metadata: {before}"
        );
        assert_djxl_decodes(&temp, &temp_dir.path().join("before.png"));

        let committed = commit_temp_to_output_with_metadata(&temp, &output, true, Some(&source))
            .unwrap_or_else(|err| panic!("commit should repair corrupt JXL EXIF: {err}"));

        assert!(committed);
        let after = exiftool_validate_text(&output);
        assert!(
            after.contains("Validate                        : OK"),
            "JXL metadata must validate after delivery repair: {after}"
        );
        assert!(
            !after.contains("Corrupted Brotli 'Exif' data"),
            "delivery repair must remove corrupt JXL EXIF box: {after}"
        );
        let orientation = exiftool_tag_value(&output, "-Orientation");
        assert_eq!(
            orientation, "",
            "JXL corrupt EXIF repair must not rehydrate Orientation"
        );
        assert_eq!(exiftool_tag_value(&output, "-Make"), "MFBTestMake");
        assert_eq!(exiftool_tag_value(&output, "-Model"), "MFBTestModel");
        assert_eq!(
            exiftool_tag_value(&output, "-DateTimeOriginal"),
            "2020:01:02 03:04:05"
        );
        assert_djxl_decodes(&output, &temp_dir.path().join("after.png"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn test_commit_runs_jxl_exif_repair_before_final_timestamp_restore() {
        let source = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/convert/conversion.rs"),
        )
        .expect("conversion source");
        let production_source = source.split("#[cfg(test)]").next().unwrap_or(&source);
        let commit_start = source
            .find("pub fn commit_temp_to_output_with_metadata(")
            .expect("commit function must exist");
        let helper_start = source[commit_start..]
            .find("fn repair_corrupt_jxl_brotli_exif_for_delivery(")
            .expect("final JXL EXIF repair helper must exist");
        let commit_source = &source[commit_start..commit_start + helper_start];
        let strip_pos = commit_source
            .find("strip_residual_orientation_tag_for_delivery(output)?;")
            .expect("generic Orientation cleanup call must exist");
        let repair_pos = commit_source
            .find("repair_corrupt_jxl_brotli_exif_for_delivery(output, Some(src))?;")
            .expect("source-aware JXL metadata repair call must exist");
        let branding_pos = commit_source
            .find("append_mfb_branding(output)")
            .expect("Finder branding call must exist");
        let exact_copy_pos = commit_source
            .find("reapply_macos_exact_copy_xattrs_for_delivery(")
            .expect("exact-copy xattr reapply call must exist");
        let timestamp_pos = commit_source
            .find("apply_file_timestamps_for_delivery(src, output, &mut ts_report)?;")
            .expect("timestamp restoration call must exist");

        assert!(
            strip_pos < branding_pos
                && strip_pos < repair_pos
                && repair_pos < branding_pos
                && branding_pos < exact_copy_pos
                && exact_copy_pos < timestamp_pos
                && repair_pos < timestamp_pos,
            "JXL corrupt-EXIF repair must run after metadata copy and before final timestamp restore"
        );
        let forbidden_downstream_helper =
            ["strip_jxl_exif", "_if_orientation_remains_for_delivery"].concat();
        assert!(
            !production_source.contains(&forbidden_downstream_helper),
            "JXL Orientation must be excluded during metadata copy, not stripped downstream"
        );
    }

    #[test]
    fn test_orientation_cleanup_skips_jxl_delivery_outputs() {
        let temp_dir = tempdir_in("/tmp").unwrap_or_else(|e| panic!("create temp dir: {e:?}"));
        let jxl_output = temp_dir.path().join("out.jxl");
        std::fs::write(
            &jxl_output,
            [
                0x00, 0x00, 0x00, 0x0C, 0x4A, 0x58, 0x4C, 0x20, 0x0D, 0x0A, 0x87, 0x0A,
            ],
        )
        .unwrap_or_else(|e| panic!("write jxl magic: {e:?}"));

        strip_residual_orientation_tag_for_delivery(&jxl_output)
            .expect("JXL Orientation cleanup must happen upstream during metadata copy");
    }

    #[test]
    fn test_orientation_pixel_verification_policy_is_global_for_delivery_formats() {
        let temp_dir = tempdir_in("/tmp").unwrap_or_else(|e| panic!("create temp dir: {e:?}"));
        let jxl_output = temp_dir.path().join("out.jxl");
        let avif_output = temp_dir.path().join("out.avif");
        std::fs::write(
            &jxl_output,
            [
                0x00, 0x00, 0x00, 0x0C, 0x4A, 0x58, 0x4C, 0x20, 0x0D, 0x0A, 0x87, 0x0A,
            ],
        )
        .unwrap_or_else(|e| panic!("write jxl magic: {e:?}"));
        std::fs::write(&avif_output, b"\x00\x00\x00\x18ftypavif")
            .unwrap_or_else(|e| panic!("write avif magic: {e:?}"));

        assert!(
            delivery_orientation_diff_tolerance_for_output(&jxl_output)
                .unwrap()
                .is_some()
        );
        assert!(
            delivery_orientation_diff_tolerance_for_output(&avif_output)
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn test_load_processed_list_rejects_invalid_blob_without_partial_load() {
        clear_processed_list();
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = crate::mfb_sqlite_store::set_test_store_path_for_tests(
            dir.path().join("mfb_store.sqlite"),
        );

        let tracked = std::env::temp_dir().join("mfb-processed-track.mp4");
        mark_as_processed(&tracked);
        clear_processed_list();

        let session_key = "test-invalid-processed-blob";
        crate::mfb_sqlite_store::blob_put(
            crate::mfb_sqlite_store::NS_PROCESSED,
            session_key,
            PROCESSED_LIST_BLOB_SCHEMA,
            None,
            b"{not-json",
        )
        .expect("seed invalid blob");

        let err = load_processed_list(session_key).expect_err("invalid blob must fail load");
        assert!(
            !is_already_processed(&tracked),
            "processed list must not be partially updated on decode failure"
        );
        assert!(
            err.kind() == std::io::ErrorKind::InvalidData,
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

    #[cfg(unix)]
    #[test]
    fn test_validate_output_path_rejects_dangling_symlink_leaf() {
        use std::os::unix::fs::symlink;
        let temp = tempfile::TempDir::new().unwrap_or_else(|e| panic!("temp dir: {e:?}"));
        let output = temp.path().join("out.jxl");
        let missing_target = temp.path().join("missing-target.jxl");
        symlink(&missing_target, &output).unwrap_or_else(|e| panic!("symlink leaf: {e:?}"));

        let err = validate_output_path(&output, None)
            .err()
            .unwrap_or_else(|| panic!("dangling symlink output leaf should be rejected"));
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
        let temp = tempfile::tempdir().expect("create temp dir");
        let input = temp.path().join("input.webp");
        fs::write(&input, b"input bytes").expect("write input");
        let options = ConvertOptions::default();
        let result = TaskResult::failed_with_fallback(
            &input,
            &options,
            "fallback preserved",
            "encode_failed",
        )
        .expect("fallback result");

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
        assert!(!opts.allow_size_tolerance());
    }

    #[test]
    fn require_output_delivery_bypasses_size_guard_skip() {
        let tmp = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e:?}"));
        let input = tmp.path().join("tiny.jpg");
        let output = tmp.path().join("tiny.JXL");
        fs::write(&input, b"jpeg").unwrap_or_else(|e| panic!("write input: {e:?}"));
        fs::write(&output, b"larger-jxl-output").unwrap_or_else(|e| panic!("write output: {e:?}"));
        let mut opts = ConvertOptions::default();
        opts.flags.set(ConvertFlags::REQUIRE_OUTPUT_DELIVERY, true);

        let result = check_size_tolerance(&input, &output, 4, 17, &opts, "JPEG lossless");

        assert!(result.is_none());
        assert!(
            output.exists(),
            "required delivery must preserve the verified output even when larger"
        );
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
        assert!(
            result.message.contains("[OK]"),
            "success_video_explored uses plain-safe OK marker: {}",
            result.message
        );
    }

    #[test]
    fn test_success_input_size_zero_does_not_fabricate_size_reduction() {
        let input = Path::new("input.jpg");
        let output = Path::new("output.avif");
        let result = TaskResult::success(input, output, 0, 123, "AVIF", None, None);
        assert!(
            result.size_reduction.is_none(),
            "input_size==0 must not fabricate percent reduction"
        );
        assert!(
            result.message.contains("ratio N/A"),
            "message should disclose unavailable ratio instead of fake 0.0%"
        );
    }

    #[test]
    fn test_success_video_explored_input_size_zero_does_not_fabricate_size_reduction() {
        let input_path = Path::new("input.mov");
        let output_path = Path::new("output.mp4");
        let metrics = VideoExplorationMetrics {
            input_size: 0,
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
        assert!(
            result.size_reduction.is_none(),
            "input_size==0 must not fabricate explored reduction percent"
        );
        assert!(
            result.message.contains("N/A"),
            "explored message should disclose unavailable reduction"
        );
    }

    #[test]
    fn test_dimensions_from_header_gif87a() {
        // GIF87a, 160x120 (0xA0 0x00, 0x78 0x00)
        let bytes = [
            b'G', b'I', b'F', b'8', b'7', b'a', 0xA0, 0x00, 0x78, 0x00, 0x00, 0x00,
        ];
        let tmp = NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), bytes).unwrap();
        assert_eq!(
            dimensions_from_header(tmp.path()).unwrap(),
            Some((160, 120))
        );
    }

    #[test]
    fn test_dimensions_from_header_gif89a() {
        let bytes = [
            b'G', b'I', b'F', b'8', b'9', b'a', 0x01, 0x02, 0x03, 0x04, 0x00, 0x00,
        ];
        let tmp = NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), bytes).unwrap();
        assert_eq!(
            dimensions_from_header(tmp.path()).unwrap(),
            Some((0x0201, 0x0403))
        );
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
        assert_eq!(
            dimensions_from_header(tmp.path()).unwrap(),
            Some((640, 480))
        );
    }

    #[test]
    fn test_media_info_without_ffprobe_header_fallback_preserves_unknown_depth() {
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

        let info = media_info_without_ffprobe(tmp.path()).unwrap().unwrap();
        assert_eq!(info.width, 640);
        assert_eq!(info.height, 480);
        assert_eq!(info.channel_type, None);
        assert_eq!(info.bit_depth, None);
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
        assert_eq!(
            dimensions_from_header(tmp.path()).unwrap(),
            Some((640, 480))
        );
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
        assert_eq!(
            dimensions_from_header(tmp.path()).unwrap(),
            Some((1024, 768))
        );
    }

    #[test]
    fn scan_jpeg_sof_truncated_sof_segment_returns_none() {
        // SOF0 marker claims length 17 but payload is truncated — must not panic.
        let bytes = [0xFF, 0xD8, 0xFF, 0xC0, 0x00, 0x11, 0x08, 0x00, 0x01];
        let tmp = NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), bytes).unwrap();
        assert_eq!(dimensions_from_header(tmp.path()).unwrap(), None);
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
        assert_eq!(
            dimensions_from_header(tmp.path()).unwrap(),
            Some((640, 480))
        );
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
        assert_eq!(
            dimensions_from_header(tmp.path()).unwrap(),
            Some((160, 120))
        );
    }

    #[test]
    fn test_dimensions_from_header_rejects_unknown() {
        let bytes = b"this is not any recognised image format at all whatsoever";
        let tmp = NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), bytes).unwrap();
        assert_eq!(dimensions_from_header(tmp.path()).unwrap(), None);
    }

    #[test]
    fn test_dimensions_from_header_rejects_truncated_gif() {
        // GIF magic but truncated before width/height
        let bytes = b"GIF89a";
        let tmp = NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), bytes).unwrap();
        assert_eq!(dimensions_from_header(tmp.path()).unwrap(), None);
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
        assert_eq!(dimensions_from_header(tmp.path()).unwrap(), None);
    }

    #[test]
    fn dimensions_from_header_missing_file_returns_error_not_none() {
        let dir = tempfile::tempdir().expect("tempdir");
        let missing = dir.path().join("missing.jpg");

        let err = dimensions_from_header(&missing).expect_err("missing file must be an error");

        assert_eq!(err.kind(), std::io::ErrorKind::NotFound);
    }

    #[test]
    fn jpeg_precision_from_header_missing_file_returns_error_not_none() {
        let dir = tempfile::tempdir().expect("tempdir");
        let missing = dir.path().join("missing.jpg");

        let err = jpeg_precision_from_header(&missing).expect_err("missing file must be an error");

        assert_eq!(err.kind(), std::io::ErrorKind::NotFound);
    }

    #[test]
    fn media_info_without_ffprobe_missing_file_returns_error_not_none() {
        let dir = tempfile::tempdir().expect("tempdir");
        let missing = dir.path().join("missing.avif");

        let err = media_info_without_ffprobe(&missing).expect_err("missing file must be an error");

        assert!(err.to_string().contains("missing.avif"));
    }

    #[test]
    fn test_reserve_unique_output_path_boundary_and_disk() {
        let _lock = TEST_RESERVATION_LOCK
            .lock()
            .unwrap_or_else(|e| panic!("{e:?}"));

        let temp_dir_handle = tempfile::tempdir().unwrap();
        let temp_dir_path = temp_dir_handle.path().canonicalize().unwrap();
        let input1 = temp_dir_path.join("input1.jpg");
        let input2 = temp_dir_path.join("input2.jpg");
        let candidate = temp_dir_path.join("output.jxl");

        // Ensure reservations are clear
        super::clear_reserved_output_paths();

        // 1. First reservation
        let path1 = super::reserve_unique_output_path(&input1, candidate.clone());
        assert_eq!(path1, candidate);

        // 2. Same input, same candidate (should return the same, no collision)
        let path1_again = super::reserve_unique_output_path(&input1, candidate.clone());
        assert_eq!(path1_again, candidate);

        // 3. Different input, same candidate (memory collision)
        let path2 = super::reserve_unique_output_path(&input2, candidate.clone());
        assert_eq!(path2, super::path_with_collision_suffix(&candidate, 1));

        // 4. Simulate a disk collision from a past run
        let candidate_disk = temp_dir_path.join("output_disk.jxl");
        std::fs::write(&candidate_disk, "dummy").unwrap();

        let input3 = temp_dir_path.join("input3.jpg");
        let path3 = super::reserve_unique_output_path(&input3, candidate_disk.clone());
        // Should detect disk file and append -1
        assert_eq!(path3, super::path_with_collision_suffix(&candidate_disk, 1));

        // 5. Verify the boundary bug fix: owner == input, but file exists on disk
        let input4 = temp_dir_path.join("input4.jpg");
        let candidate_exist = temp_dir_path.join("output_exist.jxl");

        // First reserve it
        let path4 = super::reserve_unique_output_path(&input4, candidate_exist.clone());
        assert_eq!(path4, candidate_exist);

        // Now simulate creating the file on disk
        std::fs::write(&candidate_exist, "dummy").unwrap();

        // Reserving AGAIN for the SAME input should return the original path,
        // because owner == input_key supersedes the disk check.
        let path4_again = super::reserve_unique_output_path(&input4, candidate_exist.clone());
        assert_eq!(path4_again, candidate_exist);

        // 6. Multi-level collision test (3+ inputs colliding on the same candidate)
        // This ensures clean numbering like -1, -2 instead of chained suffixes like -1-2
        let input5 = temp_dir_path.join("input5.jpg");
        let input6 = temp_dir_path.join("input6.jpg");
        let multi_candidate = temp_dir_path.join("multi_output.jxl");

        let path_multi_1 = super::reserve_unique_output_path(&input4, multi_candidate.clone());
        assert_eq!(path_multi_1, multi_candidate); // Claimed by input4

        let path_multi_2 = super::reserve_unique_output_path(&input5, multi_candidate.clone());
        assert_eq!(
            path_multi_2,
            super::path_with_collision_suffix(&multi_candidate, 1)
        ); // -1

        let path_multi_3 = super::reserve_unique_output_path(&input6, multi_candidate.clone());
        assert_eq!(
            path_multi_3,
            super::path_with_collision_suffix(&multi_candidate, 2)
        ); // -2 (NOT -1-2)

        super::clear_reserved_output_paths();
    }
}
