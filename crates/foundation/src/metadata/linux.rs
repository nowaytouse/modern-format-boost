//! Linux-specific metadata preservation

use crate::builder_base::ToolBuilder;
use std::io::{self, Write};
use std::path::Path;

pub(super) fn preserve_linux_attributes(src: &Path, dst: &Path) -> io::Result<()> {
    let mut failures = Vec::new();

    // ACL preservation via getfacl/setfacl --restore (more complete than -m per-entry)
    if which::which("getfacl").is_ok() && which::which("setfacl").is_ok() {
        let output = crate::tool_builders::AclBuilder::getfacl()
            .input(src)
            .build()
            .output();
        match output {
            Ok(out) if out.status.success() => {
                // Rewrite the path header so setfacl --restore targets dst
                let acl_text = String::from_utf8_lossy(&out.stdout);
                let dst_str = dst.to_string_lossy();
                let rewritten: String = acl_text
                    .lines()
                    .map(|line| {
                        if line.starts_with("# file:") {
                            format!("# file: {dst_str}\n")
                        } else {
                            format!("{line}\n")
                        }
                    })
                    .collect();

                // Feed rewritten ACL to setfacl --restore via stdin
                let mut child = crate::tool_builders::AclBuilder::restore()
                    .build()
                    .stdin(std::process::Stdio::piped())
                    .spawn()?;

                if let Some(mut stdin) = child.stdin.take()
                    && let Err(e) = stdin.write_all(rewritten.as_bytes())
                {
                    crate::media_conversion_gate::delivery_metadata_path_audit(
                        "delivery_metadata_platform",
                        dst,
                        format!(
                            "Failed to write ACL data to setfacl for {path}: {e}",
                            path = dst.display()
                        ),
                    );
                    failures.push(format!(
                        "failed to write ACL restore data for {}: {e}",
                        dst.display()
                    ));
                }

                match child.wait() {
                    Ok(status) if !status.success() => {
                        crate::media_conversion_gate::delivery_metadata_path_audit(
                            "delivery_metadata_platform",
                            dst,
                            format!(
                                "setfacl --restore returned non-zero status for {path}",
                                path = dst.display()
                            ),
                        );
                        failures.push(format!(
                            "setfacl --restore returned non-zero status for {}",
                            dst.display()
                        ));
                    }
                    Err(e) => {
                        crate::media_conversion_gate::delivery_metadata_path_audit(
                            "delivery_metadata_platform",
                            dst,
                            format!(
                                "Failed waiting for setfacl while restoring {path}: {e}",
                                path = dst.display()
                            ),
                        );
                        failures.push(format!(
                            "failed waiting for setfacl while restoring {}: {e}",
                            dst.display()
                        ));
                    }
                    _ => {}
                }
            }
            Ok(out) => {
                crate::media_conversion_gate::delivery_metadata_batch_audit(
                    "delivery_metadata_platform",
                    format!(
                        "getfacl returned non-zero status for {path}: {err}",
                        path = src.display(),
                        err = String::from_utf8_lossy(&out.stderr).trim()
                    ),
                );
                failures.push(format!(
                    "getfacl returned non-zero status for {}: {}",
                    src.display(),
                    String::from_utf8_lossy(&out.stderr).trim()
                ));
            }
            Err(e) => {
                crate::media_conversion_gate::delivery_metadata_path_audit(
                    "delivery_metadata_platform",
                    src,
                    format!(
                        "Failed to launch getfacl for {path}: {e}",
                        path = src.display()
                    ),
                );
                failures.push(format!(
                    "failed to launch getfacl for {}: {e}",
                    src.display()
                ));
            }
        }
    }

    // Unix permission bits
    match std::fs::metadata(src) {
        Ok(meta) => {
            use std::os::unix::fs::PermissionsExt;
            let mode = meta.permissions().mode();
            if let Err(e) = std::fs::set_permissions(dst, std::fs::Permissions::from_mode(mode)) {
                crate::media_conversion_gate::delivery_metadata_path_audit(
                    "delivery_metadata_platform",
                    dst,
                    format!(
                        "Failed to preserve Linux permission bits for {path}: {e}",
                        path = dst.display()
                    ),
                );
                failures.push(format!(
                    "failed to preserve Linux permission bits for {}: {e}",
                    dst.display()
                ));
            }
        }
        Err(e) => {
            crate::media_conversion_gate::delivery_metadata_path_audit(
                "delivery_metadata_platform",
                src,
                format!(
                    "Failed to read Linux source permission bits for {path}: {e}",
                    path = src.display()
                ),
            );
            failures.push(format!(
                "failed to read Linux permission bits from {}: {e}",
                src.display()
            ));
        }
    }

    if failures.is_empty() {
        Ok(())
    } else {
        Err(io::Error::other(failures.join("; ")))
    }
}

/// Try to set birth time on Linux (best-effort, no-op on most filesystems).
pub(super) const fn try_set_birth_time(_path: &Path, _time: std::time::SystemTime) {
    // Linux doesn't provide a standard way to set birth time (btime).
    // Most filesystems track it but don't allow modification via userspace.
}
