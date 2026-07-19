//! Common Utilities Module
//!
//! Collection of common utility functions
//!
//! This module extracts common patterns recurring in the project, including:
//! - File operation helper functions
//! - String processing tools
//! - Command execution helper functions
//! - Path processing tools
//!
//! ## Design Principles
//! - Single Responsibility: Each function does one thing
//! - Reusability: Functions are designed to be generic and context-independent
//! - Error Transparency: All errors include detailed context
//! - Comprehensive Documentation: Each function has clear docs and examples

use crate::types::ProcessHistory;
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
#[inline]
#[must_use]
pub fn get_extension_lowercase(path: &Path) -> String {
    crate::media_conversion_gate::path_extension_lowercase_or_empty_unchecked(path)
}

/// Resolve the Apple Photos library path for use with `osxphotos query --db`.
///
/// This bypasses macOS TCC/sandbox restrictions on direct container directory access.
/// It attempts to find the Photos library through standard macOS APIs and returns
/// the library path in a format compatible with the `--db` argument.
///
/// # Returns
/// A `PathBuf` representing the Photos library path
///
/// # Errors
/// Returns an error if the Photos library path cannot be determined.
pub fn photos_library_path() -> anyhow::Result<PathBuf> {
    // Check environment variable override first
    if let std::result::Result::Ok(env_path) = std::env::var("MFB_PHOTOS_LIBRARY_PATH") {
        let path = PathBuf::from(env_path);
        if path.exists() {
            return Ok(path);
        }
    }

    let home = match std::env::var("HOME") {
        Ok(h) => PathBuf::from(h),
        Err(e) => {
            let err = anyhow::anyhow!("HOME environment variable not set: {e}");
            tracing::error!("{}", err);
            return Err(err);
        }
    };

    let pictures_dir = home.join("Pictures");
    let mut candidates = Vec::new();

    if let std::result::Result::Ok(entries) = std::fs::read_dir(&pictures_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() && path.extension().and_then(|e| e.to_str()) == Some("photoslibrary") {
                let db_path = path.join("database/Photos.sqlite");
                if db_path.exists()
                    && let std::result::Result::Ok(metadata) = std::fs::metadata(&db_path)
                    && let std::result::Result::Ok(modified) = metadata.modified()
                {
                    candidates.push((path, modified));
                }
            }
        }
    }

    // Sort candidates by modification time, newest first
    candidates.sort_by_key(|b| std::cmp::Reverse(b.1));

    if let Some((best_path, _)) = candidates.first() {
        return Ok(best_path.clone());
    }

    // Common macOS Photos library paths as fallback
    let default_paths = [
        home.join("Pictures/Photos Library.photoslibrary"),
        home.join("Pictures/Photos.photoslibrary"),
        home.join("Pictures/My Photolibrary.photoslibrary"),
    ];

    // Test each potential path
    for path in &default_paths {
        if path.exists() && path.is_dir() {
            return Ok(path.clone());
        }
    }

    let err = anyhow::anyhow!(
        "Photos library path not found; tried:\n{}",
        default_paths
            .iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>()
            .join("\n")
    );
    tracing::error!("{}", err);
    Err(err)
}

/// Returns the current processing history (version and timestamp)
#[must_use]
pub fn get_current_history() -> ProcessHistory {
    ProcessHistory {
        software_version: env!("CARGO_PKG_VERSION").to_string(),
        analysis_timestamp: crate::media_conversion_gate::unix_epoch_secs_optional(),
    }
}

#[inline]
#[must_use]
pub fn has_extension(path: &Path, extensions: &[&str]) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| extensions.iter().any(|ext| ext.eq_ignore_ascii_case(e)))
}

#[inline]
#[must_use]
pub fn is_hidden_file(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.starts_with('.'))
}

#[must_use]
pub fn extract_suggested_extension(error_msg: &str) -> Option<String> {
    if let Some(start) = error_msg.find("looks more like a ") {
        let rest = &error_msg[start + "looks more like a ".len()..];
        if let Some(end) = rest.find(')') {
            return Some(rest[..end].trim().to_lowercase());
        }
    }
    None
}

/// Ensure that a directory exists, creating it if necessary.
///
/// # Errors
/// Returns an I/O error if the directory cannot be created.
pub fn ensure_dir_exists(dir: &Path) -> Result<()> {
    std::fs::create_dir_all(dir)
        .with_context(|| format!("Failed to create directory: {}", dir.display()))
}

/// Ensure that the parent directory of a file exists.
///
/// # Errors
/// Returns an I/O error if the parent directory cannot be created.
pub fn ensure_parent_dir_exists(file_path: &Path) -> Result<()> {
    if let Some(parent) = file_path.parent() {
        ensure_dir_exists(parent)?;
    }
    Ok(())
}

/// Returns the user's global project cache directory
/// (~/.`modern_format_boost/cache`/). Creates the directory if it doesn't
/// exist.
///
/// # Errors
/// Returns an I/O error if the directory cannot be determined or created.
pub fn get_user_project_cache_dir() -> anyhow::Result<PathBuf> {
    let base_dir = match crate::process_lock::get_mfb_root() {
        Ok(root) => root,
        Err(root_err) => {
            crate::media_conversion_gate::delivery_runtime_batch_audit(
                "user_project_cache_root",
                format!("MFB root unavailable for cache ({root_err}); using cwd fallback"),
            );
            crate::media_conversion_gate::delivery_cwd_or_audit("user project cache base")
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "Failed to determine cache base directory: MFB root unavailable \
                         ({root_err}) and current_dir unavailable (audited)"
                    )
                })?
        }
    };
    let mut path = base_dir;

    if path
        .file_name()
        .is_none_or(|name| name != ".modern_format_boost")
    {
        path.push(".modern_format_boost");
    }
    path.push("cache");

    if let Err(primary_err) = std::fs::create_dir_all(&path) {
        let mut fallback =
            crate::media_conversion_gate::delivery_cwd_or_audit("user project cache fallback")
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "Failed to determine project-local cache fallback after primary cache \
                         path {} creation failed: {primary_err}; current_dir unavailable (audited)",
                        path.display(),
                    )
                })?;
        fallback.push(".cache");
        std::fs::create_dir_all(&fallback).with_context(|| {
            format!(
                "Failed to create project cache directory at {} after primary cache path {} also \
                 failed: {}",
                fallback.display(),
                path.display(),
                primary_err
            )
        })?;

        crate::media_conversion_gate::delivery_runtime_path_audit(
            "delivery_runtime",
            &path,
            format!(
                "Primary cache directory unavailable ({}): {}. Falling back to {}",
                path.display(),
                primary_err,
                fallback.display()
            ),
        );
        return Ok(fallback);
    }

    Ok(path)
}

#[must_use]
pub fn compute_relative_path(path: &Path, base: &Path) -> PathBuf {
    crate::media_conversion_gate::strip_prefix_or_self(path, base, "compute_relative_path")
        .to_path_buf()
}

fn training_source_map_key(path: &Path) -> String {
    crate::media_conversion_gate::canonicalize_for_tool_input(path)
        .to_string_lossy()
        .to_string()
}

/// Resolve the original source path for a training replica when the batch
/// runner provides a replica-to-source mapping via `MFB_TRAINING_SOURCE_MAP`.
#[must_use]
pub fn resolve_training_source_path(path: &Path) -> Option<PathBuf> {
    let map_path = match std::env::var(crate::constants::ENV_MFB_TRAINING_SOURCE_MAP) {
        Ok(value) => value,
        Err(std::env::VarError::NotPresent) => return None,
        Err(e) => {
            crate::media_conversion_gate::delivery_db_batch_audit(
                "training_source_map",
                format!(
                    "failed to read {} while resolving {}: {e}",
                    crate::constants::ENV_MFB_TRAINING_SOURCE_MAP,
                    path.display()
                ),
            );
            return None;
        }
    };
    let map_path = map_path.trim();
    if map_path.is_empty() {
        return None;
    }

    let mapping_text = match std::fs::read_to_string(map_path) {
        Ok(text) => text,
        Err(e) => {
            crate::media_conversion_gate::delivery_db_batch_audit(
                "training_source_map",
                format!("failed to read training source map {map_path}: {e}"),
            );
            return None;
        }
    };
    let mapping: std::collections::HashMap<String, String> =
        match serde_json::from_str(&mapping_text) {
            Ok(mapping) => mapping,
            Err(e) => {
                crate::media_conversion_gate::delivery_db_batch_audit(
                    "training_source_map",
                    format!("failed to parse training source map {map_path}: {e}"),
                );
                return None;
            }
        };
    let lookup_key = training_source_map_key(path);
    let value_opt = match mapping.get(&lookup_key) {
        Some(v) => Some(v),
        None => mapping.get(path.to_string_lossy().as_ref()),
    };
    value_opt.map(PathBuf::from)
}

#[must_use]
pub fn training_source_path_for(path: &Path) -> PathBuf {
    crate::media_conversion_gate::delivery_training_source_path_or_input(path)
}

/// Copy a file and preserve its metadata context if possible.
///
/// # Errors
/// Returns an I/O error if the copy fails.
pub fn copy_file_with_context(source: &Path, dest: &Path) -> Result<u64> {
    std::fs::copy(source, dest).with_context(|| {
        format!(
            "Failed to copy file from {} to {}",
            source.display(),
            dest.display()
        )
    })
}

#[must_use]
pub fn detect_real_extension(path: &Path) -> Option<&'static str> {
    use std::io::Read;
    let mut file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) => {
            crate::media_conversion_gate::delivery_runtime_path_audit(
                "delivery_runtime",
                path,
                format!(
                    "Format Detection: Failed to open file for magic-byte analysis at {}: {}",
                    path.display(),
                    e
                ),
            );
            return None;
        }
    };
    let mut buffer = [0u8; 8192];
    let bytes_read = match file.read(&mut buffer) {
        Ok(n) => n,
        Err(e) => {
            crate::media_conversion_gate::delivery_runtime_path_audit(
                "delivery_runtime",
                path,
                format!(
                    "Format Detection: Failed to read file header for magic-byte analysis at {}: \
                     {}",
                    path.display(),
                    e
                ),
            );
            return None;
        }
    };

    if bytes_read < 4 {
        return None;
    }

    if buffer[0] == 0xFF && buffer[1] == 0xD8 && buffer[2] == 0xFF {
        return Some("jpg");
    }

    if buffer[0] == 0x89 && buffer[1] == 0x50 && buffer[2] == 0x4E && buffer[3] == 0x47 {
        if buffer[..bytes_read]
            .windows(4)
            .any(|chunk| chunk == b"acTL" || chunk == b"fcTL")
        {
            return Some("apng");
        }
        return Some("png");
    }

    if buffer[0] == 0x47 && buffer[1] == 0x49 && buffer[2] == 0x46 && buffer[3] == 0x38 {
        return Some("gif");
    }

    if (buffer[0] == 0x49 && buffer[1] == 0x49 && buffer[2] == 0x2A && buffer[3] == 0x00)
        || (buffer[0] == 0x4D && buffer[1] == 0x4D && buffer[2] == 0x00 && buffer[3] == 0x2A)
    {
        return Some("tif");
    }

    if buffer[0] == 0x52
        && buffer[1] == 0x49
        && buffer[2] == 0x46
        && buffer[3] == 0x46
        && bytes_read >= 12
        && buffer[8] == 0x57
        && buffer[9] == 0x45
        && buffer[10] == 0x42
        && buffer[11] == 0x50
    {
        return Some("webp");
    }

    if bytes_read >= 2 && buffer[0] == 0xFF && buffer[1] == 0x0A {
        return Some("jxl");
    }

    if bytes_read >= 12
        && buffer[0] == 0x00
        && buffer[1] == 0x00
        && buffer[2] == 0x00
        && buffer[3] == 0x0C
        && buffer[4] == 0x4A
        && buffer[5] == 0x58
        && buffer[6] == 0x4C
        && buffer[7] == 0x20
        && buffer[8] == 0x0D
        && buffer[9] == 0x0A
        && buffer[10] == 0x87
        && buffer[11] == 0x0A
    {
        return Some("jxl");
    }

    if bytes_read >= 12
        && buffer[4] == 0x66
        && buffer[5] == 0x74
        && buffer[6] == 0x79
        && buffer[7] == 0x70
    {
        let brand = &buffer[8..12];
        if matches!(
            brand,
            b"heic" | b"heix" | b"heim" | b"heis" | b"mif1" | b"msf1"
        ) {
            return Some("heic");
        }
        if matches!(brand, b"avif" | b"avis") {
            return Some("avif");
        }
        return Some("mov");
    }

    None
}

/// Calculate the BLAKE3 hash of a file.
///
/// # Errors
/// Returns an error if the file cannot be read.
pub fn calculate_blake3_hash(path: &Path) -> Result<String> {
    use std::io::Read;
    let mut file = std::fs::File::open(path)?;
    let mut hasher = blake3::Hasher::new();
    let mut buffer = vec![0u8; 65536]; // 64KB buffer on heap

    loop {
        let bytes_read = file.read(&mut buffer)?;
        if bytes_read == 0 {
            break;
        }
        if let Some(slice) = buffer.get(..bytes_read) {
            hasher.update(slice);
        } else {
            // This should never happen since bytes_read <= buffer.len()
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "Invalid byte slice length",
            )
            .into());
        }
    }

    Ok(hasher.finalize().to_hex().to_string())
}

/// Calculate the BLAKE3 hash of a file as bytes.
///
/// # Errors
/// Returns an error if the file cannot be read.
pub fn calculate_blake3_hash_bytes(path: &Path) -> Result<Vec<u8>> {
    use std::io::Read;
    let mut file = std::fs::File::open(path)?;
    let mut hasher = blake3::Hasher::new();
    let mut buffer = vec![0u8; 65536];

    loop {
        let bytes_read = file.read(&mut buffer)?;
        if bytes_read == 0 {
            break;
        }
        if let Some(slice) = buffer.get(..bytes_read) {
            hasher.update(slice);
        } else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "Invalid byte slice length",
            )
            .into());
        }
    }

    Ok(hasher.finalize().as_bytes().to_vec())
}

#[must_use]
pub fn normalize_path_string(path_str: &str) -> String {
    let mut result = path_str.replace('\\', "/");
    while result.contains("//") {
        result = result.replace("//", "/");
    }
    result
}

#[must_use]
pub fn truncate_string(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else if max_len <= 3 {
        "...".to_string()
    } else {
        format!(
            "{}...",
            crate::media_conversion_gate::utf8_prefix_or_empty(s, max_len - 3, "truncate_string")
        )
    }
}

#[must_use]
pub fn extract_digits(s: &str) -> String {
    s.chars().filter(char::is_ascii_digit).collect()
}

#[must_use]
pub fn parse_float_or_default(s: &str, default: f64) -> f64 {
    match s.parse::<f64>() {
        Ok(v) => v,
        Err(e) => {
            crate::media_conversion_gate::delivery_runtime_batch_audit(
                "delivery_numeric",
                format!("Audit: Numeric parse failure for '{s}': {e}; using default {default}"),
            );
            default
        }
    }
}

/// Execute a command and log its output.
///
/// # Errors
/// Returns an error if the command fails to execute.
pub fn execute_command_with_logging(cmd: &mut Command) -> Result<Output> {
    let command_str = format_command_for_audit(cmd);

    log_detail!(&format!(
        "{} Executing external command: {command_str}",
        crate::infra::static_logs::messages::LABEL_SYSTEM
    ));

    let output = cmd
        .output()
        .with_context(|| format!("Failed to execute command: {command_str}"))?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    if output.status.success() {
        crate::log_info!(
            crate::infra::static_logs::messages::LABEL_SYSTEM,
            format!(
                "Audit: External command successful: {} (exit {:?})",
                command_str,
                output.status.code()
            )
        );
    } else {
        crate::log_failure!(
            crate::infra::static_logs::messages::LABEL_SYSTEM,
            format!(
                "Audit: External command failed: {} (exit {:?})\nstdout: {}\nstderr: {}",
                command_str,
                output.status.code(),
                stdout,
                stderr
            )
        );
    }

    Ok(output)
}

/// Recursively find a box by type and return its payload (excluding size +
/// type). Used by ISO BMFF formats (AVIF, HEIC, JXL container).
///
/// This version correctly handles:
/// - Standard boxes (size + type + payload)
/// - Extended size boxes (size=1, followed by 64-bit size)
/// - Full boxes (with version + flags after type)
#[must_use]
pub fn find_box_data_recursive(data: &[u8], box_type: [u8; 4]) -> Option<&[u8]> {
    find_box_data_recursive_impl(data, box_type, 0, 32)
        .first()
        .copied()
}

/// Recursively find all boxes by type and return their payloads.
#[must_use]
pub fn find_all_box_data_recursive(data: &[u8], box_type: [u8; 4]) -> Vec<&[u8]> {
    find_box_data_recursive_impl(data, box_type, 0, 32)
}

fn find_box_data_recursive_impl(
    data: &[u8],
    box_type: [u8; 4],
    depth: u32,
    max_depth: u32,
) -> Vec<&[u8]> {
    if depth >= max_depth {
        return Vec::new();
    }

    let mut results = Vec::new();
    let mut pos = 0;
    while pos + 8 <= data.len() {
        let size_raw = u32::from_be_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]]);
        let size = crate::numeric_cast::u32_to_usize_strict(size_raw, "isobmff_box_size");
        let Some(size) = size else {
            pos += 4;
            continue;
        };
        let Some(current_type) = data.get(pos + 4..pos + 8) else {
            break;
        };

        let (payload_start, next_pos) = if size == 0 {
            (pos + 8, data.len())
        } else if size == 1 {
            if pos + 16 > data.len() {
                pos += 8;
                continue;
            }
            let ext_val = u64::from_be_bytes([
                data[pos + 8],
                data[pos + 9],
                data[pos + 10],
                data[pos + 11],
                data[pos + 12],
                data[pos + 13],
                data[pos + 14],
                data[pos + 15],
            ]);
            let ext = crate::numeric_cast::u64_to_usize_strict(ext_val, "isobmff_ext_size");
            if let Some(ext) = ext {
                if ext < 16 || pos + ext > data.len() {
                    pos += 16;
                    continue;
                }
                (pos + 16, pos + ext)
            } else {
                pos += 8;
                continue;
            }
        } else if size < 8 {
            pos += 8;
            continue;
        } else {
            if pos + size > data.len() {
                pos += 8;
                continue;
            }
            (pos + 8, pos + size)
        };

        if current_type == box_type
            && next_pos <= data.len()
            && payload_start < next_pos
            && let Some(p) = data.get(payload_start..next_pos)
        {
            results.push(p);
        }

        if matches!(
            current_type,
            b"moov"
                | b"trak"
                | b"mdia"
                | b"minf"
                | b"stbl"
                | b"meta"
                | b"iprp"
                | b"ipco"
                | b"moof"
                | b"traf"
        ) && next_pos > payload_start
        {
            let sub_start = if current_type == b"meta" && payload_start + 4 <= next_pos {
                payload_start + 4
            } else {
                payload_start
            };

            if sub_start < next_pos
                && let Some(sub) = data.get(sub_start..next_pos)
            {
                results.extend(find_box_data_recursive_impl(
                    sub,
                    box_type,
                    depth + 1,
                    max_depth,
                ));
            }
        }

        if next_pos <= pos {
            break;
        }
        pos = next_pos;
    }
    results
}

/// Recursively search for a box type in ISO BMFF data (e.g. "jbrd" inside "JXL
/// " container).
#[must_use]
pub fn find_any_box_recursive(data: &[u8], box_type: [u8; 4]) -> bool {
    find_any_box_recursive_impl(data, box_type, 0, 32)
}

fn find_any_box_recursive_impl(data: &[u8], box_type: [u8; 4], depth: u32, max_depth: u32) -> bool {
    if depth >= max_depth {
        return false;
    }

    let mut pos = 0;
    while pos + 8 <= data.len() {
        let Some(size) = crate::numeric_cast::u32_to_usize_strict(
            u32::from_be_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]]),
            "isobmff_box_size",
        ) else {
            pos += 4;
            continue;
        };
        let Some(current_type) = data.get(pos + 4..pos + 8) else {
            break;
        };
        if current_type == box_type {
            return true;
        }
        let (payload_start, next_pos) = if size == 0 {
            (pos + 8, data.len())
        } else if size == 1 {
            if pos + 16 > data.len() {
                pos += 8;
                continue;
            }
            let ext_val = u64::from_be_bytes([
                data[pos + 8],
                data[pos + 9],
                data[pos + 10],
                data[pos + 11],
                data[pos + 12],
                data[pos + 13],
                data[pos + 14],
                data[pos + 15],
            ]);
            let ext = crate::numeric_cast::u64_to_usize_strict(ext_val, "isobmff_ext_size");
            if let Some(ext) = ext {
                if ext < 16 || pos + ext > data.len() {
                    pos += 16;
                    continue;
                }
                (pos + 16, pos + ext)
            } else {
                pos += 8;
                continue;
            }
        } else if size < 8 {
            pos += 8;
            continue;
        } else {
            if pos + size > data.len() {
                pos += 8;
                continue;
            }
            (pos + 8, pos + size)
        };
        if next_pos > payload_start
            && let Some(sub_data) = data.get(payload_start..next_pos)
            && find_any_box_recursive_impl(sub_data, box_type, depth + 1, max_depth)
        {
            return true;
        }
        pos = next_pos;
    }
    false
}

/// Extract structural metadata (rotation/mirroring) from ISOBMFF data.
/// irot: 1 byte rotation (0=0, 1=90 CCW, 2=180, 3=270 CCW)
/// imir: 1 byte axis (0=vertical, 1=horizontal)
#[must_use]
pub fn extract_isobmff_metadata(data: &[u8]) -> std::collections::HashMap<String, String> {
    let mut metadata = std::collections::HashMap::new();

    if let Some(irot) = find_box_data_recursive(data, *b"irot")
        && let Some(&rot) = irot.first()
    {
        let angle = match rot & 0x03 {
            1 => "90",
            2 => "180",
            3 => "270",
            _ => "0",
        };
        metadata.insert("isobmff_rotation".to_string(), angle.to_string());
    }

    if let Some(imir) = find_box_data_recursive(data, *b"imir")
        && let Some(&axis) = imir.first()
    {
        let mode = if (axis & 0x01) == 0 {
            "vertical"
        } else {
            "horizontal"
        };
        metadata.insert("isobmff_mirroring".to_string(), mode.to_string());
    }

    metadata
}

static TOOL_PATH_CACHE: std::sync::OnceLock<
    std::sync::Mutex<std::collections::HashMap<String, Option<std::path::PathBuf>>>,
> = std::sync::OnceLock::new();

fn tool_override_env_name(name: &str) -> String {
    let mut key = String::from("MFB_TOOL_");
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() {
            key.push(ch.to_ascii_uppercase());
        } else {
            key.push('_');
        }
    }
    key
}

#[must_use]
pub fn resolve_tool_path(name: &str) -> Option<std::path::PathBuf> {
    let cache_mutex =
        TOOL_PATH_CACHE.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()));

    // GUI-launched macOS apps often miss shell PATH. An explicit developer
    // override and an explicitly selected PATH entry take priority over fixed
    // install locations, so nightly or locally built tools can be used safely.
    let explicit_override = std::env::var_os(tool_override_env_name(name));
    let mut fallbacks = Vec::new();
    let mut home_error = None;
    if let Some(path) = explicit_override.as_ref() {
        fallbacks.push(path.to_string_lossy().into_owned());
    } else {
        match which::which(name) {
            Ok(path) => fallbacks.push(path.to_string_lossy().into_owned()),
            Err(error) => crate::media_conversion_gate::delivery_runtime_batch_audit(
                "tool_path_lookup",
                format!("PATH lookup did not resolve {name}; checking fixed paths: {error}"),
            ),
        }
        match std::env::var(crate::constants::ENV_HOME) {
            Ok(home_dir) => {
                fallbacks.push(format!("{home_dir}/.local/bin/{name}"));
                fallbacks.push(format!("{home_dir}/.cargo/bin/{name}"));
            }
            Err(e) => {
                home_error = Some(e);
            }
        }

        fallbacks.extend([
            format!("/opt/homebrew/bin/{name}"),
            format!("/usr/local/bin/{name}"),
            format!("/usr/bin/{name}"),
            format!("/bin/{name}"),
        ]);
    }

    let name_lower = name.to_ascii_lowercase();
    let is_multimedia_tool = name_lower == "avifenc"
        || name_lower == "avifdec"
        || name_lower == "cjxl"
        || name_lower == "djxl"
        || name_lower == "ffmpeg"
        || name_lower == "ffprobe"
        || name_lower == "magick"
        || name_lower == "exiftool"
        || name_lower == "heif-convert"
        || name_lower == "heif-enc"
        || name_lower == "gif2webp"
        || name_lower == "gifski"
        || name_lower == "webpmux"
        || name_lower == "cwebp"
        || name_lower == "dwebp";

    for fallback in &fallbacks {
        let path = std::path::Path::new(fallback);
        if path.is_file() {
            let is_healthy = !is_multimedia_tool || {
                // Smoke test: confirm real tool binary runs successfully without dyld/Library not loaded crashes
                match std::process::Command::new(path).arg("-version").output() {
                    Ok(output) => {
                        let stderr = String::from_utf8_lossy(&output.stderr);
                        let stdout = String::from_utf8_lossy(&output.stdout);
                        !stderr.contains("Library not loaded")
                            && !stderr.contains("dyld:")
                            && !stdout.contains("Library not loaded")
                            && !stdout.contains("dyld:")
                    }
                    Err(err) => {
                        crate::media_conversion_gate::delivery_runtime_batch_audit(
                            "dyld_smoke_err",
                            format!("dyld/version smoke check failed with IO error: {err:?}"),
                        );
                        false
                    }
                }
            };

            if is_healthy {
                let resolved = path.to_path_buf();
                crate::media_conversion_gate::mutex_guard_or_recover(
                    "tool_path_cache",
                    cache_mutex.lock(),
                )
                .insert(name.to_string(), Some(resolved.clone()));
                return Some(resolved);
            }
            crate::media_conversion_gate::delivery_runtime_batch_audit(
                "tool_path_smoke_fail",
                format!(
                    "WARNING: Tool candidate at '{}' is corrupt or failed dyld load smoke test; skipping",
                    path.display()
                ),
            );
        }
    }

    if explicit_override.is_some() {
        crate::media_conversion_gate::delivery_runtime_batch_audit(
            "tool_path_override",
            format!(
                "explicit override for {name} is unavailable or failed its runtime health check"
            ),
        );
        return None;
    }

    {
        let mut cache = crate::media_conversion_gate::mutex_guard_or_recover(
            "tool_path_cache",
            cache_mutex.lock(),
        );
        if let Some(Some(cached_path)) = cache.get(name).cloned() {
            if cached_path.is_file() {
                return Some(cached_path);
            }
            cache.remove(name);
        }
    }

    if let Some(e) = home_error {
        crate::media_conversion_gate::delivery_runtime_batch_audit(
            "delivery_runtime",
            crate::infra::static_logs::messages::MSG_HOME_DETERMINE_FAIL
                .replace("{}", &e.to_string()),
        );
    }

    // Do NOT cache negative lookups: a transient filesystem hiccup would
    // otherwise latch the whole process into reporting the tool missing.
    // The cost of re-checking is tiny (a handful of stat calls) compared
    // to spuriously failing every file in a batch.
    match which::which(name) {
        Ok(path) => {
            let is_healthy = !is_multimedia_tool || {
                // Smoke test: confirm real tool binary runs successfully without dyld/Library not loaded crashes
                match std::process::Command::new(&path).arg("-version").output() {
                    Ok(output) => {
                        let stderr = String::from_utf8_lossy(&output.stderr);
                        let stdout = String::from_utf8_lossy(&output.stdout);
                        !stderr.contains("Library not loaded")
                            && !stderr.contains("dyld:")
                            && !stdout.contains("Library not loaded")
                            && !stdout.contains("dyld:")
                    }
                    Err(err) => {
                        crate::media_conversion_gate::delivery_runtime_batch_audit(
                            "dyld_smoke_err",
                            format!("dyld/version smoke check failed with IO error: {err:?}"),
                        );
                        false
                    }
                }
            };

            if is_healthy {
                crate::media_conversion_gate::mutex_guard_or_recover(
                    "tool_path_cache",
                    cache_mutex.lock(),
                )
                .insert(name.to_string(), Some(path.clone()));
                return Some(path);
            }
            crate::media_conversion_gate::delivery_runtime_batch_audit(
                "tool_path_smoke_fail",
                format!(
                    "WARNING: which resolved path '{}' for '{}' is corrupt or failed dyld load smoke test; skipping",
                    path.display(),
                    name
                ),
            );
            None
        }
        Err(e) => {
            crate::media_conversion_gate::delivery_runtime_batch_audit(
                "tool_path_lookup",
                format!(
                    "tool lookup failed for {name}: stable paths and PATH lookup exhausted: {e}"
                ),
            );
            None
        }
    }
}

/// Resolved external tool path, or bare `name` for `PATH` lookup (strict-gated
/// when unresolved).
#[must_use]
pub fn resolve_tool_path_or_audit(name: &str) -> std::path::PathBuf {
    crate::media_conversion_gate::delivery_tool_path_or_bare_name(name)
}

/// `ImageMagick` CLI variant (`magick` IM7 vs `convert` IM6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImagemagickCliKind {
    Magick7,
    Convert6,
}

/// Resolves an `ImageMagick` executable (`magick` preferred, else `convert`).
#[must_use]
pub fn resolve_imagemagick_cli() -> Option<(std::path::PathBuf, ImagemagickCliKind)> {
    if let Some(path) = resolve_tool_path(crate::constants::TOOL_MAGICK) {
        return Some((path, ImagemagickCliKind::Magick7));
    }
    if let Some(path) = resolve_tool_path("convert") {
        return Some((path, ImagemagickCliKind::Convert6));
    }
    None
}

/// True when the resolved `ImageMagick` CLI is IM7 (`magick`).
#[must_use]
pub fn imagemagick_uses_magick7() -> bool {
    matches!(
        resolve_imagemagick_cli(),
        Some((_, ImagemagickCliKind::Magick7))
    )
}

#[must_use]
pub fn is_command_available(command_name: &str) -> bool {
    let path = resolve_tool_path_or_audit(command_name);
    match Command::new(&path)
        .arg("--version")
        .output()
        .or_else(|_| Command::new(&path).arg("-version").output())
    {
        Ok(output) => output.status.success(),
        Err(e) => {
            crate::media_conversion_gate::delivery_runtime_batch_audit(
                "command_available",
                format!("command availability probe failed for {command_name}: {e}"),
            );
            false
        }
    }
}

#[must_use]
pub fn get_command_version(command_name: &str) -> Option<String> {
    let path = resolve_tool_path_or_audit(command_name);
    let output = match Command::new(&path)
        .arg("--version")
        .output()
        .or_else(|_| Command::new(&path).arg("-version").output())
    {
        Ok(output) => output,
        Err(e) => {
            crate::media_conversion_gate::delivery_runtime_batch_audit(
                "command_version",
                format!("version detection failed for command '{command_name}': {e}"),
            );
            return None;
        }
    };

    if output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        stdout.lines().next().map(std::string::ToString::to_string)
    } else {
        crate::media_conversion_gate::delivery_runtime_batch_audit(
            "command_version",
            format!(
                "version detection returned non-zero status for command '{command_name}': {stderr}",
                stderr = String::from_utf8_lossy(&output.stderr).trim()
            ),
        );
        None
    }
}

#[must_use]
pub fn format_command_string(command: &str, args: &[&str]) -> String {
    format_command_parts_for_audit(command, args.iter().copied())
}

#[must_use]
pub(crate) fn format_command_for_audit(cmd: &Command) -> String {
    let command = cmd.get_program().to_string_lossy();
    let args = cmd
        .get_args()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    format_command_parts_for_audit(&command, args.iter().map(String::as_str))
}

fn format_command_parts_for_audit<'a>(
    command: &str,
    args: impl IntoIterator<Item = &'a str>,
) -> String {
    let args = args.into_iter().collect::<Vec<_>>();
    if args.is_empty() {
        command.to_string()
    } else {
        let is_osascript = Path::new(command)
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.eq_ignore_ascii_case("osascript"));
        let mut previous_was_inline_script_flag = false;
        let sanitized = args
            .iter()
            .map(|arg| {
                let redacted = sanitize_command_arg_for_audit(
                    arg,
                    is_osascript && previous_was_inline_script_flag,
                );
                previous_was_inline_script_flag = is_osascript && *arg == "-e";
                redacted
            })
            .collect::<Vec<_>>();
        format!("{} {}", command, sanitized.join(" "))
    }
}

fn sanitize_command_arg_for_audit(arg: &str, redact_inline_script: bool) -> String {
    const MAX_AUDIT_ARG_CHARS: usize = 240;

    if redact_inline_script {
        return format!(
            "<inline-script bytes={} lines={}>",
            arg.len(),
            arg.lines().count().max(1)
        );
    }

    if arg.contains('\n') {
        return format!(
            "<multiline-arg bytes={} lines={}>",
            arg.len(),
            arg.lines().count().max(1)
        );
    }

    let char_count = arg.chars().count();
    if char_count > MAX_AUDIT_ARG_CHARS {
        let preview = arg.chars().take(MAX_AUDIT_ARG_CHARS).collect::<String>();
        return format!(
            "{preview}…<truncated bytes={} chars={char_count}>",
            arg.len()
        );
    }

    arg.to_string()
}

/// Validate that a file is not empty and is readable.
///
/// # Errors
/// Returns an error if the file is missing, empty, or unreadable.
pub fn validate_file_integrity(path: &std::path::Path) -> anyhow::Result<()> {
    let metadata = std::fs::metadata(path)?;
    let size = metadata.len();

    if size == 0 {
        anyhow::bail!("File is empty (0 bytes)");
    }

    if size < 12 {
        anyhow::bail!("File is too small (< 12 bytes) to be a valid image");
    }

    Ok(())
}

/// Validate that a file does not exceed a specific size limit.
///
/// # Errors
/// Returns an error if the file size cannot be determined or exceeds the limit.
pub fn validate_file_size_limit(path: &std::path::Path, max_bytes: u64) -> anyhow::Result<()> {
    let metadata = std::fs::metadata(path)?;
    let size = metadata.len();

    if size > max_bytes {
        anyhow::bail!("File is too large ({size} bytes > {max_bytes} max allowed)");
    }

    Ok(())
}

/// Escape a path for safe display in error messages.
/// Prevents ANSI escape code injection by escaping control characters.
#[must_use]
pub fn escape_path_for_display(path: &std::path::Path) -> String {
    path.display().to_string().escape_default().to_string()
}

/// A RAII guard that sets an environment variable and restores its original
/// value when dropped. Useful for thread-safe (serial) unit tests that modify
/// global environment state.
#[derive(Debug)]
pub struct EnvGuard {
    key: String,
    old_value: Option<String>,
}

impl EnvGuard {
    /// Sets an environment variable and returns a guard.
    ///
    /// # Panics
    /// Panics if the key contains an equal sign or NUL character.
    #[must_use]
    pub fn set(key: &str, value: &str) -> Self {
        let old_value = match std::env::var(key) {
            Ok(value) => Some(value),
            Err(std::env::VarError::NotPresent) => None,
            Err(e) => {
                crate::media_conversion_gate::delivery_runtime_batch_audit(
                    "env_guard",
                    format!(
                        "failed to read existing env value for {key}: {e}; restore will remove"
                    ),
                );
                None
            }
        };
        // SAFETY: EnvGuard is intended for single-threaded test context.
        unsafe {
            std::env::set_var(key, value);
        }
        Self {
            key: key.to_string(),
            old_value,
        }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        if let Some(ref old) = self.old_value {
            // SAFETY: Restoration happens when the guard is dropped.
            unsafe {
                std::env::set_var(&self.key, old);
            }
        } else {
            // SAFETY: Restoration happens when the guard is dropped.
            unsafe {
                std::env::remove_var(&self.key);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_get_extension_lowercase() {
        assert_eq!(get_extension_lowercase(Path::new("test.JPG")), "jpg");
        assert_eq!(get_extension_lowercase(Path::new("test.mp4")), "mp4");
        assert_eq!(get_extension_lowercase(Path::new("noext")), "");
        assert_eq!(get_extension_lowercase(Path::new(".hidden")), "");
    }

    #[test]
    fn test_has_extension() {
        let extensions = &["jpg", "png", "gif"];
        assert!(has_extension(Path::new("photo.JPG"), extensions));
        assert!(has_extension(Path::new("image.png"), extensions));
        assert!(!has_extension(Path::new("video.mp4"), extensions));
    }

    #[test]
    fn test_is_hidden_file() {
        assert!(is_hidden_file(Path::new(".DS_Store")));
        assert!(is_hidden_file(Path::new(".gitignore")));
        assert!(!is_hidden_file(Path::new("normal.txt")));
    }

    #[test]
    #[serial_test::serial]
    fn test_training_source_path_for_uses_env_mapping() -> anyhow::Result<()> {
        let tmp = TempDir::new()?;
        let replica = tmp.path().join("replica.gif");
        let source_root = tmp.path().join("source");
        let source = source_root.join("QQcache").join("demo.gif");
        ensure_parent_dir_exists(&replica)?;
        ensure_parent_dir_exists(&source)?;
        fs::write(&replica, b"replica")?;
        fs::write(&source, b"source")?;

        let mapping_path = tmp.path().join("source_map.json");
        let mapping = serde_json::json!({
            std::fs::canonicalize(&replica)?.to_string_lossy().to_string():
                std::fs::canonicalize(&source)?.to_string_lossy().to_string()
        });
        fs::write(&mapping_path, serde_json::to_vec(&mapping)?)?;

        let _guard = EnvGuard::set(
            crate::constants::ENV_MFB_TRAINING_SOURCE_MAP,
            &mapping_path.to_string_lossy(),
        );
        assert_eq!(
            training_source_path_for(&replica),
            std::fs::canonicalize(&source)?
        );
        Ok(())
    }

    #[test]
    fn test_training_source_path_for_falls_back_without_mapping() -> anyhow::Result<()> {
        let tmp = TempDir::new()?;
        let replica = tmp.path().join("replica.png");
        fs::write(&replica, b"replica")?;
        assert_eq!(training_source_path_for(&replica), replica);
        Ok(())
    }

    #[test]
    #[serial_test::serial]
    fn resolve_tool_path_rechecks_stable_paths_before_cached_lookup() -> anyhow::Result<()> {
        let tmp = TempDir::new()?;
        let tool_name = format!("mfb-cache-order-tool-{}", std::process::id());
        let home = tmp.path().join("home");
        let primary = home.join(".local/bin").join(&tool_name);
        let cached = tmp.path().join("path-cache").join(&tool_name);
        ensure_parent_dir_exists(&primary)?;
        ensure_parent_dir_exists(&cached)?;
        fs::write(&primary, b"primary")?;
        fs::write(&cached, b"cached")?;

        let _home_guard = EnvGuard::set(crate::constants::ENV_HOME, &home.to_string_lossy());
        let cache_mutex =
            TOOL_PATH_CACHE.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()));
        crate::media_conversion_gate::mutex_guard_or_recover(
            "tool_path_cache_test",
            cache_mutex.lock(),
        )
        .insert(tool_name.clone(), Some(cached));

        assert_eq!(resolve_tool_path(&tool_name), Some(primary));
        Ok(())
    }

    #[test]
    fn tool_override_env_name_is_stable_and_shell_safe() {
        assert_eq!(tool_override_env_name("avifenc"), "MFB_TOOL_AVIFENC");
        assert_eq!(tool_override_env_name("avif-enc"), "MFB_TOOL_AVIF_ENC");
    }

    #[test]
    fn test_ensure_dir_exists() {
        let temp = TempDir::new().unwrap_or_else(|e| panic!("error: {e:?}"));
        let nested = temp.path().join("a/b/c");

        ensure_dir_exists(&nested).unwrap_or_else(|e| panic!("error: {e:?}"));
        assert!(nested.exists());
        assert!(nested.is_dir());

        ensure_dir_exists(&nested).unwrap_or_else(|e| panic!("error: {e:?}"));
    }

    #[test]
    fn test_ensure_parent_dir_exists() {
        let temp = TempDir::new().unwrap_or_else(|e| panic!("error: {e:?}"));
        let file_path = temp.path().join("a/b/c/file.txt");

        ensure_parent_dir_exists(&file_path).unwrap_or_else(|e| panic!("error: {e:?}"));
        assert!(
            file_path
                .parent()
                .unwrap_or_else(|| panic!("missing parent"))
                .exists()
        );
    }

    #[test]
    fn test_compute_relative_path() {
        let base = Path::new("/home/user/project");
        let path = Path::new("/home/user/project/src/main.rs");
        let rel = compute_relative_path(path, base);
        assert_eq!(rel, PathBuf::from("src/main.rs"));

        let unrelated = Path::new("/tmp/file.txt");
        let rel2 = compute_relative_path(unrelated, base);
        assert_eq!(rel2, unrelated);
    }

    #[test]
    fn test_copy_file_with_context() {
        let temp = TempDir::new().unwrap_or_else(|e| panic!("error: {e:?}"));
        let source = temp.path().join("source.txt");
        let dest = temp.path().join("dest.txt");

        fs::write(&source, "test content").unwrap_or_else(|e| panic!("error: {e:?}"));

        let bytes =
            copy_file_with_context(&source, &dest).unwrap_or_else(|e| panic!("error: {e:?}"));
        assert_eq!(bytes, 12);
        assert_eq!(
            fs::read_to_string(&dest).unwrap_or_else(|e| panic!("error: {e:?}")),
            "test content"
        );
    }

    #[test]
    fn test_normalize_path_string() {
        assert_eq!(normalize_path_string("C:\\Users\\test"), "C:/Users/test");
        assert_eq!(normalize_path_string("path//to///file"), "path/to/file");
        assert_eq!(normalize_path_string("normal/path"), "normal/path");
    }

    #[test]
    fn test_truncate_string() {
        assert_eq!(truncate_string("Hello, World!", 10), "Hello, ...");
        assert_eq!(truncate_string("Short", 10), "Short");
        assert_eq!(truncate_string("Exact", 5), "Exact");
        assert_eq!(truncate_string("Too long", 3), "...");
    }

    #[test]
    fn test_extract_digits() {
        assert_eq!(extract_digits("abc123def456"), "123456");
        assert_eq!(extract_digits("no digits here"), "");
        assert_eq!(extract_digits("2024-01-15"), "20240115");
    }

    #[test]
    fn test_parse_float_or_default() {
        assert!(crate::float_compare::approx_eq_f64(
            parse_float_or_default("5.67", 0.0),
            5.67
        ));
        assert!(crate::float_compare::approx_eq_f64(
            parse_float_or_default("invalid", 1.0),
            1.0
        ));
        assert!(crate::float_compare::approx_eq_f64(
            parse_float_or_default("", 2.5),
            2.5
        ));
    }

    #[test]
    fn test_is_command_available() {
        #[cfg(unix)]
        {
            assert!(
                is_command_available("bash") || is_command_available("zsh"),
                "expected a POSIX shell that responds to --version/-version"
            );
        }

        #[cfg(windows)]
        {
            assert!(is_command_available("cmd"));
        }

        assert!(!is_command_available("nonexistent_command_xyz_123"));
    }

    #[test]
    fn test_format_command_string() {
        assert_eq!(
            format_command_string("ffmpeg", &["-i", "input.mp4", "output.mp4"]),
            "ffmpeg -i input.mp4 output.mp4"
        );
        assert_eq!(format_command_string("ls", &[]), "ls");
    }

    #[test]
    fn test_format_command_string_redacts_osascript_inline_script() {
        let command = format_command_string(
            "/usr/bin/osascript",
            &[
                "-e",
                "on run argv\nerror \"Photos returned 0 imported items\"\nend run",
                "/tmp/photo.JXL",
            ],
        );
        assert!(command.contains("/usr/bin/osascript -e <inline-script bytes="));
        assert!(command.contains("/tmp/photo.JXL"));
        assert!(!command.contains("Photos returned 0 imported items"));
    }

    #[test]
    fn test_format_command_for_audit_redacts_command_inline_script() {
        let mut cmd = Command::new("/usr/bin/osascript");
        cmd.arg("-e")
            .arg("on run argv\nerror \"hidden implementation\"\nend run")
            .arg("/tmp/photo.JXL");
        let command = format_command_for_audit(&cmd);
        assert!(command.contains("/usr/bin/osascript -e <inline-script bytes="));
        assert!(command.contains("/tmp/photo.JXL"));
        assert!(!command.contains("hidden implementation"));
    }

    #[test]
    fn test_format_command_string_summarizes_multiline_argument() {
        let command = format_command_string("tool", &["first line\nsecond line"]);
        assert_eq!(command, "tool <multiline-arg bytes=22 lines=2>");
    }

    #[test]
    fn test_execute_command_with_logging() {
        let mut cmd = Command::new("echo");
        cmd.arg("test");

        let output =
            execute_command_with_logging(&mut cmd).unwrap_or_else(|e| panic!("error: {e:?}"));
        assert!(output.status.success());

        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("test"));
    }

    #[test]
    fn test_find_box_data_extended_size_truncation() {
        // Standard box: [0, 0, 0, 1, 't', 'e', 's', 't']
        // Extended size: [0, 0, 0, 0, 0, 0, 0, 24] (total size = 16 header + 8 payload)
        // Payload: [0, 1, 2, 3, 4, 5, 6, 7]
        let mut data = vec![0, 0, 0, 1];
        data.extend_from_slice(b"test");
        data.extend_from_slice(&24u64.to_be_bytes());
        data.extend_from_slice(&[0, 1, 2, 3, 4, 5, 6, 7]);

        let found = find_box_data_recursive(&data, *b"test");
        assert_eq!(found.unwrap(), &[0, 1, 2, 3, 4, 5, 6, 7]);

        // Case 1: Truncated extended size header (only 4 bytes of 8 bytes ext size
        // present)
        let mut data_trunc = vec![0, 0, 0, 1];
        data_trunc.extend_from_slice(b"test");
        data_trunc.extend_from_slice(&[0, 0, 0, 0]); // missing 4 bytes

        let found = find_box_data_recursive(&data_trunc, *b"test");
        assert!(
            found.is_none(),
            "Should be None because header is truncated and we should continue/skip"
        );

        // Case 2: Huge ext size (larger than data)
        let mut data_huge = vec![0, 0, 0, 1];
        data_huge.extend_from_slice(b"test");
        data_huge.extend_from_slice(&1000u64.to_be_bytes()); // way larger than data.len()

        let found = find_box_data_recursive(&data_huge, *b"test");
        assert!(
            found.is_none(),
            "Should be None because size is dishonest (truncated file)"
        );
    }

    #[test]
    fn test_find_box_data_size_zero() {
        // Size 0 means "rest of file"
        let mut data = vec![0, 0, 0, 0];
        data.extend_from_slice(b"last");
        data.extend_from_slice(&[9, 8, 7, 6, 5]);

        let found = find_box_data_recursive(&data, *b"last");
        assert_eq!(found.unwrap(), &[9, 8, 7, 6, 5]);
    }
}
