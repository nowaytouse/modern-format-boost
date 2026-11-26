//! Media Metadata Utilities
//!
//! Consolidated utility functions for low-level media header parsing and
//! metadata extraction.

use std::path::Path;

/// Identify standalone GIF originals by signature and decode every frame before admission.
///
/// # Errors
/// Returns an error for unreadable files and for incomplete or undecodable GIF data.
pub fn is_verified_gif(path: &Path) -> std::io::Result<bool> {
    verify_gif_reader(crate::io_utils::open_regular_nofollow(path)?)
}

pub(crate) fn verify_gif_reader(mut file: impl std::io::Read) -> std::io::Result<bool> {
    use std::io::{self, BufRead, Read};
    let mut signature = [0; 6];
    if let Err(error) = file.read_exact(&mut signature) {
        return if error.kind() == io::ErrorKind::UnexpectedEof {
            Ok(false)
        } else {
            Err(error)
        };
    }
    if !matches!(&signature, b"GIF87a" | b"GIF89a") {
        return Ok(false);
    }
    let mut options = gif::DecodeOptions::new();
    options.check_frame_consistency(true);
    // Some encoders end the LZW stream with the GIF sub-block terminator alone.
    // read_next_frame still requires every pixel, and fill_buffer below drains the
    // final codes so an overlong raster cannot pass as a complete frame.
    options.check_lzw_end_code(false);
    let mut decoder = options
        .read_info(signature.as_slice().chain(file))
        .map_err(io::Error::other)?;
    let mut saw_frame = false;
    while let Some(frame) = decoder.read_next_frame().map_err(io::Error::other)? {
        if frame.width == 0 || frame.height == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "empty GIF frame",
            ));
        }
        if decoder
            .fill_buffer(&mut [0u8; 1])
            .map_err(io::Error::other)?
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "GIF frame contains more pixels than its declared dimensions",
            ));
        }
        saw_frame = true;
    }
    if !saw_frame {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "GIF contains no image frames",
        ));
    }
    // The decoder has consumed the GIF trailer. Permit NUL or ASCII whitespace
    // padding only. Custody hashes and imports the entire original, including padding.
    let mut tail = decoder.into_inner();
    loop {
        let chunk = tail.fill_buf()?;
        if chunk.is_empty() {
            return Ok(true);
        }
        if !chunk
            .iter()
            .all(|&byte| byte == 0 || byte.is_ascii_whitespace())
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "additional non-padding data follows the GIF trailer",
            ));
        }
        let len = chunk.len();
        tail.consume(len);
    }
}

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
/// Returns an error if the file cannot be read or if the `GIF` header is
/// malformed.
// Rationale: This function handles complex, sequential initialization or business logic where
// further fragmentation would hinder readability and maintainability.
#[allow(
    clippy::too_many_lines,
    reason = "Complex orchestration logic where fragmenting state into smaller helpers would \
              decrease readability and increase cognitive overhead."
)]
pub fn scan_gif_headers(path: &Path) -> std::io::Result<GifHeaderScan> {
    let buf = std::fs::read(path)?;
    let n = buf.len();

    if n < 13 {
        crate::media_conversion_gate::delivery_runtime_batch_audit(
            "delivery_meta_gif",
            crate::infra::static_logs::messages::MSG_GIF_TOO_SMALL.replace("{}", &n.to_string()),
        );
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "File too small to be a valid GIF (minimum 13 bytes required)",
        ));
    }

    // GIF87a / GIF89a magic check — `n >= 13` guard above ensures bytes 0..6 exist.
    let magic = &buf[0..6];
    if magic != b"GIF87a" && magic != b"GIF89a" {
        crate::media_conversion_gate::delivery_runtime_batch_audit(
            "delivery_meta_gif",
            crate::infra::static_logs::messages::MSG_GIF_INVALID_MAGIC
                .replace("{}", &String::from_utf8_lossy(magic)),
        );
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
        let Some(gct_size) = crate::media_conversion_gate::gif_palette_byte_size_optional(
            palette_size,
            "media_meta_utils gif global color table",
        ) else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "GIF global color table size unavailable; refusing fabricated 0-byte skip",
            ));
        };
        pos = pos.saturating_add(gct_size);
    }

    while pos + 2 < buf.len() {
        // All buf.get() calls below are guarded by explicit pos+N < buf.len() checks.
        // Truncated/malformed GIF data causes a clean break rather than a panic.
        let Some(&block_id) = buf.get(pos) else { break };
        match block_id {
            crate::constants::GIF_BLOCK_EXTENSION_INTRODUCER if pos + 1 < buf.len() => {
                let Some(&ext_type) = buf.get(pos + 1) else {
                    break;
                };
                match ext_type {
                    crate::constants::GIF_BLOCK_APPLICATION_EXTENSION => {
                        let Some(&block_size) = buf.get(pos + 2) else {
                            break;
                        };
                        let block_size = usize::from(block_size);
                        if block_size == 11 && pos + 3 + block_size <= buf.len() {
                            let Some(vendor_bytes) = buf.get(pos + 3..pos + 3 + block_size) else {
                                break;
                            };
                            match std::str::from_utf8(vendor_bytes) {
                                Ok(vendor) if !vendor.is_empty() => {
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
                                                loop_count =
                                                    Some(u16::from(lo) | (u16::from(hi) << 8_i32));
                                            }
                                        }
                                    }
                                }
                                Ok(_) => {}
                                Err(e) => {
                                    crate::media_conversion_gate::probe_layer_batch_audit(
                                        "gif_application_extension",
                                        format!(
                                            "failed to parse GIF application vendor as UTF-8: {e}"
                                        ),
                                    );
                                }
                            }
                        }
                        pos = pos.saturating_add(3).saturating_add(block_size);
                        pos = skip_sub_blocks(&buf, pos);
                    }
                    crate::constants::GIF_BLOCK_GRAPHICS_CONTROL_EXTENSION
                        if pos + 7 < buf.len() =>
                    {
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
            crate::constants::GIF_BLOCK_IMAGE_DESCRIPTOR => {
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
            crate::constants::GIF_BLOCK_TRAILER => break,
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

    let total_duration_secs = {
        let secs = frame_delays_cs.iter().map(|&d| f64::from(d)).sum::<f64>()
            / crate::constants::GIF_CENTISECONDS_PER_SECOND;
        // Only report a duration when the GIF actually encodes positive delay.
        // All-zero delays (secs == 0.0) are not a valid duration; returning
        // Some(0.0) would overwrite any ffprobe-derived duration and then fail
        // the `duration_secs > 0` invariant in validate_loop_training_sample.
        if frame_delays_cs.is_empty() || !secs.is_finite() || secs <= 0.0 {
            None
        } else {
            Some(secs)
        }
    };

    // A GIF with >4 billion frames would require ~4TB of data; `u32::try_from` here
    // would only fail if the Vec grew beyond u32::MAX entries, which is impossible
    // in practice. If it somehow did, that is anomalous data — return Err
    // rather than silently wrapping or using a sentinel.
    let _payload_count = u32::try_from(frame_payload_sizes.len()).map_err(|_| {
        crate::media_conversion_gate::delivery_runtime_batch_audit(
            "delivery_meta_gif",
            crate::infra::static_logs::messages::MSG_GIF_OVERFLOW_FRAME
                .replace("{}", &frame_payload_sizes.len().to_string()),
        );
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!(
                "GIF frame payload count {} exceeds u32::MAX — file is anomalous",
                frame_payload_sizes.len()
            ),
        )
    })?;
    let _delay_count = u32::try_from(frame_delays_cs.len()).map_err(|_| {
        crate::media_conversion_gate::delivery_runtime_batch_audit(
            "delivery_meta_gif",
            crate::infra::static_logs::messages::MSG_GIF_OVERFLOW_DELAY
                .replace("{}", &frame_delays_cs.len().to_string()),
        );
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!(
                "GIF frame delay count {} exceeds u32::MAX — file is anomalous",
                frame_delays_cs.len()
            ),
        )
    })?;
    // Honest reporting: `frame_count_direct` is the count of image descriptors
    // we actually saw in the GIF stream. If a file has none, we report 0 (the
    // truth). Callers must decide whether 0 frames is anomalous; we do not
    // fabricate a frame.
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
        let next_pos = pos.saturating_add(usize::from(sub_size));
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn verified_gif_checks_pixels_and_trailer_independently_of_suffix() -> anyhow::Result<()> {
        let dir = TempDir::new()?;
        let path = dir.path().join("no-extension");
        let mut data = Vec::new();
        {
            let mut encoder = gif::Encoder::new(&mut data, 1, 1, &[0, 0, 0, 255, 255, 255])?;
            for index in [0, 1] {
                encoder.write_frame(&gif::Frame {
                    width: 1,
                    height: 1,
                    delay: 5,
                    buffer: std::borrow::Cow::Owned(vec![index]),
                    ..Default::default()
                })?;
            }
        }
        fs::write(&path, &data)?;
        assert!(is_verified_gif(&path)?);
        fs::write(&path, &data[..data.len() - 1])?;
        assert!(is_verified_gif(&path).is_err());
        data.extend_from_slice(b"extra stream");
        fs::write(&path, data)?;
        assert!(is_verified_gif(&path).is_err());
        let fake = dir.path().join("video.gif");
        fs::write(&fake, b"\0\0\0\x18ftypisom")?;
        assert!(!is_verified_gif(&fake)?);
        Ok(())
    }

    #[test]
    fn verified_gif_accepts_only_padding_after_a_decoded_image() -> anyhow::Result<()> {
        let image = b"GIF89a\x01\x00\x01\x00\x80\x00\x00\x00\x00\x00\xff\xff\xff,\x00\x00\x00\x00\x01\x00\x01\x00\x00\x02\x02\x44\x01\x00;";
        let mut padded = image.to_vec();
        padded.extend_from_slice(b"\r\n\t ");
        padded.extend(std::iter::repeat_n(0, 24 * 1024));
        assert!(verify_gif_reader(padded.as_slice())?);
        for payload in [image.as_slice(), b"data".as_slice(), b"\xff".as_slice()] {
            let stream = [padded.as_slice(), payload].concat();
            let error = verify_gif_reader(stream.as_slice()).expect_err("unverified trailing data");
            assert!(error.to_string().contains("non-padding data follows"));
        }
        let missing_trailer = [&image[..image.len() - 1], b"\0\n "].concat();
        assert!(verify_gif_reader(missing_trailer.as_slice()).is_err());
        let empty = [&image[..19], b";\0"].concat();
        let error =
            verify_gif_reader(empty.as_slice()).expect_err("a palette alone is not an image");
        assert!(error.to_string().contains("no image frames"));
        Ok(())
    }

    #[test]
    fn verified_gif_accepts_legacy_lzw_termination_only_for_complete_frames() -> anyhow::Result<()>
    {
        let dir = TempDir::new()?;
        let path = dir.path().join("animation.gif");
        let screen = b"GIF89a\x01\x00\x01\x00\x80\x00\x00\x00\x00\x00\xff\xff\xff";
        let frame = b",\x00\x00\x00\x00\x01\x00\x01\x00\x00\x02";
        let without_end_code = b"\x01\x04\x00"; // clear + one pixel + sub-block terminator
        let with_end_code = b"\x01\x44\x01\x01\x00"; // end code split across sub-blocks
        let animation = [
            screen.as_slice(),
            frame,
            without_end_code,
            frame,
            with_end_code,
            b";",
        ]
        .concat();
        fs::write(&path, &animation)?;
        assert!(is_verified_gif(&path)?);
        for invalid in [
            [
                screen.as_slice(),
                frame,
                without_end_code,
                frame,
                b"\x01\x2c\x00;",
            ]
            .concat(), // no pixels in last frame
            [screen.as_slice(), frame, b"\x02\x04\x01\x00;"].concat(), // extra decoded pixel
            [screen.as_slice(), frame, b"\x01\x04;"].concat(), // missing sub-block terminator
            animation[..animation.len() - 1].to_vec(),         // missing GIF trailer
        ] {
            fs::write(&path, invalid)?;
            assert!(is_verified_gif(&path).is_err());
        }
        Ok(())
    }

    #[test]
    fn test_scan_gif_headers_invalid_too_small() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("small.gif");
        fs::write(&path, b"GIF89a").unwrap();

        let result = scan_gif_headers(&path);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("too small"));
    }

    #[test]
    fn test_scan_gif_headers_invalid_magic() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("bad_magic.gif");
        let mut data = vec![0u8; 20];
        data[0..6].copy_from_slice(b"NOTGIF");
        fs::write(&path, &data).unwrap();

        let result = scan_gif_headers(&path);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("Invalid GIF magic")
        );
    }

    #[test]
    fn test_scan_gif_headers_valid_static() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("static.gif");

        // A minimal valid static 1x1 GIF
        let gif_data: [u8; 35] = [
            0x47, 0x49, 0x46, 0x38, 0x39, 0x61, // GIF89a
            0x01, 0x00, 0x01, 0x00, // Width 1, Height 1
            0x80, 0x00, 0x00, // GCT flags (1 color, 2 bytes)
            0x00, 0x00, 0x00, 0xFF, 0xFF, 0xFF, // Colors
            0x2C, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x01, 0x00, 0x00, // Image Descriptor
            0x02, 0x02, 0x44, 0x01, 0x00, // Image Data
            0x3B, // Trailer
        ];

        fs::write(&path, gif_data).unwrap();

        let scan = scan_gif_headers(&path).unwrap();
        assert_eq!(scan.frame_count, 1);
        assert!(!scan.has_transparency);
        assert_eq!(scan.palette_size, Some(2));
        // Static GIF with no GCE: no delays → duration must be None, not Some(0.0)
        assert_eq!(
            scan.duration_secs, None,
            "static GIF with no frame delays must not emit Some(0.0)"
        );
    }

    #[test]
    fn test_scan_gif_zero_delay_frames_returns_none_duration() {
        // Regression: GIF with all-zero frame delays must return duration_secs=None.
        // Previously returned Some(0.0), which overwrote ffprobe duration and then
        // failed validate_loop_training_sample's `duration_secs > 0` invariant.
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("zero_delay.gif");

        // Minimal GIF89a with one GCE (delay=0x0000) followed by one frame.
        // GIF89a header + LSD (no GCT) = 13 bytes
        // GCE block: 0x21 0xF9 0x04 <packed=0> <delay_lo=0> <delay_hi=0> <trans=0> 0x00
        // Image descriptor: 0x2C + 9 bytes + no LCT
        // Minimal image data: LZW min=2, block=2, data, block-term
        let gif_data: Vec<u8> = vec![
            // Header
            0x47, 0x49, 0x46, 0x38, 0x39, 0x61,
            // Logical Screen Descriptor (6 bytes): 1x1, no GCT
            0x01, 0x00, 0x01, 0x00, 0x00, // packed: no GCT
            0x00, // background
            0x00, // aspect
            // Graphic Control Extension (8 bytes): delay = 0
            0x21, 0xF9, 0x04, 0x00, // packed
            0x00, 0x00, // delay = 0 centiseconds
            0x00, // transparent index
            0x00, // block terminator
            // Image Descriptor (10 bytes)
            0x2C, 0x00, 0x00, 0x00, 0x00, // left, top
            0x01, 0x00, 0x01, 0x00, // width, height
            0x00, // packed: no LCT
            // Image Data
            0x02, // LZW minimum code size
            0x02, // sub-block size
            0x4C, 0x01, // compressed data
            0x00, // sub-block terminator
            // Trailer
            0x3B,
        ];

        fs::write(&path, &gif_data).unwrap();
        let scan = scan_gif_headers(&path).unwrap();
        assert_eq!(scan.frame_count, 1);
        assert_eq!(
            scan.duration_secs, None,
            "GIF with all-zero frame delays must return duration_secs=None, not Some(0.0)"
        );
    }
}
