//! Network & cloud-related metadata verification (macOS xattrs).
//!
//! When preserving metadata from source to destination (e.g. after re-encode or copy),
//! this module checks that **network/download-related extended attributes** are not
//! silently lost on the destination. It does not copy them (copy is done elsewhere);
//! it only verifies and warns if critical xattrs were present on source but missing on dst.
//!
//! **Checked xattrs (macOS):**
//! - `com.apple.metadata:kMDItemWhereFroms` — download source (URL)
//! - `com.apple.metadata:kMDItemUserTags` — Finder tags
//! - `com.apple.quarantine` — quarantine flag (intentionally not copied; missing on dst is not warned)
//!
//! Used by [crate::metadata::preserve_pro] and [crate::metadata::preserve_metadata] on all platforms;
//! the xattr keys are macOS-specific but the `xattr` crate no-ops on non-macOS for unknown keys.

use std::io;
use std::path::Path;

/// Verifies that critical network/cloud xattrs present on `src` are present on `dst`; warns if missing.
/// `com.apple.quarantine` is excluded from the missing check (often intentionally not copied).
pub fn verify_network_metadata(src: &Path, dst: &Path) -> io::Result<()> {
    let critical_xattrs = [
        "com.apple.metadata:kMDItemWhereFroms",
        "com.apple.metadata:kMDItemUserTags",
        "com.apple.quarantine",
    ];

    for &key in &critical_xattrs {
        if let Ok(Some(_)) = xattr::get(src, key) {
            if xattr::get(dst, key).ok().flatten().is_none() && key != "com.apple.quarantine" {
                eprintln!(
                    "⚠️ [metadata] Network metadata '{}' missing on destination.",
                    key
                );
            }
        }
    }
    Ok(())
}
