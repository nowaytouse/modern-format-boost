//! Shared JXL/image preprocessing utilities
//!
//! Common functions used by both img_av1 and img_hevc lossless converters:
//! - JXL file health verification
//! - Image format preprocessing for cjxl compatibility
//! - Fallback encoding pipelines (ImageMagick, FFmpeg)
//! - ICC Profile extraction and preservation

use std::path::Path;
use std::process::Command;

/// Extract ICC Profile from source image and return temp file path
pub fn extract_icc_profile(src: &Path) -> Option<tempfile::NamedTempFile> {
    if which::which("exiftool").is_err() {
        return None;
    }

    let temp_icc = tempfile::Builder::new().suffix(".icc").tempfile().ok()?;
    let output = Command::new("exiftool")
        .arg("-icc_profile")
        .arg("-b")
        .arg(crate::safe_path_arg(src).as_ref())
        .output()
        .ok()?;

    if output.status.success() && !output.stdout.is_empty() {
        std::fs::write(temp_icc.path(), &output.stdout).ok()?;
        Some(temp_icc)
    } else {
        None
    }
}

/// Add ICC Profile argument to cjxl command if available
pub fn add_icc_to_cjxl(cmd: &mut Command, icc_file: Option<&Path>) {
    if let Some(icc_path) = icc_file {
        cmd.arg("-x")
            .arg(format!("icc_pathname={}", icc_path.display()));
    }
}

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
            crate::progress_mode::preprocessing_success();
            Ok((temp_png, Some(temp_png_file)))
        }
        _ => {
            let line = format!(
                "   {} {} {}",
                style("🔧 PRE-PROCESSING:").cyan().bold(),
                style(label).dim(),
                style("→ ⚠️ failed, trying direct cjxl").yellow()
            );
            crate::progress_mode::emit_stderr(&line);
            Ok((input.to_path_buf(), None))
        }
    }
}

/// True when cjxl failed due to grayscale PNG + ICC profile (libpng: "RGB color space not permitted on grayscale").
/// Only then do we retry with -strip to avoid metadata loss in the general case.
/// Enhanced to catch more variants of the error message.
fn is_grayscale_icc_cjxl_error(stderr: &str) -> bool {
    let s = stderr.to_lowercase();
    // Match the specific pattern: ICC profile color space mismatch on grayscale PNG
    // Example: "libpng warning: iCCP: profile 'icc': 'RGB ': RGB color space not permitted on grayscale PNG"
    // Relaxed matching: check for libpng warning + grayscale + icc/color space issues
    let has_libpng_warning = s.contains("libpng") && s.contains("warning");
    let has_grayscale_issue = s.contains("grayscale");
    let has_icc_issue = s.contains("iccp") || s.contains("icc") || s.contains("color space");
    let has_pixel_failure = s.contains("getting pixel data failed") || s.contains("pixel data");

    // Either the specific error pattern OR a combination of indicators
    (s.contains("rgb color space not permitted on grayscale")
        || (has_libpng_warning && has_grayscale_issue && has_icc_issue))
        && has_pixel_failure
}

/// True when cjxl failed with decode/pixel errors that may be helped by a simpler pipeline.
fn is_decode_or_pixel_cjxl_error(stderr: &str) -> bool {
    let s = stderr.to_lowercase();
    s.contains("getting pixel data failed")
        || s.contains("failed to decode")
        || s.contains("decoding failed")
        || s.contains("decode failed")
}

/// Read the bit depth from a PNG file's IHDR chunk (byte offset 24).
/// Returns None if the file is not a valid PNG or cannot be read.
fn get_png_bit_depth(path: &Path) -> Option<u8> {
    use std::io::Read;
    let mut f = std::fs::File::open(path).ok()?;
    let mut buf = [0u8; 25];
    f.read_exact(&mut buf).ok()?;
    // PNG signature is 8 bytes; IHDR: 4 len + 4 type + 13 data bytes.
    // Bit depth is the first byte of IHDR data, at offset 8+4+4+8 = 24.
    if &buf[0..8] != b"\x89PNG\r\n\x1a\n" {
        return None;
    }
    Some(buf[24])
}

/// One attempt of ImageMagick → cjxl pipeline.
/// - `strip`: adds -strip (drops ICC/EXIF)
/// - `depth`: PNG bit depth to emit (8 or 16); use 8 only for confirmed 8-bit sources
/// - `normalize_icc`: replaces embedded ICC with standard sRGB without truncating bit depth
/// - `apple_compat`: adds --compress_boxes=0 to cjxl for Apple device compatibility
fn run_imagemagick_cjxl_pipeline(
    input: &Path,
    output: &Path,
    distance: f32,
    max_threads: usize,
    strip: bool,
    depth: u8,
    normalize_icc: bool,
    apple_compat: bool,
) -> std::result::Result<(), (bool, bool, String)> {
    use std::process::Stdio;

    let depth_arg = if depth == 8 { "8" } else { "16" };
    let mut magick = Command::new("magick");
    magick.arg("--").arg(crate::safe_path_arg(input).as_ref());
    if strip {
        magick.arg("-strip");
    }
    if normalize_icc {
        magick
            .arg("-define")
            .arg("png:preserve-colormap=false")
            .arg("-set")
            .arg("colorspace")
            .arg("sRGB");
    }
    magick
        .arg("-depth")
        .arg(depth_arg)
        .arg("png:-")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut magick_proc = magick.spawn().map_err(|e| {
        let line = format!("   ❌ ImageMagick not available or failed to start: {}", e);
        crate::progress_mode::emit_stderr(&line);
        (false, false, String::new())
    })?;

    let magick_stdout = magick_proc.stdout.take().ok_or_else(|| {
        if let Err(err) = magick_proc.kill() {
            crate::progress_mode::emit_stderr(&format!(
                "   ⚠️ Failed to stop ImageMagick after stdout capture failure: {}",
                err
            ));
        }
        (false, false, String::new())
    })?;

    // Drain ImageMagick stderr in background to avoid blocking when pipe buffer fills.
    // Limit to 1MB to prevent memory issues in low-memory scenarios.
    let magick_stderr_thread = magick_proc.stderr.take().map(|stderr| {
        std::thread::spawn(move || {
            use std::io::Read;
            let mut s = String::new();
            if let Err(err) = stderr.take(1024 * 1024).read_to_string(&mut s) {
                crate::progress_mode::emit_stderr(&format!(
                    "   ⚠️ Failed to read ImageMagick stderr output: {}",
                    err
                ));
            }
            s
        })
    });

    let mut cjxl_cmd = Command::new("cjxl");
    cjxl_cmd
        .arg("-")
        .arg(output)
        .arg("-d")
        .arg(format!("{:.1}", distance))
        .arg("-e")
        .arg("7")
        .arg("-j")
        .arg(max_threads.to_string());
    if apple_compat {
        cjxl_cmd.arg("--compress_boxes=0");
    }
    let mut cjxl_proc = cjxl_cmd
        .stdin(magick_stdout)
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| {
            let line = format!("   ❌ Failed to start cjxl process: {}", e);
            crate::progress_mode::emit_stderr(&line);
            if let Err(err) = magick_proc.kill() {
                crate::progress_mode::emit_stderr(&format!(
                    "   ⚠️ Failed to stop ImageMagick after cjxl startup failure: {}",
                    err
                ));
            }
            (false, false, String::new())
        })?;

    // Drain cjxl stderr in background so cjxl does not block when pipe buffer fills.
    // Limit to 1MB to prevent memory issues in low-memory scenarios.
    let cjxl_stderr_thread = cjxl_proc.stderr.take().map(|stderr| {
        std::thread::spawn(move || {
            use std::io::Read;
            let mut s = String::new();
            if let Err(err) = stderr.take(1024 * 1024).read_to_string(&mut s) {
                crate::progress_mode::emit_stderr(&format!(
                    "   ⚠️ Failed to read cjxl stderr output: {}",
                    err
                ));
            }
            s.trim().to_string()
        })
    });

    let magick_status = magick_proc.wait();
    let cjxl_status = cjxl_proc.wait();

    let magick_stderr = match magick_stderr_thread {
        Some(handle) => match handle.join() {
            Ok(stderr) => stderr,
            Err(_) => {
                crate::progress_mode::emit_stderr(
                    "   ⚠️ ImageMagick stderr capture thread panicked",
                );
                String::new()
            }
        },
        None => String::new(),
    };
    let cjxl_stderr = match cjxl_stderr_thread {
        Some(handle) => match handle.join() {
            Ok(stderr) => stderr,
            Err(_) => {
                crate::progress_mode::emit_stderr("   ⚠️ cjxl stderr capture thread panicked");
                String::new()
            }
        },
        None => String::new(),
    };

    let magick_ok = match magick_status {
        Ok(status) if status.success() => true,
        Ok(status) => {
            let line = format!(
                "   ❌ ImageMagick failed with exit code: {:?}",
                status.code()
            );
            crate::progress_mode::emit_stderr(&line);
            if !magick_stderr.is_empty() {
                let line2 = format!(
                    "   📋 ImageMagick stderr: {}",
                    magick_stderr.lines().next().unwrap_or("")
                );
                crate::progress_mode::emit_stderr(&line2);
            }
            false
        }
        Err(e) => {
            let line = format!("   ❌ Failed to wait for ImageMagick: {}", e);
            crate::progress_mode::emit_stderr(&line);
            false
        }
    };

    let cjxl_ok = match cjxl_status {
        Ok(status) if status.success() => true,
        Ok(status) => {
            let exit_code = status.code();
            crate::log_upstream_error!("cjxl", "Failed with exit code: {:?}", exit_code);
            if !cjxl_stderr.is_empty() {
                crate::progress_mode::emit_stderr(&format!("   📋 cjxl stderr: {}", cjxl_stderr));
            }
            false
        }
        Err(e) => {
            crate::log_pipeline_broken!("cjxl", "Failed to wait for process: {}", e);
            false
        }
    };

    if magick_ok && cjxl_ok {
        Ok(())
    } else {
        Err((magick_ok, cjxl_ok, cjxl_stderr))
    }
}

/// ImageMagick → cjxl fallback pipeline for when direct cjxl encoding fails.
///
/// Fallback priority:
///
/// - No -strip, depth 16 (preserve metadata)
/// - grayscale+ICC error → -strip, depth 16
///   - still fails + decode/pixel error + 8-bit source → -strip, depth 8 (no quality loss)
///   - still fails + 16-bit source → normalize ICC to sRGB, keep depth 16
///     - still fails → error, refuse to downgrade
/// - decode/pixel error + 8-bit source → -strip, depth 8 (no quality loss)
/// - decode/pixel error + 16-bit source → normalize ICC to sRGB, keep depth 16
///   - still fails → error, refuse to silently downgrade
pub fn try_imagemagick_fallback(
    input: &Path,
    output: &Path,
    distance: f32,
    max_threads: usize,
    apple_compat: bool,
) -> std::result::Result<(), std::io::Error> {
    use console::style;

    // Attempt 1: no -strip, depth 16, preserve metadata
    crate::progress_mode::emit_stderr(&format!(
        "   🔄 Attempt 1: Default (16-bit, preserve metadata) - {}",
        input.display()
    ));
    match run_imagemagick_cjxl_pipeline(
        input,
        output,
        distance,
        max_threads,
        false,
        16,
        false,
        apple_compat,
    ) {
        Ok(()) => {
            crate::progress_mode::emit_stderr(&format!(
                "   {} Attempt 1 succeeded",
                style("✅").green()
            ));
            crate::progress_mode::fallback_success();
            return Ok(());
        }
        Err((magick_ok, cjxl_ok, stderr)) => {
            crate::progress_mode::emit_stderr(&format!(
                "   {} Attempt 1 failed (magick: {}, cjxl: {})",
                style("❌").red(),
                if magick_ok {
                    style("✓").green()
                } else {
                    style("✗").red()
                },
                if cjxl_ok {
                    style("✓").green()
                } else {
                    style("✗").red()
                }
            ));

            if magick_ok && !cjxl_ok && is_grayscale_icc_cjxl_error(&stderr) {
                // Attempt 2: -strip, depth 16 (drop bad ICC, keep bit depth)
                crate::progress_mode::emit_stderr(
                    "   🔄 Attempt 2: Grayscale ICC fix (-strip, 16-bit)",
                );
                match run_imagemagick_cjxl_pipeline(
                    input,
                    output,
                    distance,
                    max_threads,
                    true,
                    16,
                    false,
                    apple_compat,
                ) {
                    Ok(()) => {
                        crate::progress_mode::emit_stderr(&format!(
                            "   {} Attempt 2 succeeded",
                            style("✅").green()
                        ));
                        crate::progress_mode::fallback_success();
                        return Ok(());
                    }
                    Err((m, c, stderr2)) => {
                        crate::progress_mode::emit_stderr(&format!(
                            "   {} Attempt 2 failed (magick: {}, cjxl: {})",
                            style("❌").red(),
                            if m {
                                style("✓").green()
                            } else {
                                style("✗").red()
                            },
                            if c {
                                style("✓").green()
                            } else {
                                style("✗").red()
                            }
                        ));

                        // Check if it's still a decode/pixel error and try 8-bit for 8-bit sources
                        if m && !c && is_decode_or_pixel_cjxl_error(&stderr2) {
                            let bit_depth = get_png_bit_depth(input).unwrap_or(16);
                            if bit_depth <= 8 {
                                // Attempt 3: -depth 8 -strip for 8-bit sources (no quality loss)
                                crate::progress_mode::emit_stderr(
                                    "   🔄 Attempt 3: 8-bit depth (-depth 8 -strip, 8-bit source confirmed)",
                                );
                                match run_imagemagick_cjxl_pipeline(
                                    input,
                                    output,
                                    distance,
                                    max_threads,
                                    true,
                                    8,
                                    false,
                                    apple_compat,
                                ) {
                                    Ok(()) => {
                                        crate::progress_mode::emit_stderr(&format!(
                                            "   {} Attempt 3 succeeded",
                                            style("✅").green()
                                        ));
                                        crate::progress_mode::fallback_success();
                                        return Ok(());
                                    }
                                    Err(_) => {
                                        crate::progress_mode::emit_stderr(&format!(
                                            "   {} Attempt 3 failed",
                                            style("❌").red()
                                        ));
                                    }
                                }
                            } else {
                                // Attempt 3: normalize ICC to sRGB, preserve 16-bit depth
                                crate::progress_mode::emit_stderr(
                                    "   🔄 Attempt 3: ICC normalization (sRGB, 16-bit source)",
                                );
                                match run_imagemagick_cjxl_pipeline(
                                    input,
                                    output,
                                    distance,
                                    max_threads,
                                    false,
                                    16,
                                    true,
                                    apple_compat,
                                ) {
                                    Ok(()) => {
                                        crate::progress_mode::emit_stderr(&format!(
                                            "   {} Attempt 3 succeeded",
                                            style("✅").green()
                                        ));
                                        crate::progress_mode::fallback_success();
                                        return Ok(());
                                    }
                                    Err(_) => {
                                        crate::progress_mode::emit_stderr(&format!(
                                            "   {} Attempt 3 failed",
                                            style("❌").red()
                                        ));
                                        crate::progress_mode::emit_stderr(
                                            "   ⚠️  16-bit source: refusing to downgrade to 8-bit",
                                        );
                                    }
                                }
                            }
                        }
                    }
                }
            } else if magick_ok && !cjxl_ok && is_decode_or_pixel_cjxl_error(&stderr) {
                let bit_depth = get_png_bit_depth(input).unwrap_or(16);
                if bit_depth <= 8 {
                    // Attempt 2: -strip -depth 8 for 8-bit sources (no quality loss)
                    crate::progress_mode::emit_stderr(
                        "   🔄 Attempt 2: 8-bit depth (-depth 8 -strip, 8-bit source confirmed)",
                    );
                    match run_imagemagick_cjxl_pipeline(
                        input,
                        output,
                        distance,
                        max_threads,
                        true,
                        8,
                        false,
                        apple_compat,
                    ) {
                        Ok(()) => {
                            crate::progress_mode::emit_stderr(&format!(
                                "   {} Attempt 2 succeeded",
                                style("✅").green()
                            ));
                            crate::progress_mode::fallback_success();
                            return Ok(());
                        }
                        Err(_) => {
                            crate::progress_mode::emit_stderr(&format!(
                                "   {} Attempt 2 failed",
                                style("❌").red()
                            ));
                        }
                    }
                } else {
                    // Attempt 2: normalize ICC to sRGB, preserve 16-bit depth
                    crate::progress_mode::emit_stderr(
                        "   🔄 Attempt 2: ICC normalization (sRGB, 16-bit source)",
                    );
                    match run_imagemagick_cjxl_pipeline(
                        input,
                        output,
                        distance,
                        max_threads,
                        false,
                        16,
                        true,
                        apple_compat,
                    ) {
                        Ok(()) => {
                            crate::progress_mode::emit_stderr(&format!(
                                "   {} Attempt 2 succeeded",
                                style("✅").green()
                            ));
                            crate::progress_mode::fallback_success();
                            return Ok(());
                        }
                        Err(_) => {
                            crate::progress_mode::emit_stderr(&format!(
                                "   {} Attempt 2 failed",
                                style("❌").red()
                            ));
                            crate::progress_mode::emit_stderr(
                                "   ⚠️  16-bit source: refusing to downgrade to 8-bit",
                            );
                        }
                    }
                }
            }

            // Final fallback: if nothing worked and we haven't tried -strip yet, try it as last resort
            if magick_ok && !cjxl_ok && !stderr.contains("-strip") {
                crate::progress_mode::emit_stderr("   🔄 Attempt (final): Last resort -strip");
                match run_imagemagick_cjxl_pipeline(
                    input,
                    output,
                    distance,
                    max_threads,
                    true,
                    16,
                    false,
                    apple_compat,
                ) {
                    Ok(()) => {
                        crate::progress_mode::emit_stderr(&format!(
                            "   {} Final attempt succeeded",
                            style("✅").green()
                        ));
                        crate::progress_mode::fallback_success();
                        return Ok(());
                    }
                    Err(_) => {
                        crate::progress_mode::emit_stderr(&format!(
                            "   {} Final attempt failed",
                            style("❌").red()
                        ));
                    }
                }
            }
        }
    }

    crate::progress_mode::emit_stderr(&format!(
        "   {} All ImageMagick fallback attempts exhausted",
        style("❌").red()
    ));
    Err(std::io::Error::other(
        "ImageMagick fallback pipeline failed",
    ))
}

/// Losslessly strip trailing data after JPEG EOI (0xFF 0xD9) so cjxl can use bitstream reconstruction.
/// Returns (temp_path, guard) if tail was stripped, or None if no tail or strip failed.
pub fn strip_jpeg_tail_to_temp(
    path: &Path,
) -> std::io::Result<Option<(std::path::PathBuf, tempfile::NamedTempFile)>> {
    let data = std::fs::read(path)?;
    if data.len() < 2 {
        return Ok(None);
    }
    let last_eoi = data
        .windows(2)
        .enumerate()
        .filter(|(_, w)| w[0] == 0xFF && w[1] == 0xD9)
        .map(|(i, _)| i + 1)
        .next_back();
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
