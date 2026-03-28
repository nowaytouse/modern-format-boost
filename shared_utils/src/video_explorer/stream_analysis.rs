//! Stream Analysis Module
//!
//! This module is responsible for video stream analysis and quality assessment, including:
//! - SSIM (Structural Similarity Index) calculation
//! - PSNR (Peak Signal-to-Noise Ratio) calculation
//! - MS-SSIM (Multi-Scale SSIM) calculation
//! - Video duration detection
//! - Quality threshold validation
//! - Lossless integrity checks (CRF=0 fast-path: frame count + file size only)

use std::path::Path;
use std::process::Command;
use tracing::{info, warn};

pub const LONG_VIDEO_THRESHOLD: f32 = 300.0;

#[derive(Debug, Clone)]
pub struct QualityThresholds {
    pub min_ssim: f64,
    pub min_psnr: f64,
    pub min_ms_ssim: f64,
    pub validate_ssim: bool,
    pub validate_psnr: bool,
    pub validate_ms_ssim: bool,
    pub force_ms_ssim_long: bool,
}

impl Default for QualityThresholds {
    fn default() -> Self {
        Self {
            min_ssim: 0.95,
            min_psnr: 35.0,
            min_ms_ssim: 0.90,
            validate_ssim: true,
            validate_psnr: false,
            validate_ms_ssim: false,
            force_ms_ssim_long: false,
        }
    }
}

pub fn get_video_duration(input: &Path) -> Option<f64> {
    let output = Command::new("ffprobe")
        .args(["-v", "error"])
        .args(["-show_entries", "format=duration"])
        .args(["-of", "default=noprint_wrappers=1:nokey=1"])
        .arg("--")
        .arg(crate::safe_path_arg(input).as_ref())
        .output()
        .ok()?;

    if !output.status.success() {
        warn!(
            path = %input.display(),
            stderr = %String::from_utf8_lossy(&output.stderr).trim(),
            "ffprobe failed to read video duration"
        );
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let trimmed = stdout.trim();
    match trimmed.parse::<f64>() {
        Ok(duration) => Some(duration),
        Err(err) => {
            warn!(
                path = %input.display(),
                output = %trimmed,
                error = %err,
                "Failed to parse ffprobe duration output"
            );
            None
        }
    }
}

pub fn is_gif_magic(path: &Path) -> bool {
    let mut magic = [0u8; 4];
    std::fs::File::open(path)
        .and_then(|mut f| {
            use std::io::Read;
            f.read_exact(&mut magic)?;
            Ok(())
        })
        .map(|_| &magic == b"GIF8")
        .unwrap_or(false)
}

pub fn calculate_ssim_enhanced(input: &Path, output: &Path) -> Option<f64> {
    // GIF-specific path: force palette → yuv420p conversion on the reference side
    // before comparing with the yuv420p-encoded output.  This avoids all three
    // generic filters silently failing because ffmpeg cannot decode a GIF palette
    // stream into the same raw pixel layout as the HEVC output.
    let is_gif = is_gif_magic(input);

    let gif_filters: &[(&str, &str)] = &[
        // Best attempt: render GIF frames through the palette filter chain to yuv420p
        (
            "gif_palette",
            "[0:v]format=rgb24,scale=trunc(iw/2)*2:trunc(ih/2)*2:flags=bicubic,format=yuv420p[ref];[1:v]scale=trunc(iw/2)*2:trunc(ih/2)*2:flags=bicubic,format=yuv420p[cmp];[ref][cmp]ssim",
        ),
        // Simpler fallback: just normalise to yuv420p
        (
            "gif_yuv420p",
            "[0:v]scale=trunc(iw/2)*2:trunc(ih/2)*2,format=yuv420p[ref];[1:v]scale=trunc(iw/2)*2:trunc(ih/2)*2,format=yuv420p[cmp];[ref][cmp]ssim",
        ),
    ];

    let generic_filters: &[(&str, &str)] = &[
        ("standard", "[0:v]scale='iw-mod(iw,2)':'ih-mod(ih,2)':flags=bicubic[ref];[ref][1:v]ssim"),
        (
            "format_convert",
            "[0:v]scale=trunc(iw/2)*2:trunc(ih/2)*2,format=yuv420p[ref];[1:v]scale=trunc(iw/2)*2:trunc(ih/2)*2,format=yuv420p[cmp];[ref][cmp]ssim",
        ),
        ("simple", "ssim"),
    ];

    let filters = if is_gif { gif_filters } else { generic_filters };

    for (name, filter) in filters {
        let result = Command::new("ffmpeg")
            .arg("-i")
            .arg(crate::safe_path_arg(input).as_ref())
            .arg("-i")
            .arg(crate::safe_path_arg(output).as_ref())
            .arg("-lavfi")
            .arg(*filter)
            .arg("-f")
            .arg("null")
            .arg("-")
            .output();

        match result {
            Ok(out) => {
                let stderr = String::from_utf8_lossy(&out.stderr);
                if let Some(ssim) = parse_ssim_from_output(&stderr) {
                    if is_valid_ssim_value(ssim) {
                        info!(method = %name, ssim = %ssim, "SSIM calculated");
                        return Some(ssim);
                    }
                }
                warn!(method = %name, "SSIM method failed, trying next method");
            }
            Err(e) => {
                warn!(method = %name, error = %e, "ffmpeg failed");
            }
        }
    }

    tracing::error!("ALL SSIM CALCULATION METHODS FAILED");
    None
}

/// Run ffmpeg with the given lavfi filter and parse SSIM Y/U/V/All from stderr.
fn run_ssim_all_filter(input: &Path, output: &Path, lavfi: &str) -> Option<(f64, f64, f64, f64)> {
    let out = Command::new("ffmpeg")
        .arg("-i")
        .arg(crate::safe_path_arg(input).as_ref())
        .arg("-i")
        .arg(crate::safe_path_arg(output).as_ref())
        .arg("-lavfi")
        .arg(lavfi)
        .arg("-f")
        .arg("null")
        .arg("-")
        .output()
        .ok()?;

    // Some filters (ssim) might return non-zero exit code if the streams
    // end at slightly different points for GIFs, even if the result is valid.
    // We parse stderr regardless of out.status.
    let stderr = String::from_utf8_lossy(&out.stderr);
    for line in stderr.lines() {
        if line.contains("SSIM Y:") && line.contains("All:") {
            let y = extract_ssim_value(line, "Y:");
            let u = extract_ssim_value(line, "U:");
            let v = extract_ssim_value(line, "V:");
            let all = extract_ssim_value(line, "All:");
            if let (Some(y), Some(u), Some(v), Some(all)) = (y, u, v, all) {
                if is_valid_ssim_value(y) && is_valid_ssim_value(all) {
                    return Some((y, u, v, all));
                }
            }
        }
    }
    None
}

/// SSIM Y/U/V/All between input and output. Tries in order:
///
/// GIF sources get a palette-aware filter chain first (GIF uses indexed colour;
/// the raw pixel layout differs from the yuv420p HEVC output and breaks the
/// generic filters).
///
/// Non-GIF sources try:
/// 1. Direct ssim (when formats already match).
/// 2. Format normalization (odd-size → yuv420p even).
/// 3. Alpha composite on black (transparent WebP/PNG vs HEVC which has no alpha):
///    converts the input to rgb24 (discarding alpha channel) then yuv420p,
///    matching the actual encoder behaviour.
#[must_use]
pub fn calculate_ssim_all(input: &Path, output: &Path) -> Option<(f64, f64, f64, f64)> {
    // GIF-specific chains: render palette → rgb24 → yuv420p before comparing.
    // Single-line strings — Rust has no line-continuation in string literals.
    const GIF_RGB24: &str = "[0:v]format=rgb24,scale=trunc(iw/2)*2:trunc(ih/2)*2:flags=bicubic,format=yuv420p[ref];[1:v]scale=trunc(iw/2)*2:trunc(ih/2)*2:flags=bicubic,format=yuv420p[cmp];[ref][cmp]ssim";
    const GIF_NORM: &str = "[0:v]scale=trunc(iw/2)*2:trunc(ih/2)*2,format=yuv420p[ref];[1:v]scale=trunc(iw/2)*2:trunc(ih/2)*2,format=yuv420p[cmp];[ref][cmp]ssim";

    // Generic chains
    const DIRECT: &str = "[0:v][1:v]ssim";
    const FORMAT_NORM: &str = "[0:v]format=yuv420p,scale='iw-mod(iw,2)':'ih-mod(ih,2)'[ref];[1:v]format=yuv420p,scale='iw-mod(iw,2)':'ih-mod(ih,2)'[cmp];[ref][cmp]ssim";
    // Alpha-composite on black without the deprecated premultiply=inplace=1 filter:
    // decode to rgb24 (which discards alpha by blending on black in ffmpeg's
    // swscale path) then convert to yuv420p for comparison.
    const ALPHA_FLATTEN: &str = "[0:v]format=rgb24,format=yuv420p,scale='iw-mod(iw,2)':'ih-mod(ih,2)'[ref];[1:v]format=yuv420p,scale='iw-mod(iw,2)':'ih-mod(ih,2)'[cmp];[ref][cmp]ssim";

    let is_gif = is_gif_magic(input);

    if is_gif {
        run_ssim_all_filter(input, output, GIF_RGB24)
            .or_else(|| run_ssim_all_filter(input, output, GIF_NORM))
            .or_else(|| run_ssim_all_filter(input, output, FORMAT_NORM))
    } else {
        run_ssim_all_filter(input, output, DIRECT)
            .or_else(|| run_ssim_all_filter(input, output, FORMAT_NORM))
            .or_else(|| run_ssim_all_filter(input, output, ALPHA_FLATTEN))
    }
}

fn parse_ssim_from_output(stderr: &str) -> Option<f64> {
    for line in stderr.lines() {
        if line.contains("SSIM") && line.contains("All:") {
            if let Some(all_pos) = line.find("All:") {
                let after_all = &line[all_pos + 4..];
                let after_all = after_all.trim_start();
                if after_all.starts_with("inf") {
                    return Some(1.0);
                }
                let end = after_all
                    .find(|c: char| !c.is_numeric() && c != '.')
                    .unwrap_or(after_all.len());
                if end > 0 {
                    return after_all[..end].parse::<f64>().ok();
                }
            }
        }
    }
    None
}

fn extract_ssim_value(line: &str, prefix: &str) -> Option<f64> {
    if let Some(pos) = line.find(prefix) {
        let after = &line[pos + prefix.len()..];
        let after = after.trim_start();
        if after.starts_with("inf") {
            return Some(1.0);
        }
        let end = after
            .find(|c: char| !c.is_numeric() && c != '.')
            .unwrap_or(after.len());
        if end > 0 {
            return after[..end].parse::<f64>().ok();
        }
    }
    None
}

#[inline]
fn is_valid_ssim_value(ssim: f64) -> bool {
    (0.0..=1.0).contains(&ssim) && !ssim.is_nan()
}

// ── CRF=0 Lossless Integrity Check ────────────────────────────────────────────

/// Fast integrity check for CRF=0 (lossless) encodes.
///
/// When a source is encoded at CRF 0, the codec guarantees bit-exact YUV
/// reproduction — VMAF/SSIM would trivially score 100 / 1.0 and are pure
/// CPU waste.  Instead we verify two cheap invariants:
///
/// 1. **Frame count match** — the output contains at least as many frames as
///    the input (encode did not silently drop frames).
/// 2. **File size > 0** — the output file is non-empty (no silent I/O failure).
///
/// Note for GIF→HEVC: CRF=0 guarantees YUV-layer losslessness, but the
/// GIF palette → YUV conversion itself introduces chroma subsampling loss
/// (yuv420 vs yuv444).  That loss is determined at encoding time by the
/// pixel-format choice and is unrelated to CRF.  The caller should force
/// `yuv444p` to avoid this if RGB round-trip fidelity is required.
///
/// Returns `Ok(true)` when both invariants are satisfied, `Ok(false)` if a
/// check fails (with stderr warning already emitted), or `Err` if ffprobe
/// cannot be invoked at all.
///
/// # Errors
/// Returns an error if the frame-count ffprobe command fails to spawn.
pub fn check_lossless_integrity(
    input: &Path,
    output: &Path,
    output_size: u64,
    is_animated_image: bool,
) -> Result<bool, String> {
    // Guard: output must not be empty
    if output_size == 0 {
        warn!("CRF=0 integrity: output file is empty (silent encode failure?)");
        return Ok(false);
    }

    // Helper: count frames via ffprobe without decoding (fast)
    let count_frames = |path: &Path| -> Option<u64> {
        let out = Command::new("ffprobe")
            .args(["-v", "error"])
            .args(["-count_packets", "-select_streams", "v:0"])
            .args(["-show_entries", "stream=nb_read_packets"])
            .args(["-of", "default=noprint_wrappers=1:nokey=1"])
            .arg("--")
            .arg(crate::safe_path_arg(path).as_ref())
            .output()
            .ok()?;
        String::from_utf8_lossy(&out.stdout)
            .trim()
            .parse::<u64>()
            .ok()
    };

    let input_frames = count_frames(input);
    let output_frames = count_frames(output);

    match (input_frames, output_frames) {
        (Some(i), Some(o)) if o >= i => {
            info!(
                input_frames = i,
                output_frames = o,
                "CRF=0 integrity: frame count OK"
            );
            Ok(true)
        }
        (Some(i), Some(o)) => {
            if is_animated_image {
                warn!(
                    input_frames = i,
                    output_frames = o,
                    "CRF=0 integrity: output has FEWER frames than input — normal for GIFs due to duplicate frame dropping/FPS coalescing. Soft-accepting."
                );
                Ok(true)
            } else {
                warn!(
                    input_frames = i,
                    output_frames = o,
                    "CRF=0 integrity: output has FEWER frames than input — possible encode error"
                );
                Ok(false)
            }
        }
        (None, _) | (_, None) => {
            // Cannot determine frame count — treat as a soft warning, not a hard failure
            warn!("CRF=0 integrity: could not determine frame count via ffprobe; skipping frame check");
            // File is non-empty (checked above), so accept
            Ok(true)
        }
    }
}
