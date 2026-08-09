//! PNG genuineness validation and heuristic policy.
//!
//! Genuine PNG media receive full exemption: always treated as lossless, encoded
//! to JXL at effort 10 with no file-size gate. PNG quantization heuristics are
//! retained but disabled by default (`MFB_ENABLE_PNG_HEURISTIC`).

use crate::unified_error::{ImgQualityError, Result};
use std::io::BufReader;
use std::path::Path;

use super::format_detect::{FormatKind, detect_true_format, validate_format_forensic};

/// When set to `1`/`true`/`yes`, re-enable PNG quantization heuristics for
/// content-level lossy detection (256-color art, palette logos, etc.).
pub const ENV_ENABLE_PNG_HEURISTIC: &str = "MFB_ENABLE_PNG_HEURISTIC";

/// Effort value mandated for genuine PNG → lossless JXL encoding.
pub const PNG_LOSSLESS_JXL_EFFORT: u8 = 10;

const PNG_SIGNATURE: &[u8; 8] = b"\x89PNG\r\n\x1a\n";

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ApngAnimationInfo {
    /// Number of `fcTL` frames validated against `acTL`.
    pub frame_count: u32,
    /// Declared play count; zero means infinite looping.
    pub num_plays: u32,
    /// Sum of validated `fcTL` frame delays.
    pub duration_secs: f64,
}

fn invalid_png(message: impl Into<String>) -> ImgQualityError {
    ImgQualityError::AnalysisError(message.into())
}

/// Validate APNG control structure, CRCs, sequence numbers and frame timing.
/// A valid static PNG returns `Ok(None)`.
///
/// # Errors
/// Returns an error when PNG or APNG structure is malformed.
pub fn parse_apng_animation(data: &[u8]) -> Result<Option<ApngAnimationInfo>> {
    if data.get(..8) != Some(PNG_SIGNATURE) {
        return Err(invalid_png("Invalid PNG signature"));
    }

    let mut pos = 8usize;
    let mut canvas = None;
    let mut animation = None;
    let mut frame_count = 0u32;
    let mut next_sequence = 0u32;
    let mut duration_secs = 0.0f64;
    let mut saw_idat = false;
    let mut left_idat = false;
    let mut saw_fdat = false;
    let mut frame_has_data = false;
    let mut first_frame_uses_idat = false;
    let mut saw_iend = false;

    while pos < data.len() {
        let header = data
            .get(pos..pos.checked_add(8).ok_or_else(|| invalid_png("PNG offset overflow"))?)
            .ok_or_else(|| invalid_png("PNG chunk header is truncated"))?;
        let length_u32 = u32::from_be_bytes([header[0], header[1], header[2], header[3]]);
        if length_u32 > 0x7fff_ffff {
            return Err(invalid_png("PNG chunk length exceeds the specification limit"));
        }
        let length = usize::try_from(length_u32)
            .map_err(|_| invalid_png("PNG chunk length does not fit this platform"))?;
        let chunk_type = [header[4], header[5], header[6], header[7]];
        if !chunk_type.iter().all(|byte| byte.is_ascii_alphabetic())
            || !chunk_type[2].is_ascii_uppercase()
        {
            return Err(invalid_png("PNG chunk type contains invalid bytes or reserved bit"));
        }
        let payload_start = pos
            .checked_add(8)
            .ok_or_else(|| invalid_png("PNG payload offset overflow"))?;
        let crc_start = payload_start
            .checked_add(length)
            .ok_or_else(|| invalid_png("PNG payload length overflow"))?;
        let next = crc_start
            .checked_add(4)
            .ok_or_else(|| invalid_png("PNG CRC offset overflow"))?;
        let payload = data
            .get(payload_start..crc_start)
            .ok_or_else(|| invalid_png("PNG chunk payload is truncated"))?;
        let crc = data
            .get(crc_start..next)
            .ok_or_else(|| invalid_png("PNG chunk CRC is truncated"))?;
        let expected_crc = u32::from_be_bytes([crc[0], crc[1], crc[2], crc[3]]);
        let mut hasher = crc32fast::Hasher::new();
        hasher.update(&chunk_type);
        hasher.update(payload);
        if hasher.finalize() != expected_crc {
            return Err(invalid_png(format!(
                "PNG {} chunk CRC mismatch",
                String::from_utf8_lossy(&chunk_type)
            )));
        }

        if canvas.is_none() {
            if chunk_type != *b"IHDR" || payload.len() != 13 {
                return Err(invalid_png("PNG must begin with a 13-byte IHDR chunk"));
            }
            let width = u32::from_be_bytes([payload[0], payload[1], payload[2], payload[3]]);
            let height = u32::from_be_bytes([payload[4], payload[5], payload[6], payload[7]]);
            let bit_depth = payload[8];
            let color_type = payload[9];
            let valid_depth = match color_type {
                0 => [1, 2, 4, 8, 16].contains(&bit_depth),
                2 | 4 | 6 => [8, 16].contains(&bit_depth),
                3 => [1, 2, 4, 8].contains(&bit_depth),
                _ => false,
            };
            if width == 0
                || height == 0
                || width > 0x7fff_ffff
                || height > 0x7fff_ffff
                || !valid_depth
                || payload[10] != 0
                || payload[11] != 0
                || payload[12] > 1
            {
                return Err(invalid_png("PNG IHDR contains invalid dimensions or format fields"));
            }
            canvas = Some((width, height));
            pos = next;
            continue;
        }
        if chunk_type == *b"IHDR" {
            return Err(invalid_png("PNG contains more than one IHDR chunk"));
        }

        if saw_idat && chunk_type != *b"IDAT" {
            left_idat = true;
        }
        match &chunk_type {
            b"acTL" => {
                if animation.is_some() || saw_idat || payload.len() != 8 {
                    return Err(invalid_png(
                        "APNG acTL must be unique, 8 bytes long, and precede IDAT",
                    ));
                }
                let declared_frames =
                    u32::from_be_bytes([payload[0], payload[1], payload[2], payload[3]]);
                if declared_frames == 0 {
                    return Err(invalid_png("APNG acTL declares zero frames"));
                }
                let num_plays =
                    u32::from_be_bytes([payload[4], payload[5], payload[6], payload[7]]);
                animation = Some((declared_frames, num_plays));
            }
            b"fcTL" => {
                if animation.is_none()
                    || payload.len() != 26
                    || (!frame_has_data && frame_count > 0)
                {
                    return Err(invalid_png("APNG contains an invalid fcTL frame boundary"));
                }
                let sequence =
                    u32::from_be_bytes([payload[0], payload[1], payload[2], payload[3]]);
                if sequence != next_sequence {
                    return Err(invalid_png("APNG fcTL sequence has a gap or duplicate"));
                }
                next_sequence = next_sequence
                    .checked_add(1)
                    .ok_or_else(|| invalid_png("APNG sequence number overflow"))?;
                let width = u32::from_be_bytes([payload[4], payload[5], payload[6], payload[7]]);
                let height = u32::from_be_bytes([payload[8], payload[9], payload[10], payload[11]]);
                let x = u32::from_be_bytes([payload[12], payload[13], payload[14], payload[15]]);
                let y = u32::from_be_bytes([payload[16], payload[17], payload[18], payload[19]]);
                let Some((canvas_width, canvas_height)) = canvas else {
                    return Err(invalid_png("APNG frame appeared before PNG canvas metadata"));
                };
                if width == 0
                    || height == 0
                    || x.checked_add(width).is_none_or(|right| right > canvas_width)
                    || y.checked_add(height).is_none_or(|bottom| bottom > canvas_height)
                    || payload[24] > 2
                    || payload[25] > 1
                {
                    return Err(invalid_png("APNG fcTL contains invalid frame geometry or mode"));
                }
                let delay_num = u16::from_be_bytes([payload[20], payload[21]]);
                let delay_den = u16::from_be_bytes([payload[22], payload[23]]);
                duration_secs += f64::from(delay_num)
                    / f64::from(if delay_den == 0 { 100 } else { delay_den });
                frame_count = frame_count
                    .checked_add(1)
                    .ok_or_else(|| invalid_png("APNG frame count overflow"))?;
                if frame_count == 1 {
                    first_frame_uses_idat = !saw_idat;
                }
                frame_has_data = false;
            }
            b"fdAT" => {
                if animation.is_none()
                    || payload.len() < 4
                    || frame_count == 0
                    || (first_frame_uses_idat && frame_count == 1)
                {
                    return Err(invalid_png("APNG contains fdAT outside a valid frame"));
                }
                let sequence =
                    u32::from_be_bytes([payload[0], payload[1], payload[2], payload[3]]);
                if sequence != next_sequence {
                    return Err(invalid_png("APNG fdAT sequence has a gap or duplicate"));
                }
                next_sequence = next_sequence
                    .checked_add(1)
                    .ok_or_else(|| invalid_png("APNG sequence number overflow"))?;
                saw_fdat = true;
                frame_has_data = true;
            }
            b"IDAT" => {
                if left_idat || saw_fdat {
                    return Err(invalid_png("PNG IDAT chunks are not consecutive"));
                }
                saw_idat = true;
                if first_frame_uses_idat && frame_count == 1 {
                    frame_has_data = true;
                }
            }
            b"IEND" => {
                if !payload.is_empty() || !saw_idat || next != data.len() {
                    return Err(invalid_png("PNG has an invalid or non-final IEND chunk"));
                }
                saw_iend = true;
            }
            _ => {
                if chunk_type[0].is_ascii_uppercase()
                    && !matches!(&chunk_type, b"PLTE" | b"IDAT" | b"IEND")
                {
                    return Err(invalid_png("PNG contains an unknown critical chunk"));
                }
            }
        }

        pos = next;
        if saw_iend {
            break;
        }
    }

    if !saw_iend {
        return Err(invalid_png("PNG ended before IEND"));
    }
    let Some((declared_frames, num_plays)) = animation else {
        return Ok(None);
    };
    if frame_count != declared_frames || !frame_has_data {
        return Err(invalid_png(
            "APNG acTL frame count or final frame data does not match the stream",
        ));
    }
    Ok(Some(ApngAnimationInfo {
        frame_count,
        num_plays,
        duration_secs,
    }))
}

/// Ask the installed PNG decoder whether a file declares APNG animation.
///
/// # Errors
/// Returns an error when the file cannot be read or its PNG header is invalid.
pub fn is_apng_file(path: &Path) -> Result<bool> {
    let file = std::fs::File::open(path)?;
    let mut decoder = image::codecs::png::PngDecoder::new(BufReader::new(file));
    decoder.is_apng().map_err(|error| {
        invalid_png(format!(
            "PNG animation validation failed for {}: {error}",
            path.display()
        ))
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PngValidationOutcome {
    Confirmed,
    Rejected,
    ToolUnavailable,
}

#[must_use]
pub fn png_heuristic_enabled() -> bool {
    match std::env::var(ENV_ENABLE_PNG_HEURISTIC) {
        Ok(value) => matches!(
            value.trim(),
            "1" | "true" | "TRUE" | "yes" | "YES" | "on" | "ON"
        ),
        Err(std::env::VarError::NotPresent) => false,
        Err(e) => {
            tracing::debug!("PNG heuristic env var error: {}", e);
            false
        }
    }
}

/// Hierarchical PNG validation: pngcheck → libpng/image decode → magic bytes.
///
/// Returns `false` when magic indicates PNG but authoritative validation rejects
/// the file. Returns `true` only for structurally valid PNG media.
pub fn is_true_png(path: &Path) -> Result<bool> {
    if detect_true_format(path)? != FormatKind::Png {
        return Ok(false);
    }

    match try_pngcheck_validation(path) {
        PngValidationOutcome::Confirmed => Ok(true),
        PngValidationOutcome::Rejected => Ok(false),
        PngValidationOutcome::ToolUnavailable => match png_libpng_decode_probe(path) {
            Ok(true) => Ok(true),
            Ok(false) => Ok(false),
            Err(err) => {
                tracing::warn!(
                    target: "png_validation",
                    path = %path.display(),
                    error = %err,
                    "PNG decode probe failed; falling back to magic-bytes admission"
                );
                Ok(true)
            }
        },
    }
}

/// Validate a magic-identified PNG with the shared PNG audit tool (`pngcheck`).
pub fn validate_png_forensic(path: &Path) -> Result<super::format_detect::ForensicFormatCheck> {
    validate_format_forensic(path, FormatKind::Png)
}

fn try_pngcheck_validation(path: &Path) -> PngValidationOutcome {
    if super::format_detect::forensic_tool_for_format(FormatKind::Png).is_none() {
        return PngValidationOutcome::ToolUnavailable;
    }
    if crate::common_utils::resolve_tool_path(crate::constants::TOOL_PNGCHECK).is_none() {
        return PngValidationOutcome::ToolUnavailable;
    }
    match validate_format_forensic(path, FormatKind::Png) {
        Ok(check) => {
            tracing::debug!(
                target: "png_validation",
                path = %path.display(),
                tool = %check.tool,
                "PNG confirmed via authoritative validator"
            );
            PngValidationOutcome::Confirmed
        }
        Err(err) => {
            let message = err.to_string();
            if message.contains("requires '") && message.contains("' on PATH") {
                PngValidationOutcome::ToolUnavailable
            } else {
                tracing::debug!(
                    target: "png_validation",
                    path = %path.display(),
                    error = %message,
                    "PNG rejected by authoritative validator"
                );
                PngValidationOutcome::Rejected
            }
        }
    }
}

fn png_libpng_decode_probe(path: &Path) -> Result<bool> {
    image::open(path).map_err(|err| {
        ImgQualityError::AnalysisError(format!(
            "PNG decode probe failed for {}: {err}",
            path.display()
        ))
    })?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use std::io::Write;
    use tempfile::NamedTempFile;

    const ONE_BY_ONE_RGBA_PNG: &[u8] = &[
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F,
        0x15, 0xC4, 0x89, 0x00, 0x00, 0x00, 0x0A, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0x00,
        0x01, 0x00, 0x00, 0x05, 0x00, 0x01, 0x0D, 0x0A, 0x2D, 0xB4, 0x00, 0x00, 0x00, 0x00, 0x49,
        0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
    ];

    #[test]
    fn apng_parser_uses_validated_chunks_and_sequence_numbers() {
        fn chunk(chunk_type: &[u8; 4], payload: &[u8]) -> Vec<u8> {
            let mut bytes = u32::try_from(payload.len())
                .expect("test payload fits u32")
                .to_be_bytes()
                .to_vec();
            bytes.extend_from_slice(chunk_type);
            bytes.extend_from_slice(payload);
            let mut hasher = crc32fast::Hasher::new();
            hasher.update(chunk_type);
            hasher.update(payload);
            bytes.extend_from_slice(&hasher.finalize().to_be_bytes());
            bytes
        }

        let valid = crate::image_detection::synthetic_two_frame_apng_for_test();
        let info = parse_apng_animation(&valid)
            .expect("valid APNG structure")
            .expect("animation info");
        assert_eq!(info.frame_count, 2);
        assert!((info.duration_secs - 0.03).abs() < f64::EPSILON);

        let mut marker_in_payload = PNG_SIGNATURE.to_vec();
        marker_in_payload.extend(chunk(
            b"IHDR",
            &[0, 0, 0, 1, 0, 0, 0, 1, 8, 6, 0, 0, 0],
        ));
        marker_in_payload.extend(chunk(b"IDAT", b"acTLfcTL"));
        marker_in_payload.extend(chunk(b"IEND", &[]));
        assert_eq!(parse_apng_animation(&marker_in_payload).unwrap(), None);

        let mut sequence_gap = valid;
        let second_fctl = sequence_gap
            .windows(4)
            .enumerate()
            .filter(|(_, bytes)| *bytes == b"fcTL")
            .nth(1)
            .map(|(pos, _)| pos)
            .expect("second fcTL");
        let payload_start = second_fctl + 4;
        sequence_gap[payload_start..payload_start + 4].copy_from_slice(&3u32.to_be_bytes());
        let mut hasher = crc32fast::Hasher::new();
        hasher.update(b"fcTL");
        hasher.update(&sequence_gap[payload_start..payload_start + 26]);
        sequence_gap[payload_start + 26..payload_start + 30]
            .copy_from_slice(&hasher.finalize().to_be_bytes());
        assert!(parse_apng_animation(&sequence_gap).is_err());
    }

    #[test]
    #[serial]
    fn png_heuristic_disabled_by_default() {
        unsafe {
            std::env::remove_var(ENV_ENABLE_PNG_HEURISTIC);
        }
        assert!(!png_heuristic_enabled());
    }

    #[test]
    #[serial]
    fn png_heuristic_enabled_when_env_set() {
        unsafe {
            std::env::set_var(ENV_ENABLE_PNG_HEURISTIC, "1");
        }
        assert!(png_heuristic_enabled());
        unsafe {
            std::env::remove_var(ENV_ENABLE_PNG_HEURISTIC);
        }
    }

    #[test]
    #[serial]
    fn test_png_heuristic_enabled_all_values() {
        for (val, expected) in [
            ("1", true),
            ("true", true),
            ("TRUE", true),
            ("yes", true),
            ("YES", true),
            ("on", true),
            ("ON", true),
            ("0", false),
            ("false", false),
            ("no", false),
            ("off", false),
            ("", false),
        ] {
            unsafe {
                std::env::set_var(ENV_ENABLE_PNG_HEURISTIC, val);
            }
            assert_eq!(png_heuristic_enabled(), expected, "val={val}");
        }
        unsafe {
            std::env::remove_var(ENV_ENABLE_PNG_HEURISTIC);
        }
        assert!(!png_heuristic_enabled());
    }

    #[test]
    fn true_png_accepts_structurally_valid_bytes() -> Result<()> {
        let mut file = NamedTempFile::new().expect("temp png");
        file.write_all(ONE_BY_ONE_RGBA_PNG).expect("write png");
        assert!(is_true_png(file.path())?);
        Ok(())
    }

    #[test]
    fn true_png_rejects_non_png_magic() -> Result<()> {
        let mut file = NamedTempFile::new().expect("temp fake png");
        file.write_all(b"not a png file").expect("write junk");
        assert!(!is_true_png(file.path())?);
        Ok(())
    }
}
