//! Encoded static-image payload measurement.
//!
//! Size decisions use only encoded image payload bytes. Container metadata is
//! never used as a substitute when measurement fails.

use super::format_detect::{FormatKind, detect_true_format};
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

const JPEG_SOI: [u8; 2] = [0xFF, 0xD8];
const PNG_SIGNATURE: [u8; 8] = [137, 80, 78, 71, 13, 10, 26, 10];
const JXL_CODESTREAM: [u8; 2] = [0xFF, 0x0A];
const JXL_CONTAINER: [u8; 12] = [
    0x00, 0x00, 0x00, 0x0C, b'J', b'X', b'L', b' ', 0x0D, 0x0A, 0x87, 0x0A,
];

/// Measure encoded image bytes while excluding container metadata.
///
/// # Errors
/// Fails closed when the format is not static image media or cannot be
/// measured exactly; callers must preserve the source rather than compare
/// complete file sizes.
pub fn measure(path: &Path) -> anyhow::Result<u64> {
    let format = detect_true_format(path).map_err(|error| anyhow::anyhow!("{error}"))?;
    measure_as(path, format)
}

/// Measure an already detected static format.
///
/// # Errors
/// Returns an error for unsupported/non-static formats and malformed payloads.
pub fn measure_as(path: &Path, format: FormatKind) -> anyhow::Result<u64> {
    match format {
        FormatKind::Jpeg => jpeg(path),
        FormatKind::Png => png(path),
        FormatKind::Avif | FormatKind::Heic | FormatKind::Heif => isobmff_mdat(path),
        FormatKind::Jxl => jxl(path),
        FormatKind::WebP
        | FormatKind::Gif
        | FormatKind::Bmp
        | FormatKind::Tiff
        | FormatKind::Qoi
        | FormatKind::Jp2
        | FormatKind::Ico
        | FormatKind::Exr
        | FormatKind::Flif
        | FormatKind::Psd
        | FormatKind::Pnm
        | FormatKind::Dds => crate::metadata::stripped_embedded_metadata_size(path)
            .map_err(|error| anyhow::anyhow!("metadata-free image measurement failed: {error}")),
        FormatKind::Mp4
        | FormatKind::Mov
        | FormatKind::Mkv
        | FormatKind::Webm
        | FormatKind::Unknown => {
            anyhow::bail!("pure static-image payload measurement is unsupported for {format:?}")
        }
    }
}

/// JPEG bytes excluding `APPn` and COM metadata segments.
///
/// # Errors
/// Returns an error for malformed or truncated JPEG streams.
pub fn jpeg(path: &Path) -> anyhow::Result<u64> {
    let total_size = std::fs::metadata(path)?.len();
    let mut file = std::fs::File::open(path)?;
    let mut signature = [0_u8; 2];
    file.read_exact(&mut signature)?;
    anyhow::ensure!(signature == JPEG_SOI, "invalid JPEG SOI marker");

    let mut metadata_size = 0_u64;
    let mut in_scan = false;
    loop {
        let mut byte = [0_u8; 1];
        let (marker, marker_size) = if in_scan {
            loop {
                file.read_exact(&mut byte)?;
                if byte[0] != 0xFF {
                    continue;
                }
                let mut marker_size = 1_u64;
                let marker = loop {
                    file.read_exact(&mut byte)?;
                    marker_size += 1;
                    if byte[0] != 0xFF {
                        break byte[0];
                    }
                };
                if marker == 0x00 || (0xD0..=0xD7).contains(&marker) {
                    continue;
                }
                break (marker, marker_size);
            }
        } else {
            file.read_exact(&mut byte)?;
            anyhow::ensure!(byte[0] == 0xFF, "data outside JPEG scan");
            let mut marker_size = 1_u64;
            let marker = loop {
                file.read_exact(&mut byte)?;
                marker_size += 1;
                if byte[0] != 0xFF {
                    break byte[0];
                }
            };
            (marker, marker_size)
        };

        if marker == 0xD9 {
            return file
                .stream_position()?
                .checked_sub(metadata_size)
                .ok_or_else(|| anyhow::anyhow!("JPEG metadata exceeds file size"));
        }
        anyhow::ensure!(marker != 0x00 && marker != 0xD8, "invalid JPEG marker");
        if marker == 0x01 || (0xD0..=0xD7).contains(&marker) {
            continue;
        }

        let mut length_bytes = [0_u8; 2];
        file.read_exact(&mut length_bytes)?;
        let segment_length = u16::from_be_bytes(length_bytes);
        anyhow::ensure!(segment_length >= 2, "invalid JPEG segment length");
        let payload_size = u64::from(segment_length - 2);
        anyhow::ensure!(
            file.stream_position()?
                .checked_add(payload_size)
                .is_some_and(|end| end <= total_size),
            "truncated JPEG segment"
        );
        if (0xE0..=0xEF).contains(&marker) || marker == 0xFE {
            metadata_size = metadata_size
                .checked_add(marker_size + u64::from(segment_length))
                .ok_or_else(|| anyhow::anyhow!("JPEG metadata size overflow"))?;
        }
        file.seek(SeekFrom::Current(i64::from(segment_length) - 2))?;
        in_scan = marker == 0xDA;
    }
}

/// PNG structural and encoded pixel payload bytes.
///
/// # Errors
/// Returns an error for malformed or incomplete PNG streams.
pub fn png(path: &Path) -> anyhow::Result<u64> {
    let total_size = std::fs::metadata(path)?.len();
    let mut file = std::fs::File::open(path)?;
    let mut signature = [0_u8; 8];
    file.read_exact(&mut signature)?;
    anyhow::ensure!(signature == PNG_SIGNATURE, "invalid PNG signature");

    let mut pure_size = 0_u64;
    let mut saw_ihdr = false;
    let mut saw_idat = false;
    let mut saw_iend = false;
    while file.stream_position()? < total_size {
        let mut header = [0_u8; 8];
        file.read_exact(&mut header)?;
        let payload_size = u64::from(u32::from_be_bytes([
            header[0], header[1], header[2], header[3],
        ]));
        let chunk_type = &header[4..8];
        anyhow::ensure!(
            file.stream_position()?
                .checked_add(payload_size)
                .and_then(|end| end.checked_add(4))
                .is_some_and(|end| end <= total_size),
            "truncated PNG chunk"
        );
        if matches!(chunk_type, b"IHDR" | b"PLTE" | b"tRNS" | b"IDAT") {
            pure_size = pure_size
                .checked_add(payload_size)
                .ok_or_else(|| anyhow::anyhow!("PNG payload size overflow"))?;
        }
        saw_ihdr |= chunk_type == b"IHDR";
        saw_idat |= chunk_type == b"IDAT";
        saw_iend |= chunk_type == b"IEND";
        file.seek(SeekFrom::Current(i64::try_from(payload_size + 4)?))?;
        if saw_iend {
            break;
        }
    }
    anyhow::ensure!(saw_ihdr && saw_idat && saw_iend, "incomplete PNG image");
    Ok(pure_size)
}

/// Sum `mdat` payload bytes in an AVIF/HEIF container.
///
/// # Errors
/// Returns an error for malformed boxes or a missing media payload.
pub fn isobmff_mdat(path: &Path) -> anyhow::Result<u64> {
    let total_size = std::fs::metadata(path)?.len();
    let mut file = std::fs::File::open(path)?;
    let mut offset = 0_u64;
    let mut pure_size = 0_u64;
    let mut box_count = 0_u32;

    while offset < total_size {
        box_count = box_count.saturating_add(1);
        anyhow::ensure!(
            box_count <= crate::infra::constants::MAX_AVIF_BOXES,
            "ISOBMFF payload probe exceeded box count limit"
        );
        anyhow::ensure!(total_size - offset >= 8, "truncated ISOBMFF box header");
        let mut header = [0_u8; 8];
        file.read_exact(&mut header)?;
        let size32 = u32::from_be_bytes([header[0], header[1], header[2], header[3]]);
        let mut header_size = 8_u64;
        let box_size = match size32 {
            0 => total_size - offset,
            1 => {
                let mut extended = [0_u8; 8];
                file.read_exact(&mut extended)?;
                header_size = 16;
                u64::from_be_bytes(extended)
            }
            size => u64::from(size),
        };
        anyhow::ensure!(
            box_size >= header_size && box_size <= total_size - offset,
            "invalid ISOBMFF box size"
        );
        if &header[4..8] == b"mdat" {
            pure_size = pure_size
                .checked_add(box_size - header_size)
                .ok_or_else(|| anyhow::anyhow!("ISOBMFF payload size overflow"))?;
        }
        offset += box_size;
        file.seek(SeekFrom::Start(offset))?;
    }
    anyhow::ensure!(pure_size > 0, "ISOBMFF container has no mdat payload");
    Ok(pure_size)
}

/// JXL codestream bytes excluding all container metadata boxes.
///
/// # Errors
/// Returns an error for malformed/truncated JXL streams.
pub fn jxl(path: &Path) -> anyhow::Result<u64> {
    let total_size = std::fs::metadata(path)?.len();
    let mut file = std::fs::File::open(path)?;
    let mut magic = [0_u8; 2];
    file.read_exact(&mut magic)?;
    if magic == JXL_CODESTREAM {
        return Ok(total_size);
    }

    file.seek(SeekFrom::Start(0))?;
    let mut container_magic = [0_u8; 12];
    file.read_exact(&mut container_magic)?;
    anyhow::ensure!(
        container_magic == JXL_CONTAINER,
        "unrecognized JXL container"
    );

    file.seek(SeekFrom::Start(0))?;
    let mut offset = 0_u64;
    let mut pure_size = 0_u64;
    while offset < total_size {
        anyhow::ensure!(total_size - offset >= 8, "truncated JXL box header");
        let mut header = [0_u8; 8];
        file.read_exact(&mut header)?;
        let size32 = u32::from_be_bytes([header[0], header[1], header[2], header[3]]);
        let mut header_size = 8_u64;
        let box_size = match size32 {
            0 => total_size - offset,
            1 => {
                let mut extended = [0_u8; 8];
                file.read_exact(&mut extended)?;
                header_size = 16;
                u64::from_be_bytes(extended)
            }
            size => u64::from(size),
        };
        anyhow::ensure!(
            box_size >= header_size && box_size <= total_size - offset,
            "invalid JXL box size"
        );
        let payload = box_size - header_size;
        match &header[4..8] {
            b"jxlp" => {
                anyhow::ensure!(payload >= 4, "short JXL jxlp sequence header");
                pure_size = pure_size
                    .checked_add(payload - 4)
                    .ok_or_else(|| anyhow::anyhow!("JXL payload size overflow"))?;
            }
            b"jxlc" => {
                pure_size = pure_size
                    .checked_add(payload)
                    .ok_or_else(|| anyhow::anyhow!("JXL payload size overflow"))?;
            }
            _ => {}
        }
        offset += box_size;
        file.seek(SeekFrom::Start(offset))?;
    }
    anyhow::ensure!(pure_size > 0, "JXL container has no codestream payload");
    Ok(pure_size)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn metadata_never_counts_as_jpeg_payload() -> anyhow::Result<()> {
        let mut file = tempfile::Builder::new().suffix(".jpg").tempfile()?;
        let bytes = [
            &[0xFF, 0xD8][..],
            &[0xFF, 0xE1, 0x00, 0x06, 1, 2, 3, 4],
            &[0xFF, 0xDA, 0x00, 0x04, 5, 6, 7, 8],
            &[0xFF, 0xD9],
        ]
        .concat();
        file.write_all(&bytes)?;
        assert_eq!(jpeg(file.path())?, u64::try_from(bytes.len() - 8)?);
        Ok(())
    }

    #[test]
    fn malformed_payload_fails_instead_of_using_complete_file_size() -> anyhow::Result<()> {
        let mut file = tempfile::Builder::new().suffix(".png").tempfile()?;
        file.write_all(&PNG_SIGNATURE)?;
        file.write_all(&10_u32.to_be_bytes())?;
        file.write_all(b"IDAT")?;
        file.write_all(&[1, 2])?;
        assert!(png(file.path()).is_err());
        Ok(())
    }
}
