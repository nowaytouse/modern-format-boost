//! Linux-specific metadata preservation

use std::path::Path;
use std::process::Command;
use std::io::{self, Write};

pub fn preserve_linux_attributes(src: &Path, dst: &Path) -> io::Result<()> {
    // ACLs via getfacl/setfacl
    if which::which("getfacl").is_ok() && which::which("setfacl").is_ok() {
        let output = Command::new("getfacl").arg("--absolute-names").arg(src).output()?;
        if output.status.success() {
            if let Ok(mut child) = Command::new("setfacl").arg("--restore=-").stdin(std::process::Stdio::piped()).spawn() {
                if let Some(mut stdin) = child.stdin.take() {
                    let _ = stdin.write_all(&output.stdout);
                }
                let _ = child.wait();
            }
        }
    }
    let _ = dst; // suppress warning
    Ok(())
}
