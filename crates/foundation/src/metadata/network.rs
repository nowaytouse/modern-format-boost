//! Network & cloud-related metadata preservation (macOS xattrs).
//!
//! Copies AND verifies extended attributes needed for Finder, iCloud Photos,
//! and downloads.
//!
//! **Policy:** all `com.apple.metadata:*` keys plus explicit Finder/provenance
//! keys. **Skipped:** `XATTR_PRESERVE_SKIP_KEYS` (quarantine, `decmpfs`, etc.).

use std::{io, path::Path};

/// Copy macOS Spotlight / Finder / download xattrs from `src` to `dst`, then
/// verify priority keys.
///
/// # Errors
/// Returns an error when source metadata exists but cannot be copied or
/// verified.
pub(super) fn preserve_network_metadata(src: &Path, dst: &Path) -> io::Result<()> {
    let mut failures = Vec::new();
    match xattr::list(src) {
        Ok(iter) => {
            for name in iter {
                let Some(key) = name.to_str() else {
                    continue;
                };
                if !super::should_copy_macos_extended_xattr(key) {
                    continue;
                }
                match xattr::get(src, key) {
                    Ok(Some(value)) => {
                        if let Err(e) = xattr::set(dst, key, &value) {
                            crate::media_conversion_gate::delivery_metadata_batch_audit(
                                "delivery_metadata_platform",
                                format!("Could not copy xattr '{key}': {e}"),
                            );
                            failures.push(format!(
                                "failed to copy network xattr '{key}' from {} to {}: {e}",
                                src.display(),
                                dst.display()
                            ));
                        }
                    }
                    Ok(None) => {}
                    Err(e) => {
                        crate::media_conversion_gate::delivery_metadata_path_audit(
                            "delivery_metadata_platform",
                            src,
                            format!(
                                "Could not read source xattr '{key}' from {path}: {e}",
                                path = src.display()
                            ),
                        );
                        failures.push(format!(
                            "failed to read source network xattr '{key}' from {}: {e}",
                            src.display()
                        ));
                    }
                }
            }
        }
        Err(e) => {
            if super::delivery_policy::is_xattr_api_absence(&e) {
                crate::media_conversion_gate::delivery_metadata_batch_audit(
                    "delivery_metadata_platform",
                    crate::infra::static_logs::messages::MSG_METADATA_DELIVERY_SKIP_XATTR_ABSENCE
                        .replace("{}", &format!("{}: {e}", src.display())),
                );
                return Ok(());
            }
            crate::media_conversion_gate::delivery_metadata_path_audit(
                "delivery_metadata_platform",
                src,
                format!(
                    "Could not list extended attributes on {path}: {e}",
                    path = src.display()
                ),
            );
            failures.push(format!("failed to list xattrs on {}: {e}", src.display()));
        }
    }

    // Verify priority keys when present on source
    for &key in super::NETWORK_XATTR_PRIORITY_KEYS {
        match xattr::get(src, key) {
            Ok(Some(_)) => match xattr::get(dst, key) {
                Ok(Some(_)) => {}
                Ok(None) => {
                    crate::media_conversion_gate::delivery_metadata_batch_audit(
                        "delivery_metadata_platform",
                        format!(
                            "xattr '{key}' present on source but missing on destination after \
                             copy attempt."
                        ),
                    );
                    failures.push(format!(
                        "network xattr '{key}' was present on {} but missing on {} after copy",
                        src.display(),
                        dst.display()
                    ));
                }
                Err(e) => {
                    crate::media_conversion_gate::delivery_metadata_path_audit(
                        "delivery_metadata_platform",
                        dst,
                        format!(
                            "Could not verify destination xattr '{key}' on {path}: {e}",
                            path = dst.display()
                        ),
                    );
                    failures.push(format!(
                        "failed to verify destination network xattr '{key}' on {}: {e}",
                        dst.display()
                    ));
                }
            },
            Ok(None) => {}
            Err(e) => {
                crate::media_conversion_gate::delivery_metadata_batch_audit(
                    "delivery_metadata_platform",
                    format!("Could not re-read source xattr '{key}' during verification: {e}"),
                );
                failures.push(format!(
                    "failed to re-read source network xattr '{key}' during verification: {e}"
                ));
            }
        }
    }

    if failures.is_empty() {
        Ok(())
    } else {
        Err(io::Error::other(failures.join("; ")))
    }
}
