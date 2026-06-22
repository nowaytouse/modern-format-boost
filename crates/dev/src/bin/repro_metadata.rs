#![allow(unused_imports)]

use foundation::{
    log_anomaly, log_corruption, log_debug, log_detail, log_failure, log_fatal, log_hint,
    log_ignore, log_info, log_skip, log_success,
};

use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::thread;
use std::time::Duration;

/// Returns metadata for a path with retries.
///
/// # Errors
/// Returns an error if metadata cannot be retrieved after 3 attempts.
///
/// # Panics
/// Panics if all attempts fail but no error was recorded.
pub fn metadata_with_retry<P: AsRef<Path>>(path: P) -> std::io::Result<fs::Metadata> {
    let p = path.as_ref();
    let mut last_err = None;

    for i in 0..3 {
        match fs::metadata(p) {
            Ok(m) => return Ok(m),
            Err(e) => {
                if e.kind() == std::io::ErrorKind::NotFound {
                    return Err(e);
                }
                last_err = Some(e);
                if i < 2 {
                    log_detail!(
                        "⏳ [RETRY {}] Caught transient issue for {}",
                        i + 1,
                        p.display(),
                    );
                    thread::sleep(Duration::from_millis(100));
                }
            }
        }
    }
    Err(last_err.unwrap_or_else(|| std::io::Error::other("unknown error")))
}

fn main() {
    //  TARGET: THE ACTUAL PROBLEM FILE (COPIED)
    let target = "real_problem_file.gif";

    log_detail!("Testing Actual Problem File: {target}");

    // 1. Initial metadata check
    match metadata_with_retry(target) {
        Ok(m) => {
            log_detail!(" SUCCESS: Metadata read from real_problem_file.gif");
            log_detail!(" Size: {} bytes", m.len());
            log_detail!(" Mode: {:o}", m.mode());
        }
        Err(e) => {
            log_detail!(" FATAL: Persistent failure on real file: {e}");
        }
    }

    log_detail!(
        "\n Analysis: If this succeeds now, it confirms the 'cscachefs' lock in the log was transient and our retry logic would have saved it.",
    );
}
