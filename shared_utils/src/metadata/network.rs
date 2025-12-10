//! Network & Cloud metadata verification

use std::path::Path;
use std::io;

pub fn verify_network_metadata(src: &Path, dst: &Path) -> io::Result<()> {
    let critical_xattrs = [
        "com.apple.metadata:kMDItemWhereFroms",
        "com.apple.metadata:kMDItemUserTags",
        "com.apple.quarantine",
    ];

    for &key in &critical_xattrs {
        if let Ok(Some(_)) = xattr::get(src, key) {
            if xattr::get(dst, key).ok().flatten().is_none() && key != "com.apple.quarantine" {
                eprintln!("⚠️ [metadata] Network metadata '{}' missing on destination.", key);
            }
        }
    }
    Ok(())
}
