//! Network & cloud-related metadata preservation (macOS xattrs).
//!
//! Copies AND verifies network/download-related extended attributes from src to dst.
//!
//! **Handled xattrs:**
//! - `com.apple.metadata:kMDItemWhereFroms` — download source URL (copied)
//! - `com.apple.metadata:kMDItemUserTags`   — Finder tags (copied)
//! - `com.apple.quarantine`                 — quarantine flag (intentionally NOT copied)

use std::io;
use std::path::Path;

/// Copy critical network/cloud xattrs from `src` to `dst`, then verify.
/// `com.apple.quarantine` is intentionally skipped (security boundary).
/// All errors are tolerated — missing xattr support on the filesystem is not fatal.
pub fn preserve_network_metadata(src: &Path, dst: &Path) -> io::Result<()> {
    // Keys to copy (quarantine intentionally excluded)
    const COPY_KEYS: &[&str] = &[
        "com.apple.metadata:kMDItemWhereFroms",
        "com.apple.metadata:kMDItemUserTags",
    ];

    for &key in COPY_KEYS {
        match xattr::get(src, key) {
            Ok(Some(value)) => {
                if let Err(e) = xattr::set(dst, key, &value) {
                    // Non-fatal: target filesystem may not support xattrs (e.g. FAT32, some network mounts)
                    eprintln!("⚠️ [metadata] Could not copy xattr '{}': {}", key, e);
                }
            }
            Ok(None) => {} // not present on source, nothing to do
            Err(e) => {
                eprintln!(
                    "⚠️ [metadata] Could not read source xattr '{}' from {}: {}",
                    key,
                    src.display(),
                    e
                );
            }
        }
    }

    // Verify after copy
    for &key in COPY_KEYS {
        match xattr::get(src, key) {
            Ok(Some(_)) => match xattr::get(dst, key) {
                Ok(Some(_)) => {}
                Ok(None) => {
                    eprintln!(
                        "⚠️ [metadata] xattr '{}' present on source but missing on destination after copy attempt.",
                        key
                    );
                }
                Err(e) => {
                    eprintln!(
                        "⚠️ [metadata] Could not verify destination xattr '{}' on {}: {}",
                        key,
                        dst.display(),
                        e
                    );
                }
            },
            Ok(None) => {}
            Err(e) => {
                eprintln!(
                    "⚠️ [metadata] Could not re-read source xattr '{}' during verification: {}",
                    key, e
                );
            }
        }
    }

    Ok(())
}
