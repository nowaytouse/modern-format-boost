//! Media Metadata Utilities
//!
//! Consolidated utility functions for low-level media header parsing and metadata extraction.

use std::path::Path;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct GifHeaderScan {
    pub palette_size: Option<u32>,
    pub app_extensions: Option<Vec<String>>,
    pub has_transparency: bool,
    pub frame_payload_variation: Option<f64>,
    pub frame_delay_variation: Option<f64>,
    pub loop_count: Option<u16>,
    pub duration_secs: Option<f64>,
    pub frame_count: u32,
}

/// Scans a GIF file's bytes to extract metadata not easily provided by ffprobe,
/// such as application extensions (GIPHY/TENOR markers), loop counts, and
/// detailed frame delay variation.
///
/// # Errors
/// Returns an error if the file cannot be read or if the `GIF` header is malformed.
#[allow(clippy::too_many_lines)]
pub fn scan_gif_headers(path: &Path) -> std::io::Result<GifHeaderScan> {
    let buf = std::fs::read(path)?;
    let n = buf.len();

    if n < 13 {
        return Ok(GifHeaderScan::default());
    }

    // GIF87a / GIF89a magic check
    let magic = buf.get(0..6).unwrap_or(&[]);
    if magic != b"GIF87a" && magic != b"GIF89a" {
        return Ok(GifHeaderScan::default());
    }

    // Logical Screen Descriptor: byte 10 = packed field
    let packed = buf.get(10).copied().unwrap_or(0);
    let has_gct = (packed & 0x80) != 0;
    let palette_size: Option<u32> = if has_gct {
        let n = u32::from(packed & 0x07);
        Some(2u32.pow(n + 1))
    } else {
        None
    };

    let mut app_extensions: Vec<String> = Vec::new();
    let mut has_transparency = false;
    let mut loop_count: Option<u16> = None;
    let mut frame_payload_sizes: Vec<usize> = Vec::new();
    let mut frame_delays_cs: Vec<u16> = Vec::new();
    let mut pos = 13usize;

    if has_gct {
        let gct_size = palette_size.unwrap_or(0) as usize * 3;
        pos += gct_size;
    }

    while pos + 2 < buf.len() {
        match buf.get(pos).copied().unwrap_or(0) {
            0x21 if pos + 1 < buf.len() => match buf.get(pos + 1).copied().unwrap_or(0) {
                0xFF => {
                    let block_size = buf.get(pos + 2).copied().unwrap_or(0) as usize;
                    if block_size == 11 && pos + 3 + block_size <= buf.len() {
                        if let Ok(vendor) = std::str::from_utf8(
                            buf.get(pos + 3..pos + 3 + block_size).unwrap_or(&[]),
                        ) {
                            if !vendor.is_empty() {
                                app_extensions.push(vendor.to_owned());
                                if vendor == "NETSCAPE2.0" {
                                    let sub_pos = pos + 3 + block_size;
                                    if sub_pos + 3 < buf.len() {
                                        let sub_size = buf.get(sub_pos).copied().unwrap_or(0);
                                        if sub_size >= 3
                                            && buf.get(sub_pos + 1).copied().unwrap_or(0) == 0x01
                                        {
                                            loop_count = Some(
                                                u16::from(
                                                    buf.get(sub_pos + 2).copied().unwrap_or(0),
                                                ) | (u16::from(
                                                    buf.get(sub_pos + 3).copied().unwrap_or(0),
                                                ) << 8),
                                            );
                                        }
                                    }
                                }
                            }
                        }
                    }
                    pos += 3 + block_size;
                    pos = skip_sub_blocks(&buf, pos);
                }
                0xF9 if pos + 7 < buf.len() && buf.get(pos + 2).copied().unwrap_or(0) == 0x04 => {
                    if buf.get(pos + 3).copied().unwrap_or(0) & 0x01 != 0 {
                        has_transparency = true;
                    }
                    let delay = u16::from(buf.get(pos + 4).copied().unwrap_or(0))
                        | (u16::from(buf.get(pos + 5).copied().unwrap_or(0)) << 8);
                    frame_delays_cs.push(delay);
                    pos += 8;
                }
                0xFE | 0x01 => {
                    pos += 2;
                    pos = skip_sub_blocks(&buf, pos);
                }
                _ => {
                    pos += 1;
                }
            },
            0x2C => {
                if pos + 10 >= buf.len() {
                    break;
                }
                let packed = buf.get(pos + 9).copied().unwrap_or(0);
                pos += 10;
                if (packed & 0x80) != 0 {
                    let lct_size_pow = usize::from(packed & 0x07);
                    let lct_size = 3 * (1usize << (lct_size_pow + 1));
                    if pos + lct_size > buf.len() {
                        break;
                    }
                    pos += lct_size;
                }
                if pos >= buf.len() {
                    break;
                }
                pos += 1;
                let payload_start = pos;
                pos = skip_sub_blocks(&buf, pos);
                let payload_size = pos.saturating_sub(payload_start);
                if payload_size > 0 {
                    frame_payload_sizes.push(payload_size);
                }
            }
            0x3B => break,
            _ => {
                pos += 1;
            }
        }
    }

    let app_extensions = if app_extensions.is_empty() {
        None
    } else {
        Some(app_extensions)
    };

    let frame_payload_variation = if frame_payload_sizes.len() >= 2 {
        let mean = crate::numeric_cast::usize_to_f64(frame_payload_sizes.iter().sum::<usize>())
            / crate::numeric_cast::usize_to_f64(frame_payload_sizes.len());
        if mean > 0.0 {
            let variance = frame_payload_sizes
                .iter()
                .map(|&size| {
                    let diff = crate::numeric_cast::usize_to_f64(size) - mean;
                    diff * diff
                })
                .sum::<f64>()
                / crate::numeric_cast::usize_to_f64(frame_payload_sizes.len());
            Some((variance.sqrt() / mean).clamp(0.0, 2.0))
        } else {
            None
        }
    } else {
        None
    };

    let frame_delay_variation = if frame_delays_cs.len() >= 2 {
        let mean = frame_delays_cs.iter().map(|&d| f64::from(d)).sum::<f64>()
            / crate::numeric_cast::usize_to_f64(frame_delays_cs.len());
        if mean > 0.0 {
            let variance = frame_delays_cs
                .iter()
                .map(|&delay| {
                    let diff = f64::from(delay) - mean;
                    diff * diff
                })
                .sum::<f64>()
                / crate::numeric_cast::usize_to_f64(frame_delays_cs.len());
            Some((variance.sqrt() / mean).clamp(0.0, 2.0))
        } else {
            None
        }
    } else {
        None
    };

    let total_duration_secs = if frame_delays_cs.is_empty() {
        None
    } else {
        Some(frame_delays_cs.iter().map(|&d| f64::from(d)).sum::<f64>() / 100.0)
    };

    let frame_count_calculated = std::cmp::max(
        u32::try_from(frame_payload_sizes.len()).unwrap_or(u32::MAX),
        u32::try_from(frame_delays_cs.len()).unwrap_or(u32::MAX),
    );
    let frame_count_calculated = if frame_count_calculated == 0 {
        1
    } else {
        frame_count_calculated
    };

    Ok(GifHeaderScan {
        palette_size,
        app_extensions,
        has_transparency,
        frame_payload_variation,
        frame_delay_variation,
        loop_count,
        duration_secs: total_duration_secs,
        frame_count: frame_count_calculated,
    })
}

fn skip_sub_blocks(buf: &[u8], mut pos: usize) -> usize {
    loop {
        let Some(&sub_size) = buf.get(pos) else {
            return buf.len();
        };
        pos += 1;
        if sub_size == 0 {
            return pos;
        }
        pos = pos.saturating_add(sub_size as usize);
        if pos > buf.len() {
            return buf.len();
        }
    }
}
