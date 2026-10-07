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
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::io;
use std::path::Path;

/// Delivery policy for embedded (and sidecar-derived) metadata.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MetadataOutputPolicy {
    /// Portable embedded metadata on the output must match the paired source.
    Preserve,
    /// Every portable source tag must be preserved, while codec-created output
    /// tags are allowed. This is reserved for cross-container reconstruction.
    PreserveSource,
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

/// Return whether one file already satisfies the removable-metadata policy.
///
/// This preflight does not emit a failed delivery audit. Final outputs must
/// still call [`verify_output_embedded_metadata`].
pub fn embedded_metadata_is_clear(path: &Path) -> io::Result<bool> {
    let payload_bytes = removable_payload_bytes(path)?;
    Ok(clear_mismatches(path, payload_bytes)?.is_empty())
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
    "-Comment",
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
    "-Comment",
];

/// Codec-layout fields describe how the source container stores pixels. They
/// are verified by the pixel/orientation/color gates, not copied as portable
/// descriptive metadata into a different container.
const CROSS_CONTAINER_STRUCTURAL_TAGS: &[&str] = &[
    "BitsPerSample",
    "Compression",
    "ExtraSamples",
    "FillOrder",
    "ImageHeight",
    "ImageLength",
    "ImageWidth",
    "NewSubfileType",
    "PhotometricInterpretation",
    "PlanarConfiguration",
    "Predictor",
    "ReferenceBlackWhite",
    "RowsPerStrip",
    "SampleFormat",
    "SamplesPerPixel",
    "SMaxSampleValue",
    "SMinSampleValue",
    "StripByteCounts",
    "StripOffsets",
    "SubfileType",
    "TileByteCounts",
    "TileLength",
    "TileOffsets",
    "TileWidth",
    "YCbCrPositioning",
    "YCbCrSubSampling",
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
        MetadataOutputPolicy::Preserve | MetadataOutputPolicy::PreserveSource => (0, 0),
        MetadataOutputPolicy::Clear => {
            (removable_payload_bytes(src)?, removable_payload_bytes(dst)?)
        }
    };

    let mismatches = match policy {
        MetadataOutputPolicy::Preserve | MetadataOutputPolicy::PreserveSource => {
            let src_sidecar = super::find_xmp_sidecar(src);
            let dst_sidecar = super::find_xmp_sidecar(dst);
            let jpeg_archive = is_jpeg_archive_pair(src, dst)?;
            let [mut src_tags, mut dst_tags, mut source_xmp, output_xmp] =
                preservable_delivery_tag_maps(
                    [
                        src,
                        dst,
                        src_sidecar.as_deref().unwrap_or(src),
                        dst_sidecar.as_deref().unwrap_or(dst),
                    ],
                    jpeg_archive,
                )?;
            if src_sidecar.is_some() {
                overlay_metadata(&mut src_tags, &source_xmp);
            } else {
                source_xmp.clear();
            }
            if jpeg_archive {
                verify_jpeg_xmp_layers(
                    src,
                    dst,
                    src_sidecar.as_deref(),
                    &mut src_tags,
                    &mut dst_tags,
                )?;
            }
            verify_reconstruction_only_metadata(src, dst, &mut src_tags, &dst_tags)?;
            let mut mismatches = if matches!(policy, MetadataOutputPolicy::Preserve) {
                preserve_mismatches(&src_tags, &dst_tags)
            } else {
                preserve_source_mismatches(&src_tags, &dst_tags)
            };
            if dst_sidecar.is_some() {
                mismatches.extend(
                    preserve_mismatches(&source_xmp, &output_xmp)
                        .into_iter()
                        .map(|mismatch| format!("output sidecar {mismatch}")),
                );
            }
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
            MetadataOutputPolicy::PreserveSource => format!(
                "Metadata Audit: source metadata preserved with codec output additions allowed {} -> {}",
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
            target: "mfb::report",
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

fn is_jpeg_archive_pair(src: &Path, dst: &Path) -> io::Result<bool> {
    use crate::image::format_detect::{FormatKind, detect_true_format};
    Ok(
        detect_true_format(src).map_err(io::Error::other)? == FormatKind::Jpeg
            && detect_true_format(dst).map_err(io::Error::other)? == FormatKind::Jxl,
    )
}

fn verify_jpeg_xmp_layers(
    src: &Path,
    dst: &Path,
    sidecar: Option<&Path>,
    expected: &mut BTreeMap<String, String>,
    actual: &mut BTreeMap<String, String>,
) -> io::Result<()> {
    let (missing, extra) = unmatched_metadata_instances(expected, actual);
    if !missing
        .iter()
        .chain(&extra)
        .any(|(key, _)| key.starts_with("XMP-"))
    {
        return Ok(());
    }
    let [source, output, overlay] = metadata_tag_maps_with_exif_custody(
        [src, dst, sidecar.unwrap_or(src)],
        &["-XMP"],
        "JPEG XMP packet custody",
        "-G1:4",
        Some([src, dst]),
    )?;
    // libjxl exposes the first original packet natively; additional JPEG
    // packets remain reconstruction-owned. An appended sidecar is a separate
    // authoritative layer, not an in-place rewrite of the original packet.
    let primary = source.get("XMP:XMP");
    if primary.is_some() && primary != output.get("XMP:XMP") {
        return Err(io::Error::other(
            "original primary XMP packet is not preserved in native JXL metadata",
        ));
    }
    let mut allowed = source
        .iter()
        .filter(|(key, _)| metadata_comparison_key(key) == "XMP:XMP")
        .map(|(_, value)| value)
        .collect::<Vec<_>>();
    if let Some(sidecar) = sidecar {
        super::verify_jxl_xmp_sidecar_custody(sidecar, dst)?;
        allowed.push(
            overlay
                .get("XMP:XMP")
                .ok_or_else(|| io::Error::other("sidecar XMP packet is unreadable"))?,
        );
    }
    for (_, value) in output
        .iter()
        .filter(|(key, _)| metadata_comparison_key(key) == "XMP:XMP")
    {
        let Some(index) = allowed.iter().position(|candidate| *candidate == value) else {
            return Err(io::Error::other(
                "unexpected or duplicated native JXL XMP packet",
            ));
        };
        allowed.remove(index);
    }
    crate::image::fast_img::verify_jxl_roundtrip_integrity(src, dst).map_err(|error| {
        io::Error::other(format!(
            "layered XMP requires exact JPEG reconstruction: {error}"
        ))
    })?;
    expected.retain(|key, _| !key.starts_with("XMP-"));
    actual.retain(|key, _| !key.starts_with("XMP-"));
    tracing::info!(target: "mfb.metadata", source = %src.display(), output = %dst.display(),
        has_sidecar = sidecar.is_some(), "XMP layers verified against native packets, exact JPEG reconstruction and any current sidecar");
    Ok(())
}

/// Missing tags can use reconstruction custody only when `ExifTool` identifies
/// their actual source carrier as JPEG-only metadata. Native EXIF/XMP/ICC,
/// sidecar overrides and contradictory output values remain directly checked.
fn verify_reconstruction_only_metadata(
    src: &Path,
    dst: &Path,
    src_tags: &mut BTreeMap<String, String>,
    dst_tags: &BTreeMap<String, String>,
) -> io::Result<()> {
    use crate::image::format_detect::{FormatKind, detect_true_format};

    let (missing, unexpected) = unmatched_metadata_instances(src_tags, dst_tags);
    if missing.is_empty() {
        return Ok(());
    }
    let missing_keys = missing.iter().map(|(key, _)| *key).collect::<BTreeSet<_>>();
    let contradictory = unexpected
        .iter()
        .map(|(key, _)| metadata_comparison_key(key))
        .collect::<BTreeSet<_>>();
    if detect_true_format(src).map_err(|e| io::Error::other(e.to_string()))? != FormatKind::Jpeg
        || detect_true_format(dst).map_err(|e| io::Error::other(e.to_string()))? != FormatKind::Jxl
    {
        return Ok(());
    }
    // Request physical provenance only on the exceptional missing-tag path.
    // The leading colon keeps all group families, including the empty primary
    // instance, so no heuristic is needed to recover the field's identity.
    let [located] = metadata_tag_maps_with_groups(
        [src],
        PRESERVABLE_TAG_ARGS,
        "JPEG metadata provenance",
        "-G:0:1:4:5",
    )?;
    let reconstruction_tags = located
        .iter()
        .filter_map(|(location, value)| {
            let key = jpeg_reconstruction_tag_key(location)?;
            (missing_keys.contains(key.as_str())
                && !contradictory.contains(&metadata_comparison_key(&key))
                && src_tags.get(&key) == Some(value))
            .then_some(key)
        })
        .collect::<Vec<_>>();
    if reconstruction_tags.is_empty() {
        return Ok(());
    }
    // Neither a filename nor an advertised JBRD box is proof. Reconstruct the
    // current delivered file and compare it with the actual paired source.
    crate::image::fast_img::verify_jxl_roundtrip_integrity(src, dst).map_err(|error| {
        io::Error::other(format!(
            "JPEG-only metadata requires exact source reconstruction: {error}"
        ))
    })?;
    tracing::info!(
        target: "mfb.metadata",
        source = %src.display(),
        output = %dst.display(),
        tags = ?reconstruction_tags,
        "JPEG-only metadata preserved via verified byte-exact JPEG reconstruction"
    );
    for key in reconstruction_tags {
        src_tags.remove(&key);
    }
    Ok(())
}

fn jpeg_reconstruction_tag_key(location: &str) -> Option<String> {
    let mut groups = location.split(':');
    let (general, specific, instance, path, tag) = (
        groups.next()?,
        groups.next()?,
        groups.next()?,
        groups.next()?,
        groups.next()?,
    );
    if groups.next().is_some()
        || general.is_empty()
        || specific.is_empty()
        || tag.is_empty()
        || matches!(general, "EXIF" | "XMP" | "ICC_Profile")
        || (!instance.is_empty()
            && !instance.strip_prefix("Copy").is_some_and(|number| {
                !number.is_empty() && number.bytes().all(|byte| byte.is_ascii_digit())
            }))
    {
        return None;
    }
    let mut parts = path.split('-');
    if parts.next()? != "JPEG" {
        return None;
    }
    let carrier = parts.next()?;
    let jpeg_only = matches!(
        carrier,
        "Trailer"
            | "COM"
            | "APP0"
            | "APP1"
            | "APP2"
            | "APP3"
            | "APP4"
            | "APP5"
            | "APP6"
            | "APP7"
            | "APP8"
            | "APP9"
            | "APP10"
            | "APP11"
            | "APP12"
            | "APP13"
            | "APP14"
            | "APP15"
    );
    // MakerNotes nested inside native EXIF are not a JPEG-only carrier.
    if !jpeg_only
        || parts.any(|part| {
            part.starts_with("IFD") || part.starts_with("SubIFD") || part.ends_with("IFD")
        })
    {
        return None;
    }
    Some(if instance.is_empty() {
        format!("{specific}:{tag}")
    } else {
        format!("{specific}:{instance}:{tag}")
    })
}

fn preserve_mismatches(
    src: &BTreeMap<String, String>,
    dst: &BTreeMap<String, String>,
) -> Vec<String> {
    let (missing, unexpected) = unmatched_metadata_instances(src, dst);
    let mut mismatches = source_instance_mismatches(&missing, &unexpected);
    for (key, actual) in unexpected {
        mismatches.push(format!(
            "metadata {key} unexpected on output actual={actual:?} (possible cross-product metadata)"
        ));
    }
    mismatches
}

fn metadata_comparison_key(key: &str) -> String {
    let mut parts = key.split(':');
    if let (Some(group), Some(instance), Some(tag), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
        && !group.is_empty()
        && !tag.is_empty()
        && instance.strip_prefix("Copy").is_some_and(|number| {
            !number.is_empty() && number.bytes().all(|byte| byte.is_ascii_digit())
        })
    {
        // Family 4 numbers distinguish a dump's instances, not cross-file identity.
        return format!("{group}:{tag}");
    }
    key.to_owned()
}

fn overlay_metadata(base: &mut BTreeMap<String, String>, overlay: &BTreeMap<String, String>) {
    let replaced = overlay
        .keys()
        .map(|key| metadata_comparison_key(key))
        .collect::<BTreeSet<_>>();
    base.retain(|key, _| !replaced.contains(&metadata_comparison_key(key)));
    base.extend(overlay.clone());
}

type MetadataInstance<'a> = (&'a str, &'a str);

fn metadata_instance_groups(
    map: &BTreeMap<String, String>,
) -> BTreeMap<String, Vec<MetadataInstance<'_>>> {
    let mut grouped = BTreeMap::<String, Vec<MetadataInstance<'_>>>::new();
    for (key, value) in map {
        grouped
            .entry(metadata_comparison_key(key))
            .or_default()
            .push((key.as_str(), value.as_str()));
    }
    grouped
}

fn unmatched_metadata_instances<'a>(
    src: &'a BTreeMap<String, String>,
    dst: &'a BTreeMap<String, String>,
) -> (Vec<MetadataInstance<'a>>, Vec<MetadataInstance<'a>>) {
    let mut missing = Vec::new();
    let mut unexpected = Vec::new();
    let mut actual_groups = metadata_instance_groups(dst);
    for (key, expected) in metadata_instance_groups(src) {
        let actual = actual_groups.remove(&key).unwrap_or_default();
        let mut source_matches = vec![None; expected.len()];
        let mut output_matches: Vec<Option<usize>> = vec![None; actual.len()];
        for start in 0..expected.len() {
            let mut pending = VecDeque::from([start]);
            let mut seen_source = vec![false; expected.len()];
            let mut seen_output = vec![false; actual.len()];
            let mut parent = vec![0; actual.len()];
            seen_source[start] = true;
            // Augment the matching so tolerated numeric values cannot steal the
            // only compatible instance of another duplicate. No recursive stack.
            'search: while let Some(source) = pending.pop_front() {
                for (output, (_, value)) in actual.iter().enumerate() {
                    if seen_output[output]
                        || !metadata_values_equivalent(&key, expected[source].1, value)
                    {
                        continue;
                    }
                    seen_output[output] = true;
                    parent[output] = source;
                    if let Some(previous) = output_matches[output] {
                        if !seen_source[previous] {
                            seen_source[previous] = true;
                            pending.push_back(previous);
                        }
                    } else {
                        let mut cursor = Some(output);
                        while let Some(output) = cursor {
                            let source = parent[output];
                            cursor = source_matches[source].replace(output);
                            output_matches[output] = Some(source);
                        }
                        break 'search;
                    }
                }
            }
        }
        missing.extend(
            expected
                .into_iter()
                .zip(source_matches)
                .filter_map(|(instance, matched)| matched.is_none().then_some(instance)),
        );
        unexpected.extend(
            actual
                .into_iter()
                .zip(output_matches)
                .filter_map(|(instance, matched)| matched.is_none().then_some(instance)),
        );
    }
    unexpected.extend(actual_groups.into_values().flatten());
    (missing, unexpected)
}

fn source_instance_mismatches(
    missing: &[MetadataInstance<'_>],
    unexpected: &[MetadataInstance<'_>],
) -> Vec<String> {
    missing
        .iter()
        .map(|(key, expected)| {
            let canonical = metadata_comparison_key(key);
            match unexpected
                .iter()
                .find(|(actual, _)| metadata_comparison_key(actual) == canonical)
            {
                Some((_, actual)) => format!(
                    "metadata {key} expected={expected:?} actual={actual:?} (possible wrong-source metadata)"
                ),
                None => format!(
                    "metadata {key} missing from output (expected={expected:?})"
                ),
            }
        })
        .collect()
}

fn metadata_values_equivalent(key: &str, expected: &str, actual: &str) -> bool {
    if expected == actual {
        return true;
    }
    let tag = key.rsplit(':').next().unwrap_or(key);
    if !matches!(tag, "WhitePoint" | "PrimaryChromaticities") {
        return false;
    }
    let parse = |value: &str| {
        value
            .split_whitespace()
            .map(str::parse::<f64>)
            .collect::<Result<Vec<_>, _>>()
    };
    match (parse(expected), parse(actual)) {
        (Ok(expected), Ok(actual)) if expected.len() == actual.len() => expected
            .iter()
            .zip(actual)
            .all(|(expected, actual)| (expected - actual).abs() <= 1.0e-8),
        _ => false,
    }
}

fn preserve_source_mismatches(
    src: &BTreeMap<String, String>,
    dst: &BTreeMap<String, String>,
) -> Vec<String> {
    let (missing, unexpected) = unmatched_metadata_instances(src, dst);
    let mut mismatches = source_instance_mismatches(&missing, &unexpected);
    let source_groups = metadata_instance_groups(src);
    for (key, value) in unexpected {
        let canonical = metadata_comparison_key(key);
        if let Some(expected) = source_groups.get(&canonical)
            && !expected
                .iter()
                .any(|(_, source)| metadata_values_equivalent(&canonical, source, value))
        {
            mismatches.push(format!("metadata {key} introduces conflicting duplicate value={value:?} (possible wrong-source metadata)"));
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

fn preservable_tag_maps<const N: usize>(
    paths: [&Path; N],
) -> io::Result<[BTreeMap<String, String>; N]> {
    let mut maps = metadata_tag_maps(paths, PRESERVABLE_TAG_ARGS, "preservable")?;
    for map in &mut maps {
        map.retain(|key, _| !preserve_audit_excludes_tag(key));
    }
    Ok(maps)
}

fn preservable_delivery_tag_maps(
    paths: [&Path; 4],
    jpeg_archive: bool,
) -> io::Result<[BTreeMap<String, String>; 4]> {
    if !jpeg_archive {
        return preservable_tag_maps(paths);
    }
    let mut maps = metadata_tag_maps_with_exif_custody(
        paths,
        PRESERVABLE_TAG_ARGS,
        "preservable",
        "-G1:4",
        Some([paths[0], paths[1]]),
    )?;
    for map in &mut maps {
        map.retain(|key, _| {
            !preserve_audit_excludes_tag(key) && metadata_comparison_key(key) != "EXIF:EXIF"
        });
    }
    Ok(maps)
}

fn has_no_portable_embedded_metadata_channel(path: &Path) -> io::Result<bool> {
    crate::image::format_detect::detect_true_format(path)
        .map(|format| matches!(format, crate::image::format_detect::FormatKind::Pnm))
        .map_err(|error| io::Error::other(error.to_string()))
}

/// Verify a native container merge against an explicitly supplied XMP sidecar.
///
/// `verify_output_embedded_metadata` can discover only a sidecar adjacent to
/// `src`; native merge callers may receive a sidecar from another controlled
/// path. Keep that path explicit so a successful merge cannot be reported when
/// the writer silently ignored the requested sidecar.
///
/// # Errors
/// Returns an error when `ExifTool` cannot read the source, sidecar, or output, or
/// when any source/sidecar metadata value is absent or changed in the output.
pub(super) fn verify_output_embedded_metadata_with_explicit_xmp(
    src: &Path,
    xmp: &Path,
    dst: &Path,
) -> io::Result<()> {
    if !crate::ExiftoolBuilder::check_available() {
        return Err(io::Error::other(
            "exiftool was not found or failed its runtime health check; explicit XMP audit cannot proceed",
        ));
    }

    let [mut expected, sidecar, actual] = preservable_tag_maps([src, xmp, dst])?;
    // The native merge applies the explicit sidecar after embedded metadata.
    overlay_metadata(&mut expected, &sidecar);
    let mismatches = preserve_source_mismatches(&expected, &actual);
    if mismatches.is_empty() {
        let detail = format!(
            "Metadata Audit: explicit XMP sidecar preserved in native container merge {} + {} -> {}",
            src.display(),
            xmp.display(),
            dst.display()
        );
        tracing::info!(
            target: "mfb::report",
            src = %src.display(),
            xmp = %xmp.display(),
            dst = %dst.display(),
            "{detail}"
        );
        crate::log_info!(crate::infra::static_logs::messages::LABEL_METADATA, detail);
        return Ok(());
    }

    let detail = format!(
        "Metadata Audit: explicit XMP sidecar was not preserved in native container merge {} + {} -> {}: {}",
        src.display(),
        xmp.display(),
        dst.display(),
        mismatches.join("; ")
    );
    crate::media_conversion_gate::delivery_metadata_path_audit(
        "delivery_metadata_explicit_xmp_audit",
        dst,
        detail.clone(),
    );
    Err(io::Error::other(detail))
}

fn clearable_tag_map(path: &Path) -> io::Result<BTreeMap<String, String>> {
    let [map] = metadata_tag_maps([path], CLEARABLE_TAG_ARGS, "clearable")?;
    Ok(map)
}

fn metadata_tag_maps<const N: usize>(
    paths: [&Path; N],
    tag_args: &[&str],
    label: &str,
) -> io::Result<[BTreeMap<String, String>; N]> {
    metadata_tag_maps_with_groups(paths, tag_args, label, "-G1:4")
}

fn metadata_tag_maps_with_groups<const N: usize>(
    paths: [&Path; N],
    tag_args: &[&str],
    label: &str,
    groups: &str,
) -> io::Result<[BTreeMap<String, String>; N]> {
    metadata_tag_maps_with_exif_custody(paths, tag_args, label, groups, None)
}

fn metadata_tag_maps_with_exif_custody<const N: usize>(
    paths: [&Path; N],
    tag_args: &[&str],
    label: &str,
    groups: &str,
    pair: Option<[&Path; 2]>,
) -> io::Result<[BTreeMap<String, String>; N]> {
    let mut requested = BTreeMap::<std::path::PathBuf, Vec<usize>>::new();
    let mut maps = std::array::from_fn(|_| BTreeMap::new());
    for (index, path) in paths.iter().enumerate() {
        if !has_no_portable_embedded_metadata_channel(path)? {
            requested
                .entry(std::path::PathBuf::from(exiftool_path_arg(path).as_ref()))
                .or_default()
                .push(index);
        }
    }
    if requested.is_empty() {
        return Ok(maps);
    }
    let mut builder = crate::ExiftoolBuilder::new();
    builder
        .arg("-n")
        .arg("-j")
        .arg(groups)
        .arg("-a")
        .arg("-s")
        .arg("-b")
        .arg("-Warning")
        .arg("-Error");
    for arg in tag_args {
        builder.arg(*arg);
    }
    if pair.is_some() {
        builder.arg("-EXIF");
    }
    let mut command = builder.build();
    command.args(requested.keys());
    let output = crate::process_runner::run_command_with_liveness_timeout(
        &mut command,
        std::time::Duration::from_secs(120),
        crate::process_runner::image_process_hard_timeout(),
        "paired metadata audit",
    )?;
    if !output.status.success() {
        return Err(io::Error::other(format!(
            "exiftool {label} dump failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    let objects = if let Some([src, dst]) = pair {
        parse_metadata_records_with_exif_custody(
            &output.stdout,
            Some([
                Path::new(exiftool_path_arg(src).as_ref()),
                Path::new(exiftool_path_arg(dst).as_ref()),
            ]),
        )?
    } else {
        parse_metadata_records(&output.stdout)?
    };
    if objects.len() != requested.len() {
        return Err(io::Error::other("incomplete metadata response"));
    }
    for (path, map) in objects {
        let indices = requested
            .get(&path)
            .ok_or_else(|| io::Error::other("unexpected metadata response path"))?;
        for &index in indices {
            maps[index] = map.clone();
        }
    }
    Ok(maps)
}

fn preserve_audit_excludes_tag(key: &str) -> bool {
    let canonical = metadata_comparison_key(key);
    let key = canonical.as_str();
    let tag = key.rsplit(':').next().unwrap_or(key);
    tag.eq_ignore_ascii_case("Orientation")
        || tag.eq_ignore_ascii_case("XMPToolkit")
        || CROSS_CONTAINER_STRUCTURAL_TAGS
            .iter()
            .any(|structural| tag.eq_ignore_ascii_case(structural))
        || key.eq_ignore_ascii_case("IFD1:ThumbnailOffset")
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
    if has_no_portable_embedded_metadata_channel(path)? {
        return Ok(0);
    }
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

fn parse_metadata_records(
    raw: &[u8],
) -> io::Result<BTreeMap<std::path::PathBuf, BTreeMap<String, String>>> {
    parse_metadata_records_with_exif_custody(raw, None)
}

fn parse_metadata_records_with_exif_custody(
    raw: &[u8],
    pair: Option<[&Path; 2]>,
) -> io::Result<BTreeMap<std::path::PathBuf, BTreeMap<String, String>>> {
    struct UniqueRecord(serde_json::Map<String, Value>);
    impl<'de> serde::Deserialize<'de> for UniqueRecord {
        fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            struct RecordVisitor;
            impl<'de> serde::de::Visitor<'de> for RecordVisitor {
                type Value = UniqueRecord;

                fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                    formatter.write_str("a metadata record with unique member names")
                }

                fn visit_map<M: serde::de::MapAccess<'de>>(
                    self,
                    mut access: M,
                ) -> Result<Self::Value, M::Error> {
                    let mut object = serde_json::Map::new();
                    while let Some((key, value)) = access.next_entry::<String, Value>()? {
                        if object.insert(key.clone(), value).is_some() {
                            return Err(serde::de::Error::custom(format!(
                                "duplicate metadata member {key}"
                            )));
                        }
                    }
                    Ok(UniqueRecord(object))
                }
            }
            deserializer.deserialize_map(RecordVisitor)
        }
    }

    let records: Vec<UniqueRecord> = serde_json::from_slice(raw)?;
    let allowed_warnings = pair
        .and_then(|[src, dst]| {
            let record = |path: &Path| {
                records
                    .iter()
                    .find(|record| {
                        record
                            .0
                            .get("SourceFile")
                            .and_then(Value::as_str)
                            .map(Path::new)
                            == Some(path)
                    })
                    .map(|record| &record.0)
            };
            let source = record(src)?;
            let output = record(dst)?;
            let exif = |record: &serde_json::Map<String, Value>| {
                metadata_object_map(record)
                    .into_iter()
                    .filter(|(key, _)| metadata_comparison_key(key) == "EXIF:EXIF")
                    .collect::<BTreeMap<_, _>>()
            };
            let source_exif = exif(source);
            let output_exif = exif(output);
            if source_exif.is_empty()
                || source_exif.get("EXIF:EXIF") != output_exif.get("EXIF:EXIF")
                || !preserve_mismatches(&source_exif, &output_exif).is_empty()
            {
                return None;
            }
            Some(
                source
                    .iter()
                    .filter(|(key, _)| key.starts_with("ExifTool:") && key.ends_with(":Warning"))
                    .filter_map(|(_, value)| value.as_str())
                    .filter(|message| {
                        matches!(
                            *message,
                            "[minor] Unrecognized MakerNotes"
                                | "Invalid EXIF text encoding for UserComment"
                        )
                    })
                    .map(str::to_owned)
                    .collect::<BTreeSet<_>>(),
            )
        })
        .unwrap_or_default();
    let mut maps = BTreeMap::new();
    for UniqueRecord(object) in records {
        let path = object
            .get("SourceFile")
            .and_then(Value::as_str)
            .ok_or_else(|| io::Error::other("metadata response has no SourceFile"))?;
        for (key, diagnostic) in object.iter().filter(|(key, _)| {
            (key.starts_with("ExifTool:") || !key.contains(':'))
                && key.rsplit(':').next().is_some_and(|tag| {
                    tag.eq_ignore_ascii_case("Error") || tag.eq_ignore_ascii_case("Warning")
                })
        }) {
            if key.ends_with(":Warning")
                && pair.is_some_and(|pair| pair.contains(&Path::new(path)))
                && diagnostic
                    .as_str()
                    .is_some_and(|message| allowed_warnings.contains(message))
            {
                tracing::warn!(target: "mfb.metadata", source = path, "{diagnostic}; original EXIF payload preserved byte-for-byte");
                continue;
            }
            // A stale digest warns about source consistency, not an unreadable
            // metadata field. Compare the actual IPTC/XMP values independently.
            if key.ends_with(":Warning")
                && diagnostic.as_str().is_some_and(|message| {
                    const STALE_DIGEST: &str = "IPTCDigest is not current. XMP may be out of sync";
                    message == STALE_DIGEST
                        || message.strip_prefix(STALE_DIGEST).is_some_and(|suffix| {
                            suffix
                                .strip_prefix(" [x")
                                .and_then(|s| s.strip_suffix(']'))
                                .is_some_and(|number| {
                                    !number.is_empty()
                                        && number.bytes().all(|byte| byte.is_ascii_digit())
                                })
                        })
                })
            {
                tracing::warn!(target: "mfb.metadata", source = path, "{diagnostic}");
                continue;
            }
            return Err(io::Error::other(format!(
                "exiftool reported {key} for {path}: {diagnostic}"
            )));
        }
        if maps
            .insert(std::path::PathBuf::from(path), metadata_object_map(&object))
            .is_some()
        {
            return Err(io::Error::other("duplicate metadata response"));
        }
    }
    Ok(maps)
}

fn metadata_object_map(obj: &serde_json::Map<String, Value>) -> BTreeMap<String, String> {
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
    map
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn preservable_tag_map(path: &Path) -> io::Result<BTreeMap<String, String>> {
        let [map] = preservable_tag_maps([path])?;
        Ok(map)
    }

    #[test]
    fn metadata_records_reject_empty_duplicate_and_tool_errors() {
        for invalid in [
            b"".as_slice(),
            br#"[{"XMP:Title":"unbound"}]"#,
            br#"[{"SourceFile":"a"},{"SourceFile":"a"}]"#,
            br#"[{"SourceFile":"a","ExifTool:Error":"read failed"}]"#,
            br#"[{"SourceFile":"a","ExifTool:Copy1:Warning":"partial extraction"}]"#,
            br#"[{"SourceFile":"a","IFD0:Artist":"first","IFD0:Artist":"second"}]"#,
            br#"[{"SourceFile":"a","SourceFile":"b"}]"#,
        ] {
            assert!(parse_metadata_records(invalid).is_err());
        }
        let records = parse_metadata_records(
            br#"[{"SourceFile":"b","XMP:Title":"B"},{"SourceFile":"a","XMP:Title":"A"}]"#,
        )
        .unwrap();
        assert_eq!(records[Path::new("a")]["XMP:Title"], "A");
        assert_eq!(records[Path::new("b")]["XMP:Title"], "B");
        let records = parse_metadata_records(
            br#"[{"SourceFile":"a","XMP:Error":"ordinary metadata","IFD0:Artist":"primary","IFD0:Copy1:Artist":"other"}]"#,
        )
        .unwrap();
        assert_eq!(records[Path::new("a")]["XMP:Error"], "ordinary metadata");
        assert_eq!(records[Path::new("a")]["IFD0:Copy1:Artist"], "other");
        let records = parse_metadata_records(
            br#"[{"SourceFile":"a","ExifTool:Warning":"IPTCDigest is not current. XMP may be out of sync [x2]","IPTC:Caption-Abstract":"preserved"}]"#,
        ).unwrap();
        assert_eq!(
            records[Path::new("a")]["IPTC:Caption-Abstract"],
            "preserved"
        );
        assert!(parse_metadata_records(
            br#"[{"SourceFile":"a","ExifTool:Warning":"IPTCDigest is not current. XMP may be out of sync [x2]; read failed"}]"#,
        ).is_err());
    }

    #[test]
    fn exif_warning_custody_requires_original_warning_and_complete_equal_payloads() {
        let pair = [Path::new("source"), Path::new("output")];
        let records = serde_json::json!([
            {"SourceFile":"source", "EXIF:EXIF":"base64:YWJj", "ExifTool:Warning":"[minor] Unrecognized MakerNotes"},
            {"SourceFile":"output", "EXIF:EXIF":"base64:YWJj", "ExifTool:Warning":"[minor] Unrecognized MakerNotes"}
        ]);
        let raw = serde_json::to_vec(&records).unwrap();
        assert!(parse_metadata_records(&raw).is_err());
        assert!(parse_metadata_records_with_exif_custody(&raw, Some(pair)).is_ok());
        let mut swapped = records.clone();
        swapped[0]["EXIF:Copy1:EXIF"] = Value::from("base64:YWJk");
        swapped[1]["EXIF:EXIF"] = Value::from("base64:YWJk");
        swapped[1]["EXIF:Copy1:EXIF"] = Value::from("base64:YWJj");
        assert!(
            parse_metadata_records_with_exif_custody(
                &serde_json::to_vec(&swapped).unwrap(),
                Some(pair)
            )
            .is_err()
        );
        for (index, key, value) in [
            (1, "EXIF:EXIF", Value::from("base64:YWJk")),
            (0, "EXIF:EXIF", Value::Null),
            (0, "ExifTool:Warning", Value::Null),
            (1, "ExifTool:Warning", Value::from("Truncated EXIF data")),
            (1, "ExifTool:Error", Value::from("read failed")),
            (1, "SourceFile", Value::from("unrelated")),
            (1, "EXIF:Copy1:EXIF", Value::from("base64:YWJj")),
        ] {
            let mut invalid = records.clone();
            invalid[index][key] = value;
            assert!(
                parse_metadata_records_with_exif_custody(
                    &serde_json::to_vec(&invalid).unwrap(),
                    Some(pair)
                )
                .is_err(),
                "{index} {key}"
            );
        }
    }

    #[test]
    fn jpeg_repeated_xmp_and_authoritative_overlay_have_separate_custody() {
        if !crate::CjxlBuilder::check_available() || !crate::DjxlBuilder::check_available() {
            eprintln!("SKIP: cjxl/djxl unavailable for layered XMP regression");
            return;
        }
        let temp = TempDir::new().unwrap();
        let src = temp.path().join("source.jpg");
        let dst = temp.path().join("output.jxl");
        let sidecar = src.with_extension("xmp");
        write_minimal_jpeg(&src);
        write_metadata_tag(&src, "-XMP-dc:Title=secondary original");
        let packet = |title: &str| {
            format!(
                "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\"><rdf:Description xmlns:dc=\"http://purl.org/dc/elements/1.1/\" dc:title=\"{title}\"/></rdf:RDF></x:xmpmeta>"
            )
        };
        let primary = packet("primary original");
        let mut app1 = b"http://ns.adobe.com/xap/1.0/\0".to_vec();
        app1.extend_from_slice(primary.as_bytes());
        let jpeg = std::fs::read(&src).unwrap();
        let mut repeated = jpeg[..2].to_vec();
        repeated.extend_from_slice(&[0xff, 0xe1]);
        repeated.extend_from_slice(&u16::try_from(app1.len() + 2).unwrap().to_be_bytes());
        repeated.extend_from_slice(&app1);
        repeated.extend_from_slice(&jpeg[2..]);
        std::fs::write(&src, repeated).unwrap();
        let encoded = crate::CjxlBuilder::new()
            .input(&src)
            .output(&dst)
            .lossless_jpeg(true)
            .build()
            .output()
            .unwrap();
        assert!(
            encoded.status.success(),
            "{}",
            String::from_utf8_lossy(&encoded.stderr)
        );
        std::fs::write(&sidecar, packet("current sidecar")).unwrap();
        super::super::append_xmp_overlay_to_jxl(&sidecar, &dst).unwrap();
        verify_output_embedded_metadata(&src, &dst, MetadataOutputPolicy::Preserve).unwrap();
        verify_output_embedded_metadata(&src, &dst, MetadataOutputPolicy::PreserveSource).unwrap();
        std::fs::write(&sidecar, packet("newer sidecar")).unwrap();
        assert!(
            verify_output_embedded_metadata(&src, &dst, MetadataOutputPolicy::Preserve).is_err()
        );
        std::fs::write(&sidecar, packet("current sidecar")).unwrap();
        let foreign = temp.path().join("foreign.xmp");
        std::fs::write(&foreign, packet("foreign product")).unwrap();
        super::super::append_xmp_overlay_to_jxl(&foreign, &dst).unwrap();
        assert!(
            verify_output_embedded_metadata(&src, &dst, MetadataOutputPolicy::Preserve).is_err()
        );
    }

    #[test]
    fn jpeg_reconstruction_custody_requires_jpeg_only_physical_provenance() {
        for (location, key) in [
            (
                "MakerNotes:Samsung::JPEG-Trailer-Samsung:VendorTag",
                "Samsung:VendorTag",
            ),
            ("MakerNotes:Samsung::JPEG-APP5:UniqueID", "Samsung:UniqueID"),
            (
                "IPTC:IPTC2::JPEG-APP13-Photoshop-IPTC:ObjectName",
                "IPTC2:ObjectName",
            ),
            (
                "Photoshop:Photoshop:Copy1:JPEG-APP13-Photoshop:IPTCDigest",
                "Photoshop:Copy1:IPTCDigest",
            ),
            ("File:File::JPEG-COM:Comment", "File:Comment"),
            ("File:File::JPEG-APP0:VendorTag", "File:VendorTag"),
            ("File:File::JPEG-APP15:VendorTag", "File:VendorTag"),
        ] {
            assert_eq!(jpeg_reconstruction_tag_key(location).as_deref(), Some(key));
        }
        for location in [
            "EXIF:IFD0::JPEG-APP1-IFD0:Artist",
            "XMP:XMP-dc::JPEG-Trailer-XMP:Title",
            "ICC_Profile:ICC-header::JPEG-APP2-ICC:ProfileID",
            "MakerNotes:Samsung::JPEG-APP1-IFD0-MakerNotes:VendorTag",
            "MakerNotes:Samsung::PNG-Trailer-Samsung:VendorTag",
            "MakerNotes:Samsung::HEIC-sefd-Samsung:VendorTag",
            "MakerNotes:Samsung::JPEG-APP99:VendorTag",
            "MakerNotes:Samsung::JPEG-APP16:VendorTag",
            "MakerNotes:Samsung::JPEG-APP01:VendorTag",
            "MakerNotes:Samsung::JPEG-APP+1:VendorTag",
            "MakerNotes:Samsung::JPEG-APP:VendorTag",
            "MakerNotes:Samsung::JPEG-APP1x:VendorTag",
            "MakerNotes:Samsung:unknown:JPEG-APP5:VendorTag",
            "MakerNotes:Samsung::unknown:VendorTag",
            "Samsung:VendorTag",
        ] {
            assert!(
                jpeg_reconstruction_tag_key(location).is_none(),
                "{location}"
            );
        }
    }

    #[test]
    fn metadata_dump_keeps_repeated_same_group_tags_and_rejects_partial_copy() {
        let root = TempDir::new().unwrap();
        let src = root.path().join("duplicate.tiff");
        let dst = root.path().join("single.tiff");
        let fixture = |values: &[u8]| {
            let mut tiff = b"II".to_vec();
            tiff.extend_from_slice(&42_u16.to_le_bytes());
            tiff.extend_from_slice(&8_u32.to_le_bytes());
            tiff.extend_from_slice(&u16::try_from(values.len()).unwrap().to_le_bytes());
            for &value in values {
                tiff.extend_from_slice(&0x010f_u16.to_le_bytes());
                tiff.extend_from_slice(&2_u16.to_le_bytes());
                tiff.extend_from_slice(&2_u32.to_le_bytes());
                tiff.extend_from_slice(&[value, 0, 0, 0]);
            }
            tiff.extend_from_slice(&0_u32.to_le_bytes());
            tiff
        };
        std::fs::write(&src, fixture(b"AB")).unwrap();
        std::fs::write(&dst, fixture(b"B")).unwrap();
        let tags = preservable_tag_map(&src).unwrap();
        assert_eq!(tags["IFD0:Make"], "B");
        assert_eq!(tags["IFD0:Copy1:Make"], "A");
        let error = verify_output_embedded_metadata(&src, &dst, MetadataOutputPolicy::Preserve)
            .unwrap_err();
        assert!(error.to_string().contains("Copy1:Make"), "{error}");
        std::fs::copy(&src, &dst).unwrap();
        verify_output_embedded_metadata(&src, &dst, MetadataOutputPolicy::Preserve).unwrap();
        std::fs::write(&dst, fixture(b"BA")).unwrap();
        verify_output_embedded_metadata(&src, &dst, MetadataOutputPolicy::Preserve).unwrap();
        std::fs::write(&src, fixture(b"AA")).unwrap();
        std::fs::write(&dst, fixture(b"A")).unwrap();
        assert!(
            verify_output_embedded_metadata(&src, &dst, MetadataOutputPolicy::Preserve).is_err()
        );
    }

    #[test]
    fn paired_metadata_matches_individual_queries_and_deduplicates_paths() {
        let root = TempDir::new().unwrap();
        let src = root.path().join("source.jpg");
        let dst = root.path().join("output.jpg");
        write_minimal_jpeg(&src);
        write_minimal_jpeg(&dst);
        write_metadata_tag(&src, "-XMP-dc:Title=Source");
        write_metadata_tag(&dst, "-XMP-dc:Title=Output");
        let started = std::time::Instant::now();
        let [source, output, repeated] =
            preservable_tag_maps([src.as_path(), dst.as_path(), src.as_path()]).unwrap();
        let batch_elapsed = started.elapsed();
        let started = std::time::Instant::now();
        assert_eq!(source, preservable_tag_map(&src).unwrap());
        assert_eq!(output, preservable_tag_map(&dst).unwrap());
        assert_eq!(source, repeated);
        assert_ne!(preserve_mismatches(&source, &output), Vec::<String>::new());
        eprintln!(
            "paired metadata comparison: batch_ms={} individual_ms={}",
            batch_elapsed.as_millis(),
            started.elapsed().as_millis()
        );
    }

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
    fn copy_ordinals_do_not_change_metadata_identity() {
        let mut src = BTreeMap::new();
        let mut dst = BTreeMap::new();
        for (tag, value) in [
            ("XResolution", "72"),
            ("YResolution", "72"),
            ("ResolutionUnit", "2"),
        ] {
            src.insert(format!("IFD1:Copy2:{tag}"), value.to_owned());
            dst.insert(format!("IFD1:Copy1:{tag}"), value.to_owned());
        }
        assert_eq!(preserve_mismatches(&src, &dst), Vec::<String>::new());
        assert_eq!(preserve_source_mismatches(&src, &dst), Vec::<String>::new());
        dst.insert("IFD1:Copy1:XResolution".into(), "96".into());
        assert_ne!(preserve_mismatches(&src, &dst), Vec::<String>::new());
        for invalid in [
            "IFD1:Copy:XResolution",
            "IFD1:CopyX:XResolution",
            "Copy1:XResolution",
            "EXIF:IFD1:Copy1:XResolution",
        ] {
            assert_eq!(metadata_comparison_key(invalid), invalid);
        }
    }

    #[test]
    fn duplicate_metadata_values_and_counts_remain_mandatory() {
        let src = BTreeMap::from([
            ("XMP-dc:Title".into(), "A".into()),
            ("XMP-dc:Copy1:Title".into(), "B".into()),
        ]);
        let mut dst = BTreeMap::from([
            ("XMP-dc:Copy2:Title".into(), "A".into()),
            ("XMP-dc:Title".into(), "B".into()),
        ]);
        assert_eq!(preserve_mismatches(&src, &dst), Vec::<String>::new());
        dst.remove("XMP-dc:Copy2:Title");
        assert_ne!(preserve_source_mismatches(&src, &dst), Vec::<String>::new());
        let src = BTreeMap::from([
            ("XMP-dc:Title".into(), "A".into()),
            ("XMP-dc:Copy1:Title".into(), "A".into()),
        ]);
        dst.insert("XMP-dc:Title".into(), "A".into());
        assert_ne!(preserve_source_mismatches(&src, &dst), Vec::<String>::new());
        dst.insert("XMP-dc:Copy3:Title".into(), "A".into());
        assert_eq!(preserve_mismatches(&src, &dst), Vec::<String>::new());
        dst.insert("XMP-dc:Copy4:Title".into(), "A".into());
        assert_ne!(preserve_mismatches(&src, &dst), Vec::<String>::new());
        assert_eq!(preserve_source_mismatches(&src, &dst), Vec::<String>::new());
        dst.insert("XMP-dc:Copy3:Title".into(), "other".into());
        dst.remove("XMP-dc:Copy4:Title");
        assert_ne!(preserve_source_mismatches(&src, &dst), Vec::<String>::new());
        dst.insert("XMP-dc:Copy4:Title".into(), "A".into());
        assert_ne!(preserve_source_mismatches(&src, &dst), Vec::<String>::new());
    }

    #[test]
    fn duplicate_matching_keeps_location_and_numeric_tolerance() {
        let src = BTreeMap::from([("IFD0:Copy1:Artist".into(), "A".into())]);
        let dst = BTreeMap::from([("IFD1:Copy2:Artist".into(), "A".into())]);
        assert_ne!(preserve_mismatches(&src, &dst), Vec::<String>::new());
        let src = BTreeMap::from([
            ("IFD0:Copy1:WhitePoint".into(), "0 0 0".into()),
            ("IFD0:WhitePoint".into(), "0.000000009 0 0".into()),
        ]);
        let mut dst = BTreeMap::from([
            ("IFD0:Copy2:WhitePoint".into(), "0 0 0".into()),
            ("IFD0:WhitePoint".into(), "-0.000000009 0 0".into()),
        ]);
        assert_eq!(preserve_mismatches(&src, &dst), Vec::<String>::new());
        dst.insert("IFD0:WhitePoint".into(), "-0.00000002 0 0".into());
        assert_ne!(preserve_mismatches(&src, &dst), Vec::<String>::new());
    }

    #[test]
    fn sidecar_overrides_metadata_by_tag_not_copy_ordinal() {
        let mut embedded = BTreeMap::from([
            ("XMP-dc:Copy2:Title".into(), "embedded".into()),
            ("XMP-dc:Title".into(), "also embedded".into()),
            ("IFD0:Artist".into(), "artist".into()),
        ]);
        let sidecar = BTreeMap::from([("XMP-dc:Copy1:Title".into(), "sidecar".into())]);
        overlay_metadata(&mut embedded, &sidecar);
        assert_eq!(embedded.len(), 2);
        assert_eq!(embedded["IFD0:Artist"], "artist");
        let output = BTreeMap::from([
            ("XMP-dc:Title".into(), "sidecar".into()),
            ("IFD0:Artist".into(), "artist".into()),
        ]);
        assert_eq!(
            preserve_mismatches(&embedded, &output),
            Vec::<String>::new()
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
    fn preserve_source_mismatches_allow_codec_tags_but_not_wrong_source_values() {
        let mut src = BTreeMap::new();
        src.insert("XMP-dc:Description".into(), "source".into());
        let mut dst = src.clone();
        dst.insert("IFD0:XResolution".into(), "1".into());
        dst.insert("IFD0:YResolution".into(), "1".into());
        assert_eq!(
            preserve_source_mismatches(&src, &dst),
            [] as [std::string::String; 0]
        );

        dst.insert("XMP-dc:Description".into(), "other".into());
        assert!(
            preserve_source_mismatches(&src, &dst)
                .iter()
                .any(|mismatch| mismatch.contains("wrong-source"))
        );
    }

    #[test]
    fn preserve_audit_excludes_rewritten_technical_tags_but_not_creative_tags() {
        for generated in [
            "IFD0:Orientation",
            "XMP-x:XMPToolkit",
            "IFD1:ThumbnailOffset",
            "IFD1:Copy2:ThumbnailOffset",
            "IFD0:BitsPerSample",
            "IFD0:SMaxSampleValue",
            "IFD0:SMinSampleValue",
            "IFD0:StripOffsets",
            "IFD0:YCbCrPositioning",
            "Keys:CompatibleBrands",
            "Keys:Copy1:CompatibleBrands",
            "Keys:MajorBrand",
            "Keys:MinorVersion",
            "UserData:SoftwareVersion",
        ] {
            assert!(preserve_audit_excludes_tag(generated));
        }
        for creative in [
            "Keys:Title",
            "UserData:Description",
            "MakerNotes:ThumbnailOffset",
            "XMP-photoshop:DateCreated",
            "XMP-xmp:CreateDate",
            "IFD0:WhitePoint",
        ] {
            assert!(
                !preserve_audit_excludes_tag(creative),
                "creative metadata must remain custody-checked: {creative}"
            );
        }

        assert!(metadata_values_equivalent(
            "IFD0:WhitePoint",
            "0.3127000034 0.3289999962",
            "0.3127000032 0.3289999962"
        ));
        assert!(!metadata_values_equivalent(
            "IFD0:WhitePoint",
            "0.3127 0.3290",
            "0.3000 0.3200"
        ));
    }

    #[test]
    fn preserve_jpeg_app13_via_exact_jxl_reconstruction() {
        use crate::pipeline::verification::VerificationGate as _;

        if !crate::CjxlBuilder::check_available() || !crate::DjxlBuilder::check_available() {
            eprintln!("SKIP: cjxl/djxl unavailable for JPEG APP13 reconstruction regression");
            return;
        }
        let temp = TempDir::new().unwrap();
        let src = temp.path().join("source.jpg");
        let working_copy = temp.path().join("delivery");
        std::fs::create_dir(&working_copy).unwrap();
        let dst = working_copy.join("archive.jxl");
        write_minimal_jpeg(&src);
        write_metadata_tag(&src, "-IPTC:Caption-Abstract=archival caption");
        write_metadata_tag(
            &src,
            "-Photoshop:IPTCDigest=d41d8cd98f00b204e9800998ecf8427e",
        );
        write_metadata_tag(&src, "-EXIF:Artist=source artist");
        // Reproduce an actual second APP13/IPTC instance, not just a fabricated
        // comparison map. ExifTool names its group IPTC2 for these input bytes.
        let jpeg = std::fs::read(&src).unwrap();
        let mut offset = 2;
        let mut app13 = None;
        while offset + 4 <= jpeg.len() && jpeg[offset] == 0xff {
            let marker = jpeg[offset + 1];
            if marker == 0xda || marker == 0xd9 {
                break;
            }
            let length = usize::from(u16::from_be_bytes([jpeg[offset + 2], jpeg[offset + 3]]));
            assert!(length >= 2 && offset + 2 + length <= jpeg.len());
            if marker == 0xed {
                app13 = Some(jpeg[offset..offset + 2 + length].to_vec());
                break;
            }
            offset += length + 2;
        }
        let app13 = app13.expect("fixture must contain JPEG APP13");
        let mut repeated_jpeg = jpeg[..2].to_vec();
        repeated_jpeg.extend_from_slice(&app13);
        repeated_jpeg.extend_from_slice(&jpeg[2..]);
        std::fs::write(&src, repeated_jpeg).unwrap();
        let output = crate::CjxlBuilder::new()
            .input(&src)
            .output(&dst)
            .lossless_jpeg(true)
            .build()
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        crate::fast_img::verify_jxl_roundtrip_integrity(&src, &dst).unwrap();
        let src_tags = preservable_tag_map(&src).unwrap();
        let dst_tags = preservable_tag_map(&dst).unwrap();
        assert!(src_tags.contains_key("Photoshop:IPTCDigest"));
        assert!(
            src_tags.keys().any(|tag| tag.starts_with("IPTC2:")),
            "{src_tags:?}"
        );
        assert!(!dst_tags.contains_key("Photoshop:IPTCDigest"));
        let mut repeated_iptc = src_tags
            .iter()
            .filter(|(key, _)| key.starts_with("IPTC2:"))
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();
        verify_reconstruction_only_metadata(&src, &dst, &mut repeated_iptc, &dst_tags).unwrap();
        assert!(
            preserve_source_mismatches(&repeated_iptc, &dst_tags).is_empty(),
            "reconstruction-owned second IPTC instance must pass"
        );
        let mut contradictory_iptc = dst_tags.clone();
        contradictory_iptc.insert("IPTC2:Caption-Abstract".into(), "different".into());
        let mut expected_iptc =
            BTreeMap::from([("IPTC2:Caption-Abstract".into(), "archival caption".into())]);
        verify_reconstruction_only_metadata(&src, &dst, &mut expected_iptc, &contradictory_iptc)
            .unwrap();
        assert!(
            preserve_mismatches(&expected_iptc, &contradictory_iptc)
                .iter()
                .any(|mismatch| mismatch.contains("wrong-source"))
        );
        let mut invented = BTreeMap::from([("IPTC999:Imaginary".into(), "not in source".into())]);
        verify_reconstruction_only_metadata(&src, &dst, &mut invented, &dst_tags).unwrap();
        assert!(invented.contains_key("IPTC999:Imaginary"));
        verify_output_embedded_metadata(&src, &dst, MetadataOutputPolicy::Preserve)
            .expect("APP13 retained by exact JPEG reconstruction must pass metadata gate");
        crate::metadata::preserve_filesystem_for_delivery(&src, &dst).unwrap();
        let ctx = crate::pipeline::verification::PipelineCtx {
            working_copy,
            src_dir: temp.path().to_path_buf(),
            blake3_log: BTreeMap::from([(
                "source.jpg".into(),
                crate::pipeline::verification::Blake3Entry {
                    out_rel: Some("archive.jxl".into()),
                    src: crate::common_utils::calculate_blake3_hash(&src).unwrap(),
                    out: crate::common_utils::calculate_blake3_hash(&dst).unwrap(),
                    library_asset: None,
                },
            )]),
            expected_count: 1,
            library_handle: None,
            output_format: Some(crate::image::format_detect::FormatKind::Jxl),
        };
        let gate = crate::pipeline::verification::Gate1Local.run(&ctx);
        assert!(gate.passed, "{gate:?}");

        // Source-only cross-container audits must use the same proof boundary.
        verify_output_embedded_metadata(&src, &dst, MetadataOutputPolicy::PreserveSource).unwrap();
        let mut missing_native = src_tags.clone();
        let mut native_output = dst_tags.clone();
        native_output.remove("IFD0:Artist");
        verify_reconstruction_only_metadata(&src, &dst, &mut missing_native, &native_output)
            .unwrap();
        assert!(
            preserve_mismatches(&missing_native, &native_output)
                .iter()
                .any(|m| m.contains("Artist"))
        );
        let mut contradictory = dst_tags;
        contradictory.insert("Photoshop:IPTCDigest".into(), "different".into());
        let mut expected = src_tags;
        verify_reconstruction_only_metadata(&src, &dst, &mut expected, &contradictory).unwrap();
        assert!(
            preserve_mismatches(&expected, &contradictory)
                .iter()
                .any(|m| m.contains("wrong-source"))
        );

        let pixel_only = temp.path().join("pixel-only.jxl");
        let output = crate::CjxlBuilder::new()
            .input(&src)
            .output(&pixel_only)
            .lossless_jpeg(true)
            .allow_jpeg_reconstruction(false)
            .build()
            .output()
            .unwrap();
        assert!(output.status.success());
        assert!(
            verify_output_embedded_metadata(&src, &pixel_only, MetadataOutputPolicy::Preserve)
                .is_err()
        );

        // Same pixels but different APP13 source is not acceptable reconstruction.
        write_metadata_tag(&src, "-IPTC:Caption-Abstract=another source");
        let error = verify_output_embedded_metadata(&src, &dst, MetadataOutputPolicy::Preserve)
            .unwrap_err();
        assert!(
            error.to_string().contains("roundtrip hash mismatch"),
            "{error}"
        );
    }

    #[test]
    fn preserve_vendor_header_comment_and_opaque_jpeg_bytes_via_reconstruction() {
        let temp = TempDir::new().unwrap();
        let src = temp.path().join("source.jpg");
        let dst = temp.path().join("archive.jxl");
        write_minimal_jpeg(&src);
        write_metadata_tag(&src, "-Comment=synthetic container comment");
        let jpeg = std::fs::read(&src).unwrap();
        let mut decorated = jpeg[..2].to_vec();
        let mut unique_id = b"ssuniqueid\0".to_vec();
        unique_id.extend_from_slice(&[0; 32]);
        for (marker, payload) in [
            (0xe5_u8, unique_id.as_slice()),
            (0xef, b"MFB opaque header\0".as_slice()),
        ] {
            decorated.extend_from_slice(&[0xff, marker]);
            decorated.extend_from_slice(&u16::try_from(payload.len() + 2).unwrap().to_be_bytes());
            decorated.extend_from_slice(payload);
        }
        decorated.extend_from_slice(&jpeg[2..]);
        decorated.extend_from_slice(b"MFB opaque trailer\0");
        std::fs::write(&src, &decorated).unwrap();
        let output = crate::CjxlBuilder::new()
            .input(&src)
            .output(&dst)
            .lossless_jpeg(true)
            .build()
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let source = preservable_tag_map(&src).unwrap();
        assert!(source.contains_key("Samsung:UniqueID"), "{source:?}");
        assert!(
            source
                .values()
                .any(|value| value == "synthetic container comment")
        );
        for policy in [
            MetadataOutputPolicy::Preserve,
            MetadataOutputPolicy::PreserveSource,
        ] {
            verify_output_embedded_metadata(&src, &dst, policy).unwrap();
        }
        let pixel_only = temp.path().join("pixel-only.jxl");
        let output = crate::CjxlBuilder::new()
            .input(&src)
            .output(&pixel_only)
            .lossless_jpeg(true)
            .allow_jpeg_reconstruction(false)
            .build()
            .output()
            .unwrap();
        assert!(output.status.success());
        assert!(
            verify_output_embedded_metadata(&src, &pixel_only, MetadataOutputPolicy::Preserve)
                .is_err()
        );

        for opaque in [b"MFB opaque header".as_slice(), b"MFB opaque trailer"] {
            let mut changed = decorated.clone();
            let index = changed
                .windows(opaque.len())
                .position(|bytes| bytes == opaque)
                .unwrap();
            changed[index] = b'X';
            std::fs::write(&src, changed).unwrap();
            let error = verify_output_embedded_metadata(&src, &dst, MetadataOutputPolicy::Preserve)
                .unwrap_err();
            assert!(
                error.to_string().contains("roundtrip hash mismatch"),
                "{error}"
            );
        }
    }

    #[test]
    fn preserve_samsung_capture_info_via_exact_jxl_reconstruction() {
        use crate::pipeline::verification::VerificationGate as _;

        let temp = TempDir::new().unwrap();
        let src = temp.path().join("source.jpg");
        let working_copy = temp.path().join("delivery");
        std::fs::create_dir(&working_copy).unwrap();
        let dst = working_copy.join("archive.jxl");
        write_minimal_jpeg(&src);
        write_metadata_tag(&src, "-EXIF:Artist=synthetic source");

        // Synthetic SEFT record using ExifTool's Samsung trailer layout.
        // No real screenshot bytes or device metadata are part of this fixture.
        let name = b"Samsung_Capture_Info";
        let mut record = Vec::new();
        record.extend_from_slice(&0_u16.to_le_bytes());
        record.extend_from_slice(&0x0c51_u16.to_le_bytes());
        record.extend_from_slice(&u32::try_from(name.len()).unwrap().to_le_bytes());
        record.extend_from_slice(name);
        record.extend_from_slice(b"Screenshot");
        let record_size = u32::try_from(record.len()).unwrap();
        let mut jpeg = std::fs::read(&src).unwrap();
        jpeg.extend_from_slice(&record);
        jpeg.extend_from_slice(b"SEFH");
        jpeg.extend_from_slice(&101_u32.to_le_bytes());
        jpeg.extend_from_slice(&1_u32.to_le_bytes());
        jpeg.extend_from_slice(&0_u16.to_le_bytes());
        jpeg.extend_from_slice(&0x0c51_u16.to_le_bytes());
        jpeg.extend_from_slice(&record_size.to_le_bytes());
        jpeg.extend_from_slice(&record_size.to_le_bytes());
        jpeg.extend_from_slice(&24_u32.to_le_bytes());
        jpeg.extend_from_slice(b"SEFT");
        std::fs::write(&src, &jpeg).unwrap();

        let output = crate::CjxlBuilder::new()
            .input(&src)
            .output(&dst)
            .lossless_jpeg(true)
            .build()
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let src_tags = preservable_tag_map(&src).unwrap();
        let dst_tags = preservable_tag_map(&dst).unwrap();
        assert_eq!(
            src_tags
                .get("Samsung:SamsungCaptureInfo")
                .map(String::as_str),
            Some("Screenshot"),
            "ExifTool must recognize the synthetic SEFT record; observed tags: {src_tags:?}"
        );
        assert!(!dst_tags.contains_key("Samsung:SamsungCaptureInfo"));
        for policy in [
            MetadataOutputPolicy::Preserve,
            MetadataOutputPolicy::PreserveSource,
        ] {
            verify_output_embedded_metadata(&src, &dst, policy).unwrap();
        }
        crate::metadata::preserve_filesystem_for_delivery(&src, &dst).unwrap();
        let gate = crate::pipeline::verification::Gate1Local.run(
            &crate::pipeline::verification::PipelineCtx {
                working_copy,
                src_dir: temp.path().to_path_buf(),
                blake3_log: BTreeMap::from([(
                    "source.jpg".into(),
                    crate::pipeline::verification::Blake3Entry {
                        out_rel: Some("archive.jxl".into()),
                        src: crate::common_utils::calculate_blake3_hash(&src).unwrap(),
                        out: crate::common_utils::calculate_blake3_hash(&dst).unwrap(),
                        library_asset: None,
                    },
                )]),
                expected_count: 1,
                library_handle: None,
                output_format: Some(crate::image::format_detect::FormatKind::Jxl),
            },
        );
        assert!(gate.passed, "{gate:?}");

        let mut contradictory = dst_tags.clone();
        contradictory.insert("Samsung:SamsungCaptureInfo".into(), "Other".into());
        let mut expected = src_tags.clone();
        verify_reconstruction_only_metadata(&src, &dst, &mut expected, &contradictory).unwrap();
        assert!(
            preserve_mismatches(&expected, &contradictory)
                .iter()
                .any(|m| m.contains("wrong-source"))
        );

        let mut missing_native = dst_tags;
        missing_native.remove("IFD0:Artist");
        expected = src_tags;
        expected.insert(
            "Samsung:OtherVendorTag".into(),
            "must remain checked".into(),
        );
        verify_reconstruction_only_metadata(&src, &dst, &mut expected, &missing_native).unwrap();
        let mismatches = preserve_mismatches(&expected, &missing_native);
        assert!(mismatches.iter().any(|m| m.contains("Artist")));
        assert!(mismatches.iter().any(|m| m.contains("OtherVendorTag")));

        let pixel_only = temp.path().join("pixel-only.jxl");
        let output = crate::CjxlBuilder::new()
            .input(&src)
            .output(&pixel_only)
            .lossless_jpeg(true)
            .allow_jpeg_reconstruction(false)
            .build()
            .output()
            .unwrap();
        assert!(output.status.success());
        assert!(
            verify_output_embedded_metadata(&src, &pixel_only, MetadataOutputPolicy::Preserve)
                .is_err()
        );

        // Identical image pixels are insufficient when the trailer differs.
        let capture = jpeg
            .windows(b"Screenshot".len())
            .position(|w| w == b"Screenshot")
            .unwrap();
        jpeg[capture] = b'X';
        std::fs::write(&src, jpeg).unwrap();
        let error = verify_output_embedded_metadata(&src, &dst, MetadataOutputPolicy::Preserve)
            .unwrap_err();
        assert!(
            error.to_string().contains("roundtrip hash mismatch"),
            "{error}"
        );
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
    fn netpbm_has_no_embedded_tag_channel_but_keeps_sidecar_audit_separate() {
        let temp = TempDir::new().expect("tempdir");
        let source = temp.path().join("source.pam");
        std::fs::write(
            &source,
            b"P7\nWIDTH 1\nHEIGHT 1\nDEPTH 3\nMAXVAL 255\nTUPLTYPE RGB\nENDHDR\n\x01\x02\x03",
        )
        .expect("write PAM fixture");

        assert!(
            preservable_tag_map(&source)
                .expect("PAM metadata capability")
                .is_empty()
        );
        assert_eq!(
            reclaimable_embedded_metadata_bytes(&source).expect("PAM embedded metadata size"),
            0
        );
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
