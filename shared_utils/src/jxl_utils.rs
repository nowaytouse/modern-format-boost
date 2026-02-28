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
                "   {} {} {}",
                style("🔧 PRE-PROCESSING:").cyan().bold(),
                style(label).dim(),
                style("→ ✅ done").green()
            );
            Ok((temp_png, Some(temp_png_file)))
        }
        _ => {
            eprintln!(
                "   {} {} {}",
                style("🔧 PRE-PROCESSING:").cyan().bold(),
                style(label).dim(),
                style("→ ⚠️ failed, trying direct cjxl").yellow()
            );
            Ok((input.to_path_buf(), None))
        }
    }
}

/// True when cjxl failed due to grayscale PNG + ICC profile (libpng: "RGB color space not permitted on grayscale").
/// Only then do we retry with -strip to avoid metadata loss in the general case.
fn is_grayscale_icc_cjxl_error(stderr: &str) -> bool {
    let s = stderr.to_lowercase();
    (s.contains("rgb color space not permitted on grayscale") || s.contains("iccp"))
        && (s.contains("getting pixel data failed") || s.contains("grayscale"))
}

/// One attempt of ImageMagick → cjxl pipeline. `strip` = true adds -strip (drops ICC/EXIF) for the grayscale+ICC workaround.
fn run_imagemagick_cjxl_pipeline(
    input: &Path,
    output: &Path,
    distance: f32,
    max_threads: usize,
    strip: bool,
) -> std::result::Result<std::process::Output, (bool, bool, String)> {
    use std::process::Stdio;

    let mut magick = Command::new("magick");
    magick
        .arg("--")
        .arg(crate::safe_path_arg(input).as_ref());
    if strip {
        magick.arg("-strip");
    }
    magick
        .arg("-depth")
        .arg("16")
        .arg("png:-")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut magick_proc = magick.spawn().map_err(|e| {
        let _ = eprintln!("   ❌ ImageMagick not available or failed to start: {}", e);
        (false, false, String::new())
    })?;

    let magick_stdout = magick_proc.stdout.take().ok_or_else(|| {
        let _ = magick_proc.kill();
        (false, false, String::new())
    })?;

    // Drain ImageMagick stderr in background to avoid blocking when pipe buffer fills.
    let magick_stderr_thread = magick_proc.stderr.take().map(|mut stderr| {
        std::thread::spawn(move || {
            let mut s = String::new();
            let _ = std::io::Read::read_to_string(&mut stderr, &mut s);
            s
        })
    });

    let mut cjxl_proc = Command::new("cjxl")
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
        .spawn()
        .map_err(|e| {
            eprintln!("   ❌ Failed to start cjxl process: {}", e);
            let _ = magick_proc.kill();
            (false, false, String::new())
        })?;

    // Drain cjxl stderr in background so cjxl does not block when pipe buffer fills.
    let cjxl_stderr_thread = cjxl_proc.stderr.take().map(|mut stderr| {
        std::thread::spawn(move || {
            let mut s = String::new();
            let _ = std::io::Read::read_to_string(&mut stderr, &mut s);
            s.trim().to_string()
        })
    });

    let magick_status = magick_proc.wait();
    let cjxl_status = cjxl_proc.wait();

    let magick_stderr = magick_stderr_thread
        .and_then(|h| h.join().ok())
        .unwrap_or_default();
    let cjxl_stderr = cjxl_stderr_thread
        .and_then(|h| h.join().ok())
        .unwrap_or_default();

    let magick_ok = match magick_status {
        Ok(status) if status.success() => true,
        Ok(status) => {
            eprintln!("   ❌ ImageMagick failed with exit code: {:?}", status.code());
            if !magick_stderr.is_empty() {
                eprintln!("   📋 ImageMagick stderr: {}", magick_stderr.lines().next().unwrap_or(""));
            }
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
            if !cjxl_stderr.is_empty() {
                eprintln!("   📋 cjxl stderr: {}", cjxl_stderr);
            }
            false
        }
        Err(e) => {
            eprintln!("   ❌ Failed to wait for cjxl: {}", e);
            false
        }
    };

    if magick_ok && cjxl_ok {
        Ok(std::process::Output {
            status: std::process::ExitStatus::default(),
            stdout: Vec::new(),
            stderr: Vec::new(),
        })
    } else {
        Err((magick_ok, cjxl_ok, cjxl_stderr))
    }
}

/// ImageMagick → cjxl fallback pipeline for when direct cjxl encoding fails.
/// Tries without -strip first (preserve metadata). Only if cjxl fails with the exact
/// "grayscale PNG + ICC profile" error do we retry once with -strip.
pub fn try_imagemagick_fallback(
    input: &Path,
    output: &Path,
    distance: f32,
    max_threads: usize,
) -> std::result::Result<std::process::Output, std::io::Error> {
    eprintln!("   🔧 ImageMagick → cjxl pipeline");

    // First attempt: no -strip, preserve metadata
    match run_imagemagick_cjxl_pipeline(input, output, distance, max_threads, false) {
        Ok(out) => {
            eprintln!("   🎉 ImageMagick pipeline completed successfully");
            return Ok(out);
        }
        Err((magick_ok, cjxl_ok, stderr)) => {
            eprintln!(
                "   ❌ ImageMagick pipeline failed for file: {} (magick: {}, cjxl: {})",
                input.display(),
                if magick_ok { "✓" } else { "✗" },
                if cjxl_ok { "✓" } else { "✗" }
            );
            // Retry with -strip only when cjxl failed and reason is grayscale+ICC.
            // -strip only affects the intermediate PNG→JXL stream. The final JXL still receives
            // full metadata from the original file in finalize_conversion (ExifTool -tagsfromfile
            // from original → output), so EXIF/ICC/XMP/timestamps are preserved in the output.
            if magick_ok && !cjxl_ok && is_grayscale_icc_cjxl_error(&stderr) {
                eprintln!(
                    "   🔄 Retrying with -strip (grayscale PNG + ICC incompatible with cjxl); output will still get metadata from original in finalize step"
                );
                match run_imagemagick_cjxl_pipeline(input, output, distance, max_threads, true) {
                    Ok(out) => {
                        eprintln!("   🎉 ImageMagick pipeline completed (with -strip fallback)");
                        return Ok(out);
                    }
                    Err((m, c, _)) => {
                        eprintln!(
                            "   ❌ ImageMagick retry failed for file: {} (magick: {}, cjxl: {})",
                            input.display(),
                            if m { "✓" } else { "✗" },
                            if c { "✓" } else { "✗" }
                        );
                    }
                }
            }
        }
    }

    Err(std::io::Error::other(
        "ImageMagick fallback pipeline failed",
    ))
}

/// Losslessly strip trailing data after JPEG EOI (0xFF 0xD9) so cjxl can use bitstream reconstruction.
/// Returns (temp_path, guard) if tail was stripped, or None if no tail or strip failed.
pub fn strip_jpeg_tail_to_temp(path: &Path) -> std::io::Result<Option<(std::path::PathBuf, tempfile::NamedTempFile)>> {
    let data = std::fs::read(path)?;
    if data.len() < 2 {
        return Ok(None);
    }
    let last_eoi = data
        .windows(2)
        .enumerate()
        .filter(|(_, w)| w[0] == 0xFF && w[1] == 0xD9)
        .map(|(i, _)| i + 1)
        .last();
    let end = match last_eoi {
        Some(e) if e < data.len() => e,
        _ => return Ok(None),
    };
    if end == data.len() {
        return Ok(None);
    }
    let temp = tempfile::Builder::new().suffix(".jpg").tempfile()?;
    std::fs::write(temp.path(), &data[..end])?;
    let temp_path = temp.path().to_path_buf();
    Ok(Some((temp_path, temp)))
}
