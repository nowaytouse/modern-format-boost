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

    #[derive(Clone, Copy)]
    struct RiffChunk<'a> {
        id: [u8; 4],
        payload: &'a [u8],
    }

    #[derive(Clone, Copy, PartialEq, Eq)]
    enum FrameCodec {
        Lossy,
        Lossless,
    }

    fn parse_chunk<'a>(
        data: &'a [u8],
        pos: usize,
        end: usize,
        context: &str,
    ) -> Result<(RiffChunk<'a>, usize)> {
        if end > data.len() || end.saturating_sub(pos) < 8 {
            return Err(ImgQualityError::AnalysisError(format!(
                "WebP {context}: truncated chunk header at offset {pos}"
            )));
        }

        let id = [data[pos], data[pos + 1], data[pos + 2], data[pos + 3]];
        let size = crate::numeric_cast::u32_to_usize_strict(
            u32::from_le_bytes([
                data[pos + 4],
                data[pos + 5],
                data[pos + 6],
                data[pos + 7],
            ]),
            "webp_chunk_size",
        )
        .ok_or_else(|| {
            ImgQualityError::NumericError(format!(
                "WebP {context}: chunk size at offset {pos} does not fit usize"
            ))
        })?;
        let payload_start = pos.checked_add(8).ok_or_else(|| {
            ImgQualityError::NumericError(format!(
                "WebP {context}: chunk header offset overflow at {pos}"
            ))
        })?;
        let payload_end = payload_start.checked_add(size).ok_or_else(|| {
            ImgQualityError::NumericError(format!(
                "WebP {context}: chunk payload offset overflow at {pos}"
            ))
        })?;
        if payload_end > end {
            return Err(ImgQualityError::AnalysisError(format!(
                "WebP {context}: chunk at offset {pos} claims {size} payload bytes beyond boundary {end}"
            )));
        }

        let next = payload_end.checked_add(size & 1).ok_or_else(|| {
            ImgQualityError::NumericError(format!(
                "WebP {context}: padded chunk offset overflow at {pos}"
            ))
        })?;
        if next > end {
            return Err(ImgQualityError::AnalysisError(format!(
                "WebP {context}: odd-sized chunk at offset {pos} is missing its RIFF padding byte"
            )));
        }
        if size & 1 == 1 && data[payload_end] != 0 {
            return Err(ImgQualityError::AnalysisError(format!(
                "WebP {context}: non-zero RIFF padding byte at offset {payload_end}"
            )));
        }

        Ok((
            RiffChunk {
                id,
                payload: &data[payload_start..payload_end],
            },
            next,
        ))
    }

    fn validate_chunks(data: &[u8], start: usize, end: usize, context: &str) -> Result<()> {
        let mut pos = start;
        while pos < end {
            let (_, next) = parse_chunk(data, pos, end, context)?;
            pos = next;
        }
        Ok(())
    }

    fn validated_webp_end(data: &[u8]) -> Result<usize> {
        if data.len() < 12 || data.get(0..4) != Some(b"RIFF") || data.get(8..12) != Some(b"WEBP") {
            return Err(ImgQualityError::AnalysisError(
                "WebP: invalid or truncated RIFF/WEBP header".to_string(),
            ));
        }
        let riff_size = crate::numeric_cast::u32_to_usize_strict(
            u32::from_le_bytes([data[4], data[5], data[6], data[7]]),
            "webp_riff_size",
        )
        .ok_or_else(|| {
            ImgQualityError::NumericError("WebP: RIFF size does not fit usize".to_string())
        })?;
        if riff_size < 4 {
            return Err(ImgQualityError::AnalysisError(
                "WebP: RIFF size is smaller than the WEBP FourCC".to_string(),
            ));
        }
        let end = 8usize.checked_add(riff_size).ok_or_else(|| {
            ImgQualityError::NumericError("WebP: RIFF boundary overflow".to_string())
        })?;
        if end > data.len() {
            return Err(ImgQualityError::AnalysisError(format!(
                "WebP: RIFF declares {end} bytes but input contains only {}",
                data.len()
            )));
        }
        validate_chunks(data, 12, end, "RIFF")?;
        Ok(end)
    }

    fn frame_codec(payload: &[u8]) -> Result<FrameCodec> {
        if payload.len() < 16 {
            return Err(ImgQualityError::AnalysisError(format!(
                "WebP ANMF: frame payload is {} bytes; expected at least 16",
                payload.len()
            )));
        }
        validate_chunks(payload, 16, payload.len(), "ANMF")?;

        let mut pos = 16;
        let mut codec = None;
        while pos < payload.len() {
            let (chunk, next) = parse_chunk(payload, pos, payload.len(), "ANMF")?;
            let candidate = match &chunk.id {
                b"VP8 " => Some(FrameCodec::Lossy),
                b"VP8L" => Some(FrameCodec::Lossless),
                _ => None,
            };
            if let Some(candidate) = candidate {
                if codec.replace(candidate).is_some() {
                    return Err(ImgQualityError::AnalysisError(
                        "WebP ANMF: frame contains more than one VP8/VP8L bitstream".to_string(),
                    ));
                }
            }
            pos = next;
        }

        codec.ok_or_else(|| {
            ImgQualityError::AnalysisError(
                "WebP ANMF: frame contains no VP8 or VP8L bitstream".to_string(),
            )
        })
    }

    fn animation_timing_ms(data: &[u8]) -> Result<Option<(u32, u64)>> {
        let end = validated_webp_end(data)?;
        let mut pos = 12;
        let mut has_anim = false;
        let mut frame_count = 0u32;
        let mut total_ms = 0u64;

        while pos < end {
            let (chunk, next) = parse_chunk(data, pos, end, "RIFF")?;
            match &chunk.id {
                b"ANIM" => {
                    if chunk.payload.len() < 6 {
                        return Err(ImgQualityError::AnalysisError(
                            "WebP ANIM: payload is shorter than 6 bytes".to_string(),
                        ));
                    }
                    has_anim = true;
                }
                b"ANMF" => {
                    frame_codec(chunk.payload)?;
                    frame_count = frame_count.checked_add(1).ok_or_else(|| {
                        ImgQualityError::NumericError(
                            "WebP animation frame count overflow".to_string(),
                        )
                    })?;
                    let duration_ms = u32::from(chunk.payload[12])
                        | (u32::from(chunk.payload[13]) << 8)
                        | (u32::from(chunk.payload[14]) << 16);
                    total_ms = total_ms.checked_add(u64::from(duration_ms)).ok_or_else(|| {
                        ImgQualityError::NumericError(
                            "WebP animation duration overflow".to_string(),
                        )
                    })?;
                }
                _ => {}
            }
            pos = next;
        }

        if frame_count == 0 {
            return Ok(None);
        }
        if !has_anim {
            return Err(ImgQualityError::AnalysisError(
                "WebP animation contains ANMF frames without an ANIM chunk".to_string(),
            ));
        }
        Ok(Some((frame_count, total_ms)))
    }

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
        let end = validated_webp_end(data)?;
        let mut pos = 12;
        let mut found_any_frame = false;
        let mut all_frames_lossless = true;
        while pos < end {
            let (chunk, next) = parse_chunk(data, pos, end, "RIFF")?;
            if chunk.id == *b"ANMF" {
                found_any_frame = true;
                if frame_codec(chunk.payload)? == FrameCodec::Lossy {
                    all_frames_lossless = false;
                }
            }
            pos = next;
        }
        if !found_any_frame {
            return Err(ImgQualityError::AnalysisError(
                "Animated WebP: no ANMF frames found".to_string(),
            ));
        }
        Ok(all_frames_lossless)
    }

    /// Refuse to forge an encoder quality from a decoded WebP bitstream.
    ///
    /// # Errors
    /// Returns an error if the format is unsupported, data is corrupted, or
    /// bounds are violated.
    pub fn estimate_quality_from_bytes(data: &[u8]) -> Result<u8> {
        validated_webp_end(data)?;
        Err(ImgQualityError::AnalysisError(
            "WebP encoder quality is not stored as a recoverable container field; preserving it as unknown"
                .to_string(),
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
        let Ok(end) = validated_webp_end(data) else {
            return false;
        };
        if let Some(features) = ::webp::BitstreamFeatures::new(data) {
            if features.has_animation() {
                return detect_webp_animation_is_lossless(data).unwrap_or(false);
            }
            if let Some(format) = features.format() {
                match format {
                    ::webp::BitstreamFormat::Lossless => return true,
                    ::webp::BitstreamFormat::Lossy => return false,
                    ::webp::BitstreamFormat::Undefined => {}
                }
            }
        }
        let mut pos = 12;
        while pos < end {
            let Ok((chunk, next)) = parse_chunk(data, pos, end, "RIFF") else {
                return false;
            };
            if chunk.id == *b"VP8L" {
                return true;
            }
            if chunk.id == *b"VP8 " {
                return false;
            }
            if chunk.id == *b"ANMF" {
                return detect_webp_animation_is_lossless(data).unwrap_or(false);
            }
            pos = next;
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

    /// Canvas dimensions reported by the installed libwebp decoder.
    #[must_use]
    pub fn dimensions_from_bytes(data: &[u8]) -> Option<(u32, u32)> {
        let features = ::webp::BitstreamFeatures::new(data)?;
        let dimensions = (features.width(), features.height());
        (dimensions.0 > 0 && dimensions.1 > 0).then_some(dimensions)
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
        Ok(animation_timing_ms(data)?.map_or(0, |timing| timing.0))
    }

    /// Parse animated WebP RIFF/ANMF chunks and return total duration in
    /// seconds.
    ///
    /// ANMF payload: 16-byte header, bytes 12..15 = frame duration in ms
    /// (24-bit LE). Returns None if not animated WebP or no ANMF chunks
    /// with valid durations.
    #[must_use]
    pub fn duration_secs_from_bytes(data: &[u8]) -> Option<f32> {
        let (_, total_ms) = animation_timing_ms(data).ok()??;
        if total_ms == 0 {
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
        let Some((frame_count, total_ms)) = animation_timing_ms(data)? else {
            return Ok(None);
        };
        if frame_count <= 1 || total_ms == 0 {
            return Ok(None);
        }
        let duration_secs = crate::numeric_cast::u64_to_f64(total_ms)
            / crate::constants::MS_PER_SEC_F64;
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
            let mut payload = vec![0u8; 16];
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
        let anim = [
            b'A', b'N', b'I', b'M', 6, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        ];
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
        let mut options = gif::DecodeOptions::new();
        options.set_color_output(gif::ColorOutput::Indexed);
        let mut decoder = options.read_info(data).map_err(|err| {
            ImgQualityError::ResultAnomaly(format!("Failed to decode GIF animation stream: {err}"))
        })?;

        let has_first_frame = decoder
            .read_next_frame()
            .map_err(|err| {
                ImgQualityError::ResultAnomaly(format!(
                    "Failed to decode GIF frame during animation detection: {err}"
                ))
            })?
            .is_some();
        if !has_first_frame {
            return Ok(false);
        }

        Ok(decoder
            .read_next_frame()
            .map_err(|err| {
                ImgQualityError::ResultAnomaly(format!(
                    "Failed to decode GIF frame during animation detection: {err}"
                ))
            })?
            .is_some())
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
        let exiftool = crate::common_utils::resolve_tool_path("exiftool").ok_or_else(|| {
            ImgQualityError::AnalysisError(
                "exiftool was not found or failed its runtime health check".to_string(),
            )
        })?;
        let output = Command::new(exiftool)
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
        let webp_lossless =
            b"RIFF\x12\x00\x00\x00WEBPVP8L\x05\x00\x00\x00\x2f\x00\x00\x00\x00\x00";
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
        let webp_lossy = b"RIFF\x0c\x00\x00\x00WEBPVP8 \x00\x00\x00\x00";
        let mut file =
            NamedTempFile::new().unwrap_or_else(|_| panic!("Failed to create temporary file"));
        file.write_all(webp_lossy)
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
    fn webp_riff_parser_ignores_payload_markers_and_rejects_nonzero_padding() {
        let mut data = b"RIFF\x12\x00\x00\x00WEBPJUNK\x05\x00\x00\x00ANMF!\x00".to_vec();
        assert_eq!(webp::count_frames_from_bytes(&data).unwrap(), 0);
        assert_eq!(webp::duration_secs_from_bytes(&data), None);

        *data.last_mut().expect("padding byte") = 1;
        assert!(webp::count_frames_from_bytes(&data).is_err());
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
