//! Windows-specific metadata preservation

use std::io;
use std::path::Path;
use std::process::Command;

pub fn preserve_windows_attributes(src: &Path, dst: &Path) -> io::Result<()> {
    let mut failures = Vec::new();

    // ACL via PowerShell
    if which::which("powershell").is_ok() {
        let ps_script = format!(
            "Get-Acl -LiteralPath '{}' | Set-Acl -LiteralPath '{}'",
            src.to_string_lossy().replace('\'', "''"),
            dst.to_string_lossy().replace('\'', "''")
        );
        match crate::tool_builders::PowershellBuilder::new()
            .command(&ps_script)
            .build()
            .output()
        {
            Ok(output) if !output.status.success() => {
                crate::media_conversion_gate::delivery_metadata_path_audit(
                    "delivery_metadata_platform",
                    dst,
                    format!(
                        "PowerShell ACL copy returned non-zero status for {path}: {err}",
                        path = dst.display(),
                        err = String::from_utf8_lossy(&output.stderr).trim()
                    ),
                );
                failures.push(format!(
                    "PowerShell ACL copy returned non-zero status for {}: {}",
                    dst.display(),
                    String::from_utf8_lossy(&output.stderr).trim()
                ));
            }
            Err(e) => {
                crate::media_conversion_gate::delivery_metadata_path_audit(
                    "delivery_metadata_platform",
                    dst,
                    format!(
                        "Failed to launch PowerShell ACL copy for {path}: {e}",
                        path = dst.display()
                    ),
                );
                failures.push(format!(
                    "failed to launch PowerShell ACL copy for {}: {e}",
                    dst.display()
                ));
            }
            _ => {}
        }
    }

    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        match std::fs::metadata(src) {
            Ok(meta) => {
                let file_attrs = meta.file_attributes();
                let is_hidden = (file_attrs & 0x2) != 0;
                let is_system = (file_attrs & 0x4) != 0;
                let mut cmd = crate::tool_builders::AttribBuilder::new();
                if is_hidden {
                    cmd.arg("+h");
                }
                if is_system {
                    cmd.arg("+s");
                }
                cmd.arg(dst);
                match cmd.build().output() {
                    Ok(output) if !output.status.success() => {
                        crate::media_conversion_gate::delivery_metadata_path_audit(
                            "delivery_metadata",
                            dst,
                            format!(
                                "attrib returned non-zero status for {path}: {err}",
                                path = dst.display(),
                                err = String::from_utf8_lossy(&output.stderr).trim()
                            ),
                        );
                        failures.push(format!(
                            "attrib returned non-zero status for {}: {}",
                            dst.display(),
                            String::from_utf8_lossy(&output.stderr).trim()
                        ));
                    }
                    Err(e) => {
                        crate::media_conversion_gate::delivery_metadata_path_audit(
                            "delivery_metadata",
                            dst,
                            format!(
                                "Failed to launch attrib for {path}: {e}",
                                path = dst.display()
                            ),
                        );
                        failures.push(format!(
                            "failed to launch attrib for {}: {e}",
                            dst.display()
                        ));
                    }
                    _ => {}
                }
            }
            Err(e) => {
                crate::media_conversion_gate::delivery_metadata_path_audit(
                    "delivery_metadata",
                    src,
                    format!(
                        "Failed to read Windows source file attributes for {path}: {e}",
                        path = src.display()
                    ),
                );
                failures.push(format!(
                    "failed to read Windows source file attributes from {}: {e}",
                    src.display()
                ));
            }
        }

        // Alternate Data Streams (ADS) — enumerate via PowerShell and copy each stream
        preserve_alternate_data_streams(src, dst)?;
    }

    if failures.is_empty() {
        Ok(())
    } else {
        Err(io::Error::other(failures.join("; ")))
    }
}

#[cfg(windows)]
fn preserve_alternate_data_streams(src: &Path, dst: &Path) -> io::Result<()> {
    let mut failures = Vec::new();

    if !which::which("powershell").is_ok() {
        return Ok(());
    }
    // List all ADS names (excludes the default :$DATA stream)
    let list_script = format!(
        "Get-Item -LiteralPath '{}' -Stream * | Where-Object {{ $_.Stream -ne ':$DATA' }} | Select-Object -ExpandProperty Stream",
        src.to_string_lossy().replace('\'', "''")
    );
    let out = crate::tool_builders::PowershellBuilder::new()
        .command(&list_script)
        .build()
        .output();
    let Ok(out) = out else {
        crate::media_conversion_gate::delivery_metadata_path_audit(
            "delivery_metadata",
            src,
            format!(
                "Failed to enumerate ADS streams for {path}",
                path = src.display()
            ),
        );
        return Err(io::Error::other(format!(
            "Failed to enumerate ADS streams for {}",
            src.display()
        )));
    };
    if !out.status.success() {
        crate::media_conversion_gate::delivery_metadata_path_audit(
            "delivery_metadata",
            src,
            format!(
                "PowerShell ADS enumeration returned non-zero status for {path}: {err}",
                path = src.display(),
                err = String::from_utf8_lossy(&out.stderr).trim()
            ),
        );
        return Err(io::Error::other(format!(
            "PowerShell ADS enumeration returned non-zero status for {}: {}",
            src.display(),
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }

    let streams = String::from_utf8_lossy(&out.stdout);
    for stream_name in streams.lines().map(str::trim).filter(|s| !s.is_empty()) {
        // Read stream content and write to dst
        let copy_script = format!(
            "Get-Content -LiteralPath '{}' -Stream '{}' -Raw | Set-Content -LiteralPath '{}' -Stream '{}'",
            src.to_string_lossy().replace('\'', "''"),
            stream_name.replace('\'', "''"),
            dst.to_string_lossy().replace('\'', "''"),
            stream_name.replace('\'', "''"),
        );
        let result = crate::tool_builders::PowershellBuilder::new()
            .command(&copy_script)
            .build()
            .output();
        match result {
            Ok(r) if !r.status.success() => {
                crate::media_conversion_gate::delivery_metadata_batch_audit(
                    "delivery_metadata",
                    format!(
                        "Failed to copy ADS stream '{stream_name}': {err}",
                        err = String::from_utf8_lossy(&r.stderr)
                    ),
                );
                failures.push(format!(
                    "failed to copy ADS stream '{stream_name}' to {}: {}",
                    dst.display(),
                    String::from_utf8_lossy(&r.stderr).trim()
                ));
            }
            Ok(_) => {}
            Err(e) => {
                crate::media_conversion_gate::delivery_metadata_path_audit(
                    "delivery_metadata",
                    dst,
                    format!(
                        "Failed to launch PowerShell while copying ADS stream '{stream_name}' to {path}: {e}",
                        path = dst.display()
                    ),
                );
                failures.push(format!(
                    "failed to launch PowerShell while copying ADS stream '{stream_name}' to {}: {e}",
                    dst.display()
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
