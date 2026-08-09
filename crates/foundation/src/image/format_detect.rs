//! GAP-1: §`DetectionUniversal` — content-based format detection via magic
//! bytes.
//!
//! ⛔ No existing detection code touched (D7 preserved).
//! Reads at most 32 bytes for fixed signatures. Ambiguous `mif1`/`msf1`
//! ISOBMFF files scan only the `ftyp` compatible-brands box.

use crate::unified_error::{ImgQualityError, Result};
use std::path::Path;

/// Format identified from magic bytes only — never from file extension.
// @ANCHOR:format-magic-only — detect_true_format reads magic bytes only; file extension never used
// as format signal
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FormatKind {
    Jpeg,
    Png,
    Heic,
    Heif,
    Avif,
    WebP,
    Gif,
    Bmp,
    Jxl,
    Tiff,
    Qoi,
    Jp2,
    Ico,
    Exr,
    Flif,
    Psd,
    Pnm,
    Dds,
    Mp4,
    Mov,
    Mkv,
    Webm,
    Unknown,
}

impl FormatKind {
    #[must_use]
    pub const fn canonical_extension(self) -> Option<&'static str> {
        match self {
            Self::Jpeg => Some("jpg"),
            Self::Png => Some("png"),
            Self::Heic => Some("heic"),
            Self::Heif => Some("heif"),
            Self::Avif => Some("avif"),
            Self::WebP => Some("webp"),
            Self::Gif => Some("gif"),
            Self::Bmp => Some("bmp"),
            Self::Jxl => Some("jxl"),
            Self::Tiff => Some("tiff"),
            Self::Qoi => Some("qoi"),
            Self::Jp2 => Some("jp2"),
            Self::Ico => Some("ico"),
            Self::Exr => Some("exr"),
            Self::Flif => Some("flif"),
            Self::Psd => Some("psd"),
            Self::Pnm => Some("pnm"),
            Self::Dds => Some("dds"),
            Self::Mp4 => Some("mp4"),
            Self::Mov => Some("mov"),
            Self::Mkv => Some("mkv"),
            Self::Webm => Some("webm"),
            Self::Unknown => None,
        }
    }

    #[must_use]
    pub const fn valid_extensions(self) -> &'static [&'static str] {
        match self {
            Self::Jpeg => &["jpg", "jpeg", "jpe", "jfif"],
            Self::Png => &["png"],
            Self::Heic => &["heic", "hif"],
            Self::Heif => &["heif", "hif"],
            Self::Avif => &["avif"],
            Self::WebP => &["webp"],
            Self::Gif => &["gif"],
            Self::Bmp => &["bmp"],
            Self::Jxl => &["jxl"],
            Self::Tiff => &["tif", "tiff"],
            Self::Qoi => &["qoi"],
            Self::Jp2 => &["jp2", "j2k", "jpf", "jpx", "jpm", "mj2"],
            Self::Ico => &["ico", "cur"],
            Self::Exr => &["exr"],
            Self::Flif => &["flif"],
            Self::Psd => &["psd"],
            Self::Pnm => &["pnm", "pbm", "pgm", "ppm"],
            Self::Dds => &["dds"],
            Self::Mp4 => &["mp4", "m4v", "m4a", "m4b", "m4p", "m4r", "3gp", "3g2"],
            Self::Mov => &["mov", "qt"],
            Self::Mkv => &["mkv"],
            Self::Webm => &["webm"],
            Self::Unknown => &[],
        }
    }

    #[must_use]
    pub fn extension_matches_path(self, path: &Path) -> bool {
        let Some(ext) = path.extension().and_then(|value| value.to_str()) else {
            return false;
        };
        self.valid_extensions()
            .iter()
            .any(|expected| ext.eq_ignore_ascii_case(expected))
    }

    #[must_use]
    pub const fn is_known_media(self) -> bool {
        !matches!(self, Self::Unknown)
    }
}

/// Detect the true format of `path` from its magic bytes.
///
/// Returns `FormatKind::Unknown` for truncated, empty, or unrecognised files —
/// never an error for those cases.
///
/// # Errors
/// Returns an error only on I/O failure (cannot open/read the file).
pub fn detect_true_format(path: &Path) -> Result<FormatKind> {
    use std::io::Read;

    let mut buf = [0u8; 32];
    let n = {
        let mut f = std::fs::File::open(path)?;
        f.read(&mut buf).map_err(ImgQualityError::IoError)?
    };

    let b = &buf[..n];

    // JXL naked codestream: FF 0A
    if n >= 2 && b[0] == 0xFF && b[1] == 0x0A {
        return Ok(FormatKind::Jxl);
    }

    // BMP: BM
    if n >= 2 && b[0] == 0x42 && b[1] == 0x4D {
        return Ok(FormatKind::Bmp);
    }

    if n < 3 {
        return Ok(FormatKind::Unknown);
    }

    // JPEG: FF D8 FF
    if b[0] == 0xFF && b[1] == 0xD8 && b[2] == 0xFF {
        return Ok(FormatKind::Jpeg);
    }

    // PNG: 89 50 4E 47 0D 0A 1A 0A
    if n >= 8 && b[..8] == [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A] {
        return Ok(FormatKind::Png);
    }

    // WebP: RIFF????WEBP
    if n >= 12 && b[..4] == [0x52, 0x49, 0x46, 0x46] && b[8..12] == [0x57, 0x45, 0x42, 0x50] {
        return Ok(FormatKind::WebP);
    }

    // HEIF/AVIF/video/JP2: validated ftyp box with major brand first.
    if let Some(ftyp_payload) = crate::common_utils::isobmff_ftyp_payload(b) {
        let brand = &ftyp_payload[0..4];
        if is_avif_brand(brand) {
            return Ok(FormatKind::Avif);
        }
        if is_heic_brand(brand) {
            return Ok(FormatKind::Heic);
        }
        if is_heif_brand(brand) {
            return Ok(FormatKind::Heif);
        }
        if brand == b"mif1" || brand == b"msf1" {
            return resolve_mif1_from_compatible_brands(path, brand);
        }
        if is_mp4_brand(brand) {
            return Ok(FormatKind::Mp4);
        }
        if brand == b"qt  " {
            return Ok(FormatKind::Mov);
        }
        if is_jp2_brand(brand) {
            return Ok(FormatKind::Jp2);
        }
        return Ok(FormatKind::Unknown);
    }

    // EBML: Matroska/WebM.
    if b.starts_with(&[0x1A, 0x45, 0xDF, 0xA3]) {
        if b.windows(4).any(|window| window == b"webm") {
            return Ok(FormatKind::Webm);
        }
        return Ok(FormatKind::Mkv);
    }

    // GIF: GIF8
    if n >= 4 && b[..4] == [0x47, 0x49, 0x46, 0x38] {
        return Ok(FormatKind::Gif);
    }

    // JXL ISO box: 00 00 00 0C 4A 58 4C 20 0D 0A 87 0A
    if n >= 12
        && b[..12]
            == [
                0x00, 0x00, 0x00, 0x0C, 0x4A, 0x58, 0x4C, 0x20, 0x0D, 0x0A, 0x87, 0x0A,
            ]
    {
        return Ok(FormatKind::Jxl);
    }

    if b.starts_with(&[0x49, 0x49, 0x2A, 0x00])
        || b.starts_with(&[0x4D, 0x4D, 0x00, 0x2A])
        || b.starts_with(&[0x49, 0x49, 0x2B, 0x00])
        || b.starts_with(&[0x4D, 0x4D, 0x00, 0x2B])
    {
        return Ok(FormatKind::Tiff);
    }

    if b.starts_with(b"qoif") {
        return Ok(FormatKind::Qoi);
    }

    if n >= 12 && b[0..4] == [0x00, 0x00, 0x00, 0x0C] && b.get(4..8) == Some(b"jP  ") {
        return Ok(FormatKind::Jp2);
    }
    if b.starts_with(&[0xFF, 0x4F, 0xFF, 0x51]) {
        return Ok(FormatKind::Jp2);
    }

    if b.starts_with(&[0x00, 0x00, 0x01, 0x00]) || b.starts_with(&[0x00, 0x00, 0x02, 0x00]) {
        return Ok(FormatKind::Ico);
    }

    if b.starts_with(&[0x76, 0x2F, 0x31, 0x01]) {
        return Ok(FormatKind::Exr);
    }

    if b.starts_with(b"FLIF") {
        return Ok(FormatKind::Flif);
    }

    if b.starts_with(b"8BPS") {
        return Ok(FormatKind::Psd);
    }

    if n >= 2
        && b[0] == b'P'
        && (b'1'..=b'6').contains(&b[1])
        && (n < 3 || b[2].is_ascii_whitespace())
    {
        return Ok(FormatKind::Pnm);
    }

    if b.starts_with(b"DDS ") {
        return Ok(FormatKind::Dds);
    }

    Ok(FormatKind::Unknown)
}

/// Dedicated tool policy for deep format validation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ForensicFormatTool {
    pub format: FormatKind,
    pub tool: &'static str,
    pub args_before_path: &'static [&'static str],
    pub purpose: &'static str,
}

/// Result of a successful deep format validation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForensicFormatCheck {
    pub format: FormatKind,
    pub tool: String,
}

/// Return the strict external validator for `format`.
#[must_use]
pub const fn forensic_tool_for_format(format: FormatKind) -> Option<ForensicFormatTool> {
    match format {
        FormatKind::Jpeg => Some(ForensicFormatTool {
            format,
            tool: crate::constants::TOOL_JPEGINFO,
            args_before_path: &["-c"],
            purpose: "JPEG structural check",
        }),
        FormatKind::Png => Some(ForensicFormatTool {
            format,
            tool: crate::constants::TOOL_PNGCHECK,
            args_before_path: &["-q"],
            purpose: "PNG chunk/CRC check",
        }),
        FormatKind::Heic | FormatKind::Heif => Some(ForensicFormatTool {
            format,
            tool: crate::constants::TOOL_HEIF_INFO,
            args_before_path: &[],
            purpose: "HEIF box parser check",
        }),
        FormatKind::Avif => Some(ForensicFormatTool {
            format,
            tool: crate::constants::TOOL_AVIFDEC,
            args_before_path: &["--info"],
            purpose: "AVIF strict decoder info check",
        }),
        FormatKind::WebP => Some(ForensicFormatTool {
            format,
            tool: crate::constants::TOOL_WEBPMUX,
            args_before_path: &["-info"],
            purpose: "WebP RIFF parser check",
        }),
        FormatKind::Jxl => Some(ForensicFormatTool {
            format,
            tool: crate::constants::TOOL_JXLINFO,
            args_before_path: &[],
            purpose: "JPEG XL codestream/container parser check",
        }),
        FormatKind::Mp4 | FormatKind::Mov | FormatKind::Mkv | FormatKind::Webm => {
            Some(ForensicFormatTool {
                format,
                tool: crate::constants::TOOL_FFPROBE,
                args_before_path: &["-v", "error", "-show_format", "-show_streams"],
                purpose: "media container parser check",
            })
        }
        FormatKind::Gif
        | FormatKind::Bmp
        | FormatKind::Tiff
        | FormatKind::Qoi
        | FormatKind::Jp2
        | FormatKind::Ico
        | FormatKind::Exr
        | FormatKind::Flif
        | FormatKind::Psd
        | FormatKind::Pnm
        | FormatKind::Dds => Some(ForensicFormatTool {
            format,
            tool: crate::constants::TOOL_IDENTIFY,
            args_before_path: &["-quiet"],
            purpose: "ImageMagick decoder identification check",
        }),
        FormatKind::Unknown => None,
    }
}

/// Validate `path` with the dedicated tool for its detected format.
///
/// This function is for explicit audit/gate flows. Basic fast-path admission
/// should use [`detect_true_format`] directly and avoid duplicate tool passes.
///
/// # Errors
/// Returns an error if the format is unknown, no policy exists, the policy tool
/// is unavailable, or the tool rejects the file.
pub fn validate_detected_format_forensic(path: &Path) -> Result<ForensicFormatCheck> {
    let format = detect_true_format(path)?;
    if format == FormatKind::Unknown {
        return Err(forensic_validation_error(format!(
            "forensic validation requires known media magic: {}",
            path.display()
        )));
    }
    validate_format_forensic(path, format)
}

/// Validate `path` with the dedicated tool for `expected`.
///
/// # Errors
/// Returns an error if the detected format differs from `expected`, if the
/// required tool is unavailable, or if the tool rejects the file.
pub fn validate_format_forensic(path: &Path, expected: FormatKind) -> Result<ForensicFormatCheck> {
    let actual = detect_true_format(path)?;
    if actual != expected {
        return Err(forensic_validation_error(format!(
            "forensic validation expected {expected:?}, detected {actual:?}: {}",
            path.display()
        )));
    }
    let Some(policy) = forensic_tool_for_format(expected) else {
        return Err(forensic_validation_error(format!(
            "forensic validation has no policy for {expected:?}: {}",
            path.display()
        )));
    };
    let tool = crate::common_utils::resolve_tool_path(policy.tool).ok_or_else(|| {
        forensic_validation_error(format!(
            "forensic validation requires '{}' on PATH/stable tool paths for {expected:?}: {}",
            policy.tool,
            path.display()
        ))
    })?;
    validate_format_with_tool(path, policy, &tool)
}

/// Validate a magic-identified JPEG with the shared JPEG audit tool.
///
/// # Errors
/// Returns an error if the file is not a JPEG by magic, `jpeginfo` is missing,
/// or `jpeginfo -c` rejects the file.
pub fn validate_jpeg_forensic(path: &Path) -> Result<ForensicFormatCheck> {
    validate_format_forensic(path, FormatKind::Jpeg)
}

fn validate_format_with_tool(
    path: &Path,
    policy: ForensicFormatTool,
    tool: &Path,
) -> Result<ForensicFormatCheck> {
    if !tool.is_file() {
        return Err(forensic_validation_error(format!(
            "forensic validation requires existing '{}' binary: {}",
            policy.tool,
            tool.display()
        )));
    }

    let output = std::process::Command::new(tool)
        .args(policy.args_before_path)
        .arg(crate::safe_path_arg(path).as_ref())
        .output()
        .map_err(|err| {
            forensic_validation_error(format!(
                "forensic validation failed to execute {} for {}: {err}",
                tool.display(),
                path.display()
            ))
        })?;

    if output.status.success() {
        // Per-file success is retained in debug logs; printing thousands of lines in
        // FastImg obscures progress and actionable failures.
        tracing::debug!(
            target: "mfb.format_detect",
            format = ?policy.format,
            tool = %tool.display(),
            path = %path.display(),
            purpose = policy.purpose,
            "forensic format validation passed"
        );
        return Ok(ForensicFormatCheck {
            format: policy.format,
            tool: tool.display().to_string(),
        });
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    Err(forensic_validation_error(format!(
        "forensic validation failed for {} using {} ({}, status={}): stdout={} stderr={}",
        path.display(),
        tool.display(),
        policy.purpose,
        output.status,
        stdout.trim(),
        stderr.trim()
    )))
}

fn forensic_validation_error(message: String) -> ImgQualityError {
    tracing::error!(
        target: "mfb.format_detect",
        error = %message,
        "forensic format validation failed"
    );
    ImgQualityError::AnalysisError(message)
}

fn resolve_mif1_from_compatible_brands(path: &Path, major_brand: &[u8]) -> Result<FormatKind> {
    use std::io::Read;

    let scan_bytes =
        crate::numeric_cast::u64_to_usize_strict(crate::constants::BYTES_PER_MB, "isobmff_scan")
            .ok_or_else(|| {
                ImgQualityError::AnalysisError(
                    "ISOBMFF scan byte count does not fit usize".to_string(),
                )
            })?;
    let mut file = std::fs::File::open(path)?;
    let mut data = vec![0u8; scan_bytes];
    let read_len = file.read(&mut data).map_err(ImgQualityError::IoError)?;
    data.truncate(read_len);

    let Some(ftyp_payload) = crate::common_utils::isobmff_ftyp_payload(&data) else {
        return Ok(FormatKind::Unknown);
    };
    if ftyp_payload.get(0..4) != Some(major_brand) {
        return Ok(FormatKind::Unknown);
    }
    let (compatible_brands, remainder) = ftyp_payload[8..].as_chunks::<4>();
    if !remainder.is_empty() {
        return Ok(FormatKind::Unknown);
    }

    let mut has_heic_payload_brand = false;
    let mut has_generic_heif_container = false;
    for brand in compatible_brands {
        if is_avif_brand(brand) {
            return Ok(FormatKind::Avif);
        }
        if is_heic_brand(brand) {
            has_heic_payload_brand = true;
        }
        if is_heif_brand(brand) {
            has_generic_heif_container = true;
        }
        if is_mp4_brand(brand) {
            return Ok(FormatKind::Mp4);
        }
        if brand == b"qt  " {
            return Ok(FormatKind::Mov);
        }
        if is_jp2_brand(brand) {
            return Ok(FormatKind::Jp2);
        }
    }

    if has_heic_payload_brand {
        Ok(FormatKind::Heic)
    } else if has_generic_heif_container {
        Ok(FormatKind::Heif)
    } else {
        tracing::debug!(
            major_brand = %String::from_utf8_lossy(major_brand),
            path = %path.display(),
            "ISOBMFF compatible brands did not identify a supported media format"
        );
        Ok(FormatKind::Unknown)
    }
}

const fn is_avif_brand(brand: &[u8]) -> bool {
    matches!(
        brand,
        b"avif" | b"avis" | b"avio" | b"MA1B" | b"MA1A" | b"av01"
    )
}

const fn is_heic_brand(brand: &[u8]) -> bool {
    matches!(
        brand,
        b"heic"
            | b"heix"
            | b"heim"
            | b"heis"
            | b"hevc"
            | b"hevx"
            | b"hev1"
            | b"hvc1"
            | b"hvc2"
            | b"hvc3"
            | b"hvc4"
            | b"hevm"
            | b"hevs"
            | b"hev2"
    )
}

const fn is_heif_brand(brand: &[u8]) -> bool {
    matches!(
        brand,
        b"heif" | b"miaf" | b"miPr" | b"mif2" | b"hefb" | b"hefc"
    )
}

const fn is_jp2_brand(brand: &[u8]) -> bool {
    matches!(
        brand,
        b"mjp2" | b"mjpb" | b"mjd2" | b"mpx3" | b"mpx4" | b"mpxh"
    )
}

const fn is_mp4_brand(brand: &[u8]) -> bool {
    matches!(
        brand,
        b"mp41"
            | b"mp42"
            | b"isom"
            | b"iso2"
            | b"iso3"
            | b"iso4"
            | b"iso5"
            | b"iso6"
            | b"iso7"
            | b"iso8"
            | b"iso9"
            | b"dash"
            | b"cmfc"
            | b"m4v "
            | b"m4a "
            | b"m4b "
            | b"m4p "
            | b"m4r "
            | b"mp71"
            | b"avc1"
            | b"avc2"
            | b"avc3"
            | b"mp4v"
            | b"3gp4"
            | b"3gp5"
            | b"3gp6"
            | b"3gp1"
            | b"3gp2"
            | b"3gp3"
            | b"3g2a"
            | b"3g2b"
            | b"3g2c"
            | b"M4A "
            | b"M4B "
            | b"M4P "
            | b"M4V "
            | b"M4VH"
            | b"M4VP"
            | b"mmp4"
            | b"dvc "
            | b"dvcp"
            | b"dvpp"
            | b"dv5p"
            | b"dv5n"
            | b"dvh5"
            | b"dvh6"
            | b"dvhp"
            | b"dvhe"
            | b"dvhq"
            | b"dv6n"
            | b"dv6p"
            | b"vvcb"
            | b"vvcg"
            | b"vvcs"
            | b"evc1"
            | b"lvc1"
            | b"avc4"
            | b"avc5"
            | b"avc6"
            | b"avc7"
            | b"avc8"
            | b"hvc5"
            | b"hvc6"
            | b"hvc7"
            | b"hvc8"
            | b"vp08"
            | b"vp09"
            | b"av01"
            | b"av02"
            | b"mi11"
            | b"mi12"
            | b"mi1q"
            | b"mi1r"
            | b"mi21"
            | b"mi31"
            | b"dvh1"
            | b"dvr1"
            | b"simu"
            | b"ccff"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    fn write_tmp(bytes: &[u8]) -> NamedTempFile {
        let mut f = NamedTempFile::new().unwrap();
        f.write_all(bytes).unwrap();
        f
    }

    const ONE_BY_ONE_RGBA_PNG: &[u8] = &[
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F,
        0x15, 0xC4, 0x89, 0x00, 0x00, 0x00, 0x0A, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0x00,
        0x01, 0x00, 0x00, 0x05, 0x00, 0x01, 0x0D, 0x0A, 0x2D, 0xB4, 0x00, 0x00, 0x00, 0x00, 0x49,
        0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
    ];

    #[test]
    fn jpeg_accepted() {
        let f = write_tmp(&[0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10]);
        assert_eq!(detect_true_format(f.path()).unwrap(), FormatKind::Jpeg);
    }

    #[test]
    fn jpeg_magic_with_arbitrary_ext_returns_jpeg() {
        for suffix in [".mp4", ".png", ".heic", ".txt", ""] {
            let mut f = tempfile::Builder::new().suffix(suffix).tempfile().unwrap();
            f.write_all(&[0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10]).unwrap();
            assert_eq!(detect_true_format(f.path()).unwrap(), FormatKind::Jpeg);
        }
    }

    #[test]
    fn forensic_jpeg_validation_requires_authoritative_tool() {
        let f = write_tmp(&[0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10]);
        let policy = forensic_tool_for_format(FormatKind::Jpeg).unwrap();

        let err =
            validate_format_with_tool(f.path(), policy, Path::new("/definitely/missing/jpeginfo"))
                .expect_err("missing jpeginfo must fail closed");

        assert!(
            err.to_string().contains("jpeginfo"),
            "unexpected missing-tool error: {err}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn forensic_jpeg_validation_rejects_tool_failure() {
        let f = write_tmp(&[0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10]);
        let policy = forensic_tool_for_format(FormatKind::Jpeg).unwrap();

        let err = validate_format_with_tool(f.path(), policy, Path::new("/usr/bin/false"))
            .expect_err("validator failure must reject JPEG admission");

        assert!(
            err.to_string().contains("forensic validation failed"),
            "unexpected validator failure: {err}"
        );
    }

    #[test]
    fn forensic_tool_policy_covers_known_formats() {
        for format in [
            FormatKind::Jpeg,
            FormatKind::Png,
            FormatKind::Heic,
            FormatKind::Heif,
            FormatKind::Avif,
            FormatKind::WebP,
            FormatKind::Gif,
            FormatKind::Bmp,
            FormatKind::Jxl,
            FormatKind::Tiff,
            FormatKind::Qoi,
            FormatKind::Jp2,
            FormatKind::Ico,
            FormatKind::Exr,
            FormatKind::Flif,
            FormatKind::Psd,
            FormatKind::Pnm,
            FormatKind::Dds,
            FormatKind::Mp4,
            FormatKind::Mov,
            FormatKind::Mkv,
            FormatKind::Webm,
        ] {
            assert!(
                forensic_tool_for_format(format).is_some(),
                "missing forensic policy for {format:?}"
            );
        }
        assert!(forensic_tool_for_format(FormatKind::Unknown).is_none());
    }

    #[test]
    fn forensic_tool_policy_uses_exact_authoritative_tools() {
        let dedicated: &[(FormatKind, &str, &[&str])] = &[
            (FormatKind::Jpeg, crate::constants::TOOL_JPEGINFO, &["-c"]),
            (FormatKind::Png, crate::constants::TOOL_PNGCHECK, &["-q"]),
            (FormatKind::Heic, crate::constants::TOOL_HEIF_INFO, &[]),
            (FormatKind::Heif, crate::constants::TOOL_HEIF_INFO, &[]),
            (
                FormatKind::Avif,
                crate::constants::TOOL_AVIFDEC,
                &["--info"],
            ),
            (FormatKind::WebP, crate::constants::TOOL_WEBPMUX, &["-info"]),
            (FormatKind::Jxl, crate::constants::TOOL_JXLINFO, &[]),
        ];
        for (format, tool, args) in dedicated {
            let policy = forensic_tool_for_format(*format)
                .unwrap_or_else(|| panic!("missing forensic policy for {format:?}"));
            assert_eq!(policy.tool, *tool, "wrong tool for {format:?}");
            assert_eq!(
                policy.args_before_path, *args,
                "wrong forensic args for {format:?}"
            );
        }

        for format in [
            FormatKind::Mp4,
            FormatKind::Mov,
            FormatKind::Mkv,
            FormatKind::Webm,
        ] {
            let policy = forensic_tool_for_format(format)
                .unwrap_or_else(|| panic!("missing media-container policy for {format:?}"));
            assert_eq!(policy.tool, crate::constants::TOOL_FFPROBE);
            assert_eq!(
                policy.args_before_path,
                ["-v", "error", "-show_format", "-show_streams"],
                "wrong ffprobe args for {format:?}"
            );
        }

        for format in [
            FormatKind::Gif,
            FormatKind::Bmp,
            FormatKind::Tiff,
            FormatKind::Qoi,
            FormatKind::Jp2,
            FormatKind::Ico,
            FormatKind::Exr,
            FormatKind::Flif,
            FormatKind::Psd,
            FormatKind::Pnm,
            FormatKind::Dds,
        ] {
            let policy = forensic_tool_for_format(format)
                .unwrap_or_else(|| panic!("missing generic decoder policy for {format:?}"));
            assert_eq!(policy.tool, crate::constants::TOOL_IDENTIFY);
            assert_eq!(policy.args_before_path, ["-quiet"]);
        }
    }

    #[test]
    fn forensic_validation_missing_tools_fail_closed_for_all_policies() {
        let f = write_tmp(b"format policy only");
        for format in [
            FormatKind::Jpeg,
            FormatKind::Png,
            FormatKind::Heic,
            FormatKind::Heif,
            FormatKind::Avif,
            FormatKind::WebP,
            FormatKind::Gif,
            FormatKind::Bmp,
            FormatKind::Jxl,
            FormatKind::Tiff,
            FormatKind::Qoi,
            FormatKind::Jp2,
            FormatKind::Ico,
            FormatKind::Exr,
            FormatKind::Flif,
            FormatKind::Psd,
            FormatKind::Pnm,
            FormatKind::Dds,
            FormatKind::Mp4,
            FormatKind::Mov,
            FormatKind::Mkv,
            FormatKind::Webm,
        ] {
            let policy = forensic_tool_for_format(format)
                .unwrap_or_else(|| panic!("missing forensic policy for {format:?}"));
            let missing_tool = Path::new("/definitely/missing").join(policy.tool);
            let err = validate_format_with_tool(f.path(), policy, &missing_tool)
                .expect_err("missing forensic tool must fail closed");
            assert!(
                err.to_string().contains(policy.tool),
                "missing-tool error must identify {} for {format:?}: {err}",
                policy.tool
            );
        }
    }

    #[test]
    fn png_accepted() {
        let f = write_tmp(&[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00]);
        assert_eq!(detect_true_format(f.path()).unwrap(), FormatKind::Png);
    }

    #[test]
    fn forensic_png_validation_invokes_pngcheck_successfully() {
        let f = write_tmp(ONE_BY_ONE_RGBA_PNG);
        let policy = forensic_tool_for_format(FormatKind::Png).unwrap();

        assert_eq!(policy.tool, crate::constants::TOOL_PNGCHECK);

        let check = validate_format_forensic(f.path(), FormatKind::Png)
            .expect("pngcheck must accept a structurally valid PNG");

        assert_eq!(check.format, FormatKind::Png);
        assert!(
            check.tool.ends_with(crate::constants::TOOL_PNGCHECK),
            "unexpected PNG validator path: {}",
            check.tool
        );
    }

    #[test]
    fn webp_accepted() {
        let f = write_tmp(b"RIFF\x00\x00\x00\x00WEBP");
        assert_eq!(detect_true_format(f.path()).unwrap(), FormatKind::WebP);
    }

    #[test]
    fn gif_accepted() {
        let f = write_tmp(b"GIF89a\x01\x00\x01\x00");
        assert_eq!(detect_true_format(f.path()).unwrap(), FormatKind::Gif);
    }

    #[test]
    fn bmp_accepted() {
        let f = write_tmp(&[0x42, 0x4D, 0x00, 0x00]);
        assert_eq!(detect_true_format(f.path()).unwrap(), FormatKind::Bmp);
    }

    #[test]
    fn jxl_naked_accepted() {
        let f = write_tmp(&[0xFF, 0x0A, 0x00]);
        assert_eq!(detect_true_format(f.path()).unwrap(), FormatKind::Jxl);
    }

    #[test]
    fn jxl_iso_box_accepted() {
        let f = write_tmp(&[
            0x00, 0x00, 0x00, 0x0C, 0x4A, 0x58, 0x4C, 0x20, 0x0D, 0x0A, 0x87, 0x0A,
        ]);
        assert_eq!(detect_true_format(f.path()).unwrap(), FormatKind::Jxl);
    }

    #[test]
    fn avif_brand_detected() {
        let mut b = vec![0x00, 0x00, 0x00, 0x10];
        b.extend_from_slice(b"ftyp");
        b.extend_from_slice(b"avif");
        b.extend_from_slice(&[0; 4]);
        let f = write_tmp(&b);
        assert_eq!(detect_true_format(f.path()).unwrap(), FormatKind::Avif);
    }

    #[test]
    fn heif_heic_brand_detected() {
        let mut b = vec![0x00, 0x00, 0x00, 0x10];
        b.extend_from_slice(b"ftyp");
        b.extend_from_slice(b"heic");
        b.extend_from_slice(&[0; 4]);
        let f = write_tmp(&b);
        assert_eq!(detect_true_format(f.path()).unwrap(), FormatKind::Heic);
    }

    #[test]
    fn heif_mif1_compatible_brand_detected() {
        let mut b = vec![0x00, 0x00, 0x00, 0x14];
        b.extend_from_slice(b"ftyp");
        b.extend_from_slice(b"mif1");
        b.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]);
        b.extend_from_slice(b"heif");
        let f = write_tmp(&b);
        assert_eq!(detect_true_format(f.path()).unwrap(), FormatKind::Heif);
    }

    #[test]
    fn bare_mif1_without_compatible_brand_is_unknown() {
        let mut b = vec![0x00, 0x00, 0x00, 0x10];
        b.extend_from_slice(b"ftyp");
        b.extend_from_slice(b"mif1");
        b.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]);
        let f = write_tmp(&b);
        assert_eq!(detect_true_format(f.path()).unwrap(), FormatKind::Unknown);
    }

    #[test]
    fn png_magic_with_jpg_ext_returns_png() {
        // GAP-1 disguise test: content wins over extension
        let mut f = tempfile::Builder::new().suffix(".jpg").tempfile().unwrap();
        f.write_all(&[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00])
            .unwrap();
        assert_eq!(detect_true_format(f.path()).unwrap(), FormatKind::Png);
    }

    #[test]
    fn gif_magic_with_heic_ext_returns_gif() {
        let mut f = tempfile::Builder::new().suffix(".heic").tempfile().unwrap();
        f.write_all(b"GIF89a\x01\x00\x01\x00").unwrap();
        assert_eq!(detect_true_format(f.path()).unwrap(), FormatKind::Gif);
    }

    #[test]
    fn truncated_one_byte_returns_unknown() {
        let f = write_tmp(&[0xFF]);
        assert_eq!(detect_true_format(f.path()).unwrap(), FormatKind::Unknown);
    }

    #[test]
    fn empty_file_returns_unknown() {
        let f = write_tmp(&[]);
        assert_eq!(detect_true_format(f.path()).unwrap(), FormatKind::Unknown);
    }

    #[test]
    fn random_bytes_returns_unknown() {
        let f = write_tmp(&[
            0xAB, 0xCD, 0xEF, 0x01, 0x23, 0x45, 0x67, 0x89, 0xAB, 0xCD, 0xEF, 0x01, 0x23, 0x45,
            0x67, 0x89,
        ]);
        assert_eq!(detect_true_format(f.path()).unwrap(), FormatKind::Unknown);
    }

    #[test]
    fn non_jpeg_media_signatures_are_detected_without_extension_trust() {
        let cases: &[(&str, &[u8], FormatKind)] = &[
            (".jpg", b"II*\x00rest", FormatKind::Tiff),
            (".jpg", b"qoif\x00\x00\x00\x01", FormatKind::Qoi),
            (".jpg", b"\xFF\x4F\xFF\x51\x00\x00", FormatKind::Jp2),
            (".jpg", b"\x00\x00\x01\x00\x01\x00", FormatKind::Ico),
            (".jpg", b"\x76\x2F\x31\x01\x02\x00", FormatKind::Exr),
            (".jpg", b"FLIF\x00\x00", FormatKind::Flif),
            (".jpg", b"8BPS\x00\x01", FormatKind::Psd),
            (".jpg", b"P6\n1 1\n255\n", FormatKind::Pnm),
            (".jpg", b"DDS \x00\x00", FormatKind::Dds),
            (".jpg", b"\x00\x00\x00\x10ftypmp42\x00\x00\x00\x00", FormatKind::Mp4),
            (".jpg", b"\x00\x00\x00\x10ftypqt  \x00\x00\x00\x00", FormatKind::Mov),
            (".jpg", b"\x1A\x45\xDF\xA3webm", FormatKind::Webm),
        ];

        for (suffix, bytes, expected) in cases {
            let mut f = tempfile::Builder::new().suffix(suffix).tempfile().unwrap();
            f.write_all(bytes).unwrap();
            assert_eq!(detect_true_format(f.path()).unwrap(), *expected);
            assert!(
                !expected.extension_matches_path(f.path()),
                "spoofed extension unexpectedly accepted for {expected:?}"
            );
        }
    }

    #[test]
    fn mif1_compatible_brands_disambiguate_avif_and_heif() {
        let mut avif = tempfile::Builder::new().suffix(".heic").tempfile().unwrap();
        avif.write_all(b"\x00\x00\x00\x14ftypmif1\x00\x00\x00\x00avif")
            .unwrap();
        assert_eq!(detect_true_format(avif.path()).unwrap(), FormatKind::Avif);

        let mut heif = tempfile::Builder::new().suffix(".avif").tempfile().unwrap();
        heif.write_all(b"\x00\x00\x00\x14ftypmif1\x00\x00\x00\x00heif")
            .unwrap();
        assert_eq!(detect_true_format(heif.path()).unwrap(), FormatKind::Heif);
    }
}
