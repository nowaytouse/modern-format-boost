//! Merge metadata into a private GIF without rewriting its frames or timing.
use std::fs::File;
use std::io::{self, Read, Write};
use std::path::Path;

// The GIF application extension embeds the UTF-8 XMP packet, followed by
// Adobe's landing zone for readers that consume the packet as GIF sub-blocks.
fn application_block(path: &Path, expected: Option<&str>) -> io::Result<Vec<u8>> {
    let mut file = crate::io_utils::open_regular_nofollow(path)?;
    let length = super::validate_xmp_preview_document(&mut file, path)?;
    let mut block = b"\x21\xff\x0bXMP DataXMP".to_vec();
    let start = block.len();
    file.take(length).read_to_end(&mut block)?;
    let packet = &block[start..];
    let xml = std::str::from_utf8(packet).map_err(io::Error::other)?;
    if xml.contains('\0')
        || expected.is_some_and(|hash| blake3::hash(packet).to_hex().as_str() != hash)
    {
        return Err(io::Error::other("invalid GIF XMP or changed sidecar"));
    }
    // Some standalone sidecars omit the instruction used by GIF metadata readers
    // to separate the XML from the binary landing zone.
    if !xml
        .trim_end()
        .rsplit_once("<?")
        .is_some_and(|(_, last)| last.starts_with("xpacket end=") && last.ends_with("?>"))
    {
        block.extend_from_slice(b"<?xpacket end=\"w\"?>");
    }
    block.push(1);
    block.extend((0..=255_u8).rev());
    block.push(0);
    Ok(block)
}

impl crate::XmpMerger {
    pub(crate) fn write_gif_delivery(
        source: &Path,
        xmp: &Path,
        staged: &Path,
        source_digest: &str,
        xmp_digest: &str,
    ) -> io::Result<()> {
        let mut media = Vec::new();
        crate::io_utils::open_regular_nofollow(source)?.read_to_end(&mut media)?;
        if blake3::hash(&media).to_hex().as_str() != source_digest
            || !crate::media_meta_utils::verify_gif_reader(media.as_slice())?
        {
            return Err(io::Error::other("GIF changed since admission"));
        }
        let mut spans = Vec::new();
        crate::media_meta_utils::scan_gif_headers_from_bytes(&media, |span| spans.push(span))?;
        let mut block = application_block(xmp, Some(xmp_digest))?;
        let span = match spans.as_slice() {
            [] => {
                let trailer = media
                    .iter()
                    .rposition(|&b| b != 0 && !b.is_ascii_whitespace())
                    .filter(|&i| media[i] == b';')
                    .ok_or_else(|| io::Error::other("verified GIF trailer unavailable"))?;
                trailer..trailer
            }
            [span] => span.clone(),
            _ => {
                return Err(io::Error::other(
                    "multiple embedded GIF XMP packets are ambiguous; originals retained",
                ));
            }
        };
        let changes = media[span.clone()] != block;
        if changes
            && crate::image_formats::gif::has_c2pa_application_extension(&media)
                .map_err(io::Error::other)?
        {
            return Err(io::Error::other(
                "GIF has signed provenance; original and XMP retained",
            ));
        }
        if changes && !span.is_empty() {
            let merged =
                Self::combined_xmp_packet(xmp, source, xmp_digest).map_err(io::Error::other)?;
            block = application_block(&merged, None)?;
        }

        let mut output = File::create_new(staged)?;
        let mut expected = blake3::Hasher::new();
        let version = if changes { b"GIF89a" } else { &media[..6] };
        for part in [version, &media[6..span.start], &block, &media[span.end..]] {
            output.write_all(part)?;
            expected.update(part);
        }
        drop(output);
        let actual = blake3::Hasher::new()
            .update_reader(File::open(staged)?)?
            .finalize();
        if actual != expected.finalize() || !crate::media_meta_utils::is_verified_gif(staged)? {
            return Err(io::Error::other(
                "staged GIF differs from the verified delivery",
            ));
        }
        super::verify_output_embedded_metadata_with_explicit_xmp(source, xmp, staged)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, process::Command};
    const HEADER: &[u8] = b"GIF89a\x01\x00\x01\x00\x80\x00\x00\x00\x00\x00\xff\xff\xff";
    const FRAME: &[u8] = b",\x00\x00\x00\x00\x01\x00\x01\x00\x00\x02\x02\x44\x01\x00";
    const GCE: &[u8] = b"\x21\xf9\x04\x08\x07\x00\x00\x00";

    fn packet(fields: &str) -> String {
        format!(
            r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"><rdf:Description rdf:about="" xmlns:xmp="http://ns.adobe.com/xap/1.0/" {fields}/></rdf:RDF></x:xmpmeta>"#
        )
    }

    #[test]
    fn gif_xmp_merges_fields_and_preserves_both_frames_and_padding() -> anyhow::Result<()> {
        if !crate::ExiftoolBuilder::check_available() {
            return Ok(());
        }
        let temp = tempfile::tempdir()?;
        let base = temp.path();
        let source = base.join("animation.webp");
        let sidecar = base.join("animation.xmp");
        let first = base.join("first.gif");
        let merged = base.join("merged.gif");
        let white = b",\x00\x00\x00\x00\x01\x00\x01\x00\x00\x02\x02\x4c\x01\x00";
        let raw = [HEADER, GCE, FRAME, GCE, white, b";\0\r\n"].concat();
        fs::write(&source, &raw)?;
        let embed = |src: &Path, dst: &Path, xml: &str| -> anyhow::Result<()> {
            fs::write(&sidecar, xml)?;
            crate::XmpMerger::write_gif_delivery(
                src,
                &sidecar,
                dst,
                &blake3::hash(&fs::read(src)?).to_hex(),
                &blake3::hash(xml.as_bytes()).to_hex(),
            )?;
            Ok(())
        };
        embed(
            &source,
            &first,
            &packet(r#"xmp:CreatorTool="synthetic" xmp:Rating="1""#),
        )?;
        let overlay = packet(r#"xmp:Rating="4" xmp:Label="overlay""#);
        embed(&first, &merged, &overlay)?;
        assert!(crate::media_meta_utils::gif_has_displayed_motion(&merged)?);
        let merged_bytes = fs::read(&merged)?;
        let mut spans = Vec::new();
        let scan =
            crate::media_meta_utils::scan_gif_headers_from_bytes(&merged_bytes, |s| spans.push(s))?;
        assert_eq!(scan.frame_count, 2);
        assert_eq!(spans.len(), 1);
        let span = &spans[0];
        assert_eq!(
            [&merged_bytes[..span.start], &merged_bytes[span.end..]].concat(),
            raw
        );
        let readback = Command::new("exiftool")
            .args([
                "-s3",
                "-XMP-xmp:CreatorTool",
                "-XMP-xmp:Rating",
                "-XMP-xmp:Label",
                "-FrameCount",
            ])
            .arg(&merged)
            .output()?;
        assert!(readback.status.success());
        assert_eq!(
            String::from_utf8(readback.stdout)?
                .lines()
                .collect::<Vec<_>>(),
            ["synthetic", "4", "overlay", "2"]
        );
        assert_eq!(fs::read(source)?, raw);
        assert_eq!(fs::read(sidecar)?, overlay.as_bytes());
        Ok(())
    }

    #[test]
    fn gif_xmp_refuses_signed_changes_and_drift_before_writing() -> anyhow::Result<()> {
        let root = tempfile::tempdir()?;
        let source = root.path().join("image");
        let xmp = root.path().join("image.xmp");
        let staged = root.path().join("delivery.gif");
        let signed = [HEADER, b"\x21\xff\x0bC2PA_GIF\x01\x00\x00\x00", FRAME, b";"].concat();
        fs::write(&source, &signed)?;
        let xml = packet(r#"xmp:Label="tag""#);
        fs::write(&xmp, &xml)?;
        let hash = blake3::hash(&signed).to_hex();
        let sidecar = blake3::hash(xml.as_bytes()).to_hex();
        assert!(
            crate::XmpMerger::write_gif_delivery(&source, &xmp, &staged, &hash, &sidecar)
                .unwrap_err()
                .to_string()
                .contains("provenance")
        );
        assert!(
            crate::XmpMerger::write_gif_delivery(&source, &xmp, &staged, &hash, "stale").is_err()
        );
        assert!(!staged.exists());
        Ok(())
    }
}
