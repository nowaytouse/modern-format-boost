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
fn is_grayscale_icc_cjxl_error(stderr: &str) -> bool {
    let s = stderr.to_lowercase();
    (s.contains("rgb color space not permitted on grayscale") || s.contains("iccp"))
        && (s.contains("getting pixel data failed") || s.contains("grayscale"))
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
) -> std::result::Result<std::process::Output, (bool, bool, String)> {
    use std::process::Stdio;

    let depth_arg = if depth == 8 { "8" } else { "16" };
    let mut magick = Command::new("magick");
    magick
        .arg("--")
        .arg(crate::safe_path_arg(input).as_ref());
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
            let line = format!("   ❌ ImageMagick failed with exit code: {:?}", status.code());
            crate::progress_mode::emit_stderr(&line);
            if !magick_stderr.is_empty() {
                let line2 = format!("   📋 ImageMagick stderr: {}", magick_stderr.lines().next().unwrap_or(""));
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
            let line = format!("   ❌ cjxl failed with exit code: {:?}", status.code());
            crate::progress_mode::emit_stderr(&line);
            if !cjxl_stderr.is_empty() {
                crate::progress_mode::emit_stderr(&format!("   📋 cjxl stderr: {}", cjxl_stderr));
            }
            false
        }
        Err(e) => {
            let line = format!("   ❌ Failed to wait for cjxl: {}", e);
            crate::progress_mode::emit_stderr(&line);
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
///
/// Fallback priority:
/// 1. No -strip, depth 16 (preserve metadata)
/// 2a. grayscale+ICC error → -strip, depth 16
///     └─ still fails + decode/pixel error + 8-bit source → -strip, depth 8 (no quality loss)
///     └─ still fails + 16-bit source → normalize ICC to sRGB, keep depth 16
///        └─ still fails → error, refuse to downgrade
/// 2b. decode/pixel error + 8-bit source → -strip, depth 8 (no quality loss)
/// 2b. decode/pixel error + 16-bit source → normalize ICC to sRGB, keep depth 16
///     └─ still fails → error, refuse to silently downgrade
pub fn try_imagemagick_fallback(
    input: &Path,
    output: &Path,
    distance: f32,
    max_threads: usize,
    apple_compat: bool,
) -> std::result::Result<std::process::Output, std::io::Error> {
    // Attempt 1: no -strip, depth 16, preserve metadata
    match run_imagemagick_cjxl_pipeline(input, output, distance, max_threads, false, 16, false, apple_compat) {
        Ok(out) => {
            crate::progress_mode::fallback_success();
            return Ok(out);
        }
        Err((magick_ok, cjxl_ok, stderr)) => {
            let line = format!(
                "   ❌ ImageMagick pipeline failed for file: {} (magick: {}, cjxl: {})",
                input.display(),
                if magick_ok { "✓" } else { "✗" },
                if cjxl_ok { "✓" } else { "✗" }
            );
            crate::progress_mode::emit_stderr(&line);

            if magick_ok && !cjxl_ok && is_grayscale_icc_cjxl_error(&stderr) {
                // Attempt 2: -strip, depth 16 (drop bad ICC, keep bit depth)
                crate::progress_mode::emit_stderr(
                    "   🔄 Retrying with -strip (grayscale PNG + ICC incompatible with cjxl)",
                );
                match run_imagemagick_cjxl_pipeline(input, output, distance, max_threads, true, 16, false, apple_compat) {
                    Ok(out) => {
                        crate::progress_mode::fallback_success();
                        return Ok(out);
                    }
                    Err((m, c, stderr2)) => {
                        let line = format!(
                            "   ❌ ImageMagick retry failed for file: {} (magick: {}, cjxl: {})",
                            input.display(),
                            if m { "✓" } else { "✗" },
                            if c { "✓" } else { "✗" }
                        );
                        crate::progress_mode::emit_stderr(&line);
                        // Attempt 3: only for 8-bit sources (no quality loss)
                        if m && !c && is_decode_or_pixel_cjxl_error(&stderr2) {
                            let bit_depth = get_png_bit_depth(input).unwrap_or(16);
                            if bit_depth <= 8 {
                                crate::progress_mode::emit_stderr(
                                    "   🔄 Retrying with -depth 8 -strip (8-bit source confirmed, no quality loss)",
                                );
                                if let Ok(out) = run_imagemagick_cjxl_pipeline(
                                    input, output, distance, max_threads, true, 8, false, apple_compat,
                                ) {
                                    crate::progress_mode::fallback_success();
                                    return Ok(out);
                                }
                            } else {
                                // Attempt 4: normalize ICC to sRGB, preserve 16-bit depth
                                crate::progress_mode::emit_stderr(
                                    "   🔄 Retrying with ICC normalization to sRGB (16-bit source, no depth downgrade)",
                                );
                                match run_imagemagick_cjxl_pipeline(
                                    input, output, distance, max_threads, false, 16, true, apple_compat,
                                ) {
                                    Ok(out) => {
                                        crate::progress_mode::fallback_success();
                                        return Ok(out);
                                    }
                                    Err(_) => {
                                        crate::progress_mode::emit_stderr(
                                            "   ⚠️ 16-bit source: ICC normalization failed, refusing to downgrade to 8-bit",
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
                        "   🔄 Retrying with -depth 8 -strip (8-bit source confirmed, no quality loss)",
                    );
                    if let Ok(out) = run_imagemagick_cjxl_pipeline(
                        input, output, distance, max_threads, true, 8, false, apple_compat,
                    ) {
                        crate::progress_mode::fallback_success();
                        return Ok(out);
                    }
                } else {
                    // Attempt 2: normalize ICC to sRGB, preserve 16-bit depth
                    crate::progress_mode::emit_stderr(
                        "   🔄 Retrying with ICC normalization to sRGB (16-bit source, no depth downgrade)",
                    );
                    match run_imagemagick_cjxl_pipeline(
                        input, output, distance, max_threads, false, 16, true, apple_compat,
                    ) {
                        Ok(out) => {
                            crate::progress_mode::fallback_success();
                            return Ok(out);
                        }
                        Err(_) => {
                            crate::progress_mode::emit_stderr(
                                "   ⚠️ 16-bit source: ICC normalization failed, refusing to downgrade to 8-bit",
                            );
                        }
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
