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
                    println!(
                        "   ⏳ [RETRY {}] Caught transient issue for {}",
                        i + 1,
                        p.display()
                    );
                    thread::sleep(Duration::from_millis(100));
                }
            }
        }
    }
    Err(last_err.unwrap())
}

fn main() -> std::io::Result<()> {
    // 🎯 TARGET: THE ACTUAL PROBLEM FILE (COPIED)
    let target = "real_problem_file.gif";

    println!("🔥 Testing Actual Problem File: {target}");

    // 1. Initial metadata check
    match metadata_with_retry(target) {
        Ok(m) => {
            println!("✅ SUCCESS: Metadata read from real_problem_file.gif");
            println!("   Size: {} bytes", m.len());
            println!("   Mode: {:o}", m.mode());
        }
        Err(e) => {
            println!("❌ FATAL: Persistent failure on real file: {e}");
        }
    }

    println!("\n💡 Analysis: If this succeeds now, it confirms the 'cscachefs' lock in the log was transient and our retry logic would have saved it.");

    Ok(())
}
