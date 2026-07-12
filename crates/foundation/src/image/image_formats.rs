//! Format-specific utilities and helpers
// Note: pub mod tiff (pure IFD byte-parser) removed — zero callers.
// All TIFF/DNG lossless detection uses tiff_family::is_lossless_tiff_family
// (exiftool-based, with disciplined main-IFD selection).

pub mod jpeg {
    use crate::unified_error::Result;
    use std::path::Path;

    /// Estimate JPEG quality using standard forensic analysis.
    ///
    /// # Errors
    /// Returns an error if the JPEG markers cannot be parsed.
    pub fn estimate_quality(path: &Path) -> Result<u8> {
        crate::image_jpeg_analysis::analyze_jpeg_file(path)
            .map(|a| a.estimated_quality)
            .map_err(crate::unified_error::ImgQualityError::AnalysisError)
    }
}

pub mod webp {
    use crate::unified_error::{ImgQualityError, Result};
    use std::fs;
    use std::path::Path;

    /// Detect WebP animated compression by traversing all ANMF (animation
    /// frame) chunks.
    ///
    /// WebP animation: RIFF header → VP8X → ANIM → ANMF* frames.
    /// Each ANMF payload contains frame data starting with VP8/VP8L sub-chunk.
    /// Any VP8 (lossy) frame → Lossy. All VP8L → Lossless.
    /// Detect if a WebP animation is lossless.
    ///
    /// # Errors
    /// Returns an error if the WebP stream is invalid or truncated.
    pub fn detect_webp_animation_is_lossless(data: &[u8]) -> Result<bool> {
        // WebP structure: RIFF[size]WEBP[chunks...]
        // Walk top-level chunks to find ANMF frames
        if data.len() < 12 {
            return Err(ImgQualityError::AnalysisError(
                "WebP: data too small for format identification (need at least 12 bytes for RIFF \
                 header)"
                    .to_string(),
            ));
        }

        // Verify RIFF header
        if data.get(0..4) != Some(b"RIFF") {
            return Err(ImgQualityError::AnalysisError(
                "WebP: missing RIFF header signature".to_string(),
            ));
        }

        if data.get(8..12) != Some(b"WEBP") {
            return Err(ImgQualityError::AnalysisError(
                "WebP: missing WEBP fourCC after RIFF header".to_string(),
            ));
        }

        let mut pos = 12; // skip RIFF + size + WEBP
        let mut found_any_frame = false;

        while pos + 8 <= data.len() {
            // Explicit bounds check before slice access
            let chunk_id = data.get(pos..pos + 4).ok_or_else(|| {
                ImgQualityError::AnalysisError(format!("WebP: chunk ID truncated at offset {pos}"))
            })?;

            let chunk_size_bytes = data.get(pos + 4..pos + 8).ok_or_else(|| {
                ImgQualityError::AnalysisError(format!(
                    "WebP: chunk size truncated at offset {}",
                    pos + 4
                ))
            })?;

            let chunk_size = crate::numeric_cast::u32_to_usize_strict(
                u32::from_le_bytes([
                    chunk_size_bytes[0],
                    chunk_size_bytes[1],
                    chunk_size_bytes[2],
                    chunk_size_bytes[3],
                ]),
                "webp_chunk_size",
            )
            .ok_or_else(|| {
                ImgQualityError::AnalysisError(format!(
                    "WebP chunk size at {pos} is too large for memory (exceeds usize::MAX)"
                ))
            })?;

            let payload_start = pos + 8;

            // Validate chunk size doesn't overflow buffer
            if payload_start > data.len() || chunk_size > data.len() - payload_start {
                return Err(ImgQualityError::AnalysisError(format!(
                    "WebP: chunk at offset {pos} claims size {chunk_size} but only {} bytes remain",
                    data.len().saturating_sub(payload_start)
                )));
            }

            let payload_end = payload_start + chunk_size;

            if chunk_id == b"ANMF" && payload_start + 16 <= payload_end {
                found_any_frame = true;
                let mut frame_data_pos = payload_start + 16;
                if frame_data_pos + 8 <= payload_end {
                    let first_four = &data[frame_data_pos..frame_data_pos + 4];
                    if first_four != b"VP8 "
                        && first_four != b"VP8L"
                        && first_four != b"ALPH"
                        && payload_start + 24 + 8 <= payload_end
                    {
                        let alt_four = &data[payload_start + 24..payload_start + 28];
                        if alt_four == b"VP8 " || alt_four == b"VP8L" || alt_four == b"ALPH" {
                            frame_data_pos = payload_start + 24;
                        }
                    }
                }

                while frame_data_pos + 8 <= payload_end {
                    let sub_chunk_id = &data[frame_data_pos..frame_data_pos + 4];
                    let sub_chunk_size_bytes = &data[frame_data_pos + 4..frame_data_pos + 8];
                    let sub_chunk_size = u32::from_le_bytes([
                        sub_chunk_size_bytes[0],
                        sub_chunk_size_bytes[1],
                        sub_chunk_size_bytes[2],
                        sub_chunk_size_bytes[3],
                    ]);
                    let sub_chunk_size = crate::numeric_cast::u32_to_usize_strict(
                        sub_chunk_size,
                        "webp_sub_chunk_size",
                    )
                    .ok_or_else(|| {
                        ImgQualityError::AnalysisError(
                            "WebP: sub-chunk size does not fit usize".to_string(),
                        )
                    })?;

                    let sub_payload_start = frame_data_pos + 8;
                    if sub_payload_start > payload_end
                        || sub_chunk_size > payload_end - sub_payload_start
                    {
                        return Err(ImgQualityError::AnalysisError(format!(
                            "WebP: ANMF sub-chunk at offset {frame_data_pos} claims size \
                             {sub_chunk_size} but exceeds ANMF boundary"
                        )));
                    }

                    if sub_chunk_id == b"VP8 " {
                        return Ok(false); // Lossy frame detected
                    } else if sub_chunk_id == b"VP8L" {
                        // Lossless frame detected, continue checking other
                        // frames/sub-chunks
                    } else if sub_chunk_id != b"ALPH" {
                        return Err(ImgQualityError::AnalysisError(format!(
                            "Format Audit: WebP unknown frame type at offset {}: {:?}. Expected \
                             VP8 or VP8L.",
                            frame_data_pos,
                            String::from_utf8_lossy(sub_chunk_id)
                        )));
                    }

                    let padded = (sub_chunk_size + 1) & !1;
                    frame_data_pos = sub_payload_start + padded;
                }
            }

            // Chunks are padded to even size
            let padded = (chunk_size + 1) & !1;

            // Check for overflow before advancing position
            pos = payload_start.checked_add(padded).ok_or_else(|| {
                ImgQualityError::AnalysisError(format!(
                    "WebP: position overflow when advancing past chunk at offset {payload_start}"
                ))
            })?;

            // Safety check: prevent infinite loop on malformed data
            if pos > data.len() {
                break;
            }
        }

        if found_any_frame {
            Ok(true) // All frames were VP8L (or skipped non-frame chunks)
        } else {
            // No ANMF frames found in animated WebP — fallback to window search
            if data.windows(4).any(|w| w == b"VP8L") {
                Ok(true)
            } else if data.windows(4).any(|w| w == b"VP8 ") {
                Ok(false)
            } else {
                Err(ImgQualityError::AnalysisError(
                    "Animated WebP: no ANMF frames or VP8/VP8L chunks found; cannot determine \
                     compression"
                        .to_string(),
                ))
            }
        }
    }

    /// Estimate WebP VP8 quality by parsing the bitstream quantization index.
    /// Estimate quality from raw image bytes.
    ///
    /// # Errors
    /// Returns an error if the format is unsupported, data is corrupted, or
    /// bounds are violated.
    pub fn estimate_quality_from_bytes(data: &[u8]) -> Result<u8> {
        if data.len() < 12 {
            return Err(ImgQualityError::AnalysisError(
                "WebP: data too small for quality estimation (need at least 12 bytes)".to_string(),
            ));
        }

        // Verify RIFF/WEBP header
        if data.get(0..4) != Some(b"RIFF") || data.get(8..12) != Some(b"WEBP") {
            return Err(ImgQualityError::AnalysisError(
                "WebP: invalid RIFF/WEBP header for quality estimation".to_string(),
            ));
        }

        let mut pos = 12; // skip RIFF + size + WEBP

        while pos + 8 <= data.len() {
            // Explicit bounds check for chunk ID
            let chunk_id = data.get(pos..pos + 4).ok_or_else(|| {
                ImgQualityError::AnalysisError(format!(
                    "WebP quality estimation: chunk ID truncated at offset {pos}"
                ))
            })?;

            // Explicit bounds check for chunk size
            let chunk_size_bytes = data.get(pos + 4..pos + 8).ok_or_else(|| {
                ImgQualityError::AnalysisError(format!(
                    "WebP quality estimation: chunk size truncated at offset {}",
                    pos + 4
                ))
            })?;

            let chunk_size = crate::numeric_cast::u32_to_usize_strict(
                u32::from_le_bytes([
                    chunk_size_bytes[0],
                    chunk_size_bytes[1],
                    chunk_size_bytes[2],
                    chunk_size_bytes[3],
                ]),
                "webp_chunk_size",
            )
            .ok_or_else(|| {
                ImgQualityError::AnalysisError(format!(
                    "WebP quality estimation: chunk size at {pos} exceeds usize::MAX"
                ))
            })?;

            let payload_start = pos + 8;

            // Validate chunk size doesn't overflow buffer
            if payload_start > data.len() || chunk_size > data.len() - payload_start {
                return Err(ImgQualityError::AnalysisError(format!(
                    "WebP quality estimation: chunk at offset {pos} claims size {chunk_size} but \
                     only {} bytes remain",
                    data.len().saturating_sub(payload_start)
                )));
            }

            let chunk_end = payload_start + chunk_size;

            if chunk_id == b"VP8 " {
                // VP8 lossy chunk found - extract quality from quantization index

                // VP8 bitstream requires at least 11 bytes for header + QI
                if chunk_size < 11 {
                    // VP8 chunk too small - skip and continue scanning
                    crate::media_conversion_gate::probe_image_format_batch_audit(
                        "probe_image_format",
                        format!(
                            "WebP quality estimation: VP8 chunk at offset {pos} too small \
                             ({chunk_size} bytes, need at least 11); skipping"
                        ),
                    );
                    // Continue to next chunk instead of failing
                } else if let Some(vp8_data) = data.get(payload_start..chunk_end) {
                    // Verify VP8 frame tag signature (bytes 3-5: 0x9D 0x01 0x2A)
                    if vp8_data.len() >= 6 && vp8_data.get(3..6) == Some(&[0x9D, 0x01, 0x2A]) {
                        // Y AC quantization index is at byte 10, lower 7 bits
                        if let Some(&qi_byte) = vp8_data.get(10) {
                            let y_ac_qi = qi_byte & 0x7F;

                            // Convert QI to quality: quality = (127 - QI) * 100 / 127
                            let quality = (u32::from(127 - y_ac_qi) * 100)
                                .checked_div(127)
                                .and_then(|q| {
                                    crate::numeric_cast::u32_to_u8_strict(
                                        q.min(100),
                                        "webp_quality",
                                    )
                                })
                                .ok_or_else(|| {
                                    ImgQualityError::AnalysisError(
                                        "WebP quality estimation: quality calculation overflow or \
                                         division error"
                                            .to_string(),
                                    )
                                })?;
                            return Ok(quality);
                        }
                    }
                    // VP8 frame tag signature mismatch or QI byte missing -
                    // continue scanning This handles
                    // non-standard VP8 variants or multi-chunk files
                }
            }

            // Advance to next chunk (with padding)
            let padded = (chunk_size + 1) & !1;
            pos = payload_start.checked_add(padded).ok_or_else(|| {
                ImgQualityError::AnalysisError(format!(
                    "WebP quality estimation: position overflow when advancing past chunk at \
                     offset {payload_start}"
                ))
            })?;

            // Safety check: prevent infinite loop
            if pos > data.len() {
                break;
            }
        }

        Err(ImgQualityError::AnalysisError(
            "WebP quality estimation: no VP8 lossy chunk found in file".to_string(),
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
        if let Some(features) = ::webp::BitstreamFeatures::new(data)
            && let Some(format) = features.format()
        {
            match format {
                ::webp::BitstreamFormat::Lossless => return true,
                ::webp::BitstreamFormat::Lossy => return false,
                ::webp::BitstreamFormat::Undefined => {} // Fallback for animated or undefined
            }
        }

        if data.len() < 12 || &data[0..4] != b"RIFF" || &data[8..12] != b"WEBP" {
            return false;
        }

        let mut pos = 12;
        while pos + 8 <= data.len() {
            let chunk_id = &data[pos..pos + 4];
            let chunk_size =
                u32::from_le_bytes([data[pos + 4], data[pos + 5], data[pos + 6], data[pos + 7]]);
            let chunk_size =
                match crate::numeric_cast::u32_to_usize_strict(chunk_size, "webp_chunk_size") {
                    Some(value) => value,
                    None => return false,
                };

            if chunk_id == b"VP8L" {
                return true;
            }
            if chunk_id == b"VP8 " {
                return false;
            }

            if chunk_id == b"ANMF" && pos + 8 + 24 + 4 <= data.len() {
                let sub_chunk = &data[pos + 8 + 24..pos + 8 + 24 + 4];
                if sub_chunk == b"VP8L" {
                    return true;
                }
                if sub_chunk == b"VP8 " {
                    return false;
                }
            }

            pos += 8 + ((chunk_size + 1) & !1);
        }
        false
    }

    #[must_use]
    pub fn is_animated_from_bytes(data: &[u8]) -> bool {
        // Authoritative path: libwebp BitstreamFeatures.
        // On parse failure (truncated/corrupted), conservative answer = not animated.
        // The raw `data.windows(4).any(|w| w == b"ANIM")` scan was removed:
        // it could false-positive on VP8/VP8L payload bytes that happen to spell "ANIM".
        if let Some(features) = ::webp::BitstreamFeatures::new(data)
            && features.has_animation()
        {
            return true;
        }
        false
    }

    /// Canvas dimensions from RIFF/WebP chunk headers (VP8 / VP8L / VP8X /
    /// ANMF).
    #[must_use]
    pub fn dimensions_from_bytes(data: &[u8]) -> Option<(u32, u32)> {
        fn read_vp8x(payload: &[u8]) -> Option<(u32, u32)> {
            if payload.len() < 10 {
                return None;
            }
            let w = (u32::from(payload[4])
                | (u32::from(payload[5]) << 8)
                | (u32::from(payload[6]) << 16))
                + 1;
            let h = (u32::from(payload[7])
                | (u32::from(payload[8]) << 8)
                | (u32::from(payload[9]) << 16))
                + 1;
            (w > 0 && h > 0).then_some((w, h))
        }

        fn read_vp8(payload: &[u8]) -> Option<(u32, u32)> {
            if payload.len() < 10 {
                return None;
            }
            let w = u16::from_le_bytes([payload[6], payload[7]]) & 0x3FFF;
            let h = u16::from_le_bytes([payload[8], payload[9]]) & 0x3FFF;
            (w > 0 && h > 0).then_some((u32::from(w), u32::from(h)))
        }

        fn read_vp8l(payload: &[u8]) -> Option<(u32, u32)> {
            if payload.len() < 5 || payload[0] != 0x2F {
                return None;
            }
            let b1 = u32::from(payload[1]);
            let b2 = u32::from(payload[2]);
            let b3 = u32::from(payload[3]);
            let b4 = u32::from(payload[4]);
            let w = (b1 | ((b2 & 0x3F) << 8)) + 1;
            let h = (((b2 & 0xC0) >> 6) | (b3 << 2) | ((b4 & 0x0F) << 10)) + 1;
            (w > 0 && h > 0).then_some((w, h))
        }

        fn read_anmf(payload: &[u8]) -> Option<(u32, u32)> {
            if payload.len() < 16 {
                return None;
            }
            let w = u32::from(payload[6])
                | (u32::from(payload[7]) << 8)
                | (u32::from(payload[8]) << 16);
            let h = u32::from(payload[9])
                | (u32::from(payload[10]) << 8)
                | (u32::from(payload[11]) << 16);
            let w = w + 1;
            let h = h + 1;
            (w > 0 && h > 0).then_some((w, h))
        }

        if data.len() < 30 || data.get(0..4) != Some(b"RIFF") || data.get(8..12) != Some(b"WEBP") {
            return None;
        }

        let chunk = data.get(12..16)?;
        let first = match chunk {
            b"VP8 " => read_vp8(data.get(20..)?),
            b"VP8L" => read_vp8l(data.get(20..)?),
            b"VP8X" => read_vp8x(data.get(20..)?),
            _ => None,
        };
        if first.is_some() {
            return first;
        }

        let mut pos = 12usize;
        let mut best_canvas: Option<(u32, u32)> = None;
        while pos + 8 <= data.len() {
            let chunk_id = data.get(pos..pos + 4)?;
            let chunk_size =
                u32::from_le_bytes([data[pos + 4], data[pos + 5], data[pos + 6], data[pos + 7]]);
            let chunk_size =
                crate::numeric_cast::u32_to_usize_strict(chunk_size, "webp_chunk_size")?;
            let payload_start = pos + 8;
            if payload_start > data.len() || chunk_size > data.len().saturating_sub(payload_start) {
                break;
            }
            let payload = data.get(payload_start..payload_start + chunk_size)?;
            let dims = match chunk_id {
                b"VP8X" => read_vp8x(payload),
                b"VP8 " => read_vp8(payload),
                b"VP8L" => read_vp8l(payload),
                b"ANMF" => read_anmf(payload),
                _ => None,
            };
            if let Some((w, h)) = dims {
                best_canvas = Some(match best_canvas {
                    Some((cw, ch)) => (cw.max(w), ch.max(h)),
                    None => (w, h),
                });
            }
            let padded = (chunk_size + 1) & !1;
            pos = pos.saturating_add(8).saturating_add(padded);
            if pos > data.len() {
                break;
            }
        }
        best_canvas
    }

    /// Read up to 1MiB and parse WebP canvas dimensions when ffprobe reports
    /// 0×0.
    ///
    /// # Errors
    /// Returns an error if the file cannot be opened or read.
    pub fn dimensions_from_path(path: &Path) -> std::io::Result<Option<(u32, u32)>> {
        use std::io::Read;

        const PREFIX: usize = 1024 * 1024;
        let mut file = std::fs::File::open(path).map_err(|err| {
            crate::media_conversion_gate::probe_layer_audit(
                "webp_dimension_open_failed",
                path,
                format!("failed to open WebP for dimension probe: {err}"),
            );
            err
        })?;
        let mut buf = vec![0u8; PREFIX];
        let n = file.read(&mut buf).map_err(|err| {
            crate::media_conversion_gate::probe_layer_audit(
                "webp_dimension_read_failed",
                path,
                format!("failed to read WebP for dimension probe: {err}"),
            );
            err
        })?;
        Ok(buf.get(..n).and_then(dimensions_from_bytes))
    }

    /// Canvas dimensions for animated/static WebP when ffprobe returns 0×0.
    ///
    /// Tries a 1MiB prefix first, then a full-file parse for Safari-style
    /// exports where canvas size only appears in a late ANMF chunk.
    ///
    /// # Errors
    /// Returns an error if the file cannot be opened or read.
    pub fn canvas_dimensions_from_path(path: &Path) -> std::io::Result<Option<(u32, u32)>> {
        match dimensions_from_path(path) {
            Ok(Some(v)) => Ok(Some(v)),
            Ok(None) => {
                let data = std::fs::read(path).map_err(|err| {
                    crate::media_conversion_gate::probe_layer_audit(
                        "webp_canvas_full_read_failed",
                        path,
                        format!("failed to read full WebP for canvas probe: {err}"),
                    );
                    err
                })?;
                Ok(dimensions_from_bytes(&data))
            }
            Err(err) => Err(err),
        }
    }

    /// Count ANMF animation frames using RIFF-aware chunk traversal.
    ///
    /// The previous implementation used `data.windows(4).filter(ANMF)` which
    /// can false-positive on ANMF byte sequences inside VP8 payload data.
    /// RIFF traversal follows chunk boundaries exactly.
    ///
    /// # Errors
    /// Returns an error if a chunk size value overflows usize or data is
    /// malformed.
    pub fn count_frames_from_bytes(data: &[u8]) -> crate::unified_error::Result<u32> {
        if data.len() < 12 {
            return Ok(0);
        }

        // Verify RIFF/WEBP header
        if data.get(0..4) != Some(b"RIFF") || data.get(8..12) != Some(b"WEBP") {
            return Ok(0);
        }

        let mut count = 0u32;
        let mut pos = 12usize; // skip RIFF header + WEBP fourcc

        while pos + 8 <= data.len() {
            // Explicit bounds check for chunk ID
            let chunk_id = data.get(pos..pos + 4).ok_or_else(|| {
                crate::unified_error::ImgQualityError::AnalysisError(format!(
                    "WebP frame count: chunk ID truncated at offset {pos}"
                ))
            })?;

            // Explicit bounds check for chunk size
            let chunk_size_bytes = data.get(pos + 4..pos + 8).ok_or_else(|| {
                crate::unified_error::ImgQualityError::AnalysisError(format!(
                    "WebP frame count: chunk size truncated at offset {}",
                    pos + 4
                ))
            })?;

            let chunk_size = crate::numeric_cast::u32_to_usize_strict(
                u32::from_le_bytes([
                    chunk_size_bytes[0],
                    chunk_size_bytes[1],
                    chunk_size_bytes[2],
                    chunk_size_bytes[3],
                ]),
                "webp_chunk_size",
            )
            .ok_or_else(|| {
                crate::unified_error::ImgQualityError::NumericError(format!(
                    "WebP frame count: chunk size at offset {pos} exceeds usize::MAX"
                ))
            })?;

            // Validate chunk size doesn't overflow buffer
            let payload_start = pos + 8;
            if payload_start > data.len() || chunk_size > data.len() - payload_start {
                crate::media_conversion_gate::probe_image_format_batch_audit(
                    "probe_image_format",
                    format!(
                        "WebP frame count: chunk at offset {pos} claims size {chunk_size} but \
                         only {} bytes remain; stopping traversal",
                        data.len().saturating_sub(payload_start)
                    ),
                );
                break;
            }

            if chunk_id == b"ANMF" {
                count = count.saturating_add(1);
            }

            // Chunks are padded to even byte boundaries
            let padded = (chunk_size + 1) & !1;

            // Check for overflow before advancing
            let Some(p) = (8usize)
                .checked_add(padded)
                .and_then(|step| pos.checked_add(step))
            else {
                crate::media_conversion_gate::probe_image_format_batch_audit(
                    "probe_image_format",
                    format!(
                        "WebP frame count: position overflow at offset {pos} with chunk size \
                         {chunk_size}"
                    ),
                );
                break;
            };
            pos = p;

            // Safety check: prevent infinite loop
            if pos > data.len() {
                break;
            }
        }

        Ok(count)
    }

    /// Parse animated WebP RIFF/ANMF chunks and return total duration in
    /// seconds.
    ///
    /// ANMF payload: 24-byte header, bytes 12..15 = frame duration in ms
    /// (24-bit LE). Returns None if not animated WebP or no ANMF chunks
    /// with valid durations.
    #[must_use]
    pub fn duration_secs_from_bytes(data: &[u8]) -> Option<f32> {
        const MAX_FRAME_DURATION_MS: u32 = 60_000; // 60 seconds per frame is absurd
        const MAX_TOTAL_DURATION_MS: u64 = 600_000; // 10 minutes total is absurd

        if data.len() < 12 {
            return None;
        }

        // Verify RIFF/WEBP header
        if data.get(0..4) != Some(b"RIFF") || data.get(8..12) != Some(b"WEBP") {
            return None;
        }

        // Check for ANIM chunk (animation marker)
        if !data.windows(4).any(|w| w == b"ANIM") {
            return None;
        }

        let mut pos = 12usize; // RIFF payload start
        let mut total_ms = 0u64;

        while pos + 8 <= data.len() {
            // Explicit bounds check for chunk ID
            let Some(chunk_id) = data.get(pos..pos + 4) else {
                crate::media_conversion_gate::probe_image_format_batch_audit(
                    "probe_image_format",
                    format!("WebP duration: chunk ID truncated at offset {pos}"),
                );
                break;
            };

            // Explicit bounds check for chunk size
            let Some(chunk_size_bytes) = data.get(pos + 4..pos + 8) else {
                crate::media_conversion_gate::probe_image_format_batch_audit(
                    "probe_image_format",
                    format!("WebP duration: chunk size truncated at offset {}", pos + 4),
                );
                break;
            };

            let chunk_size_u32 = u32::from_le_bytes([
                chunk_size_bytes[0],
                chunk_size_bytes[1],
                chunk_size_bytes[2],
                chunk_size_bytes[3],
            ]);

            let Some(chunk_size) =
                crate::numeric_cast::u32_to_usize_strict(chunk_size_u32, "webp_chunk_size")
            else {
                crate::media_conversion_gate::probe_image_format_batch_audit(
                    "probe_image_format",
                    format!("WebP duration: chunk size overflow at offset {pos}"),
                );
                break;
            };

            let payload_start = pos + 8;

            // Strict bounds: if chunk_size is malformed, stop trusting RIFF traversal
            if chunk_size > data.len().saturating_sub(payload_start) {
                crate::media_conversion_gate::probe_image_format_batch_audit(
                    "probe_image_format",
                    format!(
                        "WebP duration: chunk at offset {pos} claims size {chunk_size} but only \
                         {} bytes remain",
                        data.len().saturating_sub(payload_start)
                    ),
                );
                break;
            }

            // ANMF frame header is 24 bytes. Duration is a 24-bit little-endian integer at
            // offset 12..15.
            if chunk_id == b"ANMF" && payload_start + 15 <= data.len() {
                // Explicit bounds check for duration bytes
                if let Some(dur_bytes) = data.get(payload_start + 12..payload_start + 15) {
                    let duration_ms = u32::from(dur_bytes[0])
                        | (u32::from(dur_bytes[1]) << 8)
                        | (u32::from(dur_bytes[2]) << 16);

                    // Validate duration is reasonable
                    if duration_ms > 0 && duration_ms <= MAX_FRAME_DURATION_MS {
                        total_ms = total_ms.saturating_add(u64::from(duration_ms));

                        // Early exit if total duration exceeds sanity limit
                        if total_ms > MAX_TOTAL_DURATION_MS {
                            crate::media_conversion_gate::probe_image_format_batch_audit(
                                "probe_image_format",
                                format!(
                                    "WebP duration: total duration {total_ms} ms exceeds sanity \
                                     limit {MAX_TOTAL_DURATION_MS} ms"
                                ),
                            );
                            return None;
                        }
                    }
                }
            }

            // Advance to next chunk (with padding)
            let padded = (chunk_size + 1) & !1;
            let Some(p) = payload_start.checked_add(padded) else {
                crate::media_conversion_gate::probe_image_format_batch_audit(
                    "probe_image_format",
                    format!("WebP duration: position overflow at offset {payload_start}"),
                );
                break;
            };
            pos = p;
        }

        // If RIFF traversal failed (common for Safari exports), fall back to marker
        // scan
        if total_ms == 0 {
            for idx in data
                .windows(4)
                .enumerate()
                .filter_map(|(i, w)| if w == b"ANMF" { Some(i) } else { None })
            {
                // ANMF chunk layout: "ANMF" (4) + size (4) + payload...
                // duration is 24-bit LE at payload offset 12..15 => idx + 8 + 12..15
                let dur_off = idx + 8 + 12;
                if let Some(dur_bytes) = data.get(dur_off..dur_off + 3) {
                    let duration_ms = u32::from(dur_bytes[0])
                        | (u32::from(dur_bytes[1]) << 8)
                        | (u32::from(dur_bytes[2]) << 16);

                    if duration_ms > 0 && duration_ms <= MAX_FRAME_DURATION_MS {
                        total_ms = total_ms.saturating_add(u64::from(duration_ms));

                        if total_ms > MAX_TOTAL_DURATION_MS {
                            return None;
                        }
                    }
                }
            }
        }

        // Validate final duration is reasonable
        if total_ms == 0 || total_ms > MAX_TOTAL_DURATION_MS {
            return None;
        }

        Some(crate::numeric_cast::f64_to_f32_lossy(
            crate::numeric_cast::u64_to_f64(total_ms) / crate::constants::MS_PER_SEC_F64,
        ))
    }

    #[derive(Debug, Clone, Copy, PartialEq)]
    pub struct WebpTimingStats {
        pub frame_count: u32,
        pub duration_secs: f64,
        pub fps: f64,
    }

    /// Aggregate animation timing from ANMF frame delays (same source as
    /// [`duration_secs_from_bytes`]).
    ///
    /// # Errors
    /// Returns an error if RIFF frame traversal fails.
    pub fn timing_stats_from_bytes(
        data: &[u8],
    ) -> crate::unified_error::Result<Option<WebpTimingStats>> {
        let frame_count = count_frames_from_bytes(data)?;
        if frame_count <= 1 {
            return Ok(None);
        }
        let Some(duration_secs) = duration_secs_from_bytes(data) else {
            return Ok(None);
        };
        let duration_secs = f64::from(duration_secs);
        if !duration_secs.is_finite() || duration_secs <= 0.0_f64 {
            return Ok(None);
        }
        let fps = f64::from(frame_count) / duration_secs;
        if !fps.is_finite() || fps <= 0.0_f64 {
            return Ok(None);
        }
        Ok(Some(WebpTimingStats {
            frame_count,
            duration_secs,
            fps,
        }))
    }

    /// Detects if a WebP file is lossless by reading it from disk.
    ///
    /// # Errors
    /// Returns an error if the file cannot be read or if the WebP header is
    /// corrupted.
    pub fn is_lossless(path: &Path) -> Result<bool> {
        let b = fs::read(path)?;
        Ok(is_lossless_from_bytes(&b))
    }

    /// Detects if a WebP file is animated by reading it from disk.
    ///
    /// # Errors
    /// Returns an error if the file cannot be read or if the WebP header is
    /// corrupted.
    pub fn is_animated(path: &Path) -> Result<bool> {
        let b = fs::read(path)?;
        Ok(is_animated_from_bytes(&b))
    }

    /// Minimal animated WebP with two ANMF frames (100 ms + 200 ms) for unit
    /// tests only.
    #[cfg(test)]
    pub(crate) fn synthetic_two_frame_animated_webp_for_test() -> Vec<u8> {
        fn anmf_chunk(duration_ms: u32) -> Vec<u8> {
            let mut payload = vec![0u8; 24];
            payload[12] = crate::numeric_cast::u32_shifted_byte_to_u8(duration_ms, 0);
            payload[13] = crate::numeric_cast::u32_shifted_byte_to_u8(duration_ms, 8);
            payload[14] = crate::numeric_cast::u32_shifted_byte_to_u8(duration_ms, 16);
            payload.extend_from_slice(b"VP8L\x00\x00\x00\x00");
            let size = u32::try_from(payload.len()).expect("test anmf payload fits u32");
            let mut chunk = b"ANMF".to_vec();
            chunk.extend_from_slice(&size.to_le_bytes());
            chunk.extend(payload);
            if !chunk.len().is_multiple_of(2) {
                chunk.push(0);
            }
            chunk
        }

        let vp8x = [
            b'V', b'P', b'8', b'X', 10, 0, 0, 0, 0x02, 0, 0, 0, 99, 0, 0, 79, 0, 0,
        ];
        let anim = [b'A', b'N', b'I', b'M', 0, 0, 0, 0];
        let mut body = Vec::new();
        body.extend_from_slice(&vp8x);
        body.extend_from_slice(&anim);
        body.extend(anmf_chunk(100));
        body.extend(anmf_chunk(200));

        let riff_size = u32::try_from(body.len() + 4).expect("test webp body fits u32");
        let mut out = vec![b'R', b'I', b'F', b'F'];
        out.extend_from_slice(&riff_size.to_le_bytes());
        out.extend_from_slice(b"WEBP");
        out.extend(body);
        out
    }
}

pub mod gif {
    use crate::unified_error::ImgQualityError;
    use std::fs;
    use std::path::Path;

    #[derive(Debug, Clone, Copy, PartialEq)]
    pub struct GifTimingStats {
        pub frame_count: u32,
        pub duration_secs: f64,
        pub average_delay_ms: f64,
        pub frame_delay_variation: f64,
        pub fps: f64,
    }

    /// Count GIF frames from raw bytes.
    ///
    /// # Errors
    /// Returns an error if the GIF is malformed or decoding fails.
    pub fn count_frames_from_bytes(data: &[u8]) -> crate::unified_error::Result<u32> {
        let mut options = gif::DecodeOptions::new();
        options.set_color_output(gif::ColorOutput::Indexed);

        let mut decoder = options.read_info(data).map_err(|err| {
            ImgQualityError::ResultAnomaly(format!("Failed to decode GIF frame stream: {err}"))
        })?;

        let mut count = 0u32;
        loop {
            match decoder.read_next_frame() {
                Ok(Some(_)) => count = count.saturating_add(1),
                Ok(None) => break,
                Err(err) => {
                    return Err(ImgQualityError::ResultAnomaly(format!(
                        "Failed to decode GIF frame during frame count: {err}"
                    )));
                }
            }
        }

        Ok(count)
    }

    /// Parse GIF frame delays from raw bytes and return aggregate timing
    /// statistics.
    ///
    /// # Errors
    /// Returns an error if GIF decoding fails.
    pub fn timing_stats_from_bytes(
        data: &[u8],
    ) -> crate::unified_error::Result<Option<GifTimingStats>> {
        let mut options = gif::DecodeOptions::new();
        options.set_color_output(gif::ColorOutput::Indexed);

        let mut decoder = options.read_info(data).map_err(|err| {
            ImgQualityError::ResultAnomaly(format!("Failed to decode GIF timing stream: {err}"))
        })?;
        let mut delays_secs = Vec::new();

        loop {
            match decoder.read_next_frame() {
                Ok(Some(frame)) => {
                    delays_secs.push(
                        crate::numeric_cast::u64_to_f64(u64::from(frame.delay))
                            / crate::constants::CENTISECS_PER_SEC_F64,
                    );
                }
                Ok(None) => break,
                Err(err) => {
                    return Err(ImgQualityError::ResultAnomaly(format!(
                        "Failed to decode GIF frame during timing parse: {err}"
                    )));
                }
            }
        }

        let frame_count = u32::try_from(delays_secs.len()).map_err(|err| {
            ImgQualityError::NumericError(format!("GIF timing frame count overflow: {err}"))
        })?;
        if frame_count == 0 {
            return Ok(None);
        }

        let duration_secs = delays_secs.iter().sum::<f64>();
        if !duration_secs.is_finite() || duration_secs <= 0.0_f64 {
            return Ok(None);
        }

        let mean_secs = duration_secs / f64::from(frame_count);
        if !mean_secs.is_finite() || mean_secs <= f64::EPSILON {
            return Ok(None);
        }

        let variance = delays_secs
            .iter()
            .map(|delay| (delay - mean_secs).powi(2))
            .sum::<f64>()
            / f64::from(frame_count);
        let std_dev = variance.sqrt();
        let fps = f64::from(frame_count) / duration_secs;
        if !fps.is_finite() || fps <= 0.0_f64 {
            return Ok(None);
        }

        Ok(Some(GifTimingStats {
            frame_count,
            duration_secs,
            average_delay_ms: mean_secs * crate::constants::MS_PER_SEC_F64,
            frame_delay_variation: (std_dev / mean_secs).clamp(0.0, 1.0),
            fps,
        }))
    }

    /// Parse GIF Graphic Control Extension (GCE) blocks and return total
    /// duration in seconds. Returns None if no GCE blocks found or data is
    /// truncated. # Errors
    /// Returns an error if GIF timing parsing fails.
    pub fn duration_secs_from_bytes(data: &[u8]) -> crate::unified_error::Result<Option<f32>> {
        Ok(timing_stats_from_bytes(data)?
            .map(|stats| crate::numeric_cast::f64_to_f32_lossy(stats.duration_secs)))
    }

    /// # Errors
    /// Returns an error if the file cannot be read or GIF timing parsing fails.
    pub fn get_duration_secs(path: &Path) -> crate::unified_error::Result<Option<f32>> {
        let b = fs::read(path)?;
        duration_secs_from_bytes(&b)
    }

    /// # Errors
    /// Returns an error if the file cannot be read or GIF timing parsing fails.
    pub fn get_timing_stats(path: &Path) -> crate::unified_error::Result<Option<GifTimingStats>> {
        let b = fs::read(path)?;
        timing_stats_from_bytes(&b)
    }

    /// # Errors
    /// Returns an error if the animation detection fails due to invalid data.
    pub fn is_animated_from_bytes(data: &[u8]) -> crate::unified_error::Result<bool> {
        Ok(count_frames_from_bytes(data)? > 1)
    }

    /// # Errors
    /// Returns an error if the file cannot be read or animation detection
    /// fails.
    pub fn is_animated(path: &Path) -> crate::unified_error::Result<bool> {
        let b = fs::read(path)?;
        is_animated_from_bytes(&b)
    }

    /// # Errors
    /// Returns an error if the file cannot be read or frame count detection
    /// fails.
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

    pub(crate) fn parse_pixi_max_depth(pixi_data: &[u8]) -> Result<Option<u8>> {
        if pixi_data.len() < 5 {
            return Ok(None);
        }

        let num_ch = crate::numeric_cast::u8_to_usize_strict(pixi_data[4], "avif_pixi_num_ch")
            .ok_or_else(|| {
                ImgQualityError::AnalysisError("AVIF pixi num_ch overflow".to_string())
            })?;
        if num_ch == 0 || pixi_data.len() < 5 + num_ch {
            return Ok(None);
        }

        Ok(pixi_data
            .get(5..5 + num_ch)
            .and_then(|slice| slice.iter().copied().max()))
    }

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
                } else if is_444 {
                    return Ok(false);
                }
            }

            // Dimension 3: high_bitdepth/twelve_bit
            if is_444 && (twelve_bit == 1 || (high_bitdepth == 1 && seq_profile >= 1)) {
                return Ok(true);
            }

            // NOTE: Dimension 4 (Profile 0 + 4:4:4) removed.
            // AV1 Profile 0 (Main) is 4:2:0 only per spec — the combination
            // (is_444 && seq_profile == 0) is unreachable for valid AVIF files
            // and would be a guess for malformed data.

            // Dimension 4: pixi box
            if is_444 && let Some(pixi_data) = find_box_data_recursive(data, *b"pixi") {
                if let Some(max_depth) = parse_pixi_max_depth(pixi_data)? {
                    if max_depth >= 12 {
                        return Ok(true);
                    }
                } else {
                    crate::media_conversion_gate::probe_image_format_batch_audit(
                        "probe_heic",
                        "AVIF Analysis: pixi depth unavailable; preserving unknown precision \
                         instead of defaulting to 8-bit for lossless detection",
                    );
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
    /// Returns an error if the file cannot be read, or if the AVIF header is
    /// missing critical property markers.
    pub fn is_lossless(path: &Path) -> Result<bool> {
        let b = fs::read(path)?;
        is_lossless_from_bytes(&b, path)
    }
}

pub mod jxl {
    use crate::common_utils::find_any_box_recursive;
    use crate::unified_error::{ImgQualityError, Result};
    use std::fs;
    use std::path::Path;

    /// Detect JXL (JPEG XL) lossless encoding — multi-dimension analysis.
    /// Check if the image bytes represent a lossless encoding.
    ///
    /// # Errors
    /// Returns an error if the format cannot be identified or parsed.
    pub fn is_lossless_from_bytes(data: &[u8], path: &Path) -> Result<bool> {
        use std::io::Cursor;

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

        // Dimension 2: Use jxl-oxide to parse the codestream and check xyb_encoded
        match ::jxl_oxide::JxlImage::builder().read(Cursor::new(data)) {
            Ok(image) => {
                let is_lossy = image.image_header().metadata.xyb_encoded;
                Ok(!is_lossy)
            }
            Err(e) => Err(ImgQualityError::AnalysisError(format!(
                "JXL: jxl-oxide failed to parse — {} ({})",
                path.display(),
                e
            ))),
        }
    }

    /// Verifies if a file starts with a valid JXL codestream signature.
    ///
    /// # Errors
    ///
    /// Returns an error if the file cannot be opened or read.
    pub fn verify_signature(path: &Path) -> std::io::Result<bool> {
        use std::io::Read;
        let mut file = fs::File::open(path)?;
        let mut sig = [0u8; 2];
        file.read_exact(&mut sig)?;
        Ok(sig == [0xFF, 0x0A] || sig == [0x00, 0x00])
    }

    /// Verifies if a file starts with a valid JXL codestream signature.
    ///
    /// # Errors
    ///
    /// Returns an error if the file cannot be opened or read.
    pub fn is_valid(path: &Path) -> std::io::Result<bool> {
        verify_signature(path)
    }
}

pub mod tiff_family {
    use crate::unified_error::{ImgQualityError, Result};
    use serde_json::Value;
    use std::io::{Read, Seek, SeekFrom};
    use std::path::Path;
    use std::process::Command;

    /// Helper to parse array values or space-separated strings as the first u64
    fn parse_first_u64(val: &Value) -> Result<u64> {
        if let Some(n) = val.as_u64() {
            return Ok(n);
        }
        if let Some(s) = val.as_str()
            && let Some(num_str) = s.split_whitespace().next()
        {
            return num_str
                .parse()
                .map_err(|e| ImgQualityError::AnalysisError(format!("Failed to parse u64: {e}")));
        }
        if let Some(arr) = val.as_array()
            && let Some(v) = arr.first()
        {
            if let Some(num) = v.as_u64() {
                return Ok(num);
            }
            if let Some(s) = v.as_str() {
                return s.parse().map_err(|e| {
                    ImgQualityError::AnalysisError(format!("Failed to parse u64: {e}"))
                });
            }
        }
        Err(ImgQualityError::AnalysisError(
            "Value is not a valid u64 or u64 string".into(),
        ))
    }

    /// Detect if a TIFF-family file (TIFF, DNG) is lossless using `exiftool` to target the main image IFD.
    pub fn is_lossless_tiff_family(path: &Path) -> Result<bool> {
        let output = Command::new("exiftool")
            .arg("-n")
            .arg("-j")
            .arg("-G1")
            .arg("-a")
            .arg("-Compression")
            .arg("-PhotometricInterpretation")
            .arg("-SubfileType")
            .arg("-ImageWidth")
            .arg("-ImageHeight")
            .arg("-StripOffsets")
            .arg("-StripByteCounts")
            .arg("-TileOffsets")
            .arg("-TileByteCounts")
            .arg(crate::path_safety::exiftool_path_arg(path).as_ref())
            .output()
            .map_err(|e| {
                ImgQualityError::AnalysisError(format!("Failed to execute exiftool: {e}"))
            })?;

        if !output.status.success() {
            return Err(ImgQualityError::AnalysisError(format!(
                "exiftool failed with status {}: {}",
                output.status,
                String::from_utf8_lossy(&output.stderr)
            )));
        }

        let json_str = String::from_utf8_lossy(&output.stdout);
        let parsed: Value = serde_json::from_str(&json_str).map_err(|e| {
            ImgQualityError::AnalysisError(format!("Failed to parse exiftool JSON: {e}"))
        })?;

        let obj = parsed
            .as_array()
            .and_then(|arr| arr.first())
            .and_then(|val| val.as_object())
            .ok_or_else(|| {
                ImgQualityError::AnalysisError("Invalid exiftool JSON structure".into())
            })?;

        // Group ExifTool keys by their IFD namespace (e.g., IFD0, SubIFD)
        let mut ifds: std::collections::HashMap<&str, std::collections::HashMap<&str, &Value>> =
            std::collections::HashMap::new();

        for (key, val) in obj {
            if let Some((prefix, tag)) = key.split_once(':') {
                ifds.entry(prefix).or_default().insert(tag, val);
            }
        }

        let mut best_ifd = None;
        let mut max_pixels = 0u64;

        for tags in ifds.values() {
            let photo_interp = tags
                .get("PhotometricInterpretation")
                .map(|v| parse_first_u64(v))
                .transpose()?;
            let is_raw = photo_interp == Some(32803) || photo_interp == Some(34892);

            let subfile_type = tags
                .get("SubfileType")
                .map(|v| parse_first_u64(v))
                .transpose()?;
            let is_full_res = subfile_type == Some(0) || subfile_type.is_none();

            if (is_raw || is_full_res)
                && let (Some(w), Some(h)) = (
                    tags.get("ImageWidth")
                        .map(|v| parse_first_u64(v))
                        .transpose()?,
                    tags.get("ImageHeight")
                        .map(|v| parse_first_u64(v))
                        .transpose()?,
                )
            {
                let pixels = w.saturating_mul(h);
                if pixels >= max_pixels && w > 0 && h > 0 {
                    max_pixels = pixels;
                    best_ifd = Some(tags);
                }
            }
        }

        // Fallback: pick largest image if none match raw/full-res precisely
        if best_ifd.is_none() {
            for tags in ifds.values() {
                if let (Some(w), Some(h)) = (
                    tags.get("ImageWidth")
                        .map(|v| parse_first_u64(v))
                        .transpose()?,
                    tags.get("ImageHeight")
                        .map(|v| parse_first_u64(v))
                        .transpose()?,
                ) {
                    let pixels = w.saturating_mul(h);
                    if pixels > max_pixels && w > 0 && h > 0 {
                        max_pixels = pixels;
                        best_ifd = Some(tags);
                    }
                }
            }
        }

        let main_ifd = best_ifd.ok_or_else(|| {
            ImgQualityError::AnalysisError("Could not identify main raw IFD in DNG".into())
        })?;

        let is_raw_photo = main_ifd
            .get("PhotometricInterpretation")
            .map(|v| parse_first_u64(v))
            .transpose()?
            .is_some_and(|p| p == 32803 || p == 34892);

        let compression = main_ifd
            .get("Compression")
            .map(|v| parse_first_u64(v))
            .transpose()?
            .ok_or_else(|| {
                ImgQualityError::AnalysisError("TIFF main IFD missing Compression tag".into())
            })?;

        match compression {
            // Lossless compressions (pixel-exact round-trip):
            //   1  = Uncompressed
            //   5  = LZW (lossless)
            //   8  = Deflate/ZIP (lossless)
            //  32773 = PackBits (lossless RLE)
            1 | 5 | 8 | 32773 => Ok(true),
            // Compression 7 is lossless Huffman JPEG in DNG (raw context),
            // but standard lossy JPEG in normal TIFF files.
            7 => Ok(is_raw_photo),
            // Lossy compressions:
            //   6     = Old-style JPEG (lossy)
            //  34892  = Lossy JPEG in DNG
            //  50001  = PIXARLOG — log-quantized, NOT pixel-exact
            //  34676  = SGILog — log-encoded, lossy
            //  34677  = SGILog24 — log-encoded, lossy
            6 | 34892 | 50001 | 34676 | 34677 => Ok(false),
            52546 => {
                // JPEG XL
                let offset_val = match main_ifd.get("StripOffsets") {
                    Some(v) => Some(v),
                    None => main_ifd.get("TileOffsets"),
                };
                let offset = offset_val
                    .map(|v| parse_first_u64(v))
                    .transpose()?
                    .ok_or_else(|| {
                        ImgQualityError::AnalysisError(
                            "DNG with JPEG XL missing StripOffsets/TileOffsets".into(),
                        )
                    })?;

                let length_val = match main_ifd.get("StripByteCounts") {
                    Some(v) => Some(v),
                    None => main_ifd.get("TileByteCounts"),
                };
                let length = length_val
                    .map(|v| parse_first_u64(v))
                    .transpose()?
                    .ok_or_else(|| {
                        ImgQualityError::AnalysisError(
                            "DNG with JPEG XL missing StripByteCounts/TileByteCounts".into(),
                        )
                    })?;

                let mut f = std::fs::File::open(path)?;
                f.seek(SeekFrom::Start(offset))?;

                let safe_length = std::cmp::min(length, 10 * 1024 * 1024); // max 10MB to read header
                let buffer_size =
                    crate::numeric_cast::u64_to_usize_strict(safe_length, "jxl_buffer_size")
                        .ok_or_else(|| {
                            ImgQualityError::AnalysisError("JXL buffer size overflow".to_string())
                        })?;
                let mut buffer = vec![0u8; buffer_size];
                let bytes_read = f.read(&mut buffer)?;
                buffer.truncate(bytes_read);

                crate::image_formats::jxl::is_lossless_from_bytes(&buffer, path)
            }
            _ => {
                // Conservative fallback for unknown compressions
                Ok(false)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn test_webp_lossless_detection() {
        // Test WebP lossless detection with simple VP8L header
        let webp_lossless = b"RIFF\x1A\x00\x00\x00WEBPVP8L\x08\x00\x00\x00\x10\x10\x00\x00";
        let mut file =
            NamedTempFile::new().unwrap_or_else(|_| panic!("Failed to create temporary file"));
        file.write_all(webp_lossless)
            .unwrap_or_else(|_| panic!("Failed to write to file"));

        assert!(
            webp::is_lossless(file.path()).expect("VP8L lossless probe should parse"),
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
            !webp::is_lossless(file.path()).expect("VP8 lossy probe should parse"),
            "VP8 chunk should be detected as lossy"
        );
    }

    #[test]
    fn test_avif_pixi_max_depth_preserves_unknown_when_channels_missing() {
        let pixi = [0, 0, 0, 0, 3, 8];
        assert_eq!(avif::parse_pixi_max_depth(&pixi).unwrap(), None);
    }

    #[test]
    fn test_gif_frame_count() {
        let mut gif_data = Vec::new();
        {
            let mut encoder =
                ::gif::Encoder::new(&mut gif_data, 1, 1, &[0, 0, 0, 255, 255, 255]).unwrap();
            let frame1 = ::gif::Frame {
                delay: 10,
                width: 1,
                height: 1,
                buffer: std::borrow::Cow::Borrowed(&[0]),
                ..Default::default()
            };
            encoder.write_frame(&frame1).unwrap();
            let frame2 = ::gif::Frame {
                delay: 10,
                width: 1,
                height: 1,
                buffer: std::borrow::Cow::Borrowed(&[1]),
                ..Default::default()
            };
            encoder.write_frame(&frame2).unwrap();
        }
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
    fn test_gif_timing_stats() {
        let mut gif_data = Vec::new();
        {
            let mut encoder =
                ::gif::Encoder::new(&mut gif_data, 1, 1, &[0, 0, 0, 255, 255, 255]).unwrap();
            let frame1 = ::gif::Frame {
                delay: 10,
                width: 1,
                height: 1,
                buffer: std::borrow::Cow::Borrowed(&[0]),
                ..Default::default()
            };
            let frame2 = ::gif::Frame {
                delay: 20,
                width: 1,
                height: 1,
                buffer: std::borrow::Cow::Borrowed(&[1]),
                ..Default::default()
            };
            encoder.write_frame(&frame1).unwrap();
            encoder.write_frame(&frame2).unwrap();
        }

        let stats = gif::timing_stats_from_bytes(&gif_data)
            .expect("timing stats should parse")
            .expect("animated GIF timing stats");
        assert_eq!(stats.frame_count, 2);
        assert!((stats.duration_secs - 0.3).abs() < 1.0e-6);
        assert!((stats.average_delay_ms - 150.0).abs() < 1.0e-6);
        assert!((stats.fps - (2.0 / 0.3)).abs() < 1.0e-6);
        assert!(stats.frame_delay_variation > 0.0);
    }

    #[test]
    fn webp_timing_stats_from_anmf_frame_delays() {
        let data = webp::synthetic_two_frame_animated_webp_for_test();
        let stats = webp::timing_stats_from_bytes(&data)
            .expect("animated WebP timing probe")
            .expect("animated WebP timing");
        assert_eq!(stats.frame_count, 2);
        assert!((stats.duration_secs - 0.3).abs() < 1.0e-6);
        assert!((stats.fps - (2.0 / 0.3)).abs() < 1.0e-6);
    }

    #[test]
    fn webp_canvas_dimensions_missing_file_returns_error_not_none() {
        let dir = tempfile::tempdir().expect("tempdir");
        let missing = dir.path().join("missing.webp");

        let err = webp::canvas_dimensions_from_path(&missing)
            .expect_err("missing WebP path must be an error");

        assert_eq!(err.kind(), std::io::ErrorKind::NotFound);
    }

    #[test]
    fn gif_timing_stats_malformed_bytes_returns_error_not_none() {
        let err = gif::timing_stats_from_bytes(b"GIF89a")
            .expect_err("malformed GIF timing must be an error");

        assert!(err.to_string().contains("GIF"));
    }

    #[test]
    fn gif_get_timing_stats_missing_file_returns_error_not_none() {
        let dir = tempfile::tempdir().expect("tempdir");
        let missing = dir.path().join("missing.gif");

        let err = gif::get_timing_stats(&missing).expect_err("missing GIF path must be an error");

        assert!(err.to_string().contains("No such file") || err.to_string().contains("not found"));
    }

    #[test]
    fn test_jxl_codestream_signature() {
        let jxl_codestream: &[u8] = &[0xFF, 0x0A, 0x00, 0x00];
        let mut file =
            NamedTempFile::new().unwrap_or_else(|_| panic!("Failed to create temporary file"));
        file.write_all(jxl_codestream)
            .unwrap_or_else(|_| panic!("Failed to write to file"));

        assert!(
            jxl::verify_signature(file.path())
                .expect("JXL codestream signature probe should parse"),
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
            jxl::verify_signature(path).is_err(),
            "Non-existent file should return false"
        );
    }
}
