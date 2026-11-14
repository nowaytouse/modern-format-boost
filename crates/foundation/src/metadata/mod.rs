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
use std::io::{self, BufReader, Read, Seek, SeekFrom, Write};
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
mod source_cleanup;
#[cfg(target_os = "windows")]
mod windows;
mod xmp_sidecar_index;

pub use delivery_policy::{
    MetadataDeliveryReport, MetadataLayerOutcome, apply_file_timestamps_for_delivery,
    preserve_filesystem_for_delivery, preserve_for_delivery,
};
pub use exif::preserve_internal;
pub use source_cleanup::SourceDirectoryCleanupSnapshot;

/// A phase-scoped namespace observation; file contents still require fresh proof.
pub struct XmpSidecarIndex {
    include_aae: bool,
    directories:
        std::collections::BTreeMap<PathBuf, std::sync::Mutex<xmp_sidecar_index::DirectoryView>>,
}

pub(crate) fn verify_output_embedded_metadata_with_index(
    src: &Path,
    dst: &Path,
    policy: MetadataOutputPolicy,
    index: &XmpSidecarIndex,
) -> io::Result<OutputMetadataAudit> {
    output_audit::verify_output_embedded_metadata_with_index(src, dst, policy, index)
}

pub(crate) fn verify_output_embedded_metadata_with_explicit_xmp(
    src: &Path,
    xmp: &Path,
    dst: &Path,
) -> io::Result<()> {
    output_audit::verify_output_embedded_metadata_with_explicit_xmp(src, xmp, dst)
}
pub use output_audit::{
    MetadataOutputPolicy, OutputMetadataAudit, embedded_metadata_is_clear,
    has_embedded_icc_profile, verify_output_embedded_metadata,
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
    let metadata = std::fs::symlink_metadata(path).map_err(|e| {
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
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
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
    apply_file_timestamps_inner(src, dst, None)
}

fn apply_file_timestamps_inner(
    src: &Path,
    dst: &Path,
    handles: Option<(&std::fs::File, &std::fs::File)>,
) -> io::Result<()> {
    log_detail!(format!(
        "Metadata Audit: Initiating timestamp synchronization flow from {src_path} -> {dst_path}",
        src_path = src.display(),
        dst_path = dst.display(),
    ));
    let m = handles.map_or_else(|| std::fs::metadata(src), |(source, _)| source.metadata());
    let m = m.map_err(|e| {
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
    let source_added_time = match handles.map_or_else(
        || macos::get_added_time(src),
        |(source, _)| macos::get_added_time_on_file(source),
    ) {
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

    #[cfg(target_os = "macos")]
    let set_creation_time = |time| {
        handles.map_or_else(
            || macos::set_creation_time(dst, time),
            |(_, target)| macos::set_creation_time_on_file(target, time),
        )
    };
    #[cfg(target_os = "macos")]
    let set_added_time = |time| {
        handles.map_or_else(
            || macos::set_added_time(dst, time),
            |(_, target)| macos::set_added_time_on_file(target, time),
        )
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
                if let Err(e) = set_creation_time(created) {
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
            if let Err(e) = set_added_time(added) {
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
    if let Err(e) = handles.map_or_else(
        || filetime::set_file_times(dst, atime, mtime),
        |(_, target)| filetime::set_file_handle_times(target, Some(atime), Some(mtime)),
    ) {
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
                if let Err(e) = set_creation_time(created) {
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
            && let Err(e) = set_added_time(added)
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
        let target_metadata =
            handles.map_or_else(|| std::fs::metadata(dst), |(_, target)| target.metadata());
        match (m.created(), target_metadata) {
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
            match handles.map_or_else(
                || macos::get_added_time(dst),
                |(_, target)| macos::get_added_time_on_file(target),
            ) {
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

        if let Err(e) = macos::copy_native_acl_and_stat(src, dst) {
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
            "Metadata Audit: filesystem metadata copy verified {} -> {} \
             (embedded tags and color profiles require separate checks)",
            src.display(),
            dst.display()
        );
        crate::infra::static_logs::log_metadata_audit_info(src, dst, None, None, &detail);
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
        Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "Exact metadata copy mismatch from {} to {}: {}",
                src.display(),
                dst.display(),
                mismatches.join("; ")
            ),
        ))
    }
}

fn record_exif_delivery_result(result: io::Result<()>, report: &mut MetadataDeliveryReport) {
    match result {
        Ok(()) => report.exif = MetadataLayerOutcome::Applied,
        Err(e) if delivery_policy::is_exiftool_unavailable(&e) => {
            crate::media_conversion_gate::delivery_metadata_batch_audit(
                "delivery_metadata_exif",
                format!("ExifTool unavailable; internal metadata skipped: {e}"),
            );
            report.exif = MetadataLayerOutcome::SkippedNoTool;
        }
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
        if let Err(e) = macos::copy_native_acl_and_stat(src, dst) {
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
/// merged, and `Err` when a sidecar exists but its safe merge path cannot
/// preserve it. JXL never falls back to a metadata writer that could rewrite
/// reconstruction-owned container bytes.
///
/// # Errors
/// Returns an error if a discovered XMP sidecar cannot be merged into `dst`.
pub fn merge_xmp_sidecar_into_dest(src: &Path, dst: &Path) -> io::Result<bool> {
    merge_xmp_sidecar(src, dst)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MetadataOwnership {
    ReconstructionOwned,
    OverlayOwned,
}

pub(crate) const XMP_OVERLAY_MAX_BYTES: u64 = 100 * 1024 * 1024;
const XMP_OVERLAY_MAX_DEPTH: usize = 256;
const XMP_OVERLAY_MAX_EVENTS: usize = 1_000_000;
const JXL_APPEND_MAX_BOXES: usize = 1_000_000;
const JXL_APPEND_MAX_XML_BOXES: usize = 64;

impl MetadataOwnership {
    const fn as_str(self) -> &'static str {
        match self {
            Self::ReconstructionOwned => "reconstruction-owned",
            Self::OverlayOwned => "overlay-owned",
        }
    }
}

#[derive(Clone, Copy)]
struct JxlBoxSpan {
    box_offset: u64,
    box_size: u64,
    payload_offset: u64,
    payload_size: u64,
}

struct AppendableJxlContainer {
    total_size: u64,
    last_xml: Option<JxlBoxSpan>,
    jbrd: Option<JxlBoxSpan>,
    #[cfg(feature = "jpegxl-ffi")]
    gain_map: Option<JxlBoxSpan>,
    xml_box_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct JxlFileIdentity {
    len: u64,
    modified: std::time::SystemTime,
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
    #[cfg(unix)]
    changed: (i64, i64),
}

fn jxl_file_identity(path: &Path) -> io::Result<JxlFileIdentity> {
    #[cfg(unix)]
    use std::os::unix::fs::MetadataExt;
    let metadata = std::fs::symlink_metadata(path)?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "JXL overlay target must be a regular non-symlink file: {}",
                path.display()
            ),
        ));
    }
    Ok(JxlFileIdentity {
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

fn blake3_file_hash(path: &Path) -> io::Result<String> {
    crate::common_utils::calculate_blake3_hash(path)
        .map_err(|error| io::Error::other(format!("failed to hash {}: {error}", path.display())))
}

fn blake3_open_file_hash(file: &mut std::fs::File) -> io::Result<String> {
    file.seek(SeekFrom::Start(0))?;
    let mut hasher = blake3::Hasher::new();
    let mut buffer = [0_u8; 16 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    file.seek(SeekFrom::Start(0))?;
    Ok(hasher.finalize().to_hex().to_string())
}

fn blake3_file_span(path: &Path, offset: u64, size: u64) -> io::Result<String> {
    let mut file = std::fs::File::open(path)?;
    file.seek(SeekFrom::Start(offset))?;
    let mut hasher = blake3::Hasher::new();
    let mut remaining = size;
    let mut buffer = [0_u8; 16 * 1024];
    while remaining > 0 {
        let chunk = usize::try_from(remaining.min(buffer.len() as u64))
            .map_err(|_| io::Error::other("JXL audit span size overflow"))?;
        file.read_exact(&mut buffer[..chunk])?;
        hasher.update(&buffer[..chunk]);
        remaining -= chunk as u64;
    }
    Ok(hasher.finalize().to_hex().to_string())
}

fn jxl_box_hash(path: &Path, span: Option<JxlBoxSpan>) -> io::Result<Option<String>> {
    span.map(|span| blake3_file_span(path, span.box_offset, span.box_size))
        .transpose()
}

fn format_overlay_id(hash: &str) -> io::Result<String> {
    if hash.len() < 32 || !hash.as_bytes()[..32].iter().all(u8::is_ascii_hexdigit) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "overlay audit hash cannot form a UUID",
        ));
    }
    Ok(format!(
        "{}-{}-{}-{}-{}",
        &hash[0..8],
        &hash[8..12],
        &hash[12..16],
        &hash[16..20],
        &hash[20..32]
    ))
}

fn djxl_version_for_overlay_audit() -> String {
    static VERSION: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    let probe = || {
        let path = crate::common_utils::resolve_tool_path_or_audit(crate::constants::TOOL_DJXL);
        let mut command = std::process::Command::new(path);
        command.arg("--version");
        let Ok(output) = crate::process_runner::run_command_with_liveness_timeout(
            &mut command,
            std::time::Duration::from_secs(5),
            std::time::Duration::from_secs(15),
            "JXL XMP overlay djxl version probe",
        ) else {
            return "unavailable".to_string();
        };
        if !output.status.success() {
            return "unavailable".to_string();
        }
        let diagnostic = if output.stdout.is_empty() {
            output.stderr
        } else {
            output.stdout
        };
        String::from_utf8_lossy(&diagnostic)
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .map_or_else(
                || "unavailable".to_string(),
                |line| line.chars().take(256).collect(),
            )
    };
    if crate::common_utils::verification_cache_disabled() {
        probe()
    } else {
        VERSION.get_or_init(probe).clone()
    }
}

fn validate_appendable_jxl_container(path: &Path) -> io::Result<AppendableJxlContainer> {
    let total_size = std::fs::metadata(path)?.len();
    let mut file = std::fs::File::open(path)?;
    let mut magic = [0_u8; 12];
    file.read_exact(&mut magic)?;
    if magic != *crate::constants::JXL_CONTAINER_MAGIC {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "refusing to append XMP to non-container JXL {}",
                path.display()
            ),
        ));
    }

    let mut offset = 0_u64;
    let mut last_xml = None;
    let mut jbrd = None;
    let mut gain_map = None;
    let mut box_count = 0_usize;
    let mut xml_box_count = 0_usize;
    while offset < total_size {
        box_count = box_count
            .checked_add(1)
            .ok_or_else(|| io::Error::other("JXL box counter overflow"))?;
        if box_count > JXL_APPEND_MAX_BOXES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "JXL box count exceeds the append safety limit: {}",
                    path.display()
                ),
            ));
        }
        if total_size - offset < 8 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("truncated JXL box header in {}", path.display()),
            ));
        }
        file.seek(SeekFrom::Start(offset))?;
        let mut header = [0_u8; 8];
        file.read_exact(&mut header)?;
        let size32 = u32::from_be_bytes([header[0], header[1], header[2], header[3]]);
        let (box_size, header_size) = match size32 {
            0 => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "JXL {} ends with a size-zero box that cannot be safely extended",
                        path.display()
                    ),
                ));
            }
            1 => {
                let mut extended = [0_u8; 8];
                file.read_exact(&mut extended)?;
                (u64::from_be_bytes(extended), 16_u64)
            }
            size => (u64::from(size), 8_u64),
        };
        if box_size < header_size || box_size > total_size - offset {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("invalid JXL box boundary in {}", path.display()),
            ));
        }
        let span = JxlBoxSpan {
            box_offset: offset,
            box_size,
            payload_offset: offset + header_size,
            payload_size: box_size - header_size,
        };
        if header[4..8] == *b"xml " {
            xml_box_count = xml_box_count
                .checked_add(1)
                .ok_or_else(|| io::Error::other("JXL XML box counter overflow"))?;
            last_xml = Some(span);
        } else if header[4..8] == *b"jbrd" && jbrd.replace(span).is_some() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "JXL contains multiple top-level jbrd boxes: {}",
                    path.display()
                ),
            ));
        } else if header[4..8] == *b"jhgm" && gain_map.replace(span).is_some() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "duplicate JXL gain-map box",
            ));
        }
        offset = offset
            .checked_add(box_size)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "JXL box offset overflow"))?;
    }
    Ok(AppendableJxlContainer {
        total_size,
        last_xml,
        jbrd,
        #[cfg(feature = "jpegxl-ffi")]
        gain_map,
        xml_box_count,
    })
}

#[cfg(feature = "jpegxl-ffi")]
pub(crate) fn read_gain_map_from_jxl(path: &Path) -> io::Result<Vec<u8>> {
    let container = validate_appendable_jxl_container(path)?;
    let span = container
        .gain_map
        .ok_or_else(|| io::Error::other("JXL native gain map is missing"))?;
    if span.payload_size > 256 * 1024 * 1024 {
        return Err(io::Error::other(
            "JXL gain map exceeds 256 MiB safety limit",
        ));
    }
    let mut file = std::fs::File::open(path)?;
    file.seek(SeekFrom::Start(span.payload_offset))?;
    let mut bytes = vec![0; usize::try_from(span.payload_size).map_err(io::Error::other)?];
    file.read_exact(&mut bytes)?;
    Ok(bytes)
}

#[cfg(feature = "jpegxl-ffi")]
pub(crate) fn append_gain_map_to_jxl(dst: &Path, bundle: &[u8]) -> io::Result<()> {
    let identity = jxl_file_identity(dst)?;
    let container = validate_appendable_jxl_container(dst)?;
    if container.gain_map.is_some() {
        return if read_gain_map_from_jxl(dst)? == bundle {
            Ok(())
        } else {
            Err(io::Error::other(
                "refusing to replace an existing JXL gain map",
            ))
        };
    }
    let size = u32::try_from(bundle.len())
        .map_err(io::Error::other)?
        .checked_add(8)
        .ok_or_else(|| io::Error::other("JXL gain-map box length overflow"))?;
    let hash = blake3_file_hash(dst)?;
    let parent = crate::media_conversion_gate::output_parent_or_dot(dst);
    let mut staged = crate::media_conversion_gate::delivery_named_tempfile_in_parent_or_err(
        "jxl_gain_map",
        parent,
        ".mfb-jhgm-",
        ".tmp",
    )?;
    if io::copy(&mut std::fs::File::open(dst)?, staged.as_file_mut())? != container.total_size {
        return Err(io::Error::other("JXL changed while staging its gain map"));
    }
    staged.write_all(&size.to_be_bytes())?;
    staged.write_all(b"jhgm")?;
    staged.write_all(bundle)?;
    staged.flush()?;
    staged.as_file().sync_all()?;
    if blake3_file_span(staged.path(), 0, container.total_size)? != hash
        || read_gain_map_from_jxl(staged.path())? != bundle
        || jxl_file_identity(dst)? != identity
        || blake3_file_hash(dst)? != hash
    {
        return Err(io::Error::other(
            "JXL gain-map append did not preserve its original container",
        ));
    }
    staged.persist(dst).map_err(|error| error.error)?;
    crate::io_utils::sync_committed_file_and_parent(dst)
}

fn jxl_last_xml_payload_matches(
    path: &Path,
    container: &AppendableJxlContainer,
    xmp: &mut std::fs::File,
    payload_size: u64,
) -> io::Result<bool> {
    let Some(last_xml) = container.last_xml else {
        return Ok(false);
    };
    if last_xml.payload_size != payload_size {
        return Ok(false);
    }

    let mut jxl = std::fs::File::open(path)?;
    jxl.seek(SeekFrom::Start(last_xml.payload_offset))?;
    xmp.seek(SeekFrom::Start(0))?;
    let mut remaining = payload_size;
    let mut jxl_buffer = [0_u8; 16 * 1024];
    let mut xmp_buffer = [0_u8; 16 * 1024];
    while remaining > 0 {
        let chunk = usize::try_from(remaining.min(jxl_buffer.len() as u64))
            .map_err(|_| io::Error::other("XMP comparison size overflow"))?;
        jxl.read_exact(&mut jxl_buffer[..chunk])?;
        xmp.read_exact(&mut xmp_buffer[..chunk])?;
        if jxl_buffer[..chunk] != xmp_buffer[..chunk] {
            xmp.seek(SeekFrom::Start(0))?;
            return Ok(false);
        }
        remaining -= chunk as u64;
    }
    xmp.seek(SeekFrom::Start(0))?;
    Ok(true)
}

pub(crate) fn validate_xmp_document(file: &mut std::fs::File, path: &Path) -> io::Result<u64> {
    validate_xmp_document_with(file, path, false)
}

/// Check extra XML syntax for the read-only preflight without changing archive admission.
pub(crate) fn validate_xmp_preview_document(
    file: &mut std::fs::File,
    path: &Path,
) -> io::Result<u64> {
    validate_xmp_document_with(file, path, true)
}

fn validate_xmp_preview_event(
    event: &quick_xml::events::Event<'_>,
    depth: usize,
) -> Result<(), &'static str> {
    use quick_xml::events::Event;
    match event {
        Event::Start(element) | Event::Empty(element) => {
            for attribute in element.attributes() {
                let attribute = attribute.map_err(|_| "invalid or duplicate XML attribute")?;
                if attribute.value.contains('<')
                    || quick_xml::escape::unescape(&attribute.value).is_err()
                {
                    return Err("invalid XML attribute value");
                }
            }
        }
        Event::Text(text) => {
            if (depth == 0
                && !text
                    .bytes()
                    .all(|byte| matches!(byte, b' ' | b'\t' | b'\r' | b'\n')))
                || text.contains("]]>")
            {
                return Err("invalid XML character data");
            }
        }
        Event::CData(_) if depth == 0 => return Err("CDATA outside the XML root"),
        Event::GeneralRef(reference) => {
            if depth == 0
                || !(matches!(reference.resolve_char_ref(), Ok(Some(_)))
                    || quick_xml::escape::resolve_xml_entity(reference.as_ref()).is_some())
            {
                return Err("invalid or undefined XML entity reference");
            }
        }
        _ => {}
    }
    Ok(())
}

fn validate_xmp_document_with(
    file: &mut std::fs::File,
    path: &Path,
    preview: bool,
) -> io::Result<u64> {
    let payload_size = file.metadata()?.len();
    if payload_size == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("refusing empty XMP sidecar {}", path.display()),
        ));
    }
    if payload_size > XMP_OVERLAY_MAX_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::FileTooLarge,
            format!(
                "XMP sidecar exceeds the {}-byte archive safety limit: {}",
                XMP_OVERLAY_MAX_BYTES,
                path.display()
            ),
        ));
    }

    let mut reader =
        quick_xml::Reader::from_reader(BufReader::new((&mut *file).take(payload_size)));
    reader.config_mut().check_comments = preview;
    let mut buffer = Vec::new();
    let mut depth = 0_usize;
    let mut roots = 0_usize;
    let mut event_count = 0_usize;
    loop {
        let event = reader.read_event_into(&mut buffer);
        if preview && let Ok(event) = &event {
            validate_xmp_preview_event(event, depth).map_err(|reason| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("{}: {reason}", path.display()),
                )
            })?;
        }
        match event {
            Ok(quick_xml::events::Event::Start(_)) => {
                if depth >= XMP_OVERLAY_MAX_DEPTH {
                    return Err(io::Error::new(
                        io::ErrorKind::FileTooLarge,
                        format!("XMP nesting exceeds the safety limit: {}", path.display()),
                    ));
                }
                if depth == 0 {
                    roots += 1;
                }
                depth += 1;
            }
            Ok(quick_xml::events::Event::Empty(_)) if depth == 0 => roots += 1,
            Ok(quick_xml::events::Event::DocType(_)) => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "XMP document type declarations are forbidden: {}",
                        path.display()
                    ),
                ));
            }
            Ok(quick_xml::events::Event::End(_)) => {
                depth = depth.checked_sub(1).ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("unbalanced XMP document {}", path.display()),
                    )
                })?;
            }
            Ok(quick_xml::events::Event::Eof) => break,
            Ok(_) => {}
            Err(quick_xml::Error::Io(error)) => {
                return Err(io::Error::new(error.kind(), error));
            }
            Err(error) => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("invalid XMP sidecar {}: {error}", path.display()),
                ));
            }
        }
        event_count = event_count
            .checked_add(1)
            .ok_or_else(|| io::Error::other("XMP event counter overflow"))?;
        if event_count > XMP_OVERLAY_MAX_EVENTS {
            return Err(io::Error::new(
                io::ErrorKind::FileTooLarge,
                format!(
                    "XMP event count exceeds the safety limit: {}",
                    path.display()
                ),
            ));
        }
        buffer.clear();
    }
    let unread = reader.into_inner().into_inner().limit();
    if unread != 0 || file.metadata()?.len() != payload_size {
        return Err(io::Error::other(format!(
            "XMP sidecar changed while validating XML: {}",
            path.display()
        )));
    }
    if roots != 1 || depth != 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "XMP sidecar {} is not one complete XML document",
                path.display()
            ),
        ));
    }
    file.seek(SeekFrom::Start(0))?;
    Ok(payload_size)
}

/// Validate that a sidecar is one non-empty, balanced XML document.
///
/// # Errors
/// Returns an error for non-files, empty payloads, malformed XML, or I/O
/// failures.
pub fn validate_xmp_sidecar(path: &Path) -> io::Result<()> {
    let metadata = std::fs::symlink_metadata(path)?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "XMP sidecar is not a regular non-symlink file: {}",
                path.display()
            ),
        ));
    }
    let mut file = std::fs::File::open(path)?;
    validate_xmp_document(&mut file, path).map(|_| ())
}

/// Prove that deleting this sidecar cannot discard a newer JXL XMP edit.
/// The final overlay must contain the complete current sidecar, including
/// properties that a metadata reader does not recognize.
pub(crate) fn verify_jxl_xmp_sidecar_custody(xmp_path: &Path, dst: &Path) -> io::Result<()> {
    let xmp_path = require_regular_sidecar(xmp_path.to_path_buf(), "XMP")?;
    let container = validate_appendable_jxl_container(dst)?;
    let mut xmp = std::fs::File::open(&xmp_path)?;
    let size = validate_xmp_document(&mut xmp, &xmp_path)?;
    if !jxl_last_xml_payload_matches(dst, &container, &mut xmp, size)? {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "current XMP sidecar is not preserved in the final JXL overlay; source and sidecar must be retained",
        ));
    }
    Ok(())
}

/// Recheck a recorded final JXL and the complete last XMP packet when the
/// original sidecar is no longer present. This is not permission to delete media.
pub(crate) fn verify_jxl_xmp_digest_custody(
    dst: &Path,
    expected_output: &str,
    expected_packet: &str,
) -> io::Result<()> {
    let before = jxl_file_identity(dst)?;
    let container = validate_appendable_jxl_container(dst)?;
    let packet = container.last_xml.ok_or_else(|| {
        io::Error::other("recorded JXL has no final XMP overlay; sidecar custody is unknown")
    })?;
    if blake3_file_span(dst, packet.payload_offset, packet.payload_size)? != expected_packet
        || blake3_file_hash(dst)? != expected_output
        || jxl_file_identity(dst)? != before
    {
        return Err(io::Error::other(
            "recorded JXL/XMP digest changed; sidecar custody is unknown",
        ));
    }
    Ok(())
}

/// Append one authoritative XMP overlay to a JXL container without rewriting
/// existing JBRD, Exif, ICC, XMP, JUMBF, unknown, or codestream bytes.
///
/// Returns `Ok(false)` when the same overlay is already the last `xml ` box.
pub(crate) fn append_xmp_overlay_to_jxl(xmp_path: &Path, dst: &Path) -> io::Result<bool> {
    let xmp_path = require_regular_sidecar(xmp_path.to_path_buf(), "XMP")?;
    let source_identity = jxl_file_identity(dst)?;
    let container = validate_appendable_jxl_container(dst)?;
    let mut xmp = std::fs::File::open(&xmp_path)?;
    let payload_size = validate_xmp_document(&mut xmp, &xmp_path)?;
    if jxl_last_xml_payload_matches(dst, &container, &mut xmp, payload_size)? {
        return Ok(false);
    }
    if container.xml_box_count >= JXL_APPEND_MAX_XML_BOXES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "JXL already contains {} XML boxes; refusing unbounded overlay growth: {}",
                container.xml_box_count,
                dst.display()
            ),
        ));
    }
    let original_container_hash = blake3_file_hash(dst)?;
    let original_jbrd_hash = jxl_box_hash(dst, container.jbrd)?;
    let original_xmp_hash = container
        .last_xml
        .map(|span| blake3_file_span(dst, span.payload_offset, span.payload_size))
        .transpose()?;
    // Hash the already-open descriptor rather than reopening the path. If a
    // sidecar is concurrently replaced, the audit hash still describes the
    // exact bytes copied into the committed overlay.
    let overlay_hash = blake3_open_file_hash(&mut xmp)?;

    // JBRD can reference the exact bytes of an earlier `xml `/`brob` box. Keep
    // every existing box frozen and add the effective XMP as a later overlay;
    // reconstructible callers still prove the recovered JPEG after this write.
    let original_size = container.total_size;
    let standard_size = payload_size.checked_add(8);
    let header_size = if standard_size.is_some_and(|size| u32::try_from(size).is_ok()) {
        8_u64
    } else {
        16_u64
    };
    let box_size = payload_size
        .checked_add(header_size)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "JXL XMP box size overflow"))?;

    let parent = crate::media_conversion_gate::output_parent_or_dot(dst);
    let mut staged = crate::media_conversion_gate::delivery_named_tempfile_in_parent_or_err(
        "jbrd_xmp_box",
        parent,
        ".mfb-jbrd-xmp-",
        ".tmp",
    )?;
    let copied = io::copy(&mut std::fs::File::open(dst)?, staged.as_file_mut())?;
    if copied != original_size {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "JXL copy length mismatch while appending XMP to {}: expected={original_size} actual={copied}",
                dst.display()
            ),
        ));
    }
    if header_size == 8 {
        staged.as_file_mut().write_all(
            &u32::try_from(box_size)
                .map_err(|_| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        "JXL XMP box exceeds 32-bit size",
                    )
                })?
                .to_be_bytes(),
        )?;
        staged.as_file_mut().write_all(b"xml ")?;
    } else {
        staged.as_file_mut().write_all(&1_u32.to_be_bytes())?;
        staged.as_file_mut().write_all(b"xml ")?;
        staged.as_file_mut().write_all(&box_size.to_be_bytes())?;
    }
    let xmp_copied = io::copy(&mut xmp.take(payload_size), staged.as_file_mut())?;
    if xmp_copied != payload_size {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            format!(
                "XMP sidecar changed while appending {}: expected={payload_size} actual={xmp_copied}",
                xmp_path.display()
            ),
        ));
    }
    staged.as_file_mut().flush()?;
    staged.as_file().sync_all()?;
    let expected_size = original_size
        .checked_add(box_size)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "JXL output size overflow"))?;
    if staged.as_file().metadata()?.len() != expected_size {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("JXL XMP append size mismatch for {}", dst.display()),
        ));
    }
    let staged_path = staged.path();
    let committed = validate_appendable_jxl_container(staged_path)?;
    let committed_prefix_hash = blake3_file_span(staged_path, 0, original_size)?;
    if committed_prefix_hash != original_container_hash {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "JXL reconstruction-owned prefix changed while appending XMP to {}",
                dst.display()
            ),
        ));
    }
    let final_jbrd_hash = jxl_box_hash(staged_path, committed.jbrd)?;
    if final_jbrd_hash != original_jbrd_hash {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "JXL jbrd bytes changed while appending XMP to {}",
                dst.display()
            ),
        ));
    }
    let committed_overlay = committed.last_xml.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "JXL staged overlay is missing its final XML box: {}",
                dst.display()
            ),
        )
    })?;
    let committed_overlay_hash = blake3_file_span(
        staged_path,
        committed_overlay.payload_offset,
        committed_overlay.payload_size,
    )?;
    if committed_overlay_hash != overlay_hash {
        return Err(io::Error::other(format!(
            "XMP changed concurrently while staging its JXL overlay; original retained: {}",
            xmp_path.display()
        )));
    }
    let final_container_hash = blake3_file_hash(staged_path)?;
    let live_identity = jxl_file_identity(dst)?;
    let live_hash = blake3_file_hash(dst)?;
    if live_identity != source_identity || live_hash != original_container_hash {
        return Err(io::Error::other(format!(
            "JXL changed concurrently while staging XMP overlay; original retained: {}",
            dst.display()
        )));
    }
    staged.persist(dst).map_err(|error| error.error)?;
    crate::io_utils::sync_committed_file_and_parent(dst)?;
    let overlay_chain_hash = blake3::hash(
        format!(
            "v1:{original_container_hash}:{}:{overlay_hash}:{final_container_hash}",
            original_xmp_hash.as_deref().unwrap_or("none")
        )
        .as_bytes(),
    )
    .to_hex()
    .to_string();
    let overlay_id = format_overlay_id(&overlay_chain_hash)?;
    let created_unix_seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|error| io::Error::other(format!("system clock precedes Unix epoch: {error}")))?
        .as_secs();
    tracing::info!(
        target: "jxl_xmp_audit",
        event_id = "MFB-JXL-001",
        overlay_id = %overlay_id,
        overlay_schema = 1_u8,
        reconstruction_ownership = MetadataOwnership::ReconstructionOwned.as_str(),
        overlay_ownership = MetadataOwnership::OverlayOwned.as_str(),
        original_container_blake3 = %original_container_hash,
        original_jbrd_blake3 = original_jbrd_hash.as_deref().unwrap_or("none"),
        original_xmp_blake3 = original_xmp_hash.as_deref().unwrap_or("none"),
        overlay_blake3 = %overlay_hash,
        final_container_blake3 = %final_container_hash,
        mfb_version = env!("CARGO_PKG_VERSION"),
        djxl_version = djxl_version_for_overlay_audit(),
        created_unix_seconds,
        path = %dst.display(),
        "JXL XMP overlay atomically committed; all pre-existing container bytes remain frozen"
    );
    Ok(true)
}

/// Merge an adjacent XMP sidecar into a JPEG-reconstructible JXL while keeping
/// the reconstruction-owned metadata layer byte-for-byte frozen.
pub(crate) fn merge_xmp_sidecar_into_reconstructible_jxl(
    src: &Path,
    dst: &Path,
) -> io::Result<bool> {
    let Some(xmp_path) = find_xmp_sidecar(src)? else {
        return Ok(false);
    };
    append_xmp_overlay_to_jxl(&xmp_path, dst)?;
    Ok(true)
}

/// Record the proof that closes a previously committed JXL XMP overlay.
///
/// The final-container hash links this event to `MFB-JXL-001`. Exact JPEG
/// reconstruction callers also supply the byte-identical JPEG hash; healthy
/// pixel-only JXL callers record the explicit non-JBRD classification instead.
pub(crate) fn audit_jxl_overlay_reconstruction_proof(
    dst: &Path,
    reconstructed_jpeg_hash: Option<&str>,
) -> io::Result<()> {
    let container = validate_appendable_jxl_container(dst)?;
    let final_container_hash = blake3_file_hash(dst)?;
    let final_jbrd_hash = jxl_box_hash(dst, container.jbrd)?;
    let proof = if reconstructed_jpeg_hash.is_some() {
        "exact-jpeg-reconstruction"
    } else {
        "non-jbrd-classification-preserved"
    };
    tracing::info!(
        target: "jxl_xmp_audit",
        event_id = "MFB-JXL-002",
        proof,
        final_container_blake3 = %final_container_hash,
        final_jbrd_blake3 = final_jbrd_hash.as_deref().unwrap_or("none"),
        reconstructed_jpeg_blake3 = reconstructed_jpeg_hash.unwrap_or("none"),
        mfb_version = env!("CARGO_PKG_VERSION"),
        djxl_version = djxl_version_for_overlay_audit(),
        path = %dst.display(),
        "JXL XMP overlay proof closed after reconstruction-state verification"
    );
    Ok(())
}

fn require_regular_sidecar(path: PathBuf, label: &str) -> io::Result<PathBuf> {
    let metadata = std::fs::symlink_metadata(&path).map_err(|e| {
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
    if !metadata.is_file() || metadata.file_type().is_symlink() {
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
/// A complete case-insensitive directory scan catches mixed-case imports.
/// More than one candidate is ambiguous, never an instruction to choose one.
///
/// # Errors
/// Returns an error for ambiguous/non-regular matches or incomplete discovery.
pub fn find_aae_sidecar(src: &Path) -> io::Result<Option<PathBuf>> {
    let parent = crate::media_conversion_gate::path_parent_or_dot(src);
    let stem_lower = aae_source_stem(src)?;
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
        if aae_candidate_key(&path).is_some_and(|candidate| candidate == stem_lower) {
            matches.push(path);
        }
    }
    matches.sort_by_key(|path| path.file_name().map(std::ffi::OsStr::to_ascii_lowercase));
    if matches.len() > 1 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "ambiguous AAE sidecars for {}; originals retained",
                src.display()
            ),
        ));
    }
    if let Some(path) = matches.into_iter().next() {
        require_regular_sidecar(path, "AAE").map(Some)
    } else {
        Ok(None)
    }
}

fn aae_source_stem(src: &Path) -> io::Result<String> {
    src.file_stem()
        .and_then(|value| value.to_str())
        .map(str::to_lowercase)
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "AAE source has no supported file stem",
            )
        })
}

fn aae_candidate_key(path: &Path) -> Option<String> {
    path.extension()
        .filter(|ext| ext.eq_ignore_ascii_case("aae"))
        .and_then(|_| path.file_stem())
        .and_then(|stem| stem.to_str())
        .map(str::to_lowercase)
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

/// Verify complete, current AAE bytes without following either file's leaf link.
/// # Errors
/// Rejects unreadable, nonregular, changed, or unequal companions.
pub fn verify_aae_sidecar_copy(source: &Path, destination: &Path) -> io::Result<()> {
    use crate::pipeline::verification::FinalInputObservation;
    let source = FinalInputObservation::capture(source)?;
    let destination = FinalInputObservation::capture(destination)?;
    if source.blake3() != destination.blake3() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "AAE companion bytes differ",
        ));
    }
    source.verify_unchanged()?;
    destination.verify_unchanged()
}

fn regular_files_have_same_contents(left: &Path, right: &Path) -> io::Result<bool> {
    let left_metadata = std::fs::symlink_metadata(left)?;
    let right_metadata = std::fs::symlink_metadata(right)?;
    if !left_metadata.is_file()
        || left_metadata.file_type().is_symlink()
        || !right_metadata.is_file()
        || right_metadata.file_type().is_symlink()
        || left_metadata.len() != right_metadata.len()
    {
        return Ok(false);
    }

    let mut left = BufReader::new(std::fs::File::open(left)?);
    let mut right = BufReader::new(std::fs::File::open(right)?);
    let mut left_buffer = [0_u8; 16 * 1024];
    let mut right_buffer = [0_u8; 16 * 1024];
    loop {
        let left_read = left.read(&mut left_buffer)?;
        let right_read = right.read(&mut right_buffer)?;
        if left_read != right_read {
            return Ok(false);
        }
        if left_buffer[..left_read] != right_buffer[..left_read] {
            return Ok(false);
        }
        if left_read == 0 {
            return Ok(true);
        }
    }
}

/// Copy an Apple AAE sidecar beside a converted asset.
///
/// # Errors
/// Returns an error when sidecar discovery, copy, or metadata reapplication
/// fails.
pub fn handle_aae_sidecar(input: &Path, output: &Path) -> io::Result<AaeSidecarAction> {
    let Some(aae) = find_aae_sidecar(input)? else {
        return Ok(AaeSidecarAction::Missing);
    };
    {
        let destination = aae_sidecar_destination(&aae, output)?;
        if destination == aae {
            return Ok(AaeSidecarAction::AlreadyAdjacent {
                source: aae,
                destination,
            });
        }
        match std::fs::symlink_metadata(&destination) {
            Ok(_) if regular_files_have_same_contents(&aae, &destination)? => {
                return Ok(AaeSidecarAction::AlreadyAdjacent {
                    source: aae,
                    destination,
                });
            }
            Ok(_) => {
                crate::media_conversion_gate::delivery_api_path_fallback_audit(
                    "aae_migrate_collision",
                    &destination,
                    format!(
                        "refusing to overwrite existing AAE sidecar with {}",
                        aae.display()
                    ),
                );
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    format!(
                        "AAE destination already exists with different content: {}",
                        destination.display()
                    ),
                ));
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => {
                return Err(io::Error::new(
                    e.kind(),
                    format!(
                        "failed to inspect AAE destination {}: {e}",
                        destination.display()
                    ),
                ));
            }
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
        let parent = crate::media_conversion_gate::path_parent_or_dot(&destination);
        let staged = crate::media_conversion_gate::delivery_named_tempfile_in_parent_or_err(
            "aae_sidecar",
            parent,
            ".mfb-aae-",
            ".tmp",
        )?;
        std::fs::copy(&aae, staged.path()).map_err(|e| {
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
        apply_file_timestamps(&aae, staged.path()).map_err(|e| {
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
        if let Err(e) = copy_preservable_xattrs(&aae, staged.path()) {
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
        if let Err(error) = staged.persist_noclobber(&destination) {
            if error.error.kind() == io::ErrorKind::AlreadyExists
                && regular_files_have_same_contents(&aae, &destination)?
            {
                return Ok(AaeSidecarAction::AlreadyAdjacent {
                    source: aae,
                    destination,
                });
            }
            crate::media_conversion_gate::delivery_api_path_fallback_audit(
                "aae_migrate_failed",
                &destination,
                format!(
                    "failed to publish AAE sidecar without overwrite: {}",
                    error.error
                ),
            );
            return Err(io::Error::new(
                error.error.kind(),
                format!(
                    "failed to publish AAE sidecar {} to {} without overwrite: {}",
                    aae.display(),
                    destination.display(),
                    error.error
                ),
            ));
        }
        Ok(AaeSidecarAction::Copied {
            source: aae,
            destination,
        })
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
    let meta = require_existing_directory(src_dir, "source")?;
    dir_metadata.insert(src_dir.to_path_buf(), meta);

    collect_dir_metadata(src_dir, &mut dir_metadata)?;

    let mut directories = dir_metadata.into_iter().collect::<Vec<_>>();
    directories.sort_by_key(|(path, _)| path.components().count());
    // Creating children changes parent times; finish the tree before applying any metadata.
    for (src_path, _) in &directories {
        let relative = src_path.strip_prefix(src_dir).map_err(io::Error::other)?;
        let target = dst_dir.join(relative);
        match std::fs::symlink_metadata(&target) {
            Ok(_) => {
                require_existing_directory(&target, "destination mirror")?;
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    for (src_path, _) in &directories {
        let relative = src_path.strip_prefix(src_dir).map_err(io::Error::other)?;
        let target = dst_dir.join(relative);
        std::fs::create_dir_all(&target)?;
        require_existing_directory(&target, "destination mirror")?;
    }

    apply_existing_directory_metadata(&directories, src_dir, dst_dir)
}

fn apply_existing_directory_metadata(
    directories: &[(PathBuf, std::fs::Metadata)],
    src_dir: &Path,
    dst_dir: &Path,
) -> io::Result<()> {
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    #[cfg(unix)]
    let destination_root = crate::io_utils::open_directory_nofollow(dst_dir)?;
    let mut failures = Vec::new();
    for (src_path, metadata) in directories.iter().rev() {
        let rel_path = src_path.strip_prefix(src_dir).map_err(io::Error::other)?;
        let dst_path = dst_dir.join(rel_path);
        require_existing_directory(src_path, "source mirror")?;
        let observed = require_existing_directory(&dst_path, "destination mirror")?;
        #[cfg(unix)]
        let destination = {
            let file =
                crate::io_utils::open_descendant_nofollow(&destination_root, rel_path, true)?;
            crate::io_utils::check_open_file_identity(&observed, &file.metadata()?)?;
            file
        };
        #[cfg(unix)]
        let prior_permissions = observed.permissions();
        #[cfg(unix)]
        let failure_count = failures.len();
        #[cfg(unix)]
        let retry_write =
            prior_permissions.mode() & 0o200 == 0 && metadata.permissions().mode() & 0o200 != 0;
        // A previous delivery may have mirrored the source's restrictive mode.
        #[cfg(unix)]
        if retry_write {
            destination.set_permissions(std::fs::Permissions::from_mode(
                prior_permissions.mode() | 0o200,
            ))?;
        }
        #[cfg(unix)]
        let copied = copy_dir_xattrs_to_handle(src_path, &dst_path, &destination);
        #[cfg(not(unix))]
        let copied = copy_xattrs_with_policy(src_path, &dst_path, should_preserve_xattr);
        if let Err(error) = copied {
            failures.push(format!(
                "failed to preserve directory xattrs on {}: {error}",
                dst_path.display()
            ));
        }

        #[cfg(unix)]
        {
            if let Err(e) = destination.set_permissions(metadata.permissions()) {
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

        // Copy restrictive ACLs last so they cannot block the preceding attribute restoration.
        #[cfg(target_os = "macos")]
        if let Err(error) = macos::copy_directory_acl(src_path, &dst_path) {
            failures.push(format!(
                "failed to preserve directory ACL on {}: {error}",
                dst_path.display()
            ));
        }
        if let Err(e) = verify_exact_metadata_copy(src_path, &dst_path) {
            failures.push(format!(
                "failed to verify directory metadata from {} to {}: {e}",
                src_path.display(),
                dst_path.display()
            ));
        }
        let verify = (|| -> io::Result<()> {
            let actual = require_existing_directory(&dst_path, "verified destination mirror")?;
            #[cfg(unix)]
            crate::io_utils::check_open_file_identity(&observed, &actual)?;
            if filetime::FileTime::from_last_access_time(&actual) != atime
                || filetime::FileTime::from_last_modification_time(&actual) != mtime
            {
                failures.push(format!(
                    "directory timestamps differ after preservation: {}",
                    dst_path.display()
                ));
            }
            #[cfg(target_os = "macos")]
            {
                if let Err(error) = macos::verify_directory_acl(src_path, &dst_path) {
                    failures.push(format!(
                        "failed to verify directory ACL on {}: {error}",
                        dst_path.display()
                    ));
                }
                if actual.created()? != metadata.created()? {
                    failures.push(format!(
                        "directory creation time differs: {}",
                        dst_path.display()
                    ));
                }
                if macos::get_added_time(&dst_path)? != macos::get_added_time(src_path)? {
                    failures.push(format!(
                        "directory Finder added time differs: {}",
                        dst_path.display()
                    ));
                }
            }
            Ok(())
        })();
        if let Err(error) = verify {
            failures.push(format!(
                "failed to recheck directory metadata on {}: {error}",
                dst_path.display()
            ));
        }
        #[cfg(unix)]
        if retry_write
            && failures.len() != failure_count
            && let Err(error) = destination.set_permissions(prior_permissions)
        {
            failures.push(format!(
                "failed to restore previous directory mode on {}: {error}",
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
    #[cfg(not(unix))]
    require_existing_directory(dst_root, "destination root")?;
    #[cfg(unix)]
    let destination_root = crate::io_utils::open_directory_nofollow(dst_root)?;

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
        #[cfg(unix)]
        let target = crate::io_utils::open_descendant_nofollow(&destination_root, rel_path, true);
        #[cfg(not(unix))]
        let target = require_existing_directory(&dst_path, "destination mirror").map(|_| ());
        let target = match target {
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
        total_count += 1_i32;
        #[cfg(unix)]
        let update = (|| {
            let before = target.metadata()?;
            filetime::set_file_handle_times(&target, Some(*atime), Some(*mtime))?;
            let current =
                crate::io_utils::open_descendant_nofollow(&destination_root, rel_path, true)?;
            crate::io_utils::check_open_file_identity(&before, &current.metadata()?)
        })();
        #[cfg(not(unix))]
        let update = {
            let () = target;
            filetime::set_file_times(&dst_path, *atime, *mtime)
        };
        if let Err(e) = update {
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

    #[cfg(unix)]
    crate::io_utils::check_open_file_identity(
        &destination_root.metadata()?,
        &crate::io_utils::open_directory_nofollow(dst_root)?.metadata()?,
    )?;
    Ok(())
}

fn copy_file_timestamps_from_source_tree(src_root: &Path, dst_root: &Path) -> io::Result<()> {
    const SOURCE_EXTENSIONS: &[&str] = &[
        "jpg", "jpeg", "png", "webp", "heic", "heif", "avif", "gif", "tiff", "tif", "bmp", "jxl",
    ];
    #[cfg(unix)]
    let source_root = crate::io_utils::open_directory_nofollow(src_root)?;
    #[cfg(unix)]
    let destination_root = crate::io_utils::open_directory_nofollow(dst_root)?;
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
        if !entry.file_type().is_file() {
            continue;
        }
        let dst_path = entry.path();
        let Ok(rel) = dst_path.strip_prefix(dst_root) else {
            continue;
        };
        if rel.file_stem().is_none() {
            continue;
        }
        let source_path = src_root.join(rel);
        #[cfg(unix)]
        let source_parent = match crate::io_utils::open_descendant_nofollow(
            &source_root,
            rel.parent()
                .ok_or_else(|| io::Error::other("missing timestamp source parent"))?,
            true,
        ) {
            Ok(parent) => parent,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        for ext in SOURCE_EXTENSIONS {
            let src_file = source_path.with_extension(ext);
            #[cfg(unix)]
            let result = (|| {
                use std::os::unix::fs::MetadataExt;
                let name = src_file
                    .file_name()
                    .ok_or_else(|| io::Error::other("missing timestamp source name"))?;
                let source = match crate::io_utils::open_entry_nofollow(&source_parent, name, false)
                {
                    Ok(file) if file.metadata()?.is_file() => file,
                    Ok(_) => return Ok(false),
                    Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
                    Err(error) => return Err(error),
                };
                let target =
                    crate::io_utils::open_descendant_nofollow(&destination_root, rel, false)?;
                let metadata = target.metadata()?;
                if !metadata.is_file() || metadata.nlink() != 1 {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "timestamp destination must be a standalone file",
                    ));
                }
                apply_file_timestamps_inner(&src_file, dst_path, Some((&source, &target)))?;
                let current =
                    crate::io_utils::open_descendant_nofollow(&destination_root, rel, false)?;
                crate::io_utils::check_open_file_identity(&metadata, &current.metadata()?)?;
                Ok(true)
            })();
            #[cfg(not(unix))]
            let result = if src_file.is_file() {
                apply_file_timestamps(&src_file, dst_path).map(|()| true)
            } else {
                Ok(false)
            };
            match result {
                Ok(false) => continue,
                Ok(true) => {}
                Err(error) => {
                    crate::media_conversion_gate::delivery_metadata_path_audit(
                        "delivery_metadata_timestamp",
                        dst_path,
                        format!(
                            "Failed to restore timestamps from {}: {error}",
                            src_file.display()
                        ),
                    );
                    failed_count += 1;
                    if first_error.is_none() {
                        first_error = Some(error);
                    }
                }
            }
            break;
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

    #[cfg(unix)]
    crate::io_utils::check_open_file_identity(
        &destination_root.metadata()?,
        &crate::io_utils::open_directory_nofollow(dst_root)?.metadata()?,
    )?;
    Ok(())
}

/// Copy directory and matched file timestamps from the source tree to the output tree.
///
/// # Errors
/// Returns an `io::Result` if restoration fails.
pub fn restore_timestamps_from_source_to_output(src_dir: &Path, dst_dir: &Path) -> io::Result<()> {
    let source = src_dir.canonicalize()?;
    let destination = dst_dir.canonicalize()?;
    if destination.starts_with(&source) || source.starts_with(&destination) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "source and destination timestamp trees overlap",
        ));
    }
    let saved = save_directory_timestamps(src_dir)?;
    apply_saved_timestamps_to_dst(&saved, src_dir, dst_dir)?;
    copy_file_timestamps_from_source_tree(src_dir, dst_dir)
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
    apply_delivery_directory_metadata(saved, src_dir, dst_dir)?;
    restore_directory_timestamps(saved)?;
    Ok(())
}

/// Mirror delivery directory metadata without writing back to the source tree.
///
/// # Errors
/// Returns an error if destination metadata synchronization fails.
pub fn apply_delivery_directory_metadata<S>(
    saved: &std::collections::HashMap<PathBuf, (filetime::FileTime, filetime::FileTime), S>,
    src_dir: &Path,
    dst_dir: &Path,
) -> io::Result<()>
where
    S: std::hash::BuildHasher,
{
    preserve_directory(src_dir, dst_dir)?;
    apply_saved_timestamps_to_dst(saved, src_dir, dst_dir)
}

/// Apply source directory metadata only to existing delivery ancestors.
/// # Errors
/// Rejects invalid, linked and non-directory ancestors before writing metadata.
/// Missing directories may only be skipped under the caller's verified recovery or Gate 1 scope.
pub fn apply_scoped_delivery_directory_metadata(
    saved: &DirectoryTimestampsMap,
    src_root: &Path,
    dst_root: &Path,
    ancestors: &std::collections::BTreeSet<PathBuf>,
    scope: ScopedDeliveryDirectoryMetadata,
) -> io::Result<()> {
    use crate::pipeline::verification::validate_no_symlink_components;

    let mut directories = Vec::with_capacity(ancestors.len());
    let mut timestamps = DirectoryTimestampsMap::with_capacity(ancestors.len());
    for relative in ancestors {
        validate_no_symlink_components(src_root, relative).map_err(io::Error::other)?;
        validate_no_symlink_components(dst_root, relative).map_err(io::Error::other)?;
        let source = src_root.join(relative);
        let destination = dst_root.join(relative);
        let times = saved.get(&source);
        let source_metadata = scoped_delivery_directory(
            &source,
            scope.allow_removed_source && !relative.as_os_str().is_empty() && times.is_none(),
        )?;
        let destination_metadata = scoped_delivery_directory(
            &destination,
            scope.defer_missing_output_to_gate && !relative.as_os_str().is_empty(),
        )?;
        let Some(metadata) = source_metadata else {
            continue;
        };
        let times = times.ok_or_else(|| {
            io::Error::other(format!(
                "missing saved source directory timestamps: {}",
                source.display()
            ))
        })?;
        if destination_metadata.is_none() {
            continue;
        }
        timestamps.insert(source.clone(), *times);
        directories.push((source, metadata));
    }
    if directories.is_empty() {
        return Ok(());
    }
    directories.sort_by_key(|(path, _)| path.components().count());
    apply_existing_directory_metadata(&directories, src_root, dst_root)?;
    apply_saved_timestamps_to_dst(&timestamps, src_root, dst_root)
}

#[derive(Clone, Copy, Default)]
pub struct ScopedDeliveryDirectoryMetadata {
    /// Only after existing cleanup admission accepts previously removed source files.
    pub allow_removed_source: bool,
    /// Only when the next Gate 1 will verify the recorded outputs itself.
    pub defer_missing_output_to_gate: bool,
}

fn scoped_delivery_directory(
    path: &Path,
    allow_missing: bool,
) -> io::Result<Option<std::fs::Metadata>> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_dir() => Ok(Some(metadata)),
        Ok(_) => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "scoped delivery path is not a directory: {}",
                path.display(),
            ),
        )),
        Err(error) if allow_missing && error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(io::Error::new(
            error.kind(),
            format!(
                "inspect scoped delivery directory {}: {error}",
                path.display(),
            ),
        )),
    }
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
        let meta = std::fs::symlink_metadata(&path)?;
        if meta.file_type().is_symlink() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "directory timestamp snapshot refuses symlink: {}",
                    path.display()
                ),
            ));
        }
        if meta.is_dir() {
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
        let meta = std::fs::symlink_metadata(&path)?;
        if meta.file_type().is_symlink() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "directory metadata snapshot refuses symlink: {}",
                    path.display()
                ),
            ));
        }
        if meta.is_dir() {
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
    copy_xattrs_using(src, dst, should_copy, |name, value| {
        xattr::set(dst, name, value)
    })
}

fn copy_xattrs_using(
    src: &Path,
    dst: &Path,
    should_copy: impl Fn(&str) -> bool,
    set: impl Fn(&str, &[u8]) -> io::Result<()>,
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
                        if let Err(e) = set(name_str, &value) {
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

#[cfg(unix)]
fn copy_dir_xattrs_to_handle(
    src: &Path,
    dst: &Path,
    destination: &std::fs::File,
) -> io::Result<()> {
    use xattr::FileExt;

    copy_xattrs_using(src, dst, should_preserve_xattr, |name, value| {
        destination.set_xattr(name, value)
    })
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

/// Returns whether Exiv2 may be used after the primary XMP merge failed.
///
/// JXL uses an append-only XML overlay so its existing container/JBRD bytes
/// stay frozen. AVIF/HEIF/WebP/JP2 may carry HDR/auxiliary relationships,
/// provenance, and unknown structures that a generic writer cannot prove it
/// preserved. An arbitrary secondary metadata writer is therefore unsafe for
/// these containers. Unknown destination identity is also retained rather than
/// guessed.
fn exiv2_xmp_fallback_allowed(dst: &Path) -> io::Result<bool> {
    let format = crate::image::format_detect::detect_true_format(dst).map_err(|error| {
        io::Error::other(format!(
            "refusing Exiv2 XMP fallback because destination format could not be verified for {}: {error}",
            dst.display()
        ))
    })?;
    Ok(!crate::xmp_merger::xmp_rewrite_requires_immutable_container(format))
}

fn merge_xmp_sidecar(src: &Path, dst: &Path) -> io::Result<bool> {
    let xmp_path = find_xmp_sidecar(src)?;

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
                let fallback_allowed = exiv2_xmp_fallback_allowed(dst)?;
                if !fallback_allowed {
                    crate::progress_mode::xmp_merge_failure(&err_str);
                    return Err(io::Error::other(format!(
                        "Failed to merge XMP sidecar {} into {}: {err_str}; refusing Exiv2 fallback because it cannot prove preservation of protected modern-container bytes and relationships",
                        xmp.display(),
                        dst.display()
                    )));
                }
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

fn xmp_candidate_matches_source(path: &Path, stem: &str, full_name: &str) -> bool {
    xmp_candidate_key(path).is_some_and(|candidate| candidate == stem || candidate == full_name)
}

pub(crate) fn xmp_candidate_key(path: &Path) -> Option<String> {
    if !path
        .extension()
        .is_some_and(|value| value.eq_ignore_ascii_case("xmp"))
    {
        return None;
    }
    let Some(candidate_stem) = path.file_stem().and_then(|value| value.to_str()) else {
        // A valid UTF-8 source name cannot match this stem. Do not guess by
        // replacing invalid bytes with a visible Unicode character.
        return None;
    };
    Some(candidate_stem.to_lowercase())
}

pub(crate) fn xmp_source_names(src: &Path) -> io::Result<(String, String)> {
    let stem = src
        .file_stem()
        .and_then(|value| value.to_str())
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "XMP source has no supported file stem",
            )
        })?
        .to_lowercase();
    let full_name = src
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "XMP source has no supported file name",
            )
        })?
        .to_lowercase();
    Ok((stem, full_name))
}

/// Find a unique adjacent XMP candidate using supported stem or compound names.
/// Names identify candidates, not historical ownership or valid XMP content.
///
/// # Errors
/// Refuses ambiguous matches, non-regular sidecars and incomplete directory reads.
/// A failed discovery must never be interpreted as proof that no sidecar exists.
pub fn find_xmp_sidecar(src: &Path) -> io::Result<Option<PathBuf>> {
    let parent = crate::media_conversion_gate::path_parent_or_dot(src);
    let (stem, full_name) = xmp_source_names(src)?;
    let mut selected: Option<PathBuf> = None;
    for entry in std::fs::read_dir(parent).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!("scan XMP candidates under {}: {error}", parent.display()),
        )
    })? {
        let path = entry?.path();
        if !xmp_candidate_matches_source(&path, &stem, &full_name) {
            continue;
        }
        let path = require_regular_sidecar(path, "XMP")?;
        if let Some(previous) = selected {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "ambiguous XMP sidecars for {}: {} and {}; originals retained",
                    src.display(),
                    previous.display(),
                    path.display()
                ),
            ));
        }
        selected = Some(path);
    }
    Ok(selected)
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
