//! Final-output embedded metadata audit (preserve vs clear).
//!
//! CONTRACT: every delivered media file must pass a per-file policy check:
//! - [`MetadataOutputPolicy::Preserve`]: portable embedded metadata must match
//!   the paired source (catches wrong-temp / cross-product metadata mixups).
//! - [`MetadataOutputPolicy::Clear`]: removable metadata must be absent, with
//!   an explicit source→output reclaimable-byte delta.

use crate::builder_base::ToolBuilder;
use crate::path_safety::exiftool_path_arg;
use serde_json::Value;
use std::collections::BTreeMap;
use std::io;
use std::path::Path;

/// Delivery policy for embedded (and sidecar-derived) metadata.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MetadataOutputPolicy {
    /// Portable embedded metadata on the output must match the paired source.
    Preserve,
    /// Removable embedded metadata and adjacent XMP must be absent on the output.
    Clear,
}

/// Result of a single-file output metadata audit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputMetadataAudit {
    pub passed: bool,
    pub mismatches: Vec<String>,
    pub source_payload_bytes: u64,
    pub output_payload_bytes: u64,
}

impl OutputMetadataAudit {
    #[must_use]
    pub const fn bytes_cleared(&self) -> u64 {
        self.source_payload_bytes
            .saturating_sub(self.output_payload_bytes)
    }
}

/// Portable metadata copied by the delivery layer and safe to compare across
/// different output containers. ICC and orientation are intentionally excluded:
/// color conversion may normalize ICC, and orientation is baked into pixels.
const PRESERVABLE_TAG_ARGS: &[&str] = &[
    "-EXIF:all",
    "-XMP:all",
    "-IPTC:all",
    "-Photoshop:all",
    "-MakerNotes:all",
    "-Keys:all",
    "-ItemList:all",
    "-UserData:all",
];

/// Groups that must be empty under [`MetadataOutputPolicy::Clear`].
const CLEARABLE_TAG_ARGS: &[&str] = &[
    "-EXIF:all",
    "-XMP:all",
    "-IPTC:all",
    "-Photoshop:all",
    "-MakerNotes:all",
    "-ICC_Profile:all",
    "-Keys:all",
    "-ItemList:all",
    "-UserData:all",
];

/// Verify embedded metadata for one delivered output against its paired source.
///
/// # Errors
/// Returns an error when `exiftool` cannot run, when metadata cannot be read, or
/// when the chosen policy fails closed.
pub fn verify_output_embedded_metadata(
    src: &Path,
    dst: &Path,
    policy: MetadataOutputPolicy,
) -> io::Result<OutputMetadataAudit> {
    if !crate::ExiftoolBuilder::check_available() {
        return Err(io::Error::other(
            "exiftool was not found or failed its runtime health check; output metadata \
             audit cannot proceed",
        ));
    }

    let (source_payload_bytes, output_payload_bytes) = match policy {
        MetadataOutputPolicy::Preserve => (0, 0),
        MetadataOutputPolicy::Clear => {
            (removable_payload_bytes(src)?, removable_payload_bytes(dst)?)
        }
    };

    let mismatches = match policy {
        MetadataOutputPolicy::Preserve => {
            let mut src_tags = preservable_tag_map(src)?;
            merge_source_sidecar_metadata_into(&mut src_tags, src)?;
            let dst_tags = preservable_tag_map(dst)?;
            let mut mismatches = preserve_mismatches(&src_tags, &dst_tags);
            mismatches.extend(output_sidecar_mismatches(src, dst)?);
            mismatches
        }
        MetadataOutputPolicy::Clear => clear_mismatches(dst, output_payload_bytes)?,
    };

    let audit = OutputMetadataAudit {
        passed: mismatches.is_empty(),
        mismatches,
        source_payload_bytes,
        output_payload_bytes,
    };

    if audit.passed {
        let detail = match policy {
            MetadataOutputPolicy::Preserve => format!(
                "Metadata Audit: portable embedded metadata verified {} -> {}",
                src.display(),
                dst.display()
            ),
            MetadataOutputPolicy::Clear => format!(
                "Metadata Audit: cleared embedded payload verified {} -> {} \
                 (source_payload={}B output_payload={}B cleared={}B)",
                src.display(),
                dst.display(),
                audit.source_payload_bytes,
                audit.output_payload_bytes,
                audit.bytes_cleared()
            ),
        };
        tracing::info!(
            target: "mfb.metadata",
            src = %src.display(),
            dst = %dst.display(),
            policy = ?policy,
            "{detail}"
        );
        crate::log_info!(crate::infra::static_logs::messages::LABEL_METADATA, detail);
        Ok(audit)
    } else {
        crate::media_conversion_gate::delivery_metadata_path_audit(
            "delivery_metadata_output_audit",
            dst,
            format!(
                "Metadata Audit: output embedded metadata policy {policy:?} failed {} -> {}: {}",
                src.display(),
                dst.display(),
                audit.mismatches.join("; ")
            ),
        );
        Err(io::Error::other(format!(
            "Output embedded metadata policy {policy:?} failed for {} -> {}: {}",
            src.display(),
            dst.display(),
            audit.mismatches.join("; ")
        )))
    }
}

fn preserve_mismatches(
    src: &BTreeMap<String, String>,
    dst: &BTreeMap<String, String>,
) -> Vec<String> {
    let mut mismatches = Vec::new();
    for (key, expected) in src {
        match dst.get(key) {
            Some(actual) if actual == expected => {}
            Some(actual) => mismatches.push(format!(
                "metadata {key} expected={expected:?} actual={actual:?} (possible wrong-source metadata)"
            )),
            None => mismatches.push(format!(
                "metadata {key} missing from output (expected={expected:?})"
            )),
        }
    }
    for (key, actual) in dst {
        if !src.contains_key(key) {
            mismatches.push(format!(
                "metadata {key} unexpected on output actual={actual:?} (possible cross-product metadata)"
            ));
        }
    }
    mismatches
}

fn clear_mismatches(dst: &Path, output_payload_bytes: u64) -> io::Result<Vec<String>> {
    let remaining = clearable_tag_map(dst)?;
    let mut mismatches = Vec::new();
    if !remaining.is_empty() {
        let preview = remaining
            .iter()
            .take(8)
            .map(|(k, v)| format!("{k}={v:?}"))
            .collect::<Vec<_>>()
            .join(", ");
        mismatches.push(format!(
            "cleared-policy residual removable tags remain on {}: {preview}",
            dst.display()
        ));
    }
    if output_payload_bytes > 0 {
        mismatches.push(format!(
            "cleared-policy residual removable payload {output_payload_bytes}B on {}",
            dst.display()
        ));
    }
    Ok(mismatches)
}

fn preservable_tag_map(path: &Path) -> io::Result<BTreeMap<String, String>> {
    let mut map = metadata_tag_map(path, PRESERVABLE_TAG_ARGS, "preservable")?;
    map.retain(|key, _| !preserve_audit_excludes_tag(key));
    Ok(map)
}

fn clearable_tag_map(path: &Path) -> io::Result<BTreeMap<String, String>> {
    metadata_tag_map(path, CLEARABLE_TAG_ARGS, "clearable")
}

fn metadata_tag_map(
    path: &Path,
    tag_args: &[&str],
    label: &str,
) -> io::Result<BTreeMap<String, String>> {
    let mut builder = crate::ExiftoolBuilder::new();
    builder
        .arg("-n")
        .arg("-j")
        .arg("-G1")
        .arg("-a")
        .arg("-s")
        .arg("-b");
    for arg in tag_args {
        builder.arg(*arg);
    }
    builder.arg(exiftool_path_arg(path).as_ref());
    parse_exiftool_json_object_map(
        &builder.build().output().map_err(|e| {
            io::Error::new(
                e.kind(),
                format!(
                    "failed to run exiftool {label} metadata dump for {}: {e}",
                    path.display()
                ),
            )
        })?,
        path,
        label,
    )
}

fn preserve_audit_excludes_tag(key: &str) -> bool {
    let tag = key.rsplit(':').next().unwrap_or(key);
    tag.eq_ignore_ascii_case("Orientation")
        || tag.eq_ignore_ascii_case("XMPToolkit")
        || [
            "Keys:CompatibleBrands",
            "Keys:MajorBrand",
            "Keys:MinorVersion",
            "UserData:SoftwareVersion",
        ]
        .iter()
        .any(|generated| key.eq_ignore_ascii_case(generated))
}

fn removable_payload_bytes(path: &Path) -> io::Result<u64> {
    let mut total = match reclaimable_embedded_metadata_bytes(path) {
        Ok(bytes) => bytes,
        Err(strip_error) => {
            let logical_bytes =
                clearable_tag_map(path)?
                    .iter()
                    .fold(0u64, |total, (key, value)| {
                        total.saturating_add(
                            u64::try_from(key.len().saturating_add(value.len()))
                                .unwrap_or(u64::MAX),
                        )
                    });
            crate::log_info!(
                crate::infra::static_logs::messages::LABEL_METADATA,
                &format!(
                    "Metadata Audit: physical reclaimable-byte probe unavailable for {}; \
                     using {logical_bytes}B logical metadata fallback: {strip_error}",
                    path.display()
                )
            );
            logical_bytes
        }
    };
    if let Some(sidecar) = super::find_xmp_sidecar(path) {
        total = total.saturating_add(std::fs::metadata(&sidecar)?.len());
    }
    Ok(total)
}

fn reclaimable_embedded_metadata_bytes(path: &Path) -> io::Result<u64> {
    let original_bytes = std::fs::metadata(path)?.len();
    let stripped_bytes = stripped_embedded_metadata_size(path)?;
    Ok(original_bytes.saturating_sub(stripped_bytes))
}

pub(super) fn stripped_embedded_metadata_size(path: &Path) -> io::Result<u64> {
    let original_bytes = std::fs::metadata(path)?.len();
    let output = crate::ExiftoolBuilder::new()
        .strip_all()
        .arg("-o")
        .arg("-")
        .arg(exiftool_path_arg(path).as_ref())
        .build()
        .output()
        .map_err(|e| {
            io::Error::new(
                e.kind(),
                format!(
                    "failed to run exiftool metadata size probe for {}: {e}",
                    path.display()
                ),
            )
        })?;
    if !output.status.success() {
        return Err(io::Error::other(format!(
            "exiftool metadata size probe failed for {}: {}",
            path.display(),
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    if original_bytes > 0 && output.stdout.is_empty() {
        return Err(io::Error::other(format!(
            "exiftool metadata size probe returned an empty stripped file for {}",
            path.display()
        )));
    }
    let stripped_bytes = u64::try_from(output.stdout.len()).unwrap_or(u64::MAX);
    Ok(stripped_bytes)
}

fn merge_source_sidecar_metadata_into(
    tags: &mut BTreeMap<String, String>,
    media: &Path,
) -> io::Result<()> {
    let Some(sidecar) = super::find_xmp_sidecar(media) else {
        return Ok(());
    };
    let sidecar_tags = preservable_tag_map(&sidecar)?;
    for (key, value) in sidecar_tags {
        // Delivery merges the sidecar after the embedded source metadata, so
        // sidecar values are the final expected values for duplicate tags.
        tags.insert(key, value);
    }
    Ok(())
}

fn output_sidecar_mismatches(src: &Path, dst: &Path) -> io::Result<Vec<String>> {
    let Some(dst_sidecar) = super::find_xmp_sidecar(dst) else {
        return Ok(Vec::new());
    };
    let src_tags = match super::find_xmp_sidecar(src) {
        Some(src_sidecar) => preservable_tag_map(&src_sidecar)?,
        None => BTreeMap::new(),
    };
    let dst_tags = preservable_tag_map(&dst_sidecar)?;
    Ok(preserve_mismatches(&src_tags, &dst_tags)
        .into_iter()
        .map(|mismatch| format!("output sidecar {mismatch}"))
        .collect())
}

fn parse_exiftool_json_object_map(
    output: &std::process::Output,
    path: &Path,
    label: &str,
) -> io::Result<BTreeMap<String, String>> {
    if !output.status.success() {
        return Err(io::Error::other(format!(
            "exiftool {label} dump failed for {}: {}",
            path.display(),
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    let json_str = String::from_utf8_lossy(&output.stdout);
    if json_str.trim().is_empty() {
        return Ok(BTreeMap::new());
    }
    let parsed: Value = serde_json::from_str(&json_str).map_err(|e| {
        io::Error::other(format!(
            "failed to parse exiftool {label} JSON for {}: {e}",
            path.display()
        ))
    })?;
    let obj = parsed
        .as_array()
        .and_then(|arr| arr.first())
        .and_then(Value::as_object)
        .ok_or_else(|| {
            io::Error::other(format!(
                "invalid exiftool {label} JSON structure for {}",
                path.display()
            ))
        })?;
    let mut map = BTreeMap::new();
    for (key, value) in obj {
        if key.eq_ignore_ascii_case("SourceFile")
            || key.eq_ignore_ascii_case("ExifToolVersion")
            || key.starts_with("ExifTool:")
        {
            continue;
        }
        let rendered = match value {
            Value::Null => continue,
            Value::String(s) if s.trim().is_empty() => continue,
            Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        map.insert(key.clone(), rendered);
    }
    Ok(map)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn write_minimal_jpeg(path: &Path) {
        image::RgbImage::from_pixel(1, 1, image::Rgb([0, 0, 0]))
            .save(path)
            .expect("write jpeg");
    }

    fn write_metadata_tag(path: &Path, assignment: &str) {
        let output = crate::ExiftoolBuilder::new()
            .arg(assignment)
            .arg(exiftool_path_arg(path).as_ref())
            .build()
            .output()
            .expect("run exiftool");
        assert!(
            output.status.success(),
            "write metadata tag failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn preserve_mismatches_detects_wrong_source_metadata() {
        let mut src = BTreeMap::new();
        src.insert("EXIF:UserComment".into(), "source comment".into());
        src.insert(
            "XMP-photoshop:DateCreated".into(),
            "2024-11-12T17:49:25+08:00".into(),
        );
        let mut dst = BTreeMap::new();
        dst.insert("EXIF:UserComment".into(), "other product".into());
        dst.insert(
            "XMP-photoshop:DateCreated".into(),
            "2024-11-12T17:49:25+08:00".into(),
        );
        let mismatches = preserve_mismatches(&src, &dst);
        assert!(
            mismatches.iter().any(|m| m.contains("EXIF:UserComment")),
            "wrong non-identity metadata must fail preserve audit: {mismatches:?}"
        );
    }

    #[test]
    fn preserve_mismatches_detects_unexpected_cross_product_metadata() {
        let src = BTreeMap::new();
        let mut dst = BTreeMap::new();
        dst.insert("XMP-dc:Description".into(), "from-other-temp".into());
        let mismatches = preserve_mismatches(&src, &dst);
        assert!(
            mismatches
                .iter()
                .any(|m| m.contains("unexpected") && m.contains("Description")),
            "cross-product metadata must fail: {mismatches:?}"
        );
    }

    #[test]
    fn preserve_audit_excludes_only_container_generated_video_tags() {
        for generated in [
            "Keys:CompatibleBrands",
            "Keys:MajorBrand",
            "Keys:MinorVersion",
            "UserData:SoftwareVersion",
        ] {
            assert!(preserve_audit_excludes_tag(generated));
        }
        for creative in [
            "Keys:Title",
            "UserData:Description",
            "XMP-photoshop:DateCreated",
            "XMP-xmp:CreateDate",
        ] {
            assert!(
                !preserve_audit_excludes_tag(creative),
                "creative metadata must remain custody-checked: {creative}"
            );
        }
    }

    #[test]
    fn preserve_policy_rejects_wrong_source_non_identity_metadata() {
        let temp = TempDir::new().expect("tempdir");
        let src = temp.path().join("src.jpg");
        let dst = temp.path().join("dst.jpg");
        write_minimal_jpeg(&src);
        write_minimal_jpeg(&dst);
        write_metadata_tag(&src, "-XMP-dc:Description=source product");
        write_metadata_tag(&dst, "-XMP-dc:Description=other product");

        let error = verify_output_embedded_metadata(&src, &dst, MetadataOutputPolicy::Preserve)
            .expect_err("wrong-source non-identity metadata must fail");
        assert!(
            error.to_string().contains("Description"),
            "mismatch must identify the non-identity tag: {error}"
        );
    }

    #[test]
    fn preserve_policy_accepts_delivery_metadata_and_source_sidecar() {
        let temp = TempDir::new().expect("tempdir");
        let src = temp.path().join("src.jpg");
        let dst = temp.path().join("dst.jpg");
        write_minimal_jpeg(&src);
        write_minimal_jpeg(&dst);
        write_metadata_tag(&src, "-EXIF:UserComment=embedded source");
        std::fs::write(
            temp.path().join("src.xmp"),
            br#"<x:xmpmeta xmlns:x="adobe:ns:meta/">
<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
<rdf:Description rdf:about="" xmlns:dc="http://purl.org/dc/elements/1.1/">
<dc:description><rdf:Alt><rdf:li xml:lang="x-default">sidecar source</rdf:li></rdf:Alt></dc:description>
</rdf:Description></rdf:RDF></x:xmpmeta>"#,
        )
        .expect("write source XMP");

        crate::metadata::preserve_for_delivery(&src, &dst).expect("preserve metadata");
        crate::metadata::merge_xmp_sidecar_into_dest(&src, &dst).expect("merge sidecar");
        std::fs::copy(temp.path().join("src.xmp"), temp.path().join("dst.xmp"))
            .expect("copy matching output sidecar");

        let audit = verify_output_embedded_metadata(&src, &dst, MetadataOutputPolicy::Preserve)
            .expect("correct source metadata must pass");
        assert!(audit.passed);
    }

    #[test]
    fn preserve_policy_rejects_foreign_output_sidecar() {
        let temp = TempDir::new().expect("tempdir");
        let src = temp.path().join("src.jpg");
        let dst = temp.path().join("dst.jpg");
        write_minimal_jpeg(&src);
        write_minimal_jpeg(&dst);
        std::fs::write(
            temp.path().join("dst.xmp"),
            br#"<x:xmpmeta xmlns:x="adobe:ns:meta/">
<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
<rdf:Description rdf:about="" xmlns:dc="http://purl.org/dc/elements/1.1/">
<dc:description><rdf:Alt><rdf:li xml:lang="x-default">foreign product</rdf:li></rdf:Alt></dc:description>
</rdf:Description></rdf:RDF></x:xmpmeta>"#,
        )
        .expect("write foreign output XMP");

        let error = verify_output_embedded_metadata(&src, &dst, MetadataOutputPolicy::Preserve)
            .expect_err("foreign output sidecar must fail");
        assert!(
            error.to_string().contains("output sidecar")
                && error.to_string().contains("Description"),
            "sidecar mismatch must be explicit: {error}"
        );
    }

    #[test]
    fn metadata_group_coverage_is_locked() {
        for group in [
            "-EXIF:all",
            "-XMP:all",
            "-IPTC:all",
            "-Photoshop:all",
            "-MakerNotes:all",
            "-Keys:all",
            "-ItemList:all",
            "-UserData:all",
        ] {
            assert!(PRESERVABLE_TAG_ARGS.contains(&group));
            assert!(CLEARABLE_TAG_ARGS.contains(&group));
        }
        assert!(CLEARABLE_TAG_ARGS.contains(&"-ICC_Profile:all"));
        assert!(!PRESERVABLE_TAG_ARGS.contains(&"-ICC_Profile:all"));
    }

    #[test]
    fn clear_policy_rejects_residual_payload_bytes() {
        let temp = TempDir::new().expect("tempdir");
        let dst = temp.path().join("out.avif");
        write_minimal_jpeg(&dst);
        let mismatches = clear_mismatches(&dst, 128).expect("clear check");
        assert!(
            mismatches
                .iter()
                .any(|m| m.contains("residual removable payload")),
            "non-zero payload must fail clear policy: {mismatches:?}"
        );
    }

    #[test]
    fn clear_policy_reports_source_sidecar_bytes_cleared() {
        let temp = TempDir::new().expect("tempdir");
        let src = temp.path().join("source.png");
        let sidecar = temp.path().join("source.xmp");
        let dst = temp.path().join("out.avif");
        image::RgbImage::from_pixel(1, 1, image::Rgb([1, 2, 3]))
            .save(&src)
            .expect("write source png");
        std::fs::write(&sidecar, b"<x:xmpmeta/>").expect("write source sidecar");
        std::fs::copy(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/metadata_clear_baseline.avif.fixture"),
            &dst,
        )
        .expect("copy cleared AVIF fixture");

        let audit = verify_output_embedded_metadata(&src, &dst, MetadataOutputPolicy::Clear)
            .expect("cleared output must pass");

        assert_eq!(audit.source_payload_bytes, 12);
        assert_eq!(audit.output_payload_bytes, 0);
        assert_eq!(audit.bytes_cleared(), 12);
    }

    #[test]
    fn metadata_clear_fixture_contains_only_uniform_synthetic_pixels() {
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/metadata_clear_baseline.avif.fixture");
        let temp = TempDir::new().expect("tempdir");
        let decoded = temp.path().join("fixture.png");
        let avifdec =
            crate::common_utils::resolve_tool_path("avifdec").expect("avifdec must be available");
        let output = std::process::Command::new(avifdec)
            .arg(&fixture)
            .arg(&decoded)
            .output()
            .expect("decode privacy-safe AVIF fixture");
        assert!(
            output.status.success(),
            "avifdec failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );

        let pixels = image::open(&decoded)
            .expect("open decoded AVIF fixture")
            .to_rgb8();
        assert_eq!(pixels.dimensions(), (109, 106));
        let first = pixels.get_pixel(0, 0).0;
        assert!(
            pixels.pixels().iter().all(|pixel| pixel.0 == first),
            "metadata-clear AVIF fixture must remain a single-color synthetic image"
        );
        assert!(
            std::fs::metadata(&fixture).expect("fixture metadata").len() <= 1024,
            "metadata-clear AVIF fixture unexpectedly contains excess payload"
        );
    }

    #[test]
    fn clear_policy_rejects_foreign_metadata_in_real_avif_fixture() {
        let temp = TempDir::new().expect("tempdir");
        let src = temp.path().join("source.png");
        let dst = temp.path().join("out.avif");
        image::RgbImage::from_pixel(1, 1, image::Rgb([1, 2, 3]))
            .save(&src)
            .expect("write source png");
        std::fs::copy(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/metadata_clear_baseline.avif.fixture"),
            &dst,
        )
        .expect("copy cleared AVIF fixture");
        write_metadata_tag(&dst, "-XMP-dc:Description=foreign product");

        let error = verify_output_embedded_metadata(&src, &dst, MetadataOutputPolicy::Clear)
            .expect_err("foreign metadata in Meme Mode AVIF must fail");
        let message = error.to_string();
        assert!(
            message.contains("Description") && message.contains("residual removable"),
            "foreign AVIF metadata mismatch must be explicit: {message}"
        );
    }

    #[test]
    fn metadata_size_probe_falls_back_for_read_only_mkv() {
        let fixture =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/real_hevc_hdr10plus.mkv");
        removable_payload_bytes(&fixture)
            .expect("read-only MKV size fallback must not block audit");
    }

    #[test]
    fn output_metadata_audit_bytes_cleared_locked() {
        let audit = OutputMetadataAudit {
            passed: true,
            mismatches: Vec::new(),
            source_payload_bytes: 400,
            output_payload_bytes: 40,
        };
        assert_eq!(audit.bytes_cleared(), 360);
    }
}
