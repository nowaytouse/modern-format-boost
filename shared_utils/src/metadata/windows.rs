//! Windows-specific metadata preservation

use std::io;
use std::path::Path;
use std::process::Command;

pub fn preserve_windows_attributes(src: &Path, dst: &Path) -> io::Result<()> {
    // ACL via PowerShell
    if which::which("powershell").is_ok() {
        let ps_script = format!(
            "Get-Acl -LiteralPath '{}' | Set-Acl -LiteralPath '{}'",
            src.to_string_lossy().replace('\'', "''"),
            dst.to_string_lossy().replace('\'', "''")
        );
        match Command::new("powershell")
            .arg("-NoProfile")
            .arg("-Command")
            .arg(ps_script)
            .output()
        {
            Ok(output) if !output.status.success() => {
                eprintln!(
                    "⚠️ [metadata] PowerShell ACL copy returned non-zero status for {}: {}",
                    dst.display(),
                    String::from_utf8_lossy(&output.stderr).trim()
                );
            }
            Err(e) => {
                eprintln!(
                    "⚠️ [metadata] Failed to launch PowerShell ACL copy for {}: {}",
                    dst.display(),
                    e
                );
            }
            _ => {}
        }
    }

    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if let Ok(meta) = std::fs::metadata(src) {
            let file_attrs = meta.file_attributes();
            let is_hidden = (file_attrs & 0x2) != 0;
            let is_system = (file_attrs & 0x4) != 0;
            let mut cmd = Command::new("attrib");
            if is_hidden {
                cmd.arg("+h");
            }
            if is_system {
                cmd.arg("+s");
            }
            cmd.arg(dst);
            match cmd.output() {
                Ok(output) if !output.status.success() => {
                    eprintln!(
                        "⚠️ [metadata] attrib returned non-zero status for {}: {}",
                        dst.display(),
                        String::from_utf8_lossy(&output.stderr).trim()
                    );
                }
                Err(e) => {
                    eprintln!(
                        "⚠️ [metadata] Failed to launch attrib for {}: {}",
                        dst.display(),
                        e
                    );
                }
                _ => {}
            }
        }

        // Alternate Data Streams (ADS) — enumerate via PowerShell and copy each stream
        preserve_alternate_data_streams(src, dst);
    }

    Ok(())
}

#[cfg(windows)]
fn preserve_alternate_data_streams(src: &Path, dst: &Path) {
    if !which::which("powershell").is_ok() {
        return;
    }
    // List all ADS names (excludes the default :$DATA stream)
    let list_script = format!(
        "Get-Item -LiteralPath '{}' -Stream * | Where-Object {{ $_.Stream -ne ':$DATA' }} | Select-Object -ExpandProperty Stream",
        src.to_string_lossy().replace('\'', "''")
    );
    let out = Command::new("powershell")
        .arg("-NoProfile")
        .arg("-Command")
        .arg(&list_script)
        .output();
    let Ok(out) = out else {
        eprintln!(
            "⚠️ [metadata] Failed to enumerate ADS streams for {}",
            src.display()
        );
        return;
    };
    if !out.status.success() {
        eprintln!(
            "⚠️ [metadata] PowerShell ADS enumeration returned non-zero status for {}: {}",
            src.display(),
            String::from_utf8_lossy(&out.stderr).trim()
        );
        return;
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
        let result = Command::new("powershell")
            .arg("-NoProfile")
            .arg("-Command")
            .arg(&copy_script)
            .output();
        if let Ok(r) = result {
            if !r.status.success() {
                eprintln!(
                    "⚠️ [metadata] Failed to copy ADS stream '{}': {}",
                    stream_name,
                    String::from_utf8_lossy(&r.stderr)
                );
            }
        } else if let Err(e) = result {
            eprintln!(
                "⚠️ [metadata] Failed to launch PowerShell while copying ADS stream '{}' to {}: {}",
                stream_name,
                dst.display(),
                e
            );
        }
    }
}
