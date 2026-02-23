//! Shared JXL/image preprocessing utilities
//!
//! Common functions used by both img_av1 and img_hevc lossless converters:
//! - JXL file health verification
//! - Image format preprocessing for cjxl compatibility
//! - Fallback encoding pipelines (ImageMagick, FFmpeg)

use std::path::Path;
use std::process::Command;

/// Verify that a JXL file is valid by checking its signature and optionally running jxlinfo.
pub fn verify_jxl_health(path: &Path) -> Result<(), String> {
    let mut file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut sig = [0u8; 2];
    use std::io::Read;
    file.read_exact(&mut sig).map_err(|e| e.to_string())?;

    if sig != [0xFF, 0x0A] && sig != [0x00, 0x00] {
        return Err("Invalid JXL file signature".to_string());
    }

    if which::which("jxlinfo").is_ok() {
        let result = Command::new("jxlinfo")
            .arg(crate::safe_path_arg(path).as_ref())
            .output();

        if let Ok(output) = result {
            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                return Err(format!(
                    "JXL health check failed (jxlinfo): {}",
                    stderr.trim()
                ));
            }
        }
    }

    Ok(())
}

/// Run an external tool to convert input to a temp PNG.
/// Returns (temp_path, temp_handle) on success, or (original_input, None) on failure (graceful fallback).
pub fn convert_to_temp_png(
    input: &Path,
    tool: &str,
    args_before_input: &[&str],
    args_after_input: &[&str],
    label: &str,
) -> std::io::Result<(std::path::PathBuf, Option<tempfile::NamedTempFile>)> {
    use console::style;
    eprintln!(
        "   {} {}",
        style("🔧 PRE-PROCESSING:").cyan().bold(),
        style(label).dim()
    );

    let temp_png_file = tempfile::Builder::new().suffix(".png").tempfile()?;
    let temp_png = temp_png_file.path().to_path_buf();

    let mut cmd = Command::new(tool);
    for arg in args_before_input {
        cmd.arg(arg);
    }
    cmd.arg(crate::safe_path_arg(input).as_ref());
    for arg in args_after_input {
        if *arg == "__OUTPUT__" {
            cmd.arg(crate::safe_path_arg(&temp_png).as_ref());
        } else {
            cmd.arg(arg);
        }
    }

    match cmd.output() {
        Ok(output) if output.status.success() && temp_png.exists() => {
            eprintln!(
                "   {} {}",
                style("✅").green(),
                style(format!("{} pre-processing successful", tool)).green()
            );
            Ok((temp_png, Some(temp_png_file)))
        }
        _ => {
            eprintln!(
                "   {} {}",
                style("⚠️").yellow(),
                style(format!(
                    "{} pre-processing failed, trying direct cjxl",
                    tool
                ))
                .dim()
            );
            Ok((input.to_path_buf(), None))
        }
    }
}

/// ImageMagick → cjxl fallback pipeline for when direct cjxl encoding fails.
pub fn try_imagemagick_fallback(
    input: &Path,
    output: &Path,
    distance: f32,
    max_threads: usize,
) -> std::result::Result<std::process::Output, std::io::Error> {
    use std::process::Stdio;

    eprintln!("   🔧 ImageMagick → cjxl pipeline");

    let magick_result = Command::new("magick")
        .arg("--")
        .arg(crate::safe_path_arg(input).as_ref())
        .arg("-depth")
        .arg("16")
        .arg("png:-")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn();

    match magick_result {
        Ok(mut magick_proc) => {
            if let Some(magick_stdout) = magick_proc.stdout.take() {
                let cjxl_result = Command::new("cjxl")
                    .arg("-")
                    .arg(output)
                    .arg("-d")
                    .arg(format!("{:.1}", distance))
                    .arg("-e")
                    .arg("7")
                    .arg("-j")
                    .arg(max_threads.to_string())
                    .stdin(magick_stdout)
                    .stderr(Stdio::piped())
                    .spawn();
                match cjxl_result {
                    Ok(mut cjxl_proc) => {
                        let magick_status = magick_proc.wait();
                        let cjxl_status = cjxl_proc.wait();

                        let magick_ok = match magick_status {
                            Ok(status) if status.success() => true,
                            Ok(status) => {
                                eprintln!(
                                    "   ❌ ImageMagick failed with exit code: {:?}",
                                    status.code()
                                );
                                false
                            }
                            Err(e) => {
                                eprintln!("   ❌ Failed to wait for ImageMagick: {}", e);
                                false
                            }
                        };

                        let cjxl_ok = match cjxl_status {
                            Ok(status) if status.success() => true,
                            Ok(status) => {
                                eprintln!("   ❌ cjxl failed with exit code: {:?}", status.code());
                                false
                            }
                            Err(e) => {
                                eprintln!("   ❌ Failed to wait for cjxl: {}", e);
                                false
                            }
                        };

                        if magick_ok && cjxl_ok {
                            eprintln!("   🎉 ImageMagick pipeline completed successfully");
                            Ok(std::process::Output {
                                status: std::process::ExitStatus::default(),
                                stdout: Vec::new(),
                                stderr: Vec::new(),
                            })
                        } else {
                            eprintln!(
                                "   ❌ ImageMagick pipeline failed (magick: {}, cjxl: {})",
                                if magick_ok { "✓" } else { "✗" },
                                if cjxl_ok { "✓" } else { "✗" }
                            );
                            Err(std::io::Error::other(
                                "ImageMagick fallback pipeline failed",
                            ))
                        }
                    }
                    Err(e) => {
                        eprintln!("   ❌ Failed to start cjxl process: {}", e);
                        let _ = magick_proc.kill();
                        Err(e)
                    }
                }
            } else {
                eprintln!("   ❌ Failed to capture ImageMagick stdout");
                let _ = magick_proc.kill();
                Err(std::io::Error::other(
                    "Failed to capture ImageMagick stdout",
                ))
            }
        }
        Err(e) => {
            eprintln!("   ❌ ImageMagick not available or failed to start: {}", e);
            eprintln!("      💡 Install: brew install imagemagick");
            Err(e)
        }
    }
}
