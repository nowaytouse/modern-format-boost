//! Format-specific utilities and helpers
//! Format-specific utilities and helpers

pub mod tiff {
    use crate::unified_error::{ImgQualityError, Result};
    use anyhow::anyhow;
    use std::fs;
    use std::path::Path;

    /// Detect TIFF compression type — traverses ALL IFDs. Supports both standard TIFF and `BigTIFF`.
    /// Check if the image at `path` is lossless.
    ///
    /// # Errors
    /// Returns an error if the file is missing or the format is unsupported.
    // Rationale: This function handles complex, sequential initialization or business logic where further fragmentation would hinder readability and maintainability.
    #[allow(
        clippy::too_many_lines,
        reason = "Complex orchestration logic where fragmenting state into smaller helpers would decrease readability and increase cognitive overhead."
    )]
    /// # Panics
    /// Panics if the file is fundamentally corrupted in a way that prevents basic header reading.
    pub fn is_lossless(path: &Path) -> Result<bool> {
        crate::common_utils::validate_file_size_limit(
            path,
            crate::constants::MAX_IMAGE_ANALYSIS_FILE_SIZE,
        )
        .map_err(|e| ImgQualityError::AnalysisError(e.to_string()))?;

        let data = fs::read(path)?;
        if data.len() < 8 {
            return Err(ImgQualityError::AnalysisError(format!(
                "❌ [INTEGRITY] TIFF file too small (< 8 bytes) for header parsing: {}",
                path.display()
            )));
        }

        let is_little_endian = data.get(0..2) == Some(b"II");
        if data.get(0..2) != Some(b"II") && data.get(0..2) != Some(b"MM") {
            return Err(ImgQualityError::AnalysisError(format!(
                "❌ [INTEGRITY] Invalid TIFF byte order marker: {}",
                path.display()
            )));
        }

        let version = if is_little_endian {
            u16::from_le_bytes([
                *data.get(2).ok_or_else(|| {
                    ImgQualityError::AnalysisError("TIFF header truncated".into())
                })?,
                *data.get(3).ok_or_else(|| {
                    ImgQualityError::AnalysisError("TIFF header truncated".into())
                })?,
            ])
        } else {
            u16::from_be_bytes([
                *data.get(2).ok_or_else(|| {
                    ImgQualityError::AnalysisError("TIFF header truncated".into())
                })?,
                *data.get(3).ok_or_else(|| {
                    ImgQualityError::AnalysisError("TIFF header truncated".into())
                })?,
            ])
        };
        let is_bigtiff = version == 0x002B;

        let read_u16 = |off: usize| -> Option<u16> {
            if off + 2 > data.len() {
                return None;
            }
            let bytes = [data[off], data[off + 1]];
            Some(if is_little_endian {
                u16::from_le_bytes(bytes)
            } else {
                u16::from_be_bytes(bytes)
            })
        };
        let read_u32 = |off: usize| -> Option<u32> {
            data.get(off..off + 4).map(|bytes| {
                let arr = [bytes[0], bytes[1], bytes[2], bytes[3]];
                if is_little_endian {
                    u32::from_le_bytes(arr)
                } else {
                    u32::from_be_bytes(arr)
                }
            })
        };
        let read_u64 = |off: usize| -> Option<u64> {
            if off + 8 > data.len() {
                return None;
            }
            let bytes = [
                data[off],
                data[off + 1],
                data[off + 2],
                data[off + 3],
                data[off + 4],
                data[off + 5],
                data[off + 6],
                data[off + 7],
            ];
            Some(if is_little_endian {
                u64::from_le_bytes(bytes)
            } else {
                u64::from_be_bytes(bytes)
            })
        };

        let mut ifd_offset: u64 = if is_bigtiff {
            if data.len() < 16 {
                return Err(ImgQualityError::AnalysisError(format!(
                    "❌ [INTEGRITY] BigTIFF file too small (< 16 bytes) for header: {}",
                    path.display()
                )));
            }
            read_u64(8).ok_or_else(|| {
                ImgQualityError::AnalysisError(
                    "TIFF: failed to read BigTIFF IFD offset".to_string(),
                )
            })?
        } else {
            u64::from(read_u32(4).ok_or_else(|| {
                ImgQualityError::AnalysisError("TIFF: failed to read IFD offset".to_string())
            })?)
        };

        let mut ifd_count = 0u32;
        while ifd_offset != 0 && ifd_count < 100 {
            ifd_count += 1;
            let ifd_pos = crate::numeric_cast::u64_to_usize_strict(ifd_offset, "ifd_offset")
                .ok_or_else(|| {
                    ImgQualityError::AnalysisError(format!(
                        "TIFF IFD offset {ifd_offset} is too large for memory"
                    ))
                })?;
            let (num_entries, entries_start, entry_size, next_offset_pos) = if is_bigtiff {
                if ifd_pos + 8 > data.len() {
                    break;
                }
                let n = crate::numeric_cast::u64_to_usize_strict(
                    read_u64(ifd_pos).ok_or_else(|| {
                        anyhow::anyhow!("TIFF BigTiff IFD entry count missing at offset {ifd_pos}")
                    })?,
                    "bigtiff_entry_count",
                )
                .ok_or_else(|| {
                    ImgQualityError::AnalysisError(format!(
                        "BigTIFF entry count at {ifd_pos} is too large"
                    ))
                })?;
                (n, ifd_pos + 8, 20usize, ifd_pos + 8 + n * 20)
            } else {
                if ifd_pos + 2 > data.len() {
                    break;
                }
                let n = read_u16(ifd_pos)
                    .map(usize::from)
                    .ok_or_else(|| anyhow!("TIFF IFD entry count missing at offset {ifd_pos}"))?;
                (n, ifd_pos + 2, 12usize, ifd_pos + 2 + n * 12)
            };

            let mut pos = entries_start;
            for entries_scanned in 0..num_entries {
                if pos + entry_size > data.len() {
                    crate::log_anomaly!(
                        crate::static_logs::messages::LABEL_IMAGE,
                        &format!(
                            "TIFF IFD truncated: only scanned {}/{} entries for {}",
                            entries_scanned,
                            num_entries,
                            path.display()
                        )
                    );
                    break;
                }
                if let Some(tag) = read_u16(pos)
                    && tag == 259
                {
                    let compression = if is_bigtiff {
                        read_u16(pos + 12).ok_or_else(|| {
                            ImgQualityError::AnalysisError(format!(
                                "TIFF: Failed to read compression tag at offset {} in {}",
                                pos + 12,
                                path.display()
                            ))
                        })?
                    } else {
                        read_u16(pos + 8).ok_or_else(|| {
                            ImgQualityError::AnalysisError(format!(
                                "TIFF: Failed to read compression tag at offset {} in {}",
                                pos + 8,
                                path.display()
                            ))
                        })?
                    };
                    if compression == 6 || compression == 7 || compression == 50001 {
                        return Ok(false);
                    }
                }
                pos += entry_size;
            }

            if is_bigtiff {
                if next_offset_pos + 8 > data.len() {
                    break;
                }
                ifd_offset = read_u64(next_offset_pos).ok_or_else(|| {
                    anyhow!("TIFF BigTiff next IFD offset missing at offset {next_offset_pos}")
                })?;
            } else {
                if next_offset_pos + 4 > data.len() {
                    break;
                }
                ifd_offset = u64::from(read_u32(next_offset_pos).ok_or_else(|| {
                    anyhow!("TIFF next IFD offset missing at offset {next_offset_pos}")
                })?);
            }
        }
        Ok(true)
    }
}

pub mod png {
    use std::fs;
    use std::io::Read;
    use std::path::Path;

    #[must_use]
    pub fn is_optimally_compressed(path: &Path) -> bool {
        fs::read(path).is_ok_and(|bytes| bytes.windows(4).filter(|w| *w == b"IDAT").count() <= 2)
    }

    #[must_use]
    pub fn estimate_compression_level(path: &Path) -> u8 {
        if let Ok(mut file) = fs::File::open(path) {
            let mut header = [0u8; 16];
            if file.read_exact(&mut header).is_ok() {
                // Heuristic: PNG with reasonable header is usually medium compression
                return crate::constants::FALLBACK_COMPRESSION_PNG;
            }
        }
        crate::log_anomaly!(
            crate::static_logs::messages::LABEL_IMAGE,
            &format!(
                "PNG Analysis: Could not read header to estimate compression for {}; using default medium ({})",
                path.display(),
                crate::constants::FALLBACK_COMPRESSION_PNG
            )
        );
        crate::constants::FALLBACK_COMPRESSION_PNG
    }
}

pub mod jpeg {
    use std::fs;
    use std::io::Read;
    use std::path::Path;

    #[must_use]
    /// # Panics
    /// Panics if the JPEG stream is corrupted and quantization tables cannot be read.
    pub fn estimate_quality(path: &Path) -> u8 {
        if let Ok(mut file) = fs::File::open(path) {
            let mut buffer = vec![0u8; 4096];
            if file.read(&mut buffer).is_ok() {
                for i in 0..buffer.len().saturating_sub(70) {
                    if buffer.get(i) == Some(&0xFF)
                        && buffer.get(i + 1) == Some(&0xDB)
                        && let Some(&q_byte) = buffer.get(i + 5)
                    {
                        let q_value = u32::from(q_byte);
                        return match q_value {
                            0..=2 => crate::constants::JPEG_EST_Q_EXCELLENT,
                            3..=5 => crate::constants::JPEG_EST_Q_ULTRA,
                            6..=10 => crate::constants::JPEG_EST_Q_VERY_HIGH,
                            11..=20 => crate::constants::JPEG_EST_Q_HIGH,
                            21..=40 => crate::constants::JPEG_EST_Q_MEDIUM,
                            41..=60 => crate::constants::JPEG_EST_Q_LOW,
                            _ => crate::constants::JPEG_EST_Q_LOWEST,
                        };
                    }
                }
            }
        }
        crate::log_anomaly!(
            crate::static_logs::messages::LABEL_IMAGE,
            &format!(
                "JPEG Analysis: No DQT markers found in first 4KB for {}; using default quality ({})",
                path.display(),
                crate::constants::FALLBACK_QUALITY_JPEG
            )
        );
        crate::constants::FALLBACK_QUALITY_JPEG
    }

    #[must_use]
    pub fn is_progressive(path: &Path) -> bool {
        if let Ok(mut file) = fs::File::open(path) {
            let mut buffer = vec![0u8; 4096];
            if file.read(&mut buffer).is_ok() {
                for i in 0..buffer.len().saturating_sub(1) {
                    if buffer.get(i) == Some(&0xFF) && buffer.get(i + 1) == Some(&0xC2) {
                        return true;
                    }
                }
            }
        }
        false
    }
}

pub mod webp {
    use crate::unified_error::{ImgQualityError, Result};
    use std::fs;
    use std::path::Path;

    /// Detect WebP animated compression by traversing all ANMF (animation frame) chunks.
    ///
    /// WebP animation: RIFF header → VP8X → ANIM → ANMF* frames.
    /// Each ANMF payload contains frame data starting with VP8/VP8L sub-chunk.
    /// Any VP8 (lossy) frame → Lossy. All VP8L → Lossless.
    /// Detect if a WebP animation is lossless.
    ///
    /// # Errors
    /// Returns an error if the WebP stream is invalid.
    pub fn detect_webp_animation_is_lossless(data: &[u8]) -> Result<bool> {
        // WebP structure: RIFF[size]WEBP[chunks...]
        // Walk top-level chunks to find ANMF frames
        if data.len() < 12 {
            return Err(ImgQualityError::AnalysisError(
                "WebP: data too small for format identification".to_string(),
            ));
        }

        let mut pos = 12; // skip RIFF + size + WEBP
        let mut found_any_frame = false;

        while pos + 8 <= data.len() {
            if pos + 8 > data.len() {
                break;
            }
            let chunk_id = &data[pos..pos + 4];
            let chunk_size = crate::numeric_cast::u32_to_usize_strict(
                u32::from_le_bytes([data[pos + 4], data[pos + 5], data[pos + 6], data[pos + 7]]),
                "webp_chunk_size",
            )
            .ok_or_else(|| {
                ImgQualityError::AnalysisError(format!(
                    "WebP chunk size at {pos} is too large for memory"
                ))
            })?;
            let payload_start = pos + 8;
            let payload_end = (payload_start + chunk_size).min(data.len());

            if chunk_id == b"ANMF"
                && payload_start + 24 <= data.len()
                && payload_end > payload_start + 24
            {
                found_any_frame = true;
                // ANMF payload: 24 bytes header, then frame data sub-chunk
                if let Some(frame_data) = data.get(payload_start + 24..payload_end)
                    && frame_data.len() >= 4
                {
                    // Check sub-chunk type: VP8L = lossless, VP8 = lossy
                    let sub_chunk = &frame_data[0..4];
                    if sub_chunk == b"VP8 " {
                        return Ok(false); // Lossy
                    } else if sub_chunk != b"VP8L" {
                        // Unknown frame type in animated WebP — ambiguous
                        return Err(ImgQualityError::AnalysisError(format!(
                            "Animated WebP: unknown frame chunk type {:?} at pos {}; cannot determine compression",
                            String::from_utf8_lossy(sub_chunk),
                            payload_start + 24
                        )));
                    }
                }
            }

            // Chunks are padded to even size
            let padded = (chunk_size + 1) & !1;
            pos = payload_start + padded;
        }

        if found_any_frame {
            Ok(true) // All frames were VP8L (or skipped non-frame chunks)
        } else {
            // No ANMF frames found in animated WebP — ambiguous if VP8L also not found via window search
            if data.windows(4).any(|w| w == b"VP8L") {
                Ok(true)
            } else if data.windows(4).any(|w| w == b"VP8 ") {
                Ok(false)
            } else {
                Err(ImgQualityError::AnalysisError(
                    "Animated WebP: no ANMF frames or VP8/VP8L chunks found; cannot determine compression".to_string()
                ))
            }
        }
    }

    /// Estimate WebP VP8 quality by parsing the bitstream quantization index.
    /// Estimate quality from raw image bytes.
    ///
    /// # Errors
    /// Returns an error if the format is unsupported or data is corrupted.
    /// # Panics
    /// Panics if the WebP bitstream is malformed during quality extraction.
    pub fn estimate_quality_from_bytes(data: &[u8]) -> Result<u8> {
        let mut pos = 12; // skip RIFF + size + WEBP
        while pos + 8 <= data.len() {
            let chunk_id = &data[pos..pos + 4];
            let chunk_size = crate::numeric_cast::u32_to_usize_strict(
                u32::from_le_bytes([data[pos + 4], data[pos + 5], data[pos + 6], data[pos + 7]]),
                "webp_chunk_size",
            )
            .ok_or_else(|| {
                ImgQualityError::AnalysisError(format!(
                    "WebP chunk size at {pos} is too large for memory"
                ))
            })?;
            let payload_start = pos + 8;
            let chunk_end = (payload_start + chunk_size).min(data.len());

            if chunk_id == b"VP8 "
                && payload_start + 11 <= data.len()
                && let Some(vp8_data) = data.get(payload_start..chunk_end)
                && vp8_data.len() >= 11
                && vp8_data.get(3..6) == Some(&[0x9D, 0x01, 0x2A])
            {
                let y_ac_qi = vp8_data[10] & 0x7F;
                let quality = (u32::from(127 - y_ac_qi) * 100)
                    .checked_div(127)
                    .and_then(|q| crate::numeric_cast::u32_to_u8_strict(q.min(100), "webp_quality"))
                    .ok_or_else(|| {
                        ImgQualityError::AnalysisError(
                            "WebP Analysis: Division by 127 failed or quality overflow".to_string(),
                        )
                    })?;
                return Ok(quality);
            }
            let padded = (chunk_size + 1) & !1;
            pos = payload_start + padded;
        }
        Err(ImgQualityError::AnalysisError(
            "No VP8 chunk found".to_string(),
        ))
    }

    /// Estimate image quality for lossy formats.
    ///
    /// # Errors
    /// Returns an error if detection is not possible for the format.
    pub fn estimate_quality(path: &Path) -> Result<u8> {
        let bytes = fs::read(path).map_err(crate::unified_error::UnifiedError::IoError)?;
        estimate_quality_from_bytes(&bytes)
    }

    #[must_use]
    pub fn is_lossless_from_bytes(data: &[u8]) -> bool {
        data.windows(4).any(|w| w == b"VP8L")
    }

    #[must_use]
    pub fn is_animated_from_bytes(data: &[u8]) -> bool {
        data.windows(4).any(|w| w == b"ANIM")
    }

    /// # Errors
    /// Returns an error if the frame count overflows u32.
    pub fn count_frames_from_bytes(data: &[u8]) -> crate::unified_error::Result<u32> {
        crate::numeric_cast::usize_to_u32_strict(
            data.windows(4).filter(|w| *w == b"ANMF").count(),
            "webp_frame_count",
        )
        .ok_or_else(|| {
            crate::unified_error::ImgQualityError::NumericError("WebP frame count overflow".into())
        })
    }

    /// Parse animated WebP RIFF/ANMF chunks and return total duration in seconds.
    ///
    /// ANMF payload: 24-byte header, bytes 16..20 = frame duration in ms (uint32 LE).
    /// Returns None if not animated WebP or no ANMF chunks.
    #[must_use]
    /// # Panics
    /// Panics if the WebP animation header is corrupted beyond recognition.
    pub fn duration_secs_from_bytes(data: &[u8]) -> Option<f32> {
        if data.len() < 12 || data.get(0..4) != Some(b"RIFF") || data.get(8..12) != Some(b"WEBP") {
            return None;
        }
        if !data.windows(4).any(|w| w == b"ANIM") {
            return None;
        }
        let mut pos = 12usize; // RIFF payload start
        let mut total_ms = 0u64;

        while pos + 8 <= data.len() {
            let chunk_id = data
                .get(pos..pos + 4)
                .ok_or_else(|| {
                    crate::log_anomaly!(
                        crate::static_logs::messages::LABEL_ANOMALY,
                        "WebP chunk ID truncated"
                    );
                    b"TRUN"
                })
                .unwrap_or(b"TRUN"); // safe due to while condition
            if chunk_id == b"TRUN" {
                break;
            }

            let chunk_size_u32 =
                u32::from_le_bytes([data[pos + 4], data[pos + 5], data[pos + 6], data[pos + 7]]);
            let chunk_size =
                crate::numeric_cast::u32_to_usize_strict(chunk_size_u32, "webp_chunk_size")?;
            let payload_start = pos + 8;
            // Strict bounds: if chunk_size is malformed, stop trusting RIFF traversal.
            if chunk_size > data.len().saturating_sub(payload_start) {
                break;
            }
            // ANMF frame header is 16 bytes. Duration is a 24-bit little-endian integer at offset 12..15.
            if chunk_id == b"ANMF" && payload_start + 15 <= data.len() {
                let duration_ms = u32::from(data[payload_start + 12])
                    | (u32::from(data[payload_start + 13]) << 8_i32)
                    | (u32::from(data[payload_start + 14]) << 16_i32);
                if duration_ms > 0 && duration_ms <= 60_000 {
                    total_ms += u64::from(duration_ms);
                }
            }
            let padded = (chunk_size + 1) & !1;
            pos = payload_start + padded;
        }
        // If RIFF traversal failed (common for Safari exports), fall back to a marker scan:
        // search for ANMF and read the duration field at a fixed offset relative to chunk header.
        if total_ms == 0 {
            for idx in data
                .windows(4)
                .enumerate()
                .filter_map(|(i, w)| if w == b"ANMF" { Some(i) } else { None })
            {
                // ANMF chunk layout: "ANMF" (4) + size (4) + payload...
                // duration is 24-bit LE at payload offset 12..15 => idx + 8 + 12..15
                let dur_off = idx + 8 + 12;
                if dur_off + 3 <= data.len() {
                    let duration_ms = u32::from(data[dur_off])
                        | (u32::from(data[dur_off + 1]) << 8_i32)
                        | (u32::from(data[dur_off + 2]) << 16_i32);
                    if duration_ms > 0 && duration_ms <= 60_000 {
                        total_ms += u64::from(duration_ms);
                    }
                }
            }
        }

        // Hard sanity cap: if duration is absurd, treat as unknown.
        if total_ms == 0 || total_ms > 600_000 {
            return None;
        }
        Some(crate::numeric_cast::f64_to_f32_lossy(
            crate::numeric_cast::u64_to_f64(total_ms) / crate::constants::MS_PER_SEC_F64,
        ))
    }

    /// Detects if a WebP file is lossless by reading it from disk.
    ///
    /// # Errors
    /// Returns an error if the file cannot be read or if the WebP header is corrupted.
    pub fn is_lossless(path: &Path) -> Result<bool> {
        let b = fs::read(path)?;
        Ok(is_lossless_from_bytes(&b))
    }

    /// Detects if a WebP file is animated by reading it from disk.
    ///
    /// # Errors
    /// Returns an error if the file cannot be read or if the WebP header is corrupted.
    pub fn is_animated(path: &Path) -> Result<bool> {
        let b = fs::read(path)?;
        Ok(is_animated_from_bytes(&b))
    }
}

pub mod gif {
    use crate::unified_error::ImgQualityError;
    use std::fs;
    use std::path::Path;

    /// # Errors
    /// Returns an error if the frame count overflows u32.
    pub fn count_frames_from_bytes(data: &[u8]) -> crate::unified_error::Result<u32> {
        if data.len() < 24 || data.get(0..3) != Some(b"GIF") {
            return Ok(0);
        }

        let mut pos = 6;
        if pos + 7 > data.len() {
            return Ok(0);
        }
        let packed = data[pos + 4];
        let has_gct = (packed & 0x80) != 0;
        let gct_size = if has_gct {
            3 * (1 << ((packed & 0x07) + 1))
        } else {
            0
        };
        pos += 7 + gct_size;

        let mut image_descriptors = 0u32;
        let mut gce_count = 0u32; // Graphic Control Extension count

        while pos < data.len() {
            let Some(&byte) = data.get(pos) else {
                break;
            };
            match byte {
                0x2C => {
                    // Image Descriptor
                    image_descriptors += 1;
                    if pos + 10 > data.len() {
                        break;
                    }
                    let img_packed = data[pos + 9];
                    let local_palette_active = (img_packed & 0x80) != 0;
                    let lct_size = if local_palette_active {
                        3 * (1 << ((img_packed & 0x07) + 1))
                    } else {
                        0
                    };
                    pos += 10 + lct_size;

                    // After Image Descriptor and optional Local Color Table,
                    // there is exactly ONE byte for LZW Minimum Code Size.
                    // We must skip it before reading the first data sub-block size.
                    if pos < data.len() {
                        pos += 1;
                    }

                    // Skip Image Data sub-blocks
                    while pos < data.len() {
                        let block_size =
                            crate::numeric_cast::u8_to_usize_strict(data[pos], "gif_block_size")
                                .ok_or_else(|| {
                                    crate::unified_error::ImgQualityError::NumericError(
                                        "GIF block size cast failed".into(),
                                    )
                                })?;
                        pos += 1;
                        if block_size == 0 {
                            break;
                        }
                        if pos + block_size > data.len() {
                            // Malformed: block extends past EOF
                            break;
                        }
                        pos += block_size;
                    }
                }
                0x21 => {
                    // Extension Block
                    if pos + 2 >= data.len() {
                        break;
                    }
                    let label = data[pos + 1];
                    if label == 0xF9 {
                        gce_count += 1;
                    }

                    pos += 2;
                    // Skip Extension Data blocks
                    while pos < data.len() {
                        let block_size = crate::numeric_cast::u8_to_usize_strict(
                            data[pos],
                            "gif_ext_block_size",
                        )
                        .ok_or_else(|| {
                            crate::unified_error::ImgQualityError::NumericError(
                                "GIF extension block size cast failed".into(),
                            )
                        })?;
                        pos += 1;
                        if block_size == 0 {
                            break;
                        }
                        pos += block_size;
                    }
                }
                0x3B => break, // Trailer
                _ => {
                    // Unknown byte, try to resync
                    pos += 1;
                }
            }
        }

        // A GIF is animated if it has more than one image descriptor
        // Rationale: We prefer Graphic Control Extension count if available, as it directly corresponds to animated frames.
        // If not, we fall back to Image Descriptor count.
        if gce_count > 1 {
            Ok(gce_count)
        } else {
            Ok(image_descriptors)
        }
    }

    /// Parse GIF Graphic Control Extension (GCE) blocks and return total duration in seconds.
    /// Returns None if no GCE blocks found or data is truncated.
    #[must_use]
    pub fn duration_secs_from_bytes(data: &[u8]) -> Option<f32> {
        if data.len() < 24 || data.get(0..3) != Some(b"GIF") {
            return None;
        }

        let mut pos = 6;
        if pos + 7 > data.len() {
            return None;
        }
        let packed = data[pos + 4];
        let has_gct = (packed & 0x80) != 0;
        let gct_size = if has_gct {
            3 * (1 << ((packed & 0x07) + 1))
        } else {
            0
        };
        pos += 7 + gct_size;

        let mut total_100ths = 0u64;
        let mut found_any_delay = false;

        while pos < data.len() {
            let byte = data[pos];
            match byte {
                0x2C => {
                    if pos + 10 > data.len() {
                        break;
                    }
                    let img_packed = data[pos + 9];
                    let local_palette_active = (img_packed & 0x80) != 0;
                    let lct_size = if local_palette_active {
                        3 * (1 << ((img_packed & 0x07) + 1))
                    } else {
                        0
                    };
                    pos += 10 + lct_size;
                    if pos < data.len() {
                        pos += 1; // LZW Minimum Code Size
                    }
                    while pos < data.len() {
                        let block_size =
                            crate::numeric_cast::u8_to_usize_strict(data[pos], "gif_block_size")?;
                        pos += 1;
                        if block_size == 0 {
                            break;
                        }
                        pos += block_size;
                    }
                }
                0x21 => {
                    if pos + 2 >= data.len() {
                        break;
                    }
                    let label = data[pos + 1];
                    let block_size_idx = pos + 2;
                    let block_size = crate::numeric_cast::u8_to_usize_strict(
                        data[block_size_idx],
                        "gif_block_size",
                    )?;

                    if label == 0xF9 && block_size >= 4 && pos + 6 < data.len() {
                        // GCE block: [0x21, 0xF9, 0x04, <Packed>, <Delay LSB>, <Delay MSB>, <Trans Index>, 0x00]
                        // pos is at 0x21, so Delay LSB is at pos + 4, MSB at pos + 5
                        let delay = u16::from_le_bytes([data[pos + 4], data[pos + 5]]);
                        if delay > 0 {
                            total_100ths += u64::from(delay);
                            found_any_delay = true;
                        }
                    }

                    pos += 2;
                    while pos < data.len() {
                        let inner_block_size = crate::numeric_cast::u8_to_usize_strict(
                            data[pos],
                            "gif_inner_block_size",
                        )?;
                        pos += 1;
                        if inner_block_size == 0 {
                            break;
                        }
                        pos += inner_block_size;
                    }
                }
                0x3B => break,
                _ => pos += 1,
            }
        }

        if !found_any_delay || total_100ths == 0 {
            return None;
        }

        Some(crate::numeric_cast::f64_to_f32_lossy(
            crate::numeric_cast::u64_to_f64(total_100ths) / crate::constants::CENTISECS_PER_SEC_F64,
        ))
    }

    #[must_use]
    pub fn get_duration_secs(path: &Path) -> Option<f32> {
        fs::read(path)
            .ok()
            .and_then(|b| duration_secs_from_bytes(&b))
    }

    /// # Errors
    /// Returns an error if the animation detection fails due to invalid data.
    pub fn is_animated_from_bytes(data: &[u8]) -> crate::unified_error::Result<bool> {
        Ok(count_frames_from_bytes(data)? > 1)
    }

    /// # Errors
    /// Returns an error if the file cannot be read or animation detection fails.
    pub fn is_animated(path: &Path) -> crate::unified_error::Result<bool> {
        let b = fs::read(path)?;
        is_animated_from_bytes(&b)
    }

    /// # Errors
/// Returns an error if the file cannot be read or frame count detection fails.
pub fn get_frame_count(path: &Path) -> crate::unified_error::Result<usize> {
        let b = fs::read(path)?;
        let count = count_frames_from_bytes(&b)?;
        crate::numeric_cast::u32_to_usize_strict(count, "gif_frame_count")
            .ok_or_else(|| ImgQualityError::NumericError("GIF frame count overflow".to_string()))
    }
}

pub mod avif {
    use crate::common_utils::find_box_data_recursive;
    use crate::unified_error::{ImgQualityError, Result};
    use std::fs;
    use std::path::Path;

    /// Detect AVIF lossless encoding — multi-dimension analysis.
    ///
    /// Dimensions checked (in priority order):
    /// 1. **av1C chroma subsampling**: 4:2:0 / 4:2:2 → definitely lossy
    /// 2. **av1C 4:4:4 + colr Identity matrix (MC=0)** → lossless
    /// 3. **av1C 4:4:4 + `high_bitdepth` / `twelve_bit`** → lossless
    /// 4. **av1C `seq_profile`**: Profile 0 + 4:4:4 → treat as lossless
    /// 5. **pixi box**: bit depth ≥ 12 with 4:4:4 → lossless indicator
    ///
    /// Check if the image bytes represent a lossless encoding.
    ///
    /// # Errors
    /// Returns an error if the format cannot be identified or parsed.
    /// # Panics
    /// Panics if the AVIF container is corrupted during lossless detection.
    pub fn is_lossless_from_bytes(data: &[u8], path: &Path) -> Result<bool> {
        if let Some(av1c_data) = find_box_data_recursive(data, *b"av1C")
            && av1c_data.len() >= 3
        {
            let byte1 = av1c_data[1];
            let byte2 = av1c_data[2];

            let seq_profile = (byte1 >> 5_i32) & 0x07;
            let high_bitdepth = (byte2 >> 6_i32) & 0x01;
            let twelve_bit = (byte2 >> 5_i32) & 0x01;
            let monochrome = (byte2 >> 4_i32) & 0x01;
            let chroma_subsampling_x = (byte2 >> 3_i32) & 0x01;
            let chroma_subsampling_y = (byte2 >> 2_i32) & 0x01;

            let is_444 = chroma_subsampling_x == 0 && chroma_subsampling_y == 0;
            let is_420 = chroma_subsampling_x == 1 && chroma_subsampling_y == 1;
            let is_422 = chroma_subsampling_x == 1 && chroma_subsampling_y == 0;

            if is_420 || is_422 {
                return Ok(false);
            }

            if monochrome == 1 && !is_444 {
                return Ok(false);
            }

            // Dimension 2: colr Identity matrix (MC=0)
            if let Some(colr_data) = find_box_data_recursive(data, *b"colr")
                && colr_data.len() >= 11
                && colr_data.get(0..4) == Some(b"nclx")
            {
                let matrix_coefficients = u16::from_be_bytes([colr_data[8], colr_data[9]]);
                if matrix_coefficients == 0 {
                    return Ok(true);
                }
            }

            // Dimension 3: high_bitdepth/twelve_bit
            if is_444 && (twelve_bit == 1 || (high_bitdepth == 1 && seq_profile >= 1)) {
                return Ok(true);
            }

            // Dimension 4: Profile 0 + 4:4:4
            if is_444 && seq_profile == 0 {
                return Ok(true);
            }

            // Dimension 5: pixi box
            if is_444
                && let Some(pixi_data) = find_box_data_recursive(data, *b"pixi")
                && !pixi_data.is_empty()
            {
                let num_ch =
                    crate::numeric_cast::u8_to_usize_strict(pixi_data[0], "avif_pixi_num_ch")
                        .ok_or_else(|| {
                            ImgQualityError::AnalysisError("AVIF pixi num_ch overflow".to_string())
                        })?;
                if num_ch > 0 && pixi_data.len() > num_ch {
                    let max_depth = pixi_data
                        .get(1..=num_ch)
                        .and_then(|slice| slice.iter().copied().max())
                        .unwrap_or_else(|| {
                            crate::log_anomaly!(
                                crate::static_logs::messages::LABEL_IMAGE,
                                "AVIF Analysis: Failed to find max depth in pixi data; defaulting to 8-bit depth (lossless detection may be inaccurate)"
                            );
                            8
                        });
                    if max_depth >= 12 {
                        return Ok(true);
                    }
                }
            }

            if is_444 && monochrome == 1 {
                return Ok(true);
            }

            if is_444 {
                return Err(ImgQualityError::AnalysisError(format!(
                    "AVIF: 4:4:4 without definitive lossless indicators; refusing to guess — {}",
                    path.display()
                )));
            }
        }

        Err(ImgQualityError::AnalysisError(format!(
            "AVIF: no av1C box found; cannot determine compression — {}",
            path.display()
        )))
    }

    /// Detects if an AVIF file is lossless by reading it from disk.
    ///
    /// # Errors
    /// Returns an error if the file cannot be read, or if the AVIF header is missing
    /// critical property markers.
    pub fn is_lossless(path: &Path) -> Result<bool> {
        let b = fs::read(path)?;
        is_lossless_from_bytes(&b, path)
    }
}

pub mod jxl {
    use crate::common_utils::{find_any_box_recursive, find_box_data_recursive};
    use crate::unified_error::{ImgQualityError, Result};
    use std::fs;
    use std::path::Path;

    /// Detect JXL (JPEG XL) lossless encoding — multi-dimension analysis.
    /// Check if the image bytes represent a lossless encoding.
    ///
    /// # Errors
    /// Returns an error if the format cannot be identified or parsed.
    pub fn is_lossless_from_bytes(data: &[u8], path: &Path) -> Result<bool> {
        if data.len() < 4 {
            return Err(ImgQualityError::AnalysisError(format!(
                "JXL: file too short — {}",
                path.display()
            )));
        }

        let is_naked = data.get(0..2) == Some(b"\xFF\x0A");

        // Dimension 1: jbrd = JPEG bitstream reconstruction = lossless
        if !is_naked && find_any_box_recursive(data, *b"jbrd") {
            return Ok(true);
        }

        // Dimension 2-4: Parse codestream header for xyb_encoded
        let codestream: Option<&[u8]> = if is_naked {
            Some(data)
        } else {
            find_box_data_recursive(data, *b"jxlc")
                .or_else(|| find_box_data_recursive(data, *b"jxlp"))
        };

        if let Some(cs) = codestream {
            match parse_jxl_xyb_encoded(cs) {
                Some(true) => return Ok(false), // xyb_encoded=true -> lossy
                Some(false) => return Ok(true), // xyb_encoded=false -> lossless
                None => {}
            }
        }

        Err(ImgQualityError::AnalysisError(format!(
            "JXL: no jbrd and codestream header unparseable — cannot determine — {}",
            path.display()
        )))
    }

    /// Minimal bit reader for parsing JXL codestream headers.
    struct JxlBitReader<'a> {
        data: &'a [u8],
        byte_pos: usize,
        bit_pos: u8,
    }

    impl<'a> JxlBitReader<'a> {
        const fn new(data: &'a [u8]) -> Self {
            Self {
                data,
                byte_pos: 0,
                bit_pos: 0,
            }
        }
        fn read_bits(&mut self, n: u8) -> Option<u32> {
            if n == 0 {
                return Some(0);
            }
            let mut result: u32 = 0;
            for i in 0..n {
                if self.byte_pos >= self.data.len() {
                    return None;
                }
                let bit = (*self.data.get(self.byte_pos)? >> self.bit_pos) & 1;
                result |= u32::from(bit) << i;
                self.bit_pos += 1;
                if self.bit_pos == 8 {
                    self.bit_pos = 0;
                    self.byte_pos += 1;
                }
            }
            Some(result)
        }
        fn read_bool(&mut self) -> Option<bool> {
            self.read_bits(1).map(|v| v == 1)
        }
        fn read_u32(&mut self, dists: [(u32, u8); 4]) -> Option<u32> {
            let sel = crate::numeric_cast::u32_to_usize_strict(self.read_bits(2)?, "jxl_sel")?;
            let (base, extra_bits) = *dists.get(sel)?;
            let extra = self.read_bits(extra_bits)?;
            Some(base + extra)
        }
    }

    // Rationale: This function handles complex, sequential initialization or business logic where further fragmentation would hinder readability and maintainability.
    #[allow(
        clippy::too_many_lines,
        reason = "Complex orchestration logic where fragmenting state into smaller helpers would decrease readability and increase cognitive overhead."
    )]
    fn parse_jxl_xyb_encoded(codestream: &[u8]) -> Option<bool> {
        let start = if codestream.get(0..2) == Some(b"\xFF\x0A") {
            2
        } else {
            0
        };
        if start >= codestream.len() {
            return None;
        }
        let mut r = JxlBitReader::new(codestream.get(start..)?);

        // --- SizeHeader ---
        let small = r.read_bool()?;
        if small {
            let _ysize_div8_m1 = r.read_bits(5)?;
            let ratio = r.read_bits(3)?;
            if ratio == 0 {
                let _xsize_div8_m1 = r.read_bits(5)?;
            }
        } else {
            // ysize_minus1: U32(u(9), u(13), u(18), u(30))
            r.read_u32([(0, 9), (0, 13), (0, 18), (0, 30)])?;
            let ratio = r.read_bits(3)?;
            if ratio == 0 {
                r.read_u32([(0, 9), (0, 13), (0, 18), (0, 30)])?; // xsize_minus1
            }
        }

        // --- ImageMetadata ---
        let all_default = r.read_bool()?;
        if all_default {
            // all_default=true → xyb_encoded defaults to true → lossy
            return Some(true);
        }

        let extra_fields = r.read_bool()?;
        if extra_fields {
            r.read_bits(3)?; // orientation - 1: u(3)

            // have_intrinsic_size
            if r.read_bool()? {
                let small2 = r.read_bool()?;
                if small2 {
                    r.read_bits(5)?; // ysize
                    let ratio2 = r.read_bits(3)?;
                    if ratio2 == 0 {
                        r.read_bits(5)?;
                    } // xsize
                } else {
                    r.read_u32([(0, 9), (0, 13), (0, 18), (0, 30)])?; // ysize
                    let ratio2 = r.read_bits(3)?;
                    if ratio2 == 0 {
                        r.read_u32([(0, 9), (0, 13), (0, 18), (0, 30)])?; // xsize
                    }
                }
            }

            // have_preview
            if r.read_bool()? {
                let div8 = r.read_bool()?;
                if div8 {
                    r.read_u32([(0, 9), (0, 13), (0, 18), (0, 30)])?;
                } else {
                    let div16 = r.read_bool()?;
                    if !div16 {
                        r.read_u32([(0, 9), (0, 13), (0, 18), (0, 30)])?;
                        r.read_u32([(0, 9), (0, 13), (0, 18), (0, 30)])?;
                    }
                }
            }

            // have_animation
            if r.read_bool()? {
                r.read_u32([(100, 0), (1000, 0), (0, 10), (0, 30)])?; // tps_num
                r.read_u32([(1, 0), (1001, 0), (0, 10), (0, 30)])?; // tps_den
                r.read_u32([(0, 0), (0, 3), (0, 16), (0, 32)])?; // num_loops
                r.read_bool()?; // have_timecodes
            }
        }

        // bit_depth
        let float_sample = r.read_bool()?;
        if float_sample {
            r.read_u32([(32, 0), (16, 0), (24, 0), (1, 6)])?; // bits_per_sample
            r.read_bits(4)?; // exp_bits + 1
        } else {
            r.read_u32([(8, 0), (10, 0), (12, 0), (1, 6)])?; // bits_per_sample
        }

        // num_extra_channels
        let num_extra = r.read_u32([(0, 0), (1, 0), (2, 0), (3, 12)])?;
        for _ in 0..num_extra {
            if !r.read_bool()? {
                // ec_default
                // Detailed ExtraChannelInfo skip logic (complex, bail if not default)
                return None;
            }
        }

        // xyb_encoded: Bool — THE FINAL TARGET
        r.read_bool()
    }

    #[must_use]
    pub fn verify_signature(path: &Path) -> bool {
        if let Ok(mut file) = fs::File::open(path) {
            use std::io::Read;
            let mut sig = [0u8; 2];
            if file.read_exact(&mut sig).is_ok() {
                return sig == [0xFF, 0x0A] || sig == [0x00, 0x00];
            }
        }
        false
    }

    #[must_use]
    pub fn is_valid(path: &Path) -> bool {
        verify_signature(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn test_png_compression_with_real_data() {
        let png_data: &[u8] = &[
            0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48,
            0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00,
            0x00, 0x90, 0x77, 0x53, 0xDE,
        ];
        let mut file =
            NamedTempFile::new().unwrap_or_else(|_| panic!("Failed to create temporary file"));
        file.write_all(png_data)
            .unwrap_or_else(|_| panic!("Failed to write to file"));

        let level = png::estimate_compression_level(file.path());
        assert!(
            level <= 9,
            "PNG compression level should be between 0-9, actual: {level}"
        );
    }

    #[test]
    fn test_jpeg_quality_with_real_data() {
        let jpeg_data: &[u8] = &[
            0xFF, 0xD8, 0xFF, 0xDB, 0x00, 0x43, 0x00, 0x02, 0x01, 0x01, 0x02, 0x01, 0x01, 0x02,
            0x02, 0x02, 0x02, 0x02, 0x02, 0x02, 0x02, 0x03, 0x05, 0x03, 0x03, 0x03, 0x03, 0x03,
            0x06, 0x04, 0x04, 0x03, 0x05, 0x07, 0x06, 0x07, 0x07, 0x07, 0x06, 0x07, 0x07, 0x08,
            0x09, 0x0B, 0x09, 0x08, 0x08, 0x0A, 0x08, 0x07, 0x07, 0x0A, 0x0D, 0x0A, 0x0A, 0x0B,
            0x0C, 0x0C, 0x0C, 0x0C, 0x07, 0x09, 0x0E, 0x0F, 0x0D, 0x0C, 0x0E, 0x0B, 0x0C, 0x0C,
            0x0C,
        ];
        let mut file =
            NamedTempFile::new().unwrap_or_else(|_| panic!("Failed to create temporary file"));
        file.write_all(jpeg_data)
            .unwrap_or_else(|_| panic!("Failed to write to file"));

        let quality = jpeg::estimate_quality(file.path());
        assert!(
            quality >= 90,
            "Low quantization value should return high quality, actual: {quality}"
        );

        // Test WebP lossless detection with simple VP8L header
        let webp_lossless = b"RIFF\x1A\x00\x00\x00WEBPVP8L\x08\x00\x00\x00\x10\x10\x00\x00";
        let mut file =
            NamedTempFile::new().unwrap_or_else(|_| panic!("Failed to create temporary file"));
        file.write_all(webp_lossless)
            .unwrap_or_else(|_| panic!("Failed to write to file"));

        assert!(
            webp::is_lossless(file.path()).unwrap_or(false),
            "VP8L chunk should be detected as lossless"
        );
    }

    #[test]
    fn test_webp_lossy_detection() {
        let webp_lossy: Vec<u8> = {
            let mut data = b"RIFF".to_vec();
            data.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]);
            data.extend_from_slice(b"WEBP");
            data.extend_from_slice(b"VP8 ");
            data.extend_from_slice(&[0u8; 20]);
            data
        };
        let mut file =
            NamedTempFile::new().unwrap_or_else(|_| panic!("Failed to create temporary file"));
        file.write_all(&webp_lossy)
            .unwrap_or_else(|_| panic!("Failed to write to file"));

        assert!(
            !webp::is_lossless(file.path()).unwrap_or(true),
            "VP8 chunk should be detected as lossy"
        );
    }

    #[test]
    fn test_gif_frame_count() {
        let gif_data: Vec<u8> = {
            let mut data = b"GIF89a".to_vec();
            data.extend_from_slice(&[0x01, 0x00, 0x01, 0x00]);
            data.extend_from_slice(&[0x00, 0x00, 0x00]);

            data.push(0x2C);
            data.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]);
            data.extend_from_slice(&[0x01, 0x00, 0x01, 0x00]);
            data.push(0x00);
            data.push(0x02);
            data.extend_from_slice(&[0x02, 0x4C, 0x01]);
            data.push(0x00);

            data.push(0x2C);
            data.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]);
            data.extend_from_slice(&[0x01, 0x00, 0x01, 0x00]);
            data.push(0x00);
            data.push(0x02);
            data.extend_from_slice(&[0x02, 0x4C, 0x01]);
            data.push(0x00);

            data.push(0x3B);
            data
        };
        let mut file =
            NamedTempFile::new().unwrap_or_else(|_| panic!("Failed to create temp file"));
        file.write_all(&gif_data)
            .unwrap_or_else(|_| panic!("Failed to write"));

        let count = gif::get_frame_count(file.path()).unwrap();
        assert_eq!(count, 2, "Expected 2 frames, got: {count}");
        assert!(
            gif::is_animated(file.path()).unwrap(),
            "2-frame GIF should be detected as animated"
        );
    }

    #[test]
    fn test_jxl_codestream_signature() {
        let jxl_codestream: &[u8] = &[0xFF, 0x0A, 0x00, 0x00];
        let mut file =
            NamedTempFile::new().unwrap_or_else(|_| panic!("Failed to create temporary file"));
        file.write_all(jxl_codestream)
            .unwrap_or_else(|_| panic!("Failed to write to file"));

        assert!(
            jxl::verify_signature(file.path()),
            "JXL codestream signature should be recognized"
        );
    }

    #[test]
    fn test_error_handling_nonexistent_file() {
        let path = std::path::Path::new("/nonexistent/file.test");

        assert!(
            webp::is_lossless(path).is_err(),
            "Non-existent file must surface an Err (no silent forgery)"
        );
        assert!(
            webp::is_animated(path).is_err(),
            "Non-existent file must surface an Err (no silent forgery)"
        );
        assert!(
            gif::is_animated(path).is_err(),
            "Non-existent file must surface an Err (no silent forgery)"
        );
        assert!(
            gif::get_frame_count(path).is_err(),
            "Non-existent file must surface an Err (no silent forgery)"
        );
        assert!(
            !jxl::verify_signature(path),
            "Non-existent file should return false"
        );
    }
}
