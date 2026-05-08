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
// Rationale: This function handles complex, sequential initialization or business logic where further fragmentation would hinder readability and maintainability.
#[allow(
    clippy::too_many_lines,
    reason = "Complex orchestration logic where fragmenting state into smaller helpers would decrease readability and increase cognitive overhead."
)]
pub fn scan_gif_headers(path: &Path) -> std::io::Result<GifHeaderScan> {
    let buf = std::fs::read(path)?;
    let n = buf.len();

    if n < 13 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "File too small to be a valid GIF (minimum 13 bytes required)",
        ));
    }

    // GIF87a / GIF89a magic check — `n >= 13` guard above ensures bytes 0..6 exist.
    let magic = &buf[0..6];
    if magic != b"GIF87a" && magic != b"GIF89a" {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!(
                "Invalid GIF magic: expected 'GIF87a' or 'GIF89a', found '{}'",
                String::from_utf8_lossy(magic)
            ),
        ));
    }

    // Logical Screen Descriptor: byte 10 = packed field. Bounds ensured by n >= 13.
    let packed = buf[10];
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
    let mut frame_count_direct = 0u32; // Direct count of image descriptors
    let mut pos = 13usize;

    if has_gct {
        // palette_size is Some when has_gct; set above in the same branch.
        let gct_size = palette_size.map_or(0, |p| p as usize * 3);
        pos = pos.saturating_add(gct_size);
    }

    while pos + 2 < buf.len() {
        // All buf.get() calls below are guarded by explicit pos+N < buf.len() checks.
        // Truncated/malformed GIF data causes a clean break rather than a panic.
        let Some(&block_id) = buf.get(pos) else { break };
        match block_id {
            0x21 if pos + 1 < buf.len() => {
                let Some(&ext_type) = buf.get(pos + 1) else {
                    break;
                };
                match ext_type {
                    0xFF => {
                        let Some(&block_size) = buf.get(pos + 2) else {
                            break;
                        };
                        let block_size = block_size as usize;
                        if block_size == 11
                            && pos + 3 + block_size <= buf.len()
                            && let Some(vendor_bytes) = buf.get(pos + 3..pos + 3 + block_size)
                            && let Ok(vendor) = std::str::from_utf8(vendor_bytes)
                            && !vendor.is_empty()
                        {
                            app_extensions.push(vendor.to_owned());
                            if vendor == "NETSCAPE2.0" {
                                let sub_pos = pos + 3 + block_size;
                                if sub_pos + 3 < buf.len() {
                                    let Some(&sub_size) = buf.get(sub_pos) else {
                                        break;
                                    };
                                    let Some(&sub1) = buf.get(sub_pos + 1) else {
                                        break;
                                    };
                                    if sub_size >= 3 && sub1 == 0x01 {
                                        let Some(&lo) = buf.get(sub_pos + 2) else {
                                            break;
                                        };
                                        let Some(&hi) = buf.get(sub_pos + 3) else {
                                            break;
                                        };
                                        loop_count = Some(u16::from(lo) | (u16::from(hi) << 8_i32));
                                    }
                                }
                            }
                        }
                        pos = pos.saturating_add(3).saturating_add(block_size);
                        pos = skip_sub_blocks(&buf, pos);
                    }
                    0xF9 if pos + 7 < buf.len() => {
                        let Some(&gce_size) = buf.get(pos + 2) else {
                            break;
                        };
                        if gce_size != 0x04 {
                            pos += 1;
                            continue;
                        }
                        let Some(&flags) = buf.get(pos + 3) else {
                            break;
                        };
                        if flags & 0x01 != 0 {
                            has_transparency = true;
                        }
                        let Some(&delay_lo) = buf.get(pos + 4) else {
                            break;
                        };
                        let Some(&delay_hi) = buf.get(pos + 5) else {
                            break;
                        };
                        let delay = u16::from(delay_lo) | (u16::from(delay_hi) << 8_i32);
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
                }
            }
            0x2C => {
                // Direct count of image descriptors
                frame_count_direct += 1;

                if pos + 10 >= buf.len() {
                    break;
                }
                let packed = buf[pos + 9]; // bounds: pos+10 < buf.len() checked above
                pos += 10;
                if (packed & 0x80) != 0 {
                    let lct_size_pow = usize::from(packed & 0x07);
                    let lct_size = 3usize.saturating_mul(1usize << lct_size_pow.saturating_add(1));
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
                // Use original skip_sub_blocks to correctly handle image data blocks
                pos = skip_sub_blocks(&buf, pos);
                let payload_size = pos.saturating_sub(payload_start);
                // Always count frames when we encounter an image descriptor
                frame_payload_sizes.push(payload_size.max(1));
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
        if mean > 0.0_f64 {
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
        if mean > 0.0_f64 {
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
        Some(frame_delays_cs.iter().map(|&d| f64::from(d)).sum::<f64>() / 100.0_f64)
    };

    // A GIF with >4 billion frames would require ~4TB of data; `u32::try_from` here
    // would only fail if the Vec grew beyond u32::MAX entries, which is impossible in
    // practice. If it somehow did, that is anomalous data — return Err rather than
    // silently wrapping or using a sentinel.
    let _payload_count = u32::try_from(frame_payload_sizes.len()).map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!(
                "GIF frame payload count {} exceeds u32::MAX — file is anomalous",
                frame_payload_sizes.len()
            ),
        )
    })?;
    let _delay_count = u32::try_from(frame_delays_cs.len()).map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!(
                "GIF frame delay count {} exceeds u32::MAX — file is anomalous",
                frame_delays_cs.len()
            ),
        )
    })?;
    // Honest reporting: `frame_count_direct` is the count of image descriptors
    // we actually saw in the GIF stream. If a file has none, we report 0 (the truth).
    // Callers must decide whether 0 frames is anomalous; we do not fabricate a frame.
    let frame_count_calculated = frame_count_direct;

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
        if pos >= buf.len() {
            return buf.len();
        }

        let Some(&sub_size) = buf.get(pos) else {
            return buf.len();
        };
        pos += 1;

        if sub_size == 0 {
            return pos;
        }

        // Ensure we don't skip past the buffer length
        let next_pos = pos.saturating_add(sub_size as usize);
        if next_pos > buf.len() {
            return buf.len();
        }

        pos = next_pos;

        // Add safety check to prevent infinite loop
        if pos > buf.len() + 1000 {
            return buf.len();
        }
    }
}
