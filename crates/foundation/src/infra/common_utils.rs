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
#[path = "json_evidence.rs"]
mod json_evidence;
pub(crate) use json_evidence::validate_unique_json_members;
#[inline]
#[must_use]
pub fn get_extension_lowercase(path: &Path) -> String {
    crate::media_conversion_gate::path_extension_lowercase_or_empty_unchecked(path)
}

/// Resolve and validate an explicitly selected Apple Photos library.
///
/// An explicit selection is a safety boundary: if it is present but invalid,
/// callers must not fall back to another library.
pub fn explicit_photos_library_path() -> anyhow::Result<Option<PathBuf>> {
    let Some(raw_path) = std::env::var_os("MFB_PHOTOS_LIBRARY_PATH") else {
        return Ok(None);
    };
    validate_photos_library_path(Path::new(&raw_path)).map(Some)
}

pub(crate) fn validate_photos_library_path(path: &Path) -> anyhow::Result<PathBuf> {
    if path.as_os_str().is_empty() {
        anyhow::bail!("MFB_PHOTOS_LIBRARY_PATH is set but empty");
    }
    let canonical = std::fs::canonicalize(path).with_context(|| {
        format!(
            "MFB_PHOTOS_LIBRARY_PATH does not resolve to a readable Photos library: {}",
            path.display()
        )
    })?;
    anyhow::ensure!(
        canonical.is_dir()
            && canonical
                .extension()
                .and_then(|extension| extension.to_str())
                == Some("photoslibrary"),
        "MFB_PHOTOS_LIBRARY_PATH is not a .photoslibrary package: {}",
        canonical.display()
    );
    let database = canonical.join("database/Photos.sqlite");
    anyhow::ensure!(
        database.is_file(),
        "MFB_PHOTOS_LIBRARY_PATH has no Photos database: {}",
        database.display()
    );
    std::fs::File::open(&database).with_context(|| {
        format!(
            "MFB_PHOTOS_LIBRARY_PATH database is not readable: {}",
            database.display()
        )
    })?;
    anyhow::ensure!(
        database.canonicalize()?.starts_with(&canonical),
        "Photos database must remain inside the selected library"
    );
    Ok(canonical)
}

/// Resolve candidate Apple Photos libraries for `osxphotos query --db`.
///
/// An explicit override is exclusive. Otherwise libraries in `~/Pictures` are
/// ordered by database modification time; callers must verify returned UUIDs.
pub fn photos_library_paths() -> anyhow::Result<Vec<PathBuf>> {
    if let Some(explicit) = explicit_photos_library_path()? {
        return Ok(vec![explicit]);
    }
    discovered_photos_library_paths()
}

pub(crate) fn discovered_photos_library_paths() -> anyhow::Result<Vec<PathBuf>> {
    let mut paths = Vec::new();

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

    for (path, _) in candidates {
        if !paths.contains(&path) {
            paths.push(path);
        }
    }

    // Common macOS Photos library paths as fallback
    let default_paths = [
        home.join("Pictures/Photos Library.photoslibrary"),
        home.join("Pictures/Photos.photoslibrary"),
        home.join("Pictures/My Photolibrary.photoslibrary"),
    ];

    // Test each potential path
    for path in &default_paths {
        if path.exists() && path.is_dir() && !paths.contains(path) {
            paths.push(path.clone());
        }
    }

    if !paths.is_empty() {
        return Ok(paths);
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
    let mut path =
        crate::process_lock::get_mfb_root().context("Failed to determine MFB cache root")?;
    path.push("cache");

    if let Err(primary_err) = std::fs::create_dir_all(&path) {
        let fallback = crate::media_conversion_gate::delivery_temp_mfb_root_ssot().join("cache");
        std::fs::create_dir_all(&fallback).with_context(|| {
            format!(
                "Failed to create temp cache directory at {} after primary cache path {} also \
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
        match crate::image::png_validation::is_apng_file(path) {
            Ok(true) => return Some("apng"),
            Ok(false) => {}
            Err(error) => {
                crate::media_conversion_gate::delivery_runtime_path_audit(
                    "delivery_runtime",
                    path,
                    format!("Format Detection: PNG animation validation failed: {error}"),
                );
            }
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

/// Hash an in-memory identity without writing a temporary file.
#[must_use]
pub fn blake3_hex(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex().to_string()
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
    find_all_box_data_recursive(data, box_type).first().copied()
}

/// Recursively find all boxes by type and return their payloads.
#[must_use]
pub fn find_all_box_data_recursive(data: &[u8], box_type: [u8; 4]) -> Vec<&[u8]> {
    let mut results = Vec::new();
    if collect_box_data_recursive(data, box_type, 0, &mut results) {
        results
    } else {
        Vec::new()
    }
}

fn parse_isobmff_box(data: &[u8], pos: usize) -> Option<([u8; 4], &[u8], usize)> {
    if data.len().checked_sub(pos)? < 8 {
        return None;
    }
    let size32 = u32::from_be_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]]);
    let box_type = [data[pos + 4], data[pos + 5], data[pos + 6], data[pos + 7]];
    let mut header_size = 8usize;
    let total_size = match size32 {
        0 => data.len().checked_sub(pos)?,
        1 => {
            if data.len().checked_sub(pos)? < 16 {
                return None;
            }
            header_size = 16;
            crate::numeric_cast::u64_to_usize_strict(
                u64::from_be_bytes([
                    data[pos + 8],
                    data[pos + 9],
                    data[pos + 10],
                    data[pos + 11],
                    data[pos + 12],
                    data[pos + 13],
                    data[pos + 14],
                    data[pos + 15],
                ]),
                "isobmff_ext_size",
            )?
        }
        size => crate::numeric_cast::u32_to_usize_strict(size, "isobmff_box_size")?,
    };
    if box_type == *b"uuid" {
        header_size = header_size.checked_add(16)?;
    }
    if total_size < header_size {
        return None;
    }
    let next = pos.checked_add(total_size)?;
    if next > data.len() {
        return None;
    }
    let payload_start = pos.checked_add(header_size)?;
    Some((box_type, data.get(payload_start..next)?, next))
}

/// Return the first top-level `ftyp` payload after validating its declared box
/// boundary. The payload starts with major brand and minor version.
#[must_use]
pub fn isobmff_ftyp_payload(data: &[u8]) -> Option<&[u8]> {
    let (box_type, payload, _) = parse_isobmff_box(data, 0)?;
    (box_type == *b"ftyp" && payload.len() >= 8 && payload[8..].len() % 4 == 0).then_some(payload)
}

const fn is_isobmff_container(box_type: [u8; 4]) -> bool {
    matches!(
        &box_type,
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
    )
}

fn collect_box_data_recursive<'a>(
    data: &'a [u8],
    box_type: [u8; 4],
    depth: u32,
    results: &mut Vec<&'a [u8]>,
) -> bool {
    if depth >= 32 {
        return false;
    }

    let mut pos = 0;
    while pos < data.len() {
        let Some((current_type, payload, next)) = parse_isobmff_box(data, pos) else {
            return false;
        };
        if current_type == box_type {
            results.push(payload);
        }
        if is_isobmff_container(current_type) {
            let children = if current_type == *b"meta" {
                let Some(children) = payload.get(4..) else {
                    return false;
                };
                children
            } else {
                payload
            };
            if !collect_box_data_recursive(children, box_type, depth + 1, results) {
                return false;
            }
        }
        pos = next;
    }
    true
}

/// Recursively search for a box type in ISO BMFF data (e.g. "jbrd" inside "JXL
/// " container).
#[must_use]
pub fn find_any_box_recursive(data: &[u8], box_type: [u8; 4]) -> bool {
    find_box_data_recursive(data, box_type).is_some()
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ToolIdentity {
    target: PathBuf,
    len: u64,
    modified: std::time::SystemTime,
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
    #[cfg(unix)]
    changed: (i64, i64),
}

// shortcut: metadata is not a content proof; use hashing before persisted reuse.
pub(crate) fn tool_identity(path: &Path) -> std::io::Result<ToolIdentity> {
    #[cfg(unix)]
    use std::os::unix::fs::MetadataExt;
    let target = path.canonicalize()?;
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let metadata = options.open(&target)?.metadata()?;
    if !metadata.is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "tool identity requires a regular file",
        ));
    }
    Ok(ToolIdentity {
        target,
        len: metadata.len(),
        modified: metadata.modified()?,
        #[cfg(unix)]
        device: metadata.dev(),
        #[cfg(unix)]
        inode: metadata.ino(),
        #[cfg(unix)]
        changed: (metadata.ctime(), metadata.ctime_nsec()),
    })
}

const TOOL_HEALTH_FAILURE_TTL: std::time::Duration = std::time::Duration::from_secs(5);

#[derive(Clone, Debug)]
struct FailedToolProbe {
    identity: ToolIdentity,
    checked_at: std::time::Instant,
}

impl FailedToolProbe {
    fn suppresses(&self, identity: &ToolIdentity, now: std::time::Instant) -> bool {
        self.identity == *identity && now.duration_since(self.checked_at) < TOOL_HEALTH_FAILURE_TTL
    }
}

static FAILED_TOOL_IDENTITIES: std::sync::OnceLock<
    std::sync::Mutex<std::collections::HashMap<std::path::PathBuf, FailedToolProbe>>,
> = std::sync::OnceLock::new();

fn multimedia_tool_is_healthy(path: &std::path::Path) -> bool {
    let failures = FAILED_TOOL_IDENTITIES
        .get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()));
    let identity = match tool_identity(path) {
        Ok(identity) => identity,
        Err(error) => {
            crate::media_conversion_gate::mutex_guard_or_recover(
                "failed_tool_identity_cache_write",
                failures.lock(),
            )
            .remove(path);
            crate::media_conversion_gate::delivery_runtime_path_audit(
                "tool_identity_metadata",
                path,
                format!("tool identity unavailable; not reusing its health result: {error}"),
            );
            return false;
        }
    };
    let checked_at = std::time::Instant::now();
    {
        let mut failures = crate::media_conversion_gate::mutex_guard_or_recover(
            "failed_tool_identity_cache_read",
            failures.lock(),
        );
        failures.retain(|_, failed| {
            checked_at.duration_since(failed.checked_at) < TOOL_HEALTH_FAILURE_TTL
        });
        if failures
            .get(path)
            .is_some_and(|failed| failed.suppresses(&identity, checked_at))
        {
            return false;
        }
    }

    let healthy = match std::process::Command::new(path).arg("-version").output() {
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
    };
    let mut failures = crate::media_conversion_gate::mutex_guard_or_recover(
        "failed_tool_identity_cache_write",
        failures.lock(),
    );
    if healthy {
        failures.remove(path);
    } else {
        let newly_failed = !failures
            .get(path)
            .is_some_and(|failed| failed.identity == identity);
        failures.insert(
            path.to_path_buf(),
            FailedToolProbe {
                identity,
                checked_at,
            },
        );
        drop(failures);
        if newly_failed {
            crate::media_conversion_gate::delivery_runtime_batch_audit(
                "tool_path_smoke_fail",
                format!(
                    "WARNING: Tool candidate at '{}' failed its version smoke test; retrying after its identity changes or a short timeout",
                    path.display()
                ),
            );
        }
    }
    healthy
}

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

/// An explicit configuration/CLI/environment override, excluding default PATH discovery.
#[must_use]
pub fn explicit_tool_override(name: &str) -> Option<PathBuf> {
    crate::runtime_config::active()
        .and_then(|config| config.tools.paths.get(name))
        .cloned()
        .or_else(|| std::env::var_os(tool_override_env_name(name)).map(PathBuf::from))
}

/// A shared runtime prerequisite failed; this is not an individual media failure.
#[derive(Clone, Debug)]
pub struct ToolRuntimeError {
    pub cause_code: &'static str,
    pub tool: Option<String>,
    pub message: String,
}

impl std::fmt::Display for ToolRuntimeError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "Shared IMG tool runtime blocked: {}",
            self.message
        )
    }
}

impl std::error::Error for ToolRuntimeError {}

#[derive(Debug)]
struct BoundTool {
    path: PathBuf,
    identity: ToolIdentity,
}

impl BoundTool {
    fn capture(path: &Path) -> Result<Self> {
        anyhow::ensure!(
            path.is_absolute(),
            "bound tool invocation path must be absolute"
        );
        let identity = tool_identity(path)?;
        Ok(Self {
            // Aliases such as identify/convert select behavior through argv[0].
            // Observe their resolved identity without changing the invocation name.
            path: path.to_path_buf(),
            identity,
        })
    }

    fn unchanged(&self) -> bool {
        tool_identity(&self.path).is_ok_and(|identity| identity == self.identity)
    }
}

#[derive(Debug)]
struct BoundToolRuntime {
    snapshot: Option<crate::tool_snapshot::Snapshot>,
    tools: std::collections::BTreeMap<String, BoundTool>,
    failure: std::sync::Mutex<Option<ToolRuntimeError>>,
    used: std::sync::Mutex<std::collections::BTreeSet<String>>,
}

static BOUND_TOOL_RUNTIME: std::sync::OnceLock<BoundToolRuntime> = std::sync::OnceLock::new();

fn runtime_failure(
    cause_code: &'static str,
    tool: Option<&str>,
    message: String,
) -> ToolRuntimeError {
    ToolRuntimeError {
        cause_code,
        tool: tool.map(str::to_owned),
        message,
    }
}

/// Install one immutable selection after effective configuration resolution.
/// Optional, uncaptured tools cannot later escape to a newer PATH selection.
///
/// # Errors
/// Rejects changed snapshots, unavailable selections, duplicate binding, and failed probes.
pub fn bind_tool_runtime(
    snapshot: Option<crate::tool_snapshot::Snapshot>,
    observed: std::collections::BTreeMap<String, PathBuf>,
    required: &[&str],
) -> Result<()> {
    if let Some(snapshot) = &snapshot {
        snapshot.verify()?;
    }
    let mut paths = observed;
    if let Some(snapshot) = &snapshot {
        paths.extend(snapshot.tools.clone());
    }
    anyhow::ensure!(!paths.is_empty(), "cannot bind an empty tool runtime");
    let tools = paths
        .into_iter()
        .map(|(name, path)| {
            BoundTool::capture(&path)
                .with_context(|| format!("bind selected tool {name} at {}", path.display()))
                .map(|binding| (name, binding))
        })
        .collect::<Result<_>>()?;
    BOUND_TOOL_RUNTIME
        .set(BoundToolRuntime {
            snapshot,
            tools,
            failure: std::sync::Mutex::new(None),
            used: std::sync::Mutex::new(required.iter().map(|name| (*name).to_owned()).collect()),
        })
        .map_err(|_| anyhow::anyhow!("IMG tool runtime was already bound"))?;
    verify_tool_runtime_after_failure()
}

/// The exact persisted authority to save in a checkpoint, never a fresh selection.
#[must_use]
pub fn active_tool_snapshot() -> Option<&'static crate::tool_snapshot::Snapshot> {
    BOUND_TOOL_RUNTIME
        .get()
        .and_then(|runtime| runtime.snapshot.as_ref())
}

#[must_use]
pub fn tool_runtime_is_bound() -> bool {
    BOUND_TOOL_RUNTIME.get().is_some()
}

fn latch_tool_runtime_failure(error: ToolRuntimeError) -> ToolRuntimeError {
    if let Some(runtime) = BOUND_TOOL_RUNTIME.get() {
        let mut failure = crate::media_conversion_gate::mutex_guard_or_recover(
            "tool_runtime_failure",
            runtime.failure.lock(),
        );
        if failure.is_none() {
            *failure = Some(error.clone());
        }
    }
    error
}

/// Cheap per-file boundary: no media work begins with a missing or changed tool.
///
/// # Errors
/// Returns a typed shared error if a bound executable changed or a failure was latched.
pub fn verify_tool_runtime() -> Result<()> {
    let Some(runtime) = BOUND_TOOL_RUNTIME.get() else {
        return Ok(());
    };
    let latched = crate::media_conversion_gate::mutex_guard_or_recover(
        "tool_runtime_failure",
        runtime.failure.lock(),
    )
    .clone();
    if let Some(error) = latched {
        return Err(error.into());
    }
    for (name, tool) in &runtime.tools {
        let used = crate::media_conversion_gate::mutex_guard_or_recover(
            "tool_runtime_used",
            runtime.used.lock(),
        )
        .contains(name);
        if !used
            && runtime
                .snapshot
                .as_ref()
                .is_none_or(|snapshot| !snapshot.tools.contains_key(name))
        {
            continue;
        }
        if !tool.unchanged() {
            return Err(latch_tool_runtime_failure(runtime_failure(
                "tool_changed",
                Some(name),
                format!(
                    "bound tool was removed or modified: {}",
                    tool.path.display()
                ),
            ))
            .into());
        }
    }
    Ok(())
}

/// Recheck shared prerequisites after a failed operation, before labelling media.
/// Full snapshot hashes and real bounded version commands detect broken dylibs.
///
/// # Errors
/// Returns a typed shared error for damaged snapshots or failed executable probes.
pub fn verify_tool_runtime_after_failure() -> Result<()> {
    verify_tool_runtime_after_failure_with_budget(
        std::time::Duration::from_secs(3),
        std::time::Duration::from_secs(crate::constants::FFPROBE_TIMEOUT_SECS),
    )
}

fn verify_tool_runtime_after_failure_with_budget(
    soft: std::time::Duration,
    hard: std::time::Duration,
) -> Result<()> {
    verify_tool_runtime_snapshot()?;
    let Some(runtime) = BOUND_TOOL_RUNTIME.get() else {
        return Ok(());
    };
    for (name, tool) in &runtime.tools {
        let used = crate::media_conversion_gate::mutex_guard_or_recover(
            "tool_runtime_used",
            runtime.used.lock(),
        )
        .contains(name);
        if !used
            && runtime
                .snapshot
                .as_ref()
                .is_none_or(|snapshot| !snapshot.tools.contains_key(name))
        {
            continue;
        }
        if let Err(error) = probe_tool_runtime_with_budget(&tool.path, name, soft, hard) {
            if error.chain().any(|cause| {
                cause
                    .downcast_ref::<std::io::Error>()
                    .is_some_and(|error| error.kind() == std::io::ErrorKind::Interrupted)
            }) {
                return Err(error);
            }
            // Recheck immutable evidence before classifying an inconclusive
            // probe; a real identity/hash failure remains a hard shared error.
            verify_tool_runtime_snapshot()?;
            if let Some(shared) = error.downcast_ref::<ToolRuntimeError>()
                && shared.cause_code == "tool_unavailable"
            {
                return Err(latch_tool_runtime_failure(shared.clone()).into());
            }
            return Err(error);
        }
    }
    Ok(())
}

/// Probe one selected executable with the existing external-probe budget.
/// A slow or incomplete observation never proves that the tool is damaged.
pub(crate) fn probe_tool_runtime(program: &Path, name: &str) -> Result<()> {
    probe_tool_runtime_with_budget(
        program,
        name,
        std::time::Duration::from_secs(3),
        std::time::Duration::from_secs(crate::constants::FFPROBE_TIMEOUT_SECS),
    )
}

fn probe_tool_runtime_with_budget(
    program: &Path,
    name: &str,
    soft: std::time::Duration,
    hard: std::time::Duration,
) -> Result<()> {
    let flag = match name {
        "exiftool" => "-ver",
        "jxlinfo" => "--help",
        "avifgainmaputil" => "help",
        "opj_decompress" => "-h",
        "pngcheck" => "-h",
        "sf" => "-version",
        "ffmpeg" | "ffprobe" | "magick" | "convert" | "identify" | "cwebp" | "dwebp"
        | "webpmux" | "gif2webp" => "-version",
        _ => "--version",
    };
    let mut command = Command::new(program);
    command.arg(flag);
    let result = crate::process_runner::ManagedProcess::spawn_captured_isolated(&mut command)
        .map_err(|error| (error, true))
        .and_then(|child| {
            child
                .wait_liveness_timeout_cancellable(
                    soft,
                    hard,
                    "bound tool runtime check",
                    crate::batch_control::is_cancelled,
                )
                .map_err(|error| (error, false))
        });
    match result {
        Ok(output)
            if output.status.success()
                // OpenJPEG's documented help path exits 1 even when healthy;
                // require its exact help identity, never accept an arbitrary failure.
                || (name == "opj_decompress" && output.status.code() == Some(1)
                    && output.stderr.trim().is_empty()
                    && output.stdout.lines().any(|line| line == "This is the opj_decompress utility from the OpenJPEG project.")) =>
            {}
        Ok(output) => {
            return Err(runtime_failure(
                "tool_unavailable",
                Some(name),
                format!(
                    "{} failed its runtime probe ({}): {}",
                    program.display(),
                    output.status,
                    crate::logging::combined_tool_output(&output.stdout, &output.stderr)
                ),
            )
            .into());
        }
        Err((error, failed_to_spawn)) => {
            if error.chain().any(|cause| {
                cause
                    .downcast_ref::<std::io::Error>()
                    .is_some_and(|error| error.kind() == std::io::ErrorKind::Interrupted)
            }) {
                return Err(error);
            }
            let confirmed_unavailable = failed_to_spawn
                && error.chain().any(|cause| {
                    cause.downcast_ref::<std::io::Error>().is_some_and(|error| {
                        matches!(
                            error.kind(),
                            std::io::ErrorKind::NotFound | std::io::ErrorKind::PermissionDenied
                        )
                    })
                });
            return Err(runtime_failure(if confirmed_unavailable { "tool_unavailable" } else { "tool_probe_unconfirmed" }, Some(name), format!("{} runtime probe did not complete reliably; media and pending work are retained for retry: {error:#}", program.display())).into());
        }
    }
    Ok(())
}

/// Strong private-material verification at encoding waves and delivery boundaries.
/// Metadata observations alone are not a content proof.
///
/// # Errors
/// Returns a typed shared error if any captured private member is changed or unreadable.
pub fn verify_tool_runtime_snapshot() -> Result<()> {
    verify_tool_runtime()?;
    if let Some(snapshot) = active_tool_snapshot() {
        snapshot.verify().map_err(|error| {
            latch_tool_runtime_failure(runtime_failure(
                "snapshot_invalid",
                None,
                format!("{error:#}"),
            ))
        })?;
    }
    Ok(())
}

/// Preserve a control event independently from media counts and diagnostics.
///
/// # Errors
/// Propagates stdout write and flush failures; these control events must not disappear.
pub fn emit_tool_runtime_status(state: &str, error: Option<&ToolRuntimeError>) -> Result<()> {
    use std::io::Write;
    let runtime = BOUND_TOOL_RUNTIME.get();
    let snapshot = runtime.and_then(|runtime| runtime.snapshot.as_ref());
    let names = snapshot
        .map(|snapshot| snapshot.tools.keys().collect::<Vec<_>>())
        .unwrap_or_default();
    let observed = runtime
        .map(|runtime| {
            runtime
                .tools
                .keys()
                .filter(|name| snapshot.is_none_or(|snapshot| !snapshot.tools.contains_key(*name)))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let (boundary, boundary_message) =
        tool_runtime_boundary_text(state, snapshot.is_some(), cfg!(target_os = "macos"));
    let event = crate::tool_snapshot::SnapshotEvent {
        schema_version: 1,
        pipeline: "img".to_owned(),
        state: state.to_owned(),
        boundary: boundary.to_owned(),
        identity: snapshot.map(|snapshot| snapshot.identity.clone()),
        manifest_path: snapshot.map(|snapshot| snapshot.manifest_path.clone()),
        tool_names: names.into_iter().cloned().collect(),
        observed_tool_names: observed.into_iter().cloned().collect(),
        cause_code: error.map(|error| error.cause_code.to_owned()),
        tool: error.and_then(|error| error.tool.clone()),
        message: error.map_or_else(
            || boundary_message.to_owned(),
            |error| error.message.chars().take(2048).collect::<String>(),
        ),
    };
    let event = serde_json::to_string(&event)?;
    let stdout = std::io::stdout();
    let mut output = stdout.lock();
    writeln!(output, "MFB_TOOL_SNAPSHOT={event}")?;
    output.flush()?;
    Ok(())
}

fn tool_runtime_boundary_text(
    state: &str,
    has_snapshot: bool,
    supports_snapshot: bool,
) -> (&'static str, &'static str) {
    if has_snapshot {
        (
            "persisted-external",
            "Listed tools use a verified private runtime; ancillary tools are observed at fixed executable paths, while system libraries and external user configuration remain outside the snapshot.",
        )
    } else if state == "preparing" && supports_snapshot {
        (
            "persisted-external",
            "Preparing a private IMG tool runtime; no tool snapshot has been verified or bound yet.",
        )
    } else if state == "preparing" {
        (
            "live-observed",
            "Preparing observed IMG executable selections; verification is not complete.",
        )
    } else if supports_snapshot {
        (
            "live-observed",
            "No verified private IMG tool snapshot is active; executable observations alone do not freeze dependencies.",
        )
    } else {
        (
            "live-observed",
            "Executable selections are observed and checked; this platform has no independent external dependency snapshot.",
        )
    }
}

pub(crate) fn media_tool_binding_applies(name: &str) -> bool {
    BOUND_TOOL_RUNTIME.get().is_some()
        && crate::runtime_config::IMG_MEDIA_TOOL_NAMES.contains(&name)
}

/// An absolute unavailable path prevents legacy builders from falling through
/// to a different PATH tool when a bound runtime is missing a requested tool.
// shortcut: legacy PathBuf builders cannot propagate Result; latch the typed
// shared error and use a non-executable path until their APIs migrate to Result.
#[must_use]
pub fn bound_tool_unavailable_path(name: &str) -> Option<PathBuf> {
    if !media_tool_binding_applies(name) {
        return None;
    }
    BOUND_TOOL_RUNTIME.get().and_then(|runtime| {
        latch_tool_runtime_failure(runtime_failure("tool_unavailable", Some(name), format!("tool {name} was not available in the selected IMG runtime; PATH reselection is prohibited")));
        // A component below an existing regular executable cannot execute.
        runtime.tools.values().next().map(|tool| tool.path.join("unavailable").join(name.replace(['/', '\\'], "_")))
    })
}

#[must_use]
pub fn resolve_tool_path(name: &str) -> Option<std::path::PathBuf> {
    if let Some(runtime) = BOUND_TOOL_RUNTIME
        .get()
        .filter(|_| media_tool_binding_applies(name))
    {
        if runtime.tools.contains_key(name) {
            crate::media_conversion_gate::mutex_guard_or_recover(
                "tool_runtime_used",
                runtime.used.lock(),
            )
            .insert(name.to_owned());
        }
        return runtime
            .tools
            .get(name)
            .filter(|tool| tool.unchanged())
            .map(|tool| tool.path.clone());
    }
    resolve_tool_path_with_health(name, multimedia_tool_is_healthy)
}

pub(crate) fn resolve_tool_path_with_health(
    name: &str,
    mut healthy: impl FnMut(&Path) -> bool,
) -> Option<PathBuf> {
    // GUI-launched macOS apps often miss shell PATH. An explicit developer
    // override and an explicitly selected PATH entry take priority over fixed
    // install locations, so nightly or locally built tools can be used safely.
    let configured_override = crate::runtime_config::active()
        .and_then(|config| config.tools.paths.get(name))
        .map(|path| path.as_os_str().to_os_string());
    let explicit_override = match configured_override {
        Some(path) => Some(path),
        None => std::env::var_os(tool_override_env_name(name)),
    };
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
            let is_healthy = !is_multimedia_tool || healthy(path);

            if is_healthy {
                return Some(path.to_path_buf());
            }
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
            let is_healthy = !is_multimedia_tool || healthy(&path);

            if is_healthy {
                return Some(path);
            }
        }
        Err(e) => {
            crate::media_conversion_gate::delivery_runtime_batch_audit(
                "tool_path_lookup",
                format!(
                    "tool lookup failed for {name}: stable paths and PATH lookup exhausted: {e}"
                ),
            );
        }
    }
    None
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

/// Run environment-mutating tests alone, without redirecting parallel readers
/// into a temporary state directory that disappears when this test finishes.
/// Returns true in the parent after the child has passed; the parent must return.
#[cfg(test)]
pub(crate) fn isolated_test_process() -> bool {
    const CHILD_TEST: &str = "MFB_ISOLATED_TEST";
    let thread = std::thread::current();
    let name = thread.name().expect("libtest supplies the test name");
    if std::env::var_os(CHILD_TEST).as_deref() == Some(std::ffi::OsStr::new(name)) {
        return false;
    }
    let output = std::process::Command::new(std::env::current_exe().expect("test executable"))
        .args([name, "--exact", "--nocapture", "--test-threads=1"])
        .env(CHILD_TEST, name)
        .output()
        .expect("spawn isolated environment test");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success() && stdout.contains("running 1 test"),
        "isolated test {name} failed: {stdout}\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    true
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

    #[cfg(unix)]
    #[test]
    fn tool_probe_slow_soft_success_and_unconfirmed_budget_do_not_poison_runtime() -> Result<()> {
        if isolated_test_process() {
            return Ok(());
        }
        use std::os::unix::fs::PermissionsExt;
        let directory = TempDir::new()?;
        let _state = EnvGuard::set("MFB_HOME_ROOT", &directory.path().to_string_lossy());
        let delay = directory.path().join("delay");
        let _delay = EnvGuard::set("MFB_PROBE_DELAY_FLAG", &delay.to_string_lossy());
        let program = directory.path().join("probe");
        fs::write(
            &program,
            "#!/bin/sh\nif [ -f \"$MFB_PROBE_DELAY_FLAG\" ]; then sleep 0.15; fi\nexit 0\n",
        )?;
        fs::set_permissions(&program, fs::Permissions::from_mode(0o700))?;
        bind_tool_runtime(
            None,
            std::collections::BTreeMap::from([("cjxl".to_owned(), program.clone())]),
            &["cjxl"],
        )?;
        fs::write(&delay, b"wait")?;
        let soft = std::time::Duration::from_millis(5);
        probe_tool_runtime_with_budget(&program, "cjxl", soft, std::time::Duration::from_secs(1))?;
        let error = verify_tool_runtime_after_failure_with_budget(
            soft,
            std::time::Duration::from_millis(30),
        )
        .expect_err("the observation budget must be inconclusive, not success");
        assert_eq!(
            error
                .downcast_ref::<ToolRuntimeError>()
                .map(|error| error.cause_code),
            Some("tool_probe_unconfirmed")
        );
        verify_tool_runtime()?;
        fs::remove_file(&delay)?;
        verify_tool_runtime_after_failure_with_budget(soft, std::time::Duration::from_secs(1))?;
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn killed_bound_tool_probe_latches_one_shared_failure_without_reselection() -> Result<()> {
        if isolated_test_process() {
            return Ok(());
        }
        use std::os::unix::{fs::PermissionsExt, process::ExitStatusExt};
        let directory = TempDir::new()?;
        let _state = EnvGuard::set("MFB_HOME_ROOT", &directory.path().to_string_lossy());
        let flag = directory.path().join("kill");
        let invocations = directory.path().join("invocations");
        let _flag = EnvGuard::set("MFB_TOOL_KILL_FLAG", &flag.to_string_lossy());
        let _invocations = EnvGuard::set("MFB_TOOL_PROBE_LOG", &invocations.to_string_lossy());
        let program = directory.path().join("cjxl");
        let script = b"#!/bin/sh\nprintf '%s\\n' \"$1\" >> \"$MFB_TOOL_PROBE_LOG\"\nif [ -f \"$MFB_TOOL_KILL_FLAG\" ]; then kill -KILL \"$$\"; fi\nexit 0\n";
        fs::write(&program, script)?;
        fs::set_permissions(&program, fs::Permissions::from_mode(0o700))?;
        bind_tool_runtime(
            None,
            std::collections::BTreeMap::from([("cjxl".to_owned(), program.clone())]),
            &["cjxl"],
        )?;
        assert_eq!(fs::read_to_string(&invocations)?, "--version\n");
        fs::write(&flag, b"kill candidate and probe")?;
        let candidate = Command::new(&program).arg("candidate.jpg").output()?;
        assert_eq!(candidate.status.signal(), Some(libc::SIGKILL));
        verify_tool_runtime()?;
        let error = verify_tool_runtime_after_failure()
            .expect_err("a killed version probe must block the shared runtime");
        let shared = error.downcast_ref::<ToolRuntimeError>().unwrap();
        assert_eq!(shared.cause_code, "tool_unavailable");
        assert_eq!(shared.tool.as_deref(), Some("cjxl"));
        assert!(shared.message.contains("signal: 9"), "{shared}");
        let expected_invocations = "--version\ncandidate.jpg\n--version\n";
        assert_eq!(fs::read_to_string(&invocations)?, expected_invocations);
        for recover_external_flag in [false, true] {
            if recover_external_flag {
                fs::remove_file(&flag)?;
            }
            let error = verify_tool_runtime_after_failure()
                .expect_err("a shared failure must remain latched until a new run");
            let repeated = error.downcast_ref::<ToolRuntimeError>().unwrap();
            assert_eq!(repeated.cause_code, shared.cause_code);
            assert_eq!(repeated.message, shared.message);
            assert_eq!(fs::read_to_string(&invocations)?, expected_invocations);
        }
        assert_eq!(fs::read(&program)?, script);
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn bound_runtime_never_reselects_path_and_latches_shared_failure() -> Result<()> {
        const CHILD: &str = "MFB_BOUND_RUNTIME_TEST_CHILD";
        if std::env::var_os(CHILD).is_none() {
            let isolated = TempDir::new()?;
            let output = Command::new(std::env::current_exe()?)
                .args(["--exact", "infra::common_utils::tests::bound_runtime_never_reselects_path_and_latches_shared_failure", "--nocapture"])
                .env(CHILD, "1").env("MFB_HOME_ROOT", isolated.path()).output()?;
            anyhow::ensure!(
                output.status.success(),
                "isolated runtime test failed: {} {}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            return Ok(());
        }
        use std::os::unix::fs::PermissionsExt;
        let directory = TempDir::new()?;
        let executable = directory.path().join("selected-tool");
        fs::write(&executable, "#!/bin/sh\nexit 0\n")?;
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700))?;
        let executable = executable.canonicalize()?;
        bind_tool_runtime(
            None,
            std::collections::BTreeMap::from([("cjxl".into(), executable.clone())]),
            &["cjxl"],
        )?;
        assert_eq!(resolve_tool_path("cjxl"), Some(executable.clone()));
        for helper in [
            "sysctl",
            "vm_stat",
            "ps",
            "kill",
            "hostname",
            "osascript",
            "getfacl",
            "setfacl",
            "rsync",
            "taskkill",
        ] {
            assert!(!media_tool_binding_applies(helper));
            assert!(bound_tool_unavailable_path(helper).is_none());
        }
        #[cfg(target_os = "macos")]
        {
            let output = crate::ToolBuilder::build(&crate::tool_builders::SysctlBuilder::new())
                .args(["-n", "hw.ncpu"])
                .output()?;
            anyhow::ensure!(
                output.status.success(),
                "bound media runtime blocked the actual system builder"
            );
            anyhow::ensure!(
                std::str::from_utf8(&output.stdout)?
                    .trim()
                    .parse::<usize>()?
                    > 0,
                "sysctl did not report actual CPU capacity"
            );
        }
        assert!(!crate::DjxlBuilder::check_available());
        assert!(!crate::tools::check_tool("djxl"));
        assert!(!crate::tools::check_tool_alt("djxl"));
        verify_tool_runtime()?;
        assert_eq!(resolve_tool_path("djxl"), None);
        let unavailable = crate::media_conversion_gate::delivery_tool_path_or_bare_name("djxl");
        assert!(unavailable.is_absolute());
        assert_ne!(unavailable, PathBuf::from("djxl"));
        let magick = crate::ToolBuilder::build(&crate::MagickBuilder::new());
        assert!(Path::new(magick.get_program()).is_absolute());
        assert_ne!(magick.get_program(), std::ffi::OsStr::new("magick"));
        let error =
            verify_tool_runtime().expect_err("unbound required tool must latch a shared failure");
        assert_eq!(
            error
                .downcast_ref::<ToolRuntimeError>()
                .map(|error| error.cause_code),
            Some("tool_unavailable")
        );
        fs::remove_file(executable)?;
        assert_eq!(resolve_tool_path("cjxl"), None);
        assert!(verify_tool_runtime_after_failure().is_err());
        Ok(())
    }

    #[test]
    fn observed_tool_replacement_is_detected_before_media_admission() -> Result<()> {
        let directory = TempDir::new()?;
        let executable = directory.path().join("tool");
        fs::write(&executable, "original")?;
        let binding = BoundTool::capture(&executable)?;
        assert!(binding.unchanged());
        #[cfg(unix)]
        {
            fs::write(&executable, "changed!")?;
            fs::File::options()
                .write(true)
                .open(&executable)?
                .set_times(std::fs::FileTimes::new().set_modified(binding.identity.modified))?;
            assert_eq!(fs::metadata(&executable)?.len(), binding.identity.len);
            assert_eq!(
                fs::metadata(&executable)?.modified()?,
                binding.identity.modified
            );
            assert!(
                !binding.unchanged(),
                "restoring length and mtime must not conceal a changed native identity"
            );
        }
        fs::write(&executable, "replaced by an upgrade")?;
        assert!(!binding.unchanged());
        fs::remove_file(&executable)?;
        assert!(!binding.unchanged());
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn observed_tool_keeps_invocation_alias_and_detects_retargeting() -> Result<()> {
        if isolated_test_process() {
            return Ok(());
        }
        use std::os::unix::fs::{PermissionsExt, symlink};
        let directory = TempDir::new()?;
        let executable = directory.path().join("canonical-tool");
        let alias = directory.path().join("identify");
        let convert = directory.path().join("convert");
        fs::write(&executable, "#!/bin/sh\nprintf '%s' \"$0\"\n")?;
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700))?;
        symlink(&executable, &alias)?;
        symlink(&executable, &convert)?;
        let _identify = EnvGuard::set("MFB_TOOL_IDENTIFY", &alias.to_string_lossy());
        let _convert = EnvGuard::set("MFB_TOOL_CONVERT", &convert.to_string_lossy());
        let selected = ["identify", "convert"]
            .into_iter()
            .map(|name| {
                resolve_tool_path(name)
                    .map(|path| (name.to_owned(), path))
                    .ok_or_else(|| anyhow::anyhow!("selected alias {name}"))
            })
            .collect::<Result<std::collections::BTreeMap<_, _>>>()?;
        assert_eq!(selected["identify"], alias);
        assert_eq!(selected["convert"], convert);
        bind_tool_runtime(None, selected, &[])?;
        let outputs = [
            crate::ToolBuilder::build(&crate::IdentifyBuilder::new()).output()?,
            crate::ToolBuilder::build(&crate::MagickBuilder::new()).output()?,
        ];
        for (output, expected) in outputs.iter().zip([&alias, &convert]) {
            assert!(output.status.success());
            assert_eq!(
                std::str::from_utf8(&output.stdout)?,
                expected
                    .to_str()
                    .ok_or_else(|| anyhow::anyhow!("fixture alias"))?
            );
        }
        verify_tool_runtime()?;
        let replacement = directory.path().join("replacement");
        fs::copy(&executable, &replacement)?;
        fs::remove_file(&alias)?;
        symlink(&replacement, &alias)?;
        assert!(verify_tool_runtime().is_err());
        Ok(())
    }

    #[test]
    fn preparing_tool_runtime_text_does_not_claim_missing_platform_support_or_completion() {
        let (boundary, message) = tool_runtime_boundary_text("preparing", false, true);
        assert_eq!(boundary, "persisted-external");
        assert!(message.contains("no tool snapshot has been verified or bound yet"));
        assert!(!message.contains("this platform has no"));
        let (boundary, message) = tool_runtime_boundary_text("preparing", false, false);
        assert_eq!(boundary, "live-observed");
        assert!(message.contains("verification is not complete"));
    }

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
    fn resolve_tool_path_rechecks_current_selection_without_stale_fallback() -> anyhow::Result<()> {
        if isolated_test_process() {
            return Ok(());
        }
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
        {
            let _override = EnvGuard::set(
                &tool_override_env_name(&tool_name),
                &cached.to_string_lossy(),
            );
            assert_eq!(resolve_tool_path(&tool_name), Some(cached.clone()));
        }

        assert_eq!(resolve_tool_path(&tool_name), Some(primary.clone()));
        fs::remove_file(primary)?;
        assert!(cached.is_file());
        assert_eq!(resolve_tool_path(&tool_name), None);
        Ok(())
    }

    #[test]
    fn tool_override_env_name_is_stable_and_shell_safe() {
        assert_eq!(tool_override_env_name("avifenc"), "MFB_TOOL_AVIFENC");
        assert_eq!(tool_override_env_name("avif-enc"), "MFB_TOOL_AVIF_ENC");
    }

    #[cfg(unix)]
    #[test]
    fn failed_multimedia_tool_retries_replacement_expiry_and_missing_identity() -> anyhow::Result<()>
    {
        use std::os::unix::fs::PermissionsExt;

        if isolated_test_process() {
            return Ok(());
        }
        let tmp = TempDir::new()?;
        let tool = tmp.path().join("ffprobe");
        let counter = tmp.path().join("counter");
        let original = "#!/bin/sh\nprintf x >> \"$MFB_TEST_TOOL_COUNTER\"\n[ -n \"$MFB_TEST_TOOL_REPAIR\" ] && exit 0\necho 'dyld: Library not loaded' >&2\nexit 1\n";
        fs::write(&tool, original)?;
        let mut permissions = fs::metadata(&tool)?.permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&tool, permissions.clone())?;
        let _counter = EnvGuard::set("MFB_TEST_TOOL_COUNTER", &counter.to_string_lossy());
        let _repair_state = EnvGuard::set("MFB_TEST_TOOL_REPAIR", "");

        // The direct seam makes retry counts independent of fixed-path candidates.
        assert!(!multimedia_tool_is_healthy(&tool));
        assert!(!multimedia_tool_is_healthy(&tool));
        assert_eq!(fs::read_to_string(&counter)?, "x");

        let metadata = fs::metadata(&tool)?;
        let identity = tool_identity(&tool)?;
        let replacement = original
            .replace("dyld: Library not loaded", "pass: Library was loaded")
            .replace("exit 1", "exit 0");
        assert_eq!(original.len(), replacement.len());
        fs::write(&tool, &replacement)?;
        fs::set_permissions(&tool, permissions)?;
        filetime::set_file_mtime(
            &tool,
            filetime::FileTime::from_last_modification_time(&metadata),
        )?;
        assert_eq!(metadata.len(), fs::metadata(&tool)?.len());
        assert_eq!(metadata.modified()?, fs::metadata(&tool)?.modified()?);
        assert_ne!(identity, tool_identity(&tool)?);
        assert!(multimedia_tool_is_healthy(&tool));
        assert_eq!(fs::read_to_string(&counter)?, "xx");

        fs::write(&tool, original)?;
        assert!(!multimedia_tool_is_healthy(&tool));
        let failures = FAILED_TOOL_IDENTITIES.get().expect("cache initialized");
        {
            let mut entries = failures.lock().expect("cache lock");
            let failed = entries.get_mut(&tool).expect("failed tool observation");
            let now = std::time::Instant::now();
            assert!(failed.suppresses(&failed.identity, now));
            failed.checked_at = now
                .checked_sub(TOOL_HEALTH_FAILURE_TTL)
                .expect("fixture retry deadline is representable");
            assert!(!failed.suppresses(&failed.identity, now));
            drop(entries);
        }
        {
            let _repair = EnvGuard::set("MFB_TEST_TOOL_REPAIR", "1");
            assert!(multimedia_tool_is_healthy(&tool));
        }
        assert_eq!(fs::read_to_string(counter)?, "xxxx");
        assert!(!failures.lock().expect("cache lock").contains_key(&tool));
        assert!(!multimedia_tool_is_healthy(&tool));
        fs::remove_file(&tool)?;
        assert!(!multimedia_tool_is_healthy(&tool));
        assert!(!failures.lock().expect("cache lock").contains_key(&tool));
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn tool_identity_tracks_symlink_targets_and_rejects_unavailable_inputs() -> anyhow::Result<()> {
        use std::os::unix::fs::symlink;
        let tmp = TempDir::new()?;
        let first = tmp.path().join("first");
        let second = tmp.path().join("second");
        let link = tmp.path().join("tool");
        fs::write(&first, b"first")?;
        fs::write(&second, b"other")?;
        symlink(&first, &link)?;
        let original = tool_identity(&link)?;
        assert_eq!(original, tool_identity(&first)?);
        fs::remove_file(&link)?;
        symlink(&second, &link)?;
        assert_ne!(original, tool_identity(&link)?);
        fs::remove_file(second)?;
        assert!(tool_identity(&link).is_err());
        assert!(tool_identity(tmp.path()).is_err());
        Ok(())
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
            "Should be None because the extended-size header is truncated"
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
    fn isobmff_search_does_not_resync_or_descend_into_media_payloads() {
        let mut malformed = vec![0, 0, 0, 4];
        malformed.extend_from_slice(b"junk");
        malformed.extend_from_slice(&12u32.to_be_bytes());
        malformed.extend_from_slice(b"testdata");
        assert!(find_box_data_recursive(&malformed, *b"test").is_none());

        let mut embedded = Vec::new();
        embedded.extend_from_slice(&24u32.to_be_bytes());
        embedded.extend_from_slice(b"mdat");
        embedded.extend_from_slice(&16u32.to_be_bytes());
        embedded.extend_from_slice(b"jbrddata");
        assert!(!find_any_box_recursive(&embedded, *b"jbrd"));

        let malformed_ftyp = b"\0\0\0\x11ftypavif\0\0\0\0x";
        assert!(isobmff_ftyp_payload(malformed_ftyp).is_none());
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
