//! Metadata Preservation Module
//!
//! Layered preservation: Internal (`ExifTool`) / XMP sidecar / macOS Spotlight
//! xattrs / supplemental xattrs / platform ACL+attributes / timestamps (always
//! last). Unified entry point for timestamps: single files via
//! `apply_file_timestamps(src, dst)`, directory trees via
//! `save_directory_timestamps` → `apply_saved_timestamps_to_dst` /
//! `restore_directory_timestamps`, Avoids redundant implementations. `ExifTool`
//! rewrites files, so timestamps are always set after write operations.
//!
//! **Delivery:** [`preserve_for_delivery`] and
//! [`apply_file_timestamps_for_delivery`] implement M23 best-effort semantics —
//! missing source metadata must not block conversion (see
//! `delivery_policy.rs`).

use crate::builder_base::ToolBuilder;
use std::io;
use std::path::{Path, PathBuf};

mod delivery_policy;
mod exif;
#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
mod network;
mod output_audit;
#[cfg(target_os = "windows")]
mod windows;

pub use delivery_policy::{
    MetadataDeliveryReport, MetadataLayerOutcome, apply_file_timestamps_for_delivery,
    preserve_for_delivery,
};
pub(crate) use delivery_policy::preserve_filesystem_for_delivery;
pub use exif::preserve_internal;
pub use output_audit::{
    MetadataOutputPolicy, OutputMetadataAudit, verify_output_embedded_metadata,
};

/// Measure the file emitted by `ExifTool` after removing all embedded metadata.
///
/// # Errors
/// Returns an error when `ExifTool` cannot produce a non-empty stripped image.
pub fn stripped_embedded_metadata_size(path: &Path) -> io::Result<u64> {
    output_audit::stripped_embedded_metadata_size(path)
}
#[cfg(target_os = "macos")]
pub use macos::append_mfb_branding;

pub(crate) fn rehydrate_jxl_internal_metadata_without_orientation(
    src: &Path,
    dst: &Path,
) -> io::Result<()> {
    exif::rehydrate_jxl_internal_metadata_without_orientation(src, dst)
}

/// Outcome of Apple AAE sidecar handling.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AaeSidecarAction {
    /// No matching AAE sidecar exists for the source asset.
    Missing,
    /// Sidecar already matches the converted output stem and location.
    AlreadyAdjacent {
        source: PathBuf,
        destination: PathBuf,
    },
    /// Sidecar bytes and filesystem metadata were copied beside the output.
    Copied {
        source: PathBuf,
        destination: PathBuf,
    },
    /// Source sidecar was removed because Apple compatibility was not
    /// requested.
    Deleted { source: PathBuf },
}

/// CONTRACT: xattrs never copied (security / APFS-specific / iCloud import
/// safety).
pub(crate) const XATTR_PRESERVE_SKIP_KEYS: &[&str] = &["com.apple.quarantine", "com.apple.decmpfs"];

/// CONTRACT: minimum macOS keys verified after Spotlight-style copy (M23).
#[cfg(target_os = "macos")]
pub(crate) const NETWORK_XATTR_PRIORITY_KEYS: &[&str] = &[
    "com.apple.metadata:kMDItemWhereFroms",
    "com.apple.metadata:kMDItemUserTags",
];

/// CONTRACT: non-Spotlight macOS xattrs preserved explicitly.
#[cfg(target_os = "macos")]
pub(crate) const XATTR_MACOS_EXPLICIT_KEYS: &[&str] =
    &["com.apple.FinderInfo", "com.apple.provenance"];

/// CONTRACT: macOS xattr prefixes preserved as part of asset history.
/// Includes full Spotlight metadata namespace and per-app last-used timestamps.
/// `com.apple.lastuseddate#App` records which app last accessed the asset —
/// preserved for the same reason as EXIF `DateTimeOriginal` and
/// `kMDItemWhereFroms`: the JXL is a format-converted version of the same
/// asset, so its history is inherited. macOS overwrites with a fresh timestamp
/// on next real app access.
#[cfg(target_os = "macos")]
pub(crate) const XATTR_MACOS_METADATA_PREFIXES: &[&str] =
    &["com.apple.metadata:", "com.apple.lastuseddate"];

/// CONTRACT: skip destructive/special keys when bulk-copying xattrs.
#[must_use]
pub(crate) fn is_xattr_preserve_skipped(key: &str) -> bool {
    XATTR_PRESERVE_SKIP_KEYS.contains(&key)
}

#[cfg(target_os = "macos")]
const XATTR_EXACT_COPY_SOURCE_SKIP_KEYS: &[&str] = &[
    "com.apple.cscachefs",
    "com.apple.metadata:kMDItemContentCreationDate",
    "com.apple.provenance",
    // MAC label injected by sandboxed apps — non-transferable, skip on both sides.
    "com.apple.macl",
];

#[cfg(target_os = "macos")]
const XATTR_EXACT_COPY_DESTINATION_GENERATED_KEYS: &[&str] = &[
    // Spotlight re-indexes new files and may stamp a fresh creation date.
    "com.apple.metadata:kMDItemContentCreationDate",
    // macOS provenance tracking; written to new files by the OS.
    "com.apple.provenance",
    // Mandatory-Access-Control label injected by the macOS sandbox when any
    // sandboxed process (e.g. Photos, Preview) opens the destination file.
    // Never present on the source at copy time, so it must not be flagged as
    // an "unexpected" xattr during exact-copy verification.
    "com.apple.macl",
];

#[cfg(target_os = "macos")]
const fn str_eq_const(left: &str, right: &str) -> bool {
    let left = left.as_bytes();
    let right = right.as_bytes();
    if left.len() != right.len() {
        return false;
    }
    let mut index = 0;
    while index < left.len() {
        if left[index] != right[index] {
            return false;
        }
        index += 1;
    }
    true
}

/// CONTRACT: all platforms — copy every xattr except the skip list.
#[must_use]
pub(crate) fn should_preserve_xattr(key: &str) -> bool {
    !is_xattr_preserve_skipped(key)
}

#[must_use]
fn should_verify_exact_copy_xattr(key: &str) -> bool {
    if is_xattr_preserve_skipped(key) {
        return false;
    }
    #[cfg(target_os = "macos")]
    if XATTR_EXACT_COPY_SOURCE_SKIP_KEYS.contains(&key) {
        return false;
    }
    true
}

#[cfg(target_os = "macos")]
fn copy_macos_exact_copy_xattrs(src: &Path, dst: &Path) -> io::Result<()> {
    copy_xattrs_with_policy(src, dst, should_verify_exact_copy_xattr)
}

#[must_use]
const fn is_destination_generated_exact_copy_xattr(key: &str) -> bool {
    #[cfg(target_os = "macos")]
    {
        let mut index = 0;
        while index < XATTR_EXACT_COPY_DESTINATION_GENERATED_KEYS.len() {
            if str_eq_const(key, XATTR_EXACT_COPY_DESTINATION_GENERATED_KEYS[index]) {
                return true;
            }
            index += 1;
        }
        false
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = key;
        false
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetadataCopyCheck {
    pub passed: bool,
    pub mismatches: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct MetadataCopySignature {
    readonly: bool,
    modified: filetime::FileTime,
    #[cfg(unix)]
    mode: u32,
    xattrs: Vec<(String, String)>,
}

/// CONTRACT: macOS Finder / iCloud / download metadata xattrs (prefix +
/// explicit keys).
#[must_use]
#[cfg(target_os = "macos")]
pub(crate) fn should_copy_macos_extended_xattr(key: &str) -> bool {
    if is_xattr_preserve_skipped(key) {
        return false;
    }
    XATTR_MACOS_EXPLICIT_KEYS.contains(&key)
        || XATTR_MACOS_METADATA_PREFIXES
            .iter()
            .any(|prefix| key.starts_with(prefix))
}

#[cfg(target_os = "macos")]
const TOKEN_DEBUG: &str = "{:?}";

fn require_existing_directory(path: &Path, label: &str) -> io::Result<std::fs::Metadata> {
    let metadata = std::fs::metadata(path).map_err(|e| {
        crate::media_conversion_gate::delivery_metadata_path_audit(
            "delivery_metadata",
            path,
            format!("Metadata Audit: Failed to read {label} directory metadata: {e}"),
        );
        io::Error::new(
            e.kind(),
            format!(
                "Failed to read {label} directory metadata {}: {e}",
                path.display()
            ),
        )
    })?;
    if !metadata.is_dir() {
        crate::media_conversion_gate::delivery_metadata_path_audit(
            "delivery_metadata",
            path,
            format!(
                "Metadata Audit: {label} path is not a directory: {}",
                path.display()
            ),
        );
        return Err(io::Error::other(format!(
            "{label} path is not a directory: {}",
            path.display()
        )));
    }
    Ok(metadata)
}

#[cfg(target_os = "macos")]
fn system_time_matches_with_slack(
    expected: std::time::SystemTime,
    actual: std::time::SystemTime,
    slack: std::time::Duration,
) -> bool {
    match (expected.checked_add(slack), expected.checked_sub(slack)) {
        (Some(upper), Some(lower)) => actual >= lower && actual <= upper,
        _ => false,
    }
}

#[cfg(target_os = "macos")]
fn system_time_diff_label(
    expected: std::time::SystemTime,
    actual: std::time::SystemTime,
) -> String {
    if actual > expected {
        match actual.duration_since(expected) {
            Ok(d) => format!("{d:?}"),
            Err(e) => format!("Err: Clock drifted backwards ({e})"),
        }
    } else {
        match expected.duration_since(actual) {
            Ok(d) => format!("-{d:?}"),
            Err(e) => format!("Err: Clock drifted backwards ({e})"),
        }
    }
}

// Rationale: This function handles complex, sequential initialization or
// business logic where further fragmentation would hinder readability and
// maintainability.
/// # Errors
/// Returns an error when the source metadata cannot be read or the destination
/// timestamps cannot be fully restored after file mutations.
pub fn apply_file_timestamps(src: &Path, dst: &Path) -> io::Result<()> {
    log_detail!(format!(
        "Metadata Audit: Initiating timestamp synchronization flow from {src_path} -> {dst_path}",
        src_path = src.display(),
        dst_path = dst.display(),
    ));
    let m = std::fs::metadata(src).map_err(|e| {
        crate::log_info!(
            crate::infra::static_logs::messages::LABEL_METADATA,
            crate::infra::static_logs::messages::MSG_METADATA_SRC_FAIL
        );
        io::Error::new(
            e.kind(),
            format!(
                "Failed to read source metadata before timestamp preservation from {}: {e}",
                src.display()
            ),
        )
    })?;
    let mut failures = Vec::new();

    #[cfg(target_os = "macos")]
    let source_added_time = match macos::get_added_time(src) {
        Ok(added) => Some(added),
        // macOS reports a sentinel when Date Added was never set (e.g. volumes that
        // don't track it). That is absence, not failure: skip the field, never
        // fabricate a value, and do not block timestamp preservation.
        Err(e) if e.to_string().contains("macOS unset sentinel") => {
            crate::log_info!(
                crate::infra::static_logs::messages::LABEL_METADATA,
                &format!(
                    "[SKIP] Finder added time: not present on source {}",
                    src.display()
                )
            );
            None
        }
        Err(e) => {
            crate::log_info!(
                crate::infra::static_logs::messages::LABEL_METADATA,
                &crate::infra::static_logs::messages::MSG_METADATA_SET_ADDED_FAIL
                    .replace("{}", &e.to_string())
            );
            failures.push(format!(
                "failed to read Finder added time from {} before timestamp preservation: {e}",
                src.display()
            ));
            None
        }
    };

    // Platform-specific creation time preservation FIRST (before atime/mtime)
    // This is critical because filetime::set_file_times may reset creation time on
    // some systems
    #[cfg(target_os = "macos")]
    {
        match m.created() {
            Ok(created) => {
                crate::log_info!(
                    crate::infra::static_logs::messages::LABEL_METADATA,
                    &crate::infra::static_logs::messages::MSG_METADATA_CREATION_TIME
                        .replace(TOKEN_DEBUG, &format!("{created:?}")),
                );
                if let Err(e) = macos::set_creation_time(dst, created) {
                    crate::media_conversion_gate::delivery_metadata_batch_audit(
                        "delivery_metadata_timestamp",
                        crate::infra::static_logs::messages::MSG_METADATA_SET_CREATION_FAIL
                            .replace("{}", &e.to_string()),
                    );
                    failures.push(format!(
                        "failed to set creation time on {}: {e}",
                        dst.display()
                    ));
                } else {
                    log_detail!(
                        crate::infra::static_logs::messages::MSG_METADATA_SET_CREATION_SUCCESS
                            .replace("{}", crate::infra::static_logs::messages::LABEL_METADATA)
                    );
                }
            }
            Err(e) => {
                crate::media_conversion_gate::delivery_metadata_batch_audit(
                    "delivery_metadata_timestamp",
                    crate::infra::static_logs::messages::MSG_METADATA_READ_CREATION_FAIL
                        .replace("{}", &e.to_string()),
                );
            }
        }
        if let Some(added) = source_added_time {
            if let Err(e) = macos::set_added_time(dst, added) {
                crate::log_info!(
                    crate::infra::static_logs::messages::LABEL_METADATA,
                    &crate::infra::static_logs::messages::MSG_METADATA_SET_ADDED_FAIL
                        .replace("{}", &e.to_string())
                );
                failures.push(format!(
                    "failed to set Finder added time on {} from {} before filetime write: {e}",
                    dst.display(),
                    src.display()
                ));
            } else {
                crate::log_info!(
                    crate::infra::static_logs::messages::LABEL_METADATA,
                    crate::infra::static_logs::messages::MSG_METADATA_SET_ADDED_SUCCESS
                );
            }
        }
    }

    #[cfg(target_os = "windows")]
    {
        // Windows: Use filetime crate's set_file_times which preserves creation time
        match m.created() {
            Ok(created) => {
                let ctime = filetime::FileTime::from_system_time(created);
                let atime = filetime::FileTime::from_last_access_time(&m);
                // On Windows, filetime::set_file_times also sets creation time
                if let Err(e) = filetime::set_file_times(dst, atime, ctime) {
                    crate::media_conversion_gate::delivery_metadata_batch_audit(
                        "delivery_metadata_platform",
                        &crate::infra::static_logs::messages::MSG_METADATA_WINDOWS_CREATION_FAIL
                            .replace("{}", &e.to_string()),
                    );
                    failures.push(format!(
                        "failed to set Windows creation/access time on {}: {e}",
                        dst.display()
                    ));
                }
            }
            Err(e) => {
                crate::media_conversion_gate::delivery_metadata_batch_audit(
                    "delivery_metadata_platform",
                    crate::infra::static_logs::messages::MSG_METADATA_READ_CREATION_FAIL
                        .replace("{}", &e.to_string()),
                );
            }
        }
    }

    #[cfg(target_os = "linux")]
    {
        // Linux: Try to preserve birth time if available (requires statx on newer
        // kernels) Note: Most Linux filesystems don't support setting birth
        // time, so this is best-effort
        match m.created() {
            Ok(created) => {
                linux::try_set_birth_time(dst, created);
            }
            Err(e) => {
                crate::media_conversion_gate::delivery_metadata_batch_audit(
                    "delivery_metadata_timestamp",
                    crate::infra::static_logs::messages::MSG_METADATA_READ_CREATION_FAIL
                        .replace("{}", &e.to_string()),
                );
            }
        }
    }

    // Set atime/mtime AFTER creation time
    let atime = filetime::FileTime::from_last_access_time(&m);
    let mtime = filetime::FileTime::from_last_modification_time(&m);
    if let Err(e) = filetime::set_file_times(dst, atime, mtime) {
        crate::media_conversion_gate::delivery_metadata_batch_audit(
            "delivery_metadata",
            crate::infra::static_logs::messages::MSG_METADATA_SET_FILE_TIMES_FAIL
                .replace("{}", &e.to_string()),
        );
        failures.push(format!(
            "failed to set access/modify time on {}: {e}",
            dst.display()
        ));
    } else {
        log_detail!(
            crate::infra::static_logs::messages::MSG_METADATA_SET_TIMES_SUCCESS
                .replace("{}", crate::infra::static_logs::messages::LABEL_METADATA)
        );
    }

    // RE-APPLY creation time on macOS after setting atime/mtime
    // This is necessary because filetime::set_file_times may reset creation time
    #[cfg(target_os = "macos")]
    {
        match m.created() {
            Ok(created) => {
                crate::log_info!(
                    crate::infra::static_logs::messages::LABEL_METADATA,
                    &crate::infra::static_logs::messages::MSG_METADATA_REAPPLY_CREATION
                        .replace(TOKEN_DEBUG, &format!("{created:?}")),
                );
                if let Err(e) = macos::set_creation_time(dst, created) {
                    crate::media_conversion_gate::delivery_metadata_batch_audit(
                        "delivery_metadata_timestamp",
                        crate::infra::static_logs::messages::MSG_METADATA_REAPPLY_CREATION_FAIL
                            .replace("{}", &e.to_string()),
                    );
                    failures.push(format!(
                        "failed to reapply creation time on {}: {e}",
                        dst.display()
                    ));
                }
            }
            Err(e) => {
                crate::media_conversion_gate::delivery_metadata_batch_audit(
                    "delivery_metadata_timestamp",
                    crate::infra::static_logs::messages::MSG_METADATA_READ_CREATION_FAIL
                        .replace("{}", &e.to_string()),
                );
            }
        }
        if let Some(added) = source_added_time
            && let Err(e) = macos::set_added_time(dst, added)
        {
            crate::media_conversion_gate::delivery_metadata_batch_audit(
                "delivery_metadata_timestamp",
                crate::infra::static_logs::messages::MSG_METADATA_SET_ADDED_FAIL
                    .replace("{}", &e.to_string()),
            );
            failures.push(format!(
                "failed to reapply Finder added time on {} from {} after filetime write: {e}",
                dst.display(),
                src.display()
            ));
        }
    }

    // Verify creation time was preserved (macOS only)
    #[cfg(target_os = "macos")]
    {
        match (m.created(), std::fs::metadata(dst)) {
            (Ok(expected_created), Ok(dst_meta)) => match dst_meta.created() {
                Ok(actual_created) => {
                    log_detail!(format!(
                        "Metadata Audit: Verifying creation time integrity ({label}) -> \
                         expected={expected_created:?} actual={actual_created:?}",
                        label = crate::infra::static_logs::messages::LABEL_METADATA,
                    ));
                    let creation_time_slack = std::time::Duration::from_secs(1);
                    let match_passed = system_time_matches_with_slack(
                        expected_created,
                        actual_created,
                        creation_time_slack,
                    );

                    if !match_passed {
                        let diff_str = system_time_diff_label(expected_created, actual_created);

                        crate::media_conversion_gate::delivery_metadata_batch_audit(
                            "delivery_metadata_timestamp",
                            format!(
                                "Metadata Audit: Creation time mismatch detected \
                                 (expected={expected_created:?}, actual={actual_created:?}, \
                                 diff={diff_str})"
                            ),
                        );
                        failures.push(format!(
                            "creation time mismatch on {}: expected={expected_created:?} \
                             actual={actual_created:?}",
                            dst.display()
                        ));
                    }
                }
                Err(e) => {
                    crate::media_conversion_gate::delivery_metadata_batch_audit(
                        "delivery_metadata_timestamp",
                        crate::infra::static_logs::messages::MSG_METADATA_READ_CREATION_FAIL
                            .replace("{}", &e.to_string()),
                    );
                    failures.push(format!(
                        "failed to verify creation time on {}: {e}",
                        dst.display()
                    ));
                }
            },
            (Err(e), _) => {
                crate::media_conversion_gate::delivery_metadata_batch_audit(
                    "delivery_metadata_timestamp",
                    crate::infra::static_logs::messages::MSG_METADATA_READ_CREATION_FAIL
                        .replace("{}", &e.to_string()),
                );
                failures.push(format!(
                    "failed to read source creation time from {}: {e}",
                    src.display()
                ));
            }
            (_, Err(e)) => {
                crate::media_conversion_gate::delivery_metadata_path_audit(
                    "delivery_metadata_timestamp",
                    dst,
                    format!(
                        "Metadata Audit: Failed to read destination metadata for creation-time \
                        verification: {e}"
                    ),
                );
                failures.push(format!(
                    "failed to read destination creation time from {}: {e}",
                    dst.display()
                ));
            }
        }
        if let Some(expected_added) = source_added_time {
            match macos::get_added_time(dst) {
                Ok(actual_added) => {
                    log_detail!(format!(
                        "Metadata Audit: Verifying Finder added time integrity ({label}) -> \
                         expected={expected_added:?} actual={actual_added:?}",
                        label = crate::infra::static_logs::messages::LABEL_METADATA,
                    ));
                    let added_time_slack = std::time::Duration::from_secs(1);
                    if !system_time_matches_with_slack(
                        expected_added,
                        actual_added,
                        added_time_slack,
                    ) {
                        let diff_str = system_time_diff_label(expected_added, actual_added);
                        crate::media_conversion_gate::delivery_metadata_batch_audit(
                            "delivery_metadata_timestamp",
                            format!(
                                "Metadata Audit: Finder added time mismatch detected \
                                 (expected={expected_added:?}, actual={actual_added:?}, \
                                 diff={diff_str})"
                            ),
                        );
                        failures.push(format!(
                            "Finder added time mismatch on {}: expected={expected_added:?} \
                             actual={actual_added:?}",
                            dst.display()
                        ));
                    }
                }
                Err(e) => {
                    crate::media_conversion_gate::delivery_metadata_path_audit(
                        "delivery_metadata_timestamp",
                        dst,
                        format!(
                            "Metadata Audit: Failed to read destination Finder added time for \
                             verification: {e}"
                        ),
                    );
                    failures.push(format!(
                        "failed to verify Finder added time on {}: {e}",
                        dst.display()
                    ));
                }
            }
        }
    }

    if failures.is_empty() {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "Timestamp preservation from {} to {} was incomplete: {}",
            src.display(),
            dst.display(),
            failures.join("; ")
        )))
    }
}

/// Preserve "Pro" metadata (XMP, ICC, etc.).
///
/// # Errors
/// Returns an `io::Result` if preservation fails.
pub fn preserve_pro(src: &Path, dst: &Path) -> io::Result<()> {
    #[cfg(target_os = "macos")]
    {
        use std::os::unix::fs::PermissionsExt;

        // copyfile: copies ACL + STAT + xattr in one syscall
        if let Err(e) = macos::copy_native_metadata(src, dst) {
            crate::media_conversion_gate::delivery_metadata_batch_audit(
                "delivery_metadata",
                crate::infra::static_logs::messages::MSG_METADATA_MACOS_COPY_FAIL
                    .replace("{}", &e.to_string()),
            );
            // Fallback: manual xattr copy if copyfile failed
            copy_preservable_xattrs(src, dst)?;
        }
        // ExifTool: EXIF/IPTC/XMP internal tags
        if let Err(e) = exif::preserve_internal(src, dst) {
            crate::media_conversion_gate::delivery_metadata_batch_audit(
                "delivery_metadata",
                format!("Internal metadata failed: {e}"),
            );
            return Err(e);
        }
        // Spotlight / Finder / download xattrs — copy + verify priority keys
        network::preserve_network_metadata(src, dst)?;
        // Supplemental xattrs (e.g. `user.*`) not covered by Spotlight prefix
        copy_supplemental_xattrs(src, dst)?;
        // JXL Spotlight does not read embedded JPEG EXIF/XMP dates; expose the
        // source's resolved content creation date at the filesystem metadata layer.
        macos::apply_spotlight_content_creation_date(src, dst)?;
        // ExifTool/Spotlight can rewrite Apple provenance xattrs on the
        // destination. Replay exact-copy keys last so strict verification sees
        // byte-for-byte source metadata instead of post-rewrite generated values.
        copy_macos_exact_copy_xattrs(src, dst)?;

        // Unix permission bits (copyfile covers STAT but be explicit)
        let meta = std::fs::metadata(src).map_err(|e| {
            crate::media_conversion_gate::delivery_metadata_batch_audit(
                "delivery_metadata",
                crate::infra::static_logs::messages::MSG_METADATA_MACOS_PERM_FAIL
                    .replace("{}", &e.to_string()),
            );
            io::Error::new(
                e.kind(),
                format!(
                    "Failed to read source permission bits from {}: {e}",
                    src.display()
                ),
            )
        })?;
        let mode = meta.permissions().mode();
        std::fs::set_permissions(dst, std::fs::Permissions::from_mode(mode)).map_err(|e| {
            crate::media_conversion_gate::delivery_metadata_batch_audit(
                "delivery_metadata",
                crate::infra::static_logs::messages::MSG_METADATA_MACOS_PERM_FAIL
                    .replace("{}", &e.to_string()),
            );
            io::Error::new(
                e.kind(),
                format!(
                    "Failed to preserve permission bits from {} to {}: {e}",
                    src.display(),
                    dst.display()
                ),
            )
        })?;
        // Timestamps last (ExifTool rewrites file, so must come after)
        apply_file_timestamps(src, dst)?;
        Ok(())
    }

    #[cfg(not(target_os = "macos"))]
    {
        // ExifTool: EXIF/IPTC/XMP internal tags
        if let Err(e) = exif::preserve_internal(src, dst) {
            crate::media_conversion_gate::delivery_metadata_batch_audit(
                "delivery_metadata",
                crate::infra::static_logs::messages::MSG_METADATA_INTERNAL_FAIL
                    .replace("{}", &e.to_string()),
            );
            return Err(e);
        }
        // Apple network xattrs are macOS-only (see macOS block above).
        // Platform-specific attributes
        #[cfg(target_os = "linux")]
        linux::preserve_linux_attributes(src, dst).inspect_err(|e| {
            crate::media_conversion_gate::delivery_metadata_batch_audit(
                "delivery_metadata",
                crate::infra::static_logs::messages::MSG_METADATA_PRESERVE_FAIL
                    .replace("{}", &e.to_string()),
            );
        })?;
        #[cfg(target_os = "windows")]
        windows::preserve_windows_attributes(src, dst).map_err(|e| {
            crate::media_conversion_gate::delivery_metadata_batch_audit(
                "delivery_metadata",
                &crate::infra::static_logs::messages::MSG_METADATA_PRESERVE_FAIL
                    .replace("{}", &e.to_string()),
            );
            e
        })?;
        // Generic xattr copy (all except quarantine / decmpfs)
        copy_preservable_xattrs(src, dst)?;
        // Unix permission bits
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let meta = std::fs::metadata(src).map_err(|e| {
                crate::media_conversion_gate::delivery_metadata_batch_audit(
                    "delivery_metadata",
                    crate::infra::static_logs::messages::MSG_METADATA_MACOS_PERM_FAIL
                        .replace("{}", &e.to_string()),
                );
                io::Error::new(
                    e.kind(),
                    format!(
                        "Failed to read source permission bits from {}: {e}",
                        src.display()
                    ),
                )
            })?;
            let mode = meta.permissions().mode();
            std::fs::set_permissions(dst, std::fs::Permissions::from_mode(mode)).map_err(|e| {
                crate::media_conversion_gate::delivery_metadata_batch_audit(
                    "delivery_metadata",
                    crate::infra::static_logs::messages::MSG_METADATA_MACOS_PERM_FAIL
                        .replace("{}", &e.to_string()),
                );
                io::Error::new(
                    e.kind(),
                    format!(
                        "Failed to preserve permission bits from {} to {}: {e}",
                        src.display(),
                        dst.display()
                    ),
                )
            })?;
        }
        // Timestamps last
        apply_file_timestamps(src, dst)?;
        Ok(())
    }
}

/// Preserve all metadata from source to destination (strict — propagates layer
/// errors).
///
/// For conversion delivery, prefer [`preserve_for_delivery`].
///
/// # Errors
/// Returns an `io::Result` if preservation fails.
pub fn preserve(src: &Path, dst: &Path) -> io::Result<()> {
    std::fs::metadata(src).map_err(|e| {
        io::Error::new(
            e.kind(),
            format!(
                "Failed to read source metadata for preservation from {}: {e}",
                src.display()
            ),
        )
    })?;
    std::fs::metadata(dst).map_err(|e| {
        io::Error::new(
            e.kind(),
            format!(
                "Failed to read destination metadata for preservation into {}: {e}",
                dst.display()
            ),
        )
    })?;
    preserve_pro(src, dst)?;
    verify_exact_metadata_copy(src, dst).map(|_| ())
}

fn metadata_copy_signature(path: &Path) -> io::Result<MetadataCopySignature> {
    let metadata = std::fs::metadata(path).map_err(|e| {
        io::Error::new(
            e.kind(),
            format!(
                "Failed to read metadata signature for {}: {e}",
                path.display()
            ),
        )
    })?;
    let modified = metadata.modified().map_err(|e| {
        io::Error::new(
            e.kind(),
            format!(
                "Failed to read modified time for exact metadata verification on {}: {e}",
                path.display()
            ),
        )
    })?;
    #[cfg(unix)]
    let mode = {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode()
    };
    Ok(MetadataCopySignature {
        readonly: metadata.permissions().readonly(),
        modified: filetime::FileTime::from_system_time(modified),
        #[cfg(unix)]
        mode,
        xattrs: exact_copy_xattr_signature(path)?,
    })
}

fn exact_copy_xattr_signature(path: &Path) -> io::Result<Vec<(String, String)>> {
    let mut xattrs = Vec::new();
    match xattr::list(path) {
        Ok(iter) => {
            for name in iter {
                let Some(name_str) = name.to_str() else {
                    continue;
                };
                if !should_verify_exact_copy_xattr(name_str) {
                    continue;
                }
                match xattr::get(path, name_str) {
                    Ok(Some(value)) => {
                        xattrs.push((name_str.to_string(), blake3::hash(&value).to_string()));
                    }
                    Ok(None) => {}
                    Err(e) => {
                        return Err(io::Error::new(
                            e.kind(),
                            format!(
                                "Failed to read extended attribute '{name_str}' from {} for exact \
                                 metadata verification: {e}",
                                path.display()
                            ),
                        ));
                    }
                }
            }
        }
        Err(e) if delivery_policy::is_xattr_api_absence(&e) => {}
        Err(e) => {
            return Err(io::Error::new(
                e.kind(),
                format!(
                    "Failed to list extended attributes for exact metadata verification on {}: {e}",
                    path.display()
                ),
            ));
        }
    }
    xattrs.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(xattrs)
}

fn metadata_xattr_mismatches(
    src_xattrs: &[(String, String)],
    dst_xattrs: &[(String, String)],
) -> Vec<String> {
    let dst_by_name = dst_xattrs
        .iter()
        .map(|(name, hash)| (name.as_str(), hash.as_str()))
        .collect::<std::collections::BTreeMap<_, _>>();
    let src_names = src_xattrs
        .iter()
        .map(|(name, _)| name.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    let mut mismatches = Vec::new();

    for (name, expected_hash) in src_xattrs {
        match dst_by_name.get(name.as_str()).copied() {
            Some(actual_hash) if actual_hash == expected_hash.as_str() => {}
            Some(actual_hash) => mismatches.push(format!(
                "xattrs {name} expected={expected_hash} actual={actual_hash}"
            )),
            None => mismatches.push(format!("xattrs {name} missing from destination")),
        }
    }

    let unexpected = dst_xattrs
        .iter()
        .filter(|(name, _)| {
            !src_names.contains(name.as_str())
                && !is_destination_generated_exact_copy_xattr(name.as_str())
        })
        .cloned()
        .collect::<Vec<_>>();
    if !unexpected.is_empty() {
        mismatches.push(format!("xattrs unexpected actual_only={unexpected:?}"));
    }

    mismatches
}

fn metadata_signature_mismatches(
    src: &MetadataCopySignature,
    dst: &MetadataCopySignature,
) -> Vec<String> {
    let mut mismatches = Vec::new();
    if src.readonly != dst.readonly {
        mismatches.push(format!(
            "readonly expected={} actual={}",
            src.readonly, dst.readonly
        ));
    }
    if src.modified != dst.modified {
        mismatches.push(format!(
            "modified expected={:?} actual={:?}",
            src.modified, dst.modified
        ));
    }
    #[cfg(unix)]
    if src.mode != dst.mode {
        mismatches.push(format!(
            "mode expected={:o} actual={:o}",
            src.mode, dst.mode
        ));
    }
    mismatches.extend(metadata_xattr_mismatches(&src.xattrs, &dst.xattrs));
    mismatches
}

/// Verify that the shared filesystem metadata copy surface exactly matches.
///
/// This is intentionally format-agnostic bottom-layer validation: callers use
/// it after metadata copy/preservation to catch source/output pair misalignment
/// before higher-level codec or Photos checks can mask it.
///
/// # Errors
/// Returns an `io::Error` if source/destination metadata cannot be read or if
/// any copied metadata field differs.
pub fn verify_exact_metadata_copy(src: &Path, dst: &Path) -> io::Result<MetadataCopyCheck> {
    let src_signature = metadata_copy_signature(src)?;
    let dst_signature = metadata_copy_signature(dst)?;
    let mismatches = metadata_signature_mismatches(&src_signature, &dst_signature);
    if mismatches.is_empty() {
        let detail = format!(
            "Metadata Audit: exact metadata copy verified {} -> {}",
            src.display(),
            dst.display()
        );
        tracing::info!(
            target: "mfb.metadata",
            src = %src.display(),
            dst = %dst.display(),
            "{detail}"
        );
        crate::log_info!(crate::infra::static_logs::messages::LABEL_METADATA, detail);
        Ok(MetadataCopyCheck {
            passed: true,
            mismatches,
        })
    } else {
        crate::media_conversion_gate::delivery_metadata_path_audit(
            "delivery_metadata_exact_copy",
            dst,
            format!(
                "Metadata Audit: exact metadata copy mismatch {} -> {}: {}",
                src.display(),
                dst.display(),
                mismatches.join("; ")
            ),
        );
        Err(io::Error::other(format!(
            "Exact metadata copy mismatch from {} to {}: {}",
            src.display(),
            dst.display(),
            mismatches.join("; ")
        )))
    }
}

fn record_exif_delivery_result(result: io::Result<()>, report: &mut MetadataDeliveryReport) {
    match result {
        Ok(()) => report.exif = MetadataLayerOutcome::Applied,
        Err(e) if delivery_policy::is_metadata_delivery_soft_error(&e) => {
            crate::media_conversion_gate::delivery_metadata_batch_audit(
                "delivery_metadata_exif",
                crate::infra::static_logs::messages::MSG_METADATA_DELIVERY_SKIP_NO_SOURCE_EXIF
                    .replace("{}", &e.to_string()),
            );
            report.exif = MetadataLayerOutcome::SkippedNoSourceMetadata;
        }
        Err(e) => {
            crate::media_conversion_gate::delivery_metadata_batch_audit(
                "delivery_metadata",
                format!("Internal metadata delivery audit (continuing): {e}"),
            );
            report.exif = MetadataLayerOutcome::PartialAudit;
        }
    }
}

fn record_xattr_delivery_result(result: io::Result<()>, report: &mut MetadataDeliveryReport) {
    match result {
        Ok(()) => {
            if !matches!(report.xattr, MetadataLayerOutcome::PartialAudit) {
                report.xattr = MetadataLayerOutcome::Applied;
            }
        }
        Err(e) if delivery_policy::is_xattr_api_absence(&e) => {
            crate::media_conversion_gate::delivery_metadata_batch_audit(
                "delivery_metadata",
                crate::infra::static_logs::messages::MSG_METADATA_DELIVERY_SKIP_XATTR_ABSENCE
                    .replace("{}", &e.to_string()),
            );
            report.xattr = MetadataLayerOutcome::SkippedNoSourceMetadata;
        }
        Err(e) => {
            crate::media_conversion_gate::delivery_metadata_batch_audit(
                "delivery_metadata",
                crate::infra::static_logs::messages::MSG_METADATA_DELIVERY_XATTR_PARTIAL
                    .replace("{}", &e.to_string()),
            );
            report.xattr = MetadataLayerOutcome::PartialAudit;
        }
    }
}

#[cfg(target_os = "macos")]
pub(crate) fn reapply_spotlight_content_creation_date_for_delivery(
    src: &Path,
    dst: &Path,
    report: &mut MetadataDeliveryReport,
) {
    record_xattr_delivery_result(
        macos::apply_spotlight_content_creation_date(src, dst),
        report,
    );
}

#[cfg(target_os = "macos")]
pub(crate) fn reapply_macos_exact_copy_xattrs_for_delivery(
    src: &Path,
    dst: &Path,
    report: &mut MetadataDeliveryReport,
) {
    record_xattr_delivery_result(copy_macos_exact_copy_xattrs(src, dst), report);
    reapply_spotlight_content_creation_date_for_delivery(src, dst, report);
}

/// Best-effort `preserve_pro` for conversion delivery (see
/// [`preserve_for_delivery`]).
pub(super) fn preserve_pro_for_delivery(
    src: &Path,
    dst: &Path,
    report: &mut MetadataDeliveryReport,
    copy_internal_metadata: bool,
) -> io::Result<()> {
    #[cfg(target_os = "macos")]
    {
        if let Err(e) = macos::copy_native_metadata(src, dst) {
            crate::media_conversion_gate::delivery_metadata_batch_audit(
                "delivery_metadata",
                crate::infra::static_logs::messages::MSG_METADATA_MACOS_COPY_FAIL
                    .replace("{}", &e.to_string()),
            );
            record_xattr_delivery_result(copy_preservable_xattrs(src, dst), report);
        }
        if copy_internal_metadata {
            record_exif_delivery_result(exif::preserve_internal(src, dst), report);
        }
        record_xattr_delivery_result(network::preserve_network_metadata(src, dst), report);
        record_xattr_delivery_result(copy_supplemental_xattrs(src, dst), report);
        record_xattr_delivery_result(
            macos::apply_spotlight_content_creation_date(src, dst),
            report,
        );
        record_xattr_delivery_result(copy_macos_exact_copy_xattrs(src, dst), report);

        match std::fs::metadata(src) {
            Ok(meta) => {
                use std::os::unix::fs::PermissionsExt;
                let mode = meta.permissions().mode();
                if let Err(e) = std::fs::set_permissions(dst, std::fs::Permissions::from_mode(mode))
                {
                    crate::media_conversion_gate::delivery_metadata_batch_audit(
                        "delivery_metadata",
                        crate::infra::static_logs::messages::MSG_METADATA_MACOS_PERM_FAIL
                            .replace("{}", &e.to_string()),
                    );
                    report.xattr = MetadataLayerOutcome::PartialAudit;
                }
            }
            Err(e) => {
                crate::media_conversion_gate::delivery_metadata_batch_audit(
                    "delivery_metadata",
                    crate::infra::static_logs::messages::MSG_METADATA_MACOS_PERM_FAIL
                        .replace("{}", &e.to_string()),
                );
                report.xattr = MetadataLayerOutcome::PartialAudit;
            }
        }
        apply_file_timestamps_for_delivery(src, dst, report)
    }

    #[cfg(not(target_os = "macos"))]
    {
        if copy_internal_metadata {
            record_exif_delivery_result(exif::preserve_internal(src, dst), report);
        }
        #[cfg(target_os = "linux")]
        if let Err(e) = linux::preserve_linux_attributes(src, dst) {
            crate::media_conversion_gate::delivery_metadata_batch_audit(
                "delivery_metadata",
                crate::infra::static_logs::messages::MSG_METADATA_PRESERVE_FAIL
                    .replace("{}", &e.to_string()),
            );
            report.xattr = MetadataLayerOutcome::PartialAudit;
        }
        #[cfg(target_os = "windows")]
        if let Err(e) = windows::preserve_windows_attributes(src, dst) {
            crate::media_conversion_gate::delivery_metadata_batch_audit(
                "delivery_metadata",
                crate::infra::static_logs::messages::MSG_METADATA_PRESERVE_FAIL
                    .replace("{}", &e.to_string()),
            );
            report.xattr = MetadataLayerOutcome::PartialAudit;
        }
        record_xattr_delivery_result(copy_preservable_xattrs(src, dst), report);
        #[cfg(unix)]
        {
            match std::fs::metadata(src) {
                Ok(meta) => {
                    use std::os::unix::fs::PermissionsExt;
                    let mode = meta.permissions().mode();
                    if let Err(e) =
                        std::fs::set_permissions(dst, std::fs::Permissions::from_mode(mode))
                    {
                        crate::media_conversion_gate::delivery_metadata_batch_audit(
                            "delivery_metadata",
                            crate::infra::static_logs::messages::MSG_METADATA_MACOS_PERM_FAIL
                                .replace("{}", &e.to_string()),
                        );
                        report.xattr = MetadataLayerOutcome::PartialAudit;
                    }
                }
                Err(e) => {
                    crate::media_conversion_gate::delivery_metadata_batch_audit(
                        "delivery_metadata",
                        crate::infra::static_logs::messages::MSG_METADATA_MACOS_PERM_FAIL
                            .replace("{}", &e.to_string()),
                    );
                    report.xattr = MetadataLayerOutcome::PartialAudit;
                }
            }
        }
        apply_file_timestamps_for_delivery(src, dst, report)?;
        Ok(())
    }
}

/// Merge source's XMP sidecar into destination (for conversion output).
///
/// Returns `Ok(false)` when no sidecar exists, `Ok(true)` when a sidecar was
/// merged, and `Err` when a sidecar exists but neither the primary nor fallback
/// merge path can preserve it.
///
/// # Errors
/// Returns an error if a discovered XMP sidecar cannot be merged into `dst`.
pub fn merge_xmp_sidecar_into_dest(src: &Path, dst: &Path) -> io::Result<bool> {
    merge_xmp_sidecar(src, dst)
}

fn require_regular_sidecar(path: PathBuf, label: &str) -> io::Result<PathBuf> {
    let metadata = std::fs::metadata(&path).map_err(|e| {
        crate::media_conversion_gate::delivery_metadata_path_audit(
            "delivery_metadata_sidecar",
            &path,
            format!("Metadata Audit: Failed to inspect {label} sidecar: {e}"),
        );
        io::Error::new(
            e.kind(),
            format!("failed to inspect {label} sidecar {}: {e}", path.display()),
        )
    })?;
    if !metadata.is_file() {
        crate::media_conversion_gate::delivery_metadata_path_audit(
            "delivery_metadata_sidecar",
            &path,
            format!(
                "Metadata Audit: Refusing non-file {label} sidecar {}",
                path.display()
            ),
        );
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{label} sidecar is not a regular file: {}", path.display()),
        ));
    }
    Ok(path)
}

/// Find an adjacent Apple Adjustment Envelope sidecar for a source asset.
///
/// Exact `with_extension("AAE")`/`with_extension("aae")` candidates are checked
/// first, then a case-insensitive directory scan catches mixed-case imports.
///
/// # Errors
/// Returns an error when a matching path exists but is not a regular file, or
/// when the source parent cannot be scanned for case-insensitive matches.
pub fn find_aae_sidecar(src: &Path) -> io::Result<Option<PathBuf>> {
    let Some(parent) = src.parent() else {
        return Ok(None);
    };
    let Some(stem) = src.file_stem().and_then(|value| value.to_str()) else {
        return Ok(None);
    };
    let stem_lower = stem.to_lowercase();
    let mut matches = Vec::new();
    for entry in std::fs::read_dir(parent).map_err(|e| {
        crate::media_conversion_gate::delivery_metadata_path_audit(
            "delivery_metadata_sidecar",
            parent,
            format!(
                "Metadata Audit: Failed to scan source parent for AAE sidecars {}: {e}",
                parent.display()
            ),
        );
        io::Error::new(
            e.kind(),
            format!(
                "failed to scan source parent for AAE sidecars {}: {e}",
                parent.display()
            ),
        )
    })? {
        let entry = entry?;
        let path = entry.path();
        let is_aae = path
            .extension()
            .and_then(|value| value.to_str())
            .is_some_and(|ext| ext.eq_ignore_ascii_case("aae"));
        if !is_aae {
            continue;
        }
        let stem_matches = path
            .file_stem()
            .and_then(|value| value.to_str())
            .is_some_and(|candidate_stem| candidate_stem.to_lowercase() == stem_lower);
        if stem_matches {
            matches.push(path);
        }
    }
    matches.sort_by_key(|path| path.file_name().map(std::ffi::OsStr::to_ascii_lowercase));
    if let Some(path) = matches.into_iter().next() {
        require_regular_sidecar(path, "AAE").map(Some)
    } else {
        Ok(None)
    }
}

/// Compute where an AAE sidecar belongs after conversion.
///
/// # Errors
/// Returns an error if the sidecar extension is not AAE or the output path has
/// no file stem.
pub fn aae_sidecar_destination(aae: &Path, output: &Path) -> io::Result<PathBuf> {
    let ext = aae
        .extension()
        .and_then(|value| value.to_str())
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("AAE sidecar has no extension: {}", aae.display()),
            )
        })?;
    if !ext.eq_ignore_ascii_case("aae") {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("sidecar extension is not AAE: {}", aae.display()),
        ));
    }
    if output.file_stem().is_none() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("output path has no file stem: {}", output.display()),
        ));
    }
    Ok(output.with_extension(ext))
}

/// Copy or delete an Apple AAE sidecar for a converted asset.
///
/// # Errors
/// Returns an error when sidecar discovery, copy, metadata reapplication, or
/// deletion fails.
pub fn handle_aae_sidecar(
    input: &Path,
    output: &Path,
    apple_compat: bool,
) -> io::Result<AaeSidecarAction> {
    let Some(aae) = find_aae_sidecar(input)? else {
        return Ok(AaeSidecarAction::Missing);
    };
    if apple_compat {
        let destination = aae_sidecar_destination(&aae, output)?;
        if destination == aae {
            return Ok(AaeSidecarAction::AlreadyAdjacent {
                source: aae,
                destination,
            });
        }
        if let Some(parent) = destination.parent()
            && !parent.is_dir()
        {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!(
                    "AAE destination parent does not exist: {}",
                    parent.display()
                ),
            ));
        }
        std::fs::copy(&aae, &destination).map_err(|e| {
            crate::media_conversion_gate::delivery_api_path_fallback_audit(
                "aae_migrate_failed",
                &destination,
                format!("failed to migrate AAE sidecar: {e}"),
            );
            io::Error::new(
                e.kind(),
                format!(
                    "failed to migrate AAE sidecar {} to {}: {e}",
                    aae.display(),
                    destination.display()
                ),
            )
        })?;
        apply_file_timestamps(&aae, &destination).map_err(|e| {
            crate::media_conversion_gate::delivery_metadata_path_audit(
                "delivery_metadata_sidecar",
                &destination,
                format!("Metadata Audit: Failed to restore AAE sidecar timestamps: {e}"),
            );
            io::Error::new(
                e.kind(),
                format!(
                    "failed to restore AAE sidecar timestamps {} -> {}: {e}",
                    aae.display(),
                    destination.display()
                ),
            )
        })?;
        if let Err(e) = copy_preservable_xattrs(&aae, &destination) {
            crate::media_conversion_gate::delivery_metadata_path_audit(
                "delivery_metadata_sidecar",
                &destination,
                format!("Metadata Audit: Failed to copy AAE sidecar xattrs: {e}"),
            );
            return Err(io::Error::new(
                e.kind(),
                format!(
                    "failed to copy AAE sidecar xattrs {} -> {}: {e}",
                    aae.display(),
                    destination.display()
                ),
            ));
        }
        Ok(AaeSidecarAction::Copied {
            source: aae,
            destination,
        })
    } else {
        std::fs::remove_file(&aae).map_err(|e| {
            crate::media_conversion_gate::delivery_cleanup_audit(&aae, "orphaned_aae_delete", &e);
            io::Error::new(
                e.kind(),
                format!(
                    "failed to delete orphaned AAE sidecar {}: {e}",
                    aae.display()
                ),
            )
        })?;
        Ok(AaeSidecarAction::Deleted { source: aae })
    }
}

/// # Errors
/// Returns an error if metadata preservation or the final timestamp
/// re-application fails.
pub fn copy(src: &Path, dst: &Path) -> io::Result<()> {
    preserve(src, dst)?;
    merge_xmp_sidecar(src, dst)?;
    apply_file_timestamps(src, dst)
}

/// Preserve directory metadata (timestamps, etc.).
///
/// # Errors
/// Returns an `io::Result` if preservation fails.
pub fn preserve_directory(src_dir: &Path, dst_dir: &Path) -> io::Result<()> {
    use std::collections::HashMap;

    let mut dir_metadata: HashMap<std::path::PathBuf, std::fs::Metadata> = HashMap::new();
    let mut failures = Vec::new();

    let meta = require_existing_directory(src_dir, "source")?;
    dir_metadata.insert(src_dir.to_path_buf(), meta);

    collect_dir_metadata(src_dir, &mut dir_metadata)?;

    for (src_path, metadata) in &dir_metadata {
        let rel_path = crate::media_conversion_gate::strip_prefix_or_self(
            src_path,
            src_dir,
            "delivery_metadata",
        );
        let dst_path = dst_dir.join(rel_path);

        if !dst_path.exists()
            && let Err(e) = std::fs::create_dir_all(&dst_path)
        {
            crate::media_conversion_gate::delivery_metadata_path_audit(
                "delivery_metadata",
                &dst_path,
                format!(
                    "Metadata Audit: Failed to create target directory {dst_display}: {e}",
                    dst_display = dst_path.display(),
                ),
            );
            failures.push(format!(
                "failed to create directory {} while mirroring {}: {e}",
                dst_path.display(),
                src_path.display()
            ));
            continue;
        }

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = metadata.permissions().mode();
            if let Err(e) =
                std::fs::set_permissions(&dst_path, std::fs::Permissions::from_mode(mode))
            {
                crate::media_conversion_gate::delivery_metadata_path_audit(
                    "delivery_metadata",
                    &dst_path,
                    format!(
                        "Metadata Audit: Failed to set permissions on {dst_display}: {e}",
                        dst_display = dst_path.display(),
                    ),
                );
                failures.push(format!(
                    "failed to set permissions on {} from {}: {e}",
                    dst_path.display(),
                    src_path.display()
                ));
            }
        }

        // macOS: set creation time BEFORE atime/mtime (will re-apply after)
        #[cfg(target_os = "macos")]
        {
            match metadata.created() {
                Ok(created) => {
                    if let Err(e) = macos::set_creation_time(&dst_path, created) {
                        crate::media_conversion_gate::delivery_metadata_path_audit(
                            "delivery_metadata_timestamp",
                            &dst_path,
                            format!(
                                "Failed to set creation time for {path}: {e}",
                                path = dst_path.display()
                            ),
                        );
                        failures.push(format!(
                            "failed to set creation time on {} from {}: {e}",
                            dst_path.display(),
                            src_path.display()
                        ));
                    }
                }
                Err(e) => {
                    crate::media_conversion_gate::delivery_metadata_path_audit(
                        "delivery_metadata_timestamp",
                        src_path,
                        format!(
                            "Metadata Audit: Failed to read source directory creation time for \
                             {src_display}: {e}",
                            src_display = src_path.display(),
                        ),
                    );
                    failures.push(format!(
                        "failed to read creation time from {} while mirroring {}: {e}",
                        src_path.display(),
                        dst_path.display()
                    ));
                }
            }
        }

        let atime = filetime::FileTime::from_last_access_time(metadata);
        let mtime = filetime::FileTime::from_last_modification_time(metadata);
        if let Err(e) = filetime::set_file_times(&dst_path, atime, mtime) {
            crate::media_conversion_gate::delivery_metadata_path_audit(
                "delivery_metadata_timestamp",
                &dst_path,
                format!(
                    "Metadata Audit: Failed to set timestamps for directory {dst_display}: {e}",
                    dst_display = dst_path.display(),
                ),
            );
            failures.push(format!(
                "failed to set directory timestamps on {} from {}: {e}",
                dst_path.display(),
                src_path.display()
            ));
        }

        // macOS: re-apply creation time AFTER atime/mtime (filetime may reset it)
        #[cfg(target_os = "macos")]
        {
            match metadata.created() {
                Ok(created) => {
                    if let Err(e) = macos::set_creation_time(&dst_path, created) {
                        crate::media_conversion_gate::delivery_metadata_path_audit(
                            "delivery_metadata_timestamp",
                            &dst_path,
                            format!(
                                "Failed to set creation time for {path}: {e}",
                                path = dst_path.display()
                            ),
                        );
                        failures.push(format!(
                            "failed to reapply creation time on {} from {}: {e}",
                            dst_path.display(),
                            src_path.display()
                        ));
                    }
                }
                Err(e) => {
                    crate::media_conversion_gate::delivery_metadata_path_audit(
                        "delivery_metadata_timestamp",
                        src_path,
                        format!(
                            "Metadata Audit: Failed to reread source directory creation time for \
                             {src_display}: {e}",
                            src_display = src_path.display(),
                        ),
                    );
                    failures.push(format!(
                        "failed to reread creation time from {} while mirroring {}: {e}",
                        src_path.display(),
                        dst_path.display()
                    ));
                }
            }
            // Also preserve added time for directories
            match macos::get_added_time(src_path) {
                Ok(added) => {
                    if let Err(e) = macos::set_added_time(&dst_path, added) {
                        crate::media_conversion_gate::delivery_metadata_path_audit(
                            "delivery_metadata",
                            &dst_path,
                            format!(
                                "Metadata Audit: Failed to set 'added' time for directory \
                                 {dst_display}: {e}",
                                dst_display = dst_path.display(),
                            ),
                        );
                        failures.push(format!(
                            "failed to set Finder added time on {} from {}: {e}",
                            dst_path.display(),
                            src_path.display()
                        ));
                    }
                }
                Err(e) => {
                    crate::media_conversion_gate::delivery_metadata_path_audit(
                        "delivery_metadata",
                        src_path,
                        format!(
                            "Metadata Audit: Failed to read Finder added time for directory \
                             {src_display}: {e}",
                            src_display = src_path.display(),
                        ),
                    );
                    failures.push(format!(
                        "failed to read Finder added time from {} while mirroring {}: {e}",
                        src_path.display(),
                        dst_path.display()
                    ));
                }
            }
        }

        if let Err(e) = copy_dir_xattrs(src_path, &dst_path) {
            failures.push(format!(
                "failed to preserve directory xattrs from {} to {}: {e}",
                src_path.display(),
                dst_path.display()
            ));
        }
    }

    if failures.is_empty() {
        Ok(())
    } else {
        Err(io::Error::other(failures.join(" | ")))
    }
}

/// Preserve directory-level metadata and emit audit logs around the attempt.
///
/// # Errors
/// Returns the underlying `io::Error` if directory metadata preservation fails.
pub fn preserve_directory_with_log(base_dir: &Path, output_dir: &Path) -> io::Result<()> {
    log_detail!(
        &crate::infra::static_logs::messages::MSG_METADATA_TREE_PRESERVE
            .replace("{}", crate::infra::static_logs::messages::LABEL_METADATA)
    );
    if let Err(e) = preserve_directory(base_dir, output_dir) {
        crate::media_conversion_gate::delivery_metadata_batch_audit(
            "delivery_metadata",
            crate::infra::static_logs::messages::MSG_METADATA_TREE_FAIL
                .replace("{}", &e.to_string()),
        );
        return Err(e);
    }

    log_stat!(
        crate::infra::static_logs::messages::LABEL_METADATA,
        crate::infra::static_logs::messages::MSG_METADATA_TREE_SUCCESS
    );
    Ok(())
}

/// Save directory timestamps to a map.
///
/// # Errors
pub type DirectoryTimestampsMap =
    std::collections::HashMap<std::path::PathBuf, (filetime::FileTime, filetime::FileTime)>;

/// Returns an `io::Result` if saving fails.
pub fn save_directory_timestamps(dir: &Path) -> io::Result<DirectoryTimestampsMap> {
    use std::collections::HashMap;
    let mut saved = HashMap::new();
    let meta = require_existing_directory(dir, "source timestamp root")?;
    let atime = filetime::FileTime::from_last_access_time(&meta);
    let mtime = filetime::FileTime::from_last_modification_time(&meta);
    saved.insert(dir.to_path_buf(), (atime, mtime));
    collect_dir_timestamps(dir, &mut saved)?;
    Ok(saved)
}

/// Restore saved directory timestamps back onto the source tree.
///
/// # Errors
/// Returns an aggregated `io::Error` if any directory timestamp restoration
/// fails.
pub fn restore_directory_timestamps<S>(
    saved: &std::collections::HashMap<
        std::path::PathBuf,
        (filetime::FileTime, filetime::FileTime),
        S,
    >,
) -> io::Result<()>
where
    S: std::hash::BuildHasher,
{
    let mut failed_count = 0_i32;
    let mut total_count = 0_i32;
    let mut first_error: Option<io::Error> = None;

    for (path, (atime, mtime)) in saved {
        if path.exists() && path.is_dir() {
            total_count += 1_i32;
            if let Err(e) = filetime::set_file_times(path, *atime, *mtime) {
                failed_count += 1_i32;
                if first_error.is_none() {
                    first_error = Some(io::Error::new(e.kind(), e.to_string()));
                }
                crate::media_conversion_gate::delivery_metadata_path_audit(
                    "delivery_metadata",
                    path,
                    format!(
                        "Metadata Audit: Restoration failed for {path_display}: {e}",
                        path_display = path.display(),
                    ),
                );
            }
        }
    }

    if failed_count > 0_i32 {
        crate::media_conversion_gate::delivery_metadata_batch_audit(
            "delivery_metadata",
            crate::infra::static_logs::messages::MSG_METADATA_RESTORE_VERIFY_FAIL
                .replacen("{}", &failed_count.to_string(), 1)
                .replacen("{}", &total_count.to_string(), 1),
        );
        return Err(io::Error::other(format!(
            "Failed to restore timestamps for {failed_count} of {total_count} directories: {}",
            crate::media_conversion_gate::io_error_or_metadata_label(
                first_error,
                "unknown restore timestamp error",
            )
        )));
    }

    Ok(())
}

/// Apply saved source directory timestamps onto the mirrored destination tree.
///
/// # Errors
/// Returns an aggregated `io::Error` if any destination directory timestamp
/// update fails.
pub fn apply_saved_timestamps_to_dst<S>(
    saved: &std::collections::HashMap<
        std::path::PathBuf,
        (filetime::FileTime, filetime::FileTime),
        S,
    >,
    src_root: &Path,
    dst_root: &Path,
) -> io::Result<()>
where
    S: std::hash::BuildHasher,
{
    let mut failed_count = 0_i32;
    let mut total_count = 0_i32;
    let mut first_error: Option<io::Error> = None;

    require_existing_directory(src_root, "source root")?;
    require_existing_directory(dst_root, "destination root")?;

    for (src_path, (atime, mtime)) in saved {
        let rel_path = src_path.strip_prefix(src_root).map_err(|e| {
            crate::media_conversion_gate::delivery_metadata_path_audit(
                "delivery_metadata",
                src_path,
                format!(
                    "Metadata Audit: Failed to map saved timestamp path under source root {}: {e}",
                    src_root.display()
                ),
            );
            io::Error::other(format!(
                "saved timestamp path {} is outside source root {}: {e}",
                src_path.display(),
                src_root.display()
            ))
        })?;
        let dst_path = dst_root.join(rel_path);
        let dst_metadata = match std::fs::metadata(&dst_path) {
            Ok(metadata) => metadata,
            Err(e) => {
                failed_count += 1_i32;
                if first_error.is_none() {
                    first_error = Some(io::Error::new(
                        e.kind(),
                        format!(
                            "missing destination directory mirror {} for saved timestamp path {}: \
                             {e}",
                            dst_path.display(),
                            src_path.display()
                        ),
                    ));
                }
                crate::media_conversion_gate::delivery_metadata_path_audit(
                    "delivery_metadata",
                    &dst_path,
                    format!(
                        "Metadata Audit: Missing destination directory mirror for saved timestamp \
                         path {src_display}: {e}",
                        src_display = src_path.display(),
                    ),
                );
                continue;
            }
        };
        if !dst_metadata.is_dir() {
            failed_count += 1_i32;
            if first_error.is_none() {
                first_error = Some(io::Error::other(format!(
                    "missing destination directory mirror: {} is not a directory",
                    dst_path.display()
                )));
            }
            crate::media_conversion_gate::delivery_metadata_path_audit(
                "delivery_metadata",
                &dst_path,
                format!(
                    "Metadata Audit: Missing destination directory mirror for saved timestamp \
                     path {}: destination is not a directory",
                    src_path.display(),
                ),
            );
            continue;
        }
        total_count += 1_i32;
        if let Err(e) = filetime::set_file_times(&dst_path, *atime, *mtime) {
            failed_count += 1_i32;
            if first_error.is_none() {
                first_error = Some(io::Error::new(e.kind(), e.to_string()));
            }
            crate::media_conversion_gate::delivery_metadata_path_audit(
                "delivery_metadata",
                &dst_path,
                format!(
                    "Metadata Audit: Failed to apply forensic tags to {dst_display}: {e}",
                    dst_display = dst_path.display(),
                ),
            );
        }
    }

    if failed_count > 0_i32 {
        crate::media_conversion_gate::delivery_metadata_batch_audit(
            "delivery_metadata",
            crate::infra::static_logs::messages::MSG_METADATA_APPLY_VERIFY_FAIL
                .replacen("{}", &failed_count.to_string(), 1)
                .replacen("{}", &total_count.to_string(), 1),
        );
        return Err(io::Error::other(format!(
            "Failed to apply saved timestamps to {failed_count} of {total_count} directories \
             under {}: {}",
            dst_root.display(),
            crate::media_conversion_gate::io_error_or_metadata_label(
                first_error,
                "missing destination directory mirror",
            )
        )));
    }

    Ok(())
}

fn copy_file_timestamps_only(src: &Path, dst: &Path) -> io::Result<()> {
    apply_file_timestamps(src, dst).map_err(|e| {
        crate::media_conversion_gate::delivery_metadata_path_audit(
            "delivery_metadata_timestamp",
            src,
            format!(
                "Failed to restore file timestamps from {} to {}: {}",
                src.display(),
                dst.display(),
                e
            ),
        );
        e
    })
}

fn copy_file_timestamps_from_source_tree(src_root: &Path, dst_root: &Path) -> io::Result<()> {
    const SOURCE_EXTENSIONS: &[&str] = &[
        "jpg", "jpeg", "png", "webp", "heic", "heif", "avif", "gif", "tiff", "tif", "bmp", "jxl",
    ];
    let mut failed_count = 0_u32;
    let mut first_error: Option<io::Error> = None;
    for entry in walkdir::WalkDir::new(dst_root).follow_links(false) {
        let entry = match entry {
            Ok(entry) => entry,
            Err(err) => {
                crate::media_conversion_gate::delivery_metadata_batch_audit(
                    "delivery_metadata_timestamp",
                    format!(
                        "Failed to inspect destination file while restoring timestamps from \
                         source tree (dir={}): {}",
                        dst_root.display(),
                        err
                    ),
                );
                failed_count += 1;
                if first_error.is_none() {
                    first_error = Some(io::Error::other(err.to_string()));
                }
                continue;
            }
        };
        let dst_path = entry.path();
        if !dst_path.is_file() {
            continue;
        }
        let Ok(rel) = dst_path.strip_prefix(dst_root) else {
            continue;
        };
        let parent = crate::media_conversion_gate::path_relative_parent_or_self(rel);
        let stem = crate::media_conversion_gate::path_file_stem_or_empty(
            dst_path,
            "metadata:timestamp_restore",
        );
        if stem.is_empty() {
            continue;
        }
        let src_parent = src_root.join(parent);
        for ext in SOURCE_EXTENSIONS {
            let src_file = src_parent.join(format!("{stem}.{ext}"));
            if src_file.exists() && src_file.is_file() {
                if let Err(e) = copy_file_timestamps_only(&src_file, dst_path) {
                    failed_count += 1;
                    if first_error.is_none() {
                        first_error = Some(e);
                    }
                }
                break;
            }
        }
    }

    if failed_count > 0 {
        return Err(io::Error::other(format!(
            "Failed to restore timestamps for {failed_count} output files under {}: {}",
            dst_root.display(),
            crate::media_conversion_gate::io_error_or_metadata_label(
                first_error,
                "unknown file timestamp restore error",
            )
        )));
    }

    Ok(())
}

/// Restore timestamps from source directory to output directory.
///
/// # Errors
/// Returns an `io::Result` if restoration fails.
pub fn restore_timestamps_from_source_to_output(src_dir: &Path, dst_dir: &Path) -> io::Result<()> {
    let saved = save_directory_timestamps(src_dir)?;
    apply_saved_timestamps_to_dst(&saved, src_dir, dst_dir)?;
    copy_file_timestamps_from_source_tree(src_dir, dst_dir)?;
    restore_directory_timestamps(&saved)?;
    Ok(())
}

/// Restore source and destination directory metadata after a destructive
/// delivery.
///
/// Fast image mode uses this after verified JPEG deletion: the source directory
/// timestamps are restored from the pre-delete snapshot, while directory-level
/// metadata is mirrored into the JXL-only output tree.
///
/// # Errors
/// Returns an aggregated `io::Error` if any source or destination metadata
/// restore step fails.
pub fn restore_delivery_directory_metadata<S>(
    saved: &std::collections::HashMap<
        std::path::PathBuf,
        (filetime::FileTime, filetime::FileTime),
        S,
    >,
    src_dir: &Path,
    dst_dir: &Path,
) -> io::Result<()>
where
    S: std::hash::BuildHasher,
{
    preserve_directory(src_dir, dst_dir)?;
    apply_saved_timestamps_to_dst(saved, src_dir, dst_dir)?;
    restore_directory_timestamps(saved)?;
    Ok(())
}

fn collect_dir_timestamps<S>(
    dir: &Path,
    map: &mut std::collections::HashMap<
        std::path::PathBuf,
        (filetime::FileTime, filetime::FileTime),
        S,
    >,
) -> io::Result<()>
where
    S: std::hash::BuildHasher,
{
    let entries = std::fs::read_dir(dir)?;
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            let meta = std::fs::metadata(&path)?;
            let atime = filetime::FileTime::from_last_access_time(&meta);
            let mtime = filetime::FileTime::from_last_modification_time(&meta);
            map.insert(path.clone(), (atime, mtime));
            collect_dir_timestamps(&path, map)?;
        }
    }
    Ok(())
}

fn collect_dir_metadata<S>(
    dir: &Path,
    map: &mut std::collections::HashMap<std::path::PathBuf, std::fs::Metadata, S>,
) -> io::Result<()>
where
    S: std::hash::BuildHasher,
{
    let entries = std::fs::read_dir(dir)?;
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            let meta = std::fs::metadata(&path)?;
            map.insert(path.clone(), meta);
            collect_dir_metadata(&path, map)?;
        }
    }
    Ok(())
}

/// Copy every xattr for which `should_copy` returns true.
fn copy_xattrs_with_policy(
    src: &Path,
    dst: &Path,
    should_copy: impl Fn(&str) -> bool,
) -> io::Result<()> {
    const AUDIT: &str = "delivery_metadata";
    let mut failures = Vec::new();
    match xattr::list(src) {
        Ok(iter) => {
            for name in iter {
                let Some(name_str) = name.to_str() else {
                    continue;
                };
                if !should_copy(name_str) {
                    continue;
                }
                match xattr::get(src, name_str) {
                    Ok(Some(value)) => {
                        if let Err(e) = xattr::set(dst, name_str, &value) {
                            crate::media_conversion_gate::delivery_metadata_path_audit(
                                AUDIT,
                                dst,
                                format!(
                                    "Metadata Audit: Failed to copy extended attribute \
                                     '{name_str}' to {dst_display}: {e}",
                                    dst_display = dst.display(),
                                ),
                            );
                            failures.push(format!(
                                "failed to copy xattr '{name_str}' from {} to {}: {e}",
                                src.display(),
                                dst.display()
                            ));
                        }
                    }
                    Ok(None) => {}
                    Err(e) => {
                        crate::media_conversion_gate::delivery_metadata_path_audit(
                            AUDIT,
                            src,
                            format!(
                                "Metadata Audit: Failed to read extended attribute '{name_str}' \
                                 from {src_display}: {e}",
                                src_display = src.display(),
                            ),
                        );
                        failures.push(format!(
                            "failed to read xattr '{name_str}' from {}: {e}",
                            src.display()
                        ));
                    }
                }
            }
        }
        Err(e) => {
            if delivery_policy::is_xattr_api_absence(&e) {
                crate::media_conversion_gate::delivery_metadata_batch_audit(
                    AUDIT,
                    crate::infra::static_logs::messages::MSG_METADATA_DELIVERY_SKIP_XATTR_ABSENCE
                        .replace("{}", &format!("{}: {e}", src.display())),
                );
                return Ok(());
            }
            crate::media_conversion_gate::delivery_metadata_path_audit(
                AUDIT,
                src,
                format!(
                    "Metadata Audit: Failed to list extended attributes for {src_display}: {e}",
                    src_display = src.display(),
                ),
            );
            failures.push(format!("failed to list xattrs for {}: {e}", src.display()));
        }
    }

    if failures.is_empty() {
        Ok(())
    } else {
        Err(io::Error::other(failures.join("; ")))
    }
}

fn copy_dir_xattrs(src: &Path, dst: &Path) -> io::Result<()> {
    copy_xattrs_with_policy(src, dst, should_preserve_xattr)
}

/// All non-skipped xattrs (Linux/Windows and macOS fallback).
fn copy_preservable_xattrs(src: &Path, dst: &Path) -> io::Result<()> {
    copy_xattrs_with_policy(src, dst, should_preserve_xattr)
}

/// macOS: `user.*` and other non-Spotlight xattrs after the dedicated network
/// pass.
#[cfg(target_os = "macos")]
fn copy_supplemental_xattrs(src: &Path, dst: &Path) -> io::Result<()> {
    copy_xattrs_with_policy(src, dst, |key| {
        should_preserve_xattr(key) && !should_copy_macos_extended_xattr(key)
    })
}

/// Fallback: try exiv2 to merge XMP into the destination (exiv2 -i expects
/// sidecar named \\<stem\\>.xmp beside image). Returns true if exiv2 merge
/// succeeded. No fake success; only when exiv2 actually succeeds do we return
/// true.
fn try_merge_xmp_exiv2(xmp_path: &Path, dst: &Path) -> bool {
    let Some(parent) = dst.parent() else {
        return false;
    };
    let Some(stem_raw) = dst.file_stem() else {
        return false;
    };
    let stem = stem_raw.to_string_lossy();
    let sidecar_for_exiv2 = parent.join(format!("{stem}.xmp"));
    if sidecar_for_exiv2 == *xmp_path {
        return false;
    }
    if let Err(e) = std::fs::copy(xmp_path, &sidecar_for_exiv2) {
        crate::media_conversion_gate::delivery_metadata_path_audit(
            "delivery_metadata_xmp",
            dst,
            format!(
                "Failed to prepare temporary XMP sidecar for exiv2 fallback (xmp={}, dst={}): {}",
                xmp_path.display(),
                dst.display(),
                e
            ),
        );
        return false;
    }
    let out = crate::tool_builders::Exiv2Builder::new()
        .arg("-ix")
        .input(dst)
        .build()
        .output();
    let ok = match &out {
        Ok(out) if out.status.success() => true,
        Ok(out) => {
            crate::media_conversion_gate::delivery_metadata_path_audit(
                "delivery_metadata_xmp",
                dst,
                format!(
                    "Metadata Audit: exiv2 returned non-zero exit code during XMP harvest for \
                     {dst_display}: {stderr}",
                    dst_display = dst.display(),
                    stderr = String::from_utf8_lossy(&out.stderr).trim(),
                ),
            );
            false
        }
        Err(err) => {
            crate::media_conversion_gate::delivery_metadata_path_audit(
                "delivery_metadata_xmp",
                dst,
                format!(
                    "Failed to launch exiv2 for XMP fallback (dst={}): {}",
                    dst.display(),
                    err
                ),
            );
            false
        }
    };
    crate::media_conversion_gate::delivery_remove_file_or_audit(
        "metadata_exiv2_sidecar",
        &sidecar_for_exiv2,
    );
    ok
}

fn merge_xmp_sidecar(src: &Path, dst: &Path) -> io::Result<bool> {
    let xmp_path = find_xmp_sidecar(src);

    if let Some(xmp) = xmp_path {
        let dst_meta = std::fs::metadata(dst).map_err(|e| {
            io::Error::new(
                e.kind(),
                format!(
                    "Failed to merge XMP sidecar {} into {}: destination is unavailable: {e}",
                    xmp.display(),
                    dst.display()
                ),
            )
        })?;
        if !dst_meta.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "Failed to merge XMP sidecar {} into {}: destination is not a regular file",
                    xmp.display(),
                    dst.display()
                ),
            ));
        }

        if crate::progress_mode::is_verbose_mode() {
            log_detail!(format!(
                "Metadata Audit: XMP sidecar or embedded block discovered ({label}) at {xmp_path}",
                label = crate::infra::static_logs::messages::LABEL_METADATA,
                xmp_path = xmp.display(),
            ));
        }

        let config = crate::xmp_merger::Config {
            delete_xmp_after_merge: false,
            overwrite_mode: crate::xmp_merger::OverwriteMode::Original,
            preserve_timestamps: true,
            log_level: crate::xmp_merger::LogLevel::Quiet,
        };

        let merger = crate::xmp_merger::XmpMerger::new(config);

        crate::progress_mode::xmp_merge_attempt();
        match merger.merge_xmp(&xmp, dst) {
            Ok(()) => {
                crate::progress_mode::xmp_merge_success();
                Ok(true)
            }
            Err(e) => {
                let err_str = e.to_string();
                let format_unsupported = err_str.to_lowercase().contains("format error in file");
                if format_unsupported {
                    crate::ui_stderr::line(
                        crate::modern_ui::symbols::WARNING,
                        crate::modern_ui::symbols::plain::WARNING,
                        "XMP merge skipped (ExifTool does not support writing to this file format)",
                    );
                } else {
                    crate::progress_mode::xmp_merge_failure(&err_str);
                }
                let fallback_ok = try_merge_xmp_exiv2(&xmp, dst);
                if fallback_ok {
                    crate::progress_mode::xmp_merge_success();
                    crate::log_info!(
                        crate::infra::static_logs::messages::LABEL_METADATA,
                        crate::infra::static_logs::messages::MSG_METADATA_XMP_FALLBACK_SUCCESS
                    );
                    Ok(true)
                } else if crate::progress_mode::has_log_file() && !format_unsupported {
                    crate::log_info!(
                        crate::infra::static_logs::messages::LABEL_METADATA,
                        crate::infra::static_logs::messages::MSG_METADATA_XMP_FALLBACK_FAIL
                    );
                    Err(io::Error::other(format!(
                        "Failed to merge XMP sidecar {} into {}: {err_str}; exiv2 fallback failed",
                        xmp.display(),
                        dst.display()
                    )))
                } else {
                    Err(io::Error::other(format!(
                        "Failed to merge XMP sidecar {} into {}: {err_str}; exiv2 fallback failed",
                        xmp.display(),
                        dst.display()
                    )))
                }
            }
        }
    } else {
        Ok(false)
    }
}

pub(crate) fn find_xmp_sidecar(src: &Path) -> Option<std::path::PathBuf> {
    if let Some(ext) = src.extension() {
        let ext_str = ext.to_str()?;
        let xmp_full = src.with_extension(format!("{ext_str}.xmp"));
        if let Some(path) = existing_path_with_exact_name(&xmp_full) {
            return Some(path);
        }
        let xmp_full_upper = src.with_extension(format!("{ext_str}.XMP"));
        if let Some(path) = existing_path_with_exact_name(&xmp_full_upper) {
            return Some(path);
        }
    }

    let xmp_stem = src.with_extension("xmp");
    if let Some(path) = existing_path_with_exact_name(&xmp_stem) {
        return Some(path);
    }
    let xmp_stem_upper = src.with_extension("XMP");
    if let Some(path) = existing_path_with_exact_name(&xmp_stem_upper) {
        return Some(path);
    }

    if let Some(parent) = src.parent()
        && let Some(src_stem_raw) = src.file_stem()
    {
        let src_stem = src_stem_raw.to_string_lossy().to_lowercase();
        let src_ext = src.extension().map(|e| e.to_string_lossy().to_lowercase());
        let src_compound = if let Some(ref ext) = src_ext
            && !ext.is_empty()
        {
            format!("{src_stem}.{ext}")
        } else {
            src_stem.clone()
        };

        match std::fs::read_dir(parent) {
            Ok(entries) => {
                for entry in entries {
                    let entry = match entry {
                        Ok(entry) => entry,
                        Err(err) => {
                            crate::media_conversion_gate::delivery_metadata_path_audit(
                                "delivery_metadata",
                                parent,
                                crate::infra::static_logs::messages::MSG_METADATA_RESTORE_INSPECT_FAIL
                                    .replacen("{}", &parent.display().to_string(), 1)
                                    .replacen("{}", &err.to_string(), 1),
                            );
                            continue;
                        }
                    };
                    let path = entry.path();

                    if !path.is_file() {
                        continue;
                    }

                    if !path
                        .extension()
                        .is_some_and(|e| e.to_string_lossy().eq_ignore_ascii_case("xmp"))
                    {
                        continue;
                    }

                    if let Some(xmp_stem_raw) = path.file_stem() {
                        let xmp_stem = xmp_stem_raw.to_string_lossy().to_lowercase();

                        if xmp_stem == src_stem || xmp_stem == src_compound {
                            return Some(path);
                        }
                    }
                }
            }
            Err(err) => {
                crate::media_conversion_gate::delivery_metadata_path_audit(
                    "delivery_metadata",
                    parent,
                    crate::infra::static_logs::messages::MSG_METADATA_RESTORE_INSPECT_FAIL
                        .replacen("{}", &parent.display().to_string(), 1)
                        .replacen("{}", &err.to_string(), 1),
                );
            }
        }
    }

    None
}

fn existing_path_with_exact_name(path: &Path) -> Option<std::path::PathBuf> {
    let file_name = path.file_name()?;
    let parent = path.parent()?;
    let mut entries = match std::fs::read_dir(parent) {
        Ok(entries) => entries,
        Err(err) => {
            eprintln!("[METADATA] read_dir failed ({}): {err}", parent.display());
            return None;
        }
    };
    entries.find_map(|entry| match entry {
        Ok(entry) if entry.file_name() == file_name => Some(entry.path()),
        Ok(_) => None,
        Err(err) => {
            eprintln!(
                "[METADATA] dir entry probe failed under {}: {err}",
                parent.display()
            );
            None
        }
    })
}

/// CONTRACT: ordered Pro metadata delivery layers (`preserve_pro`); timestamps
/// must stay last.
#[must_use]
#[cfg(test)]
pub(crate) const fn preserve_pro_delivery_layer_order() -> &'static [&'static str] {
    #[cfg(target_os = "macos")]
    {
        &[
            "native_copyfile",
            "exif_internal",
            "network_xattr",
            "supplemental_xattr",
            "spotlight_content_creation_date",
            "exact_copy_xattr_reapply",
            "unix_permissions",
            "timestamps",
        ]
    }
    #[cfg(not(target_os = "macos"))]
    {
        &[
            "exif_internal",
            "platform_attributes",
            "manual_xattr",
            "unix_permissions",
            "timestamps",
        ]
    }
}

#[cfg(test)]
mod metadata_preservation_contract {
    include!("../../tests/internal/metadata_preservation_contract.rs");
}
