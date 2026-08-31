use crate::ExiftoolBuilder;
use crate::builder_base::ToolBuilder;
use crate::path_safety::{exiftool_path_arg, safe_path_arg};
use anyhow::{Context, Result, bail};
use quick_xml::events::Event;
use serde_json::{Map, Value};
use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::Duration;
use walkdir::WalkDir;

const EXCLUDED_EXTENSIONS: &[&str] = &[
    "xmp",
    "txt",
    "md",
    "json",
    "xml",
    "yaml",
    "yml",
    "toml",
    "ini",
    "cfg",
    "conf",
    "log",
    "rs",
    "py",
    "js",
    "ts",
    "html",
    "css",
    "sh",
    "bash",
    "zsh",
    "c",
    "cpp",
    "h",
    "hpp",
    "java",
    "zip",
    "tar",
    "gz",
    "bz2",
    "xz",
    "7z",
    "rar",
    "ds_store",
    "thumbs.db",
    "desktop.ini",
];

#[inline]
fn is_potential_media(ext: &str) -> bool {
    !EXCLUDED_EXTENSIONS.contains(&ext.to_lowercase().as_str())
}

fn is_regular_non_symlink(path: &Path) -> bool {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata.is_file() && !metadata.file_type().is_symlink(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(error) => {
            crate::media_conversion_gate::delivery_metadata_path_audit(
                "xmp_candidate_stat",
                path,
                format!("cannot inspect candidate while matching XMP sidecar: {error}"),
            );
            false
        }
    }
}

#[derive(Debug, Clone)]
pub struct XmpFile {
    pub path: PathBuf,
    pub document_id: Option<String>,
    pub derived_from: Option<String>,
    pub source: Option<String>,
}

#[derive(Debug)]
pub struct MergeResult {
    pub xmp_path: PathBuf,
    pub media_path: Option<PathBuf>,
    pub success: bool,
    pub message: String,
    pub match_strategy: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Config {
    pub delete_xmp_after_merge: bool,
    pub overwrite_mode: OverwriteMode,
    pub preserve_timestamps: bool,
    pub log_level: LogLevel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverwriteMode {
    Original, // Corresponds to overwrite_original: true
    Never,    // Corresponds to overwrite_original: false
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogLevel {
    Quiet,   // Corresponds to verbose: false
    Verbose, // Corresponds to verbose: true
}

impl Default for Config {
    fn default() -> Self {
        Self {
            delete_xmp_after_merge: false,
            overwrite_mode: OverwriteMode::Original,
            preserve_timestamps: true,
            log_level: LogLevel::Quiet,
        }
    }
}

pub struct XmpMerger {
    config: Config,
}

fn is_jxl_container(path: &Path) -> Result<bool> {
    let mut file = std::fs::File::open(path)
        .with_context(|| format!("failed to open JXL container candidate {}", path.display()))?;
    let mut magic = [0_u8; 12];
    match file.read_exact(&mut magic) {
        Ok(()) => Ok(magic == *crate::constants::JXL_CONTAINER_MAGIC),
        Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => Ok(false),
        Err(error) => Err(error)
            .with_context(|| format!("failed to read JXL container signature {}", path.display())),
    }
}

/// Formats whose archival structure cannot be proved intact after a generic
/// metadata writer rewrites the container.
///
/// JXL has its dedicated append-only overlay path. JPEG and the other formats
/// use a staged native writer only when payload and archival-feature proofs can
/// establish that auxiliary images, HDR relationships, provenance data,
/// unknown properties/chunks, and codec bytes survived unchanged.
#[must_use]
pub(crate) const fn xmp_rewrite_requires_immutable_container(
    format: crate::image::format_detect::FormatKind,
) -> bool {
    use crate::image::format_detect::FormatKind;
    matches!(
        format,
        FormatKind::Jpeg
            | FormatKind::Jxl
            | FormatKind::Avif
            | FormatKind::Heic
            | FormatKind::Heif
            | FormatKind::WebP
            | FormatKind::Jp2
            | FormatKind::Unknown
    )
}

fn jpeg_has_protected_app11(path: &Path) -> Result<bool> {
    let total_len = std::fs::metadata(path)?.len();
    let mut file = std::fs::File::open(path)?;
    let mut soi = [0_u8; 2];
    file.read_exact(&mut soi)?;
    if soi != [0xFF, 0xD8] {
        bail!("invalid JPEG signature while probing protected APP11 structure");
    }

    loop {
        let mut byte = [0_u8; 1];
        file.read_exact(&mut byte)?;
        if byte[0] != 0xFF {
            bail!("invalid JPEG marker stream while probing protected APP11 structure");
        }
        let marker = loop {
            file.read_exact(&mut byte)?;
            if byte[0] != 0xFF {
                break byte[0];
            }
        };
        if marker == 0xD9 || marker == 0xDA {
            return Ok(false);
        }
        if marker == 0x00 || marker == 0xD8 {
            bail!("invalid JPEG marker while probing protected APP11 structure");
        }
        if marker == 0x01 || (0xD0..=0xD7).contains(&marker) {
            continue;
        }

        let mut length = [0_u8; 2];
        file.read_exact(&mut length)?;
        let segment_len = u64::from(u16::from_be_bytes(length));
        if segment_len < 2 {
            bail!("invalid JPEG segment length while probing protected provenance");
        }
        let payload_len = segment_len - 2;
        let end = file
            .stream_position()?
            .checked_add(payload_len)
            .filter(|end| *end <= total_len)
            .ok_or_else(|| anyhow::anyhow!("truncated JPEG APP11 probe segment"))?;
        if marker == 0xEB {
            return Ok(true);
        }
        file.seek(SeekFrom::Start(end))?;
    }
}

fn png_has_c2pa_chunk(path: &Path) -> Result<bool> {
    const PNG_SIGNATURE: &[u8; 8] = b"\x89PNG\r\n\x1a\n";
    let total_len = std::fs::metadata(path)?.len();
    let mut file = std::fs::File::open(path)?;
    let mut signature = [0_u8; 8];
    file.read_exact(&mut signature)?;
    if &signature != PNG_SIGNATURE {
        bail!("invalid PNG signature while probing protected provenance");
    }

    loop {
        let mut header = [0_u8; 8];
        file.read_exact(&mut header)?;
        let payload_len = u64::from(u32::from_be_bytes([
            header[0], header[1], header[2], header[3],
        ]));
        let chunk_type = &header[4..8];
        let end = file
            .stream_position()?
            .checked_add(payload_len)
            .and_then(|value| value.checked_add(4))
            .filter(|end| *end <= total_len)
            .ok_or_else(|| anyhow::anyhow!("truncated PNG provenance chunk"))?;
        if chunk_type == b"caBX" {
            return Ok(true);
        }
        if chunk_type == b"IEND" {
            return Ok(false);
        }
        file.seek(SeekFrom::Start(end))?;
    }
}

fn protected_container_reason(
    path: &Path,
    format: crate::image::format_detect::FormatKind,
) -> Result<Option<&'static str>> {
    use crate::image::format_detect::FormatKind;
    match format {
        FormatKind::Jpeg if jpeg_has_protected_app11(path)? => {
            Ok(Some("JPEG APP11 (JPEG XT/JUMBF-capable) segment"))
        }
        FormatKind::Png if png_has_c2pa_chunk(path)? => Ok(Some("PNG caBX provenance chunk")),
        _ => Ok(None),
    }
}

fn reconstruct_jpeg_to_temp(jxl: &Path) -> Result<tempfile::NamedTempFile> {
    let temp = crate::media_conversion_gate::delivery_named_tempfile_in_scratch_or_err(
        "xmp_jbrd_baseline",
        None,
        Some(".jpg"),
    )?;
    crate::image::jxl_utils::run_exact_jpeg_reconstruction(
        jxl,
        temp.path(),
        "XMP JBRD baseline reconstruction",
    )
    .map_err(anyhow::Error::msg)?;
    Ok(temp)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SourceFileIdentity {
    len: u64,
    modified: std::time::SystemTime,
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
}

fn source_file_identity(path: &Path) -> Result<SourceFileIdentity> {
    #[cfg(unix)]
    use std::os::unix::fs::MetadataExt;
    let metadata = std::fs::symlink_metadata(path)
        .with_context(|| format!("failed to inspect XMP merge target {}", path.display()))?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        bail!(
            "XMP merge target must be a regular non-symlink file: {}",
            path.display()
        );
    }
    Ok(SourceFileIdentity {
        len: metadata.len(),
        modified: metadata
            .modified()
            .with_context(|| format!("failed to read XMP merge target mtime {}", path.display()))?,
        #[cfg(unix)]
        device: metadata.dev(),
        #[cfg(unix)]
        inode: metadata.ino(),
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SidecarDeleteProof {
    identity: SourceFileIdentity,
    blake3: String,
}

fn sidecar_delete_proof(path: &Path) -> Result<SidecarDeleteProof> {
    Ok(SidecarDeleteProof {
        identity: source_file_identity(path)?,
        blake3: crate::common_utils::calculate_blake3_hash(path)
            .with_context(|| format!("failed to hash XMP sidecar {}", path.display()))?,
    })
}

fn delete_unchanged_sidecar(path: &Path, expected: &SidecarDeleteProof) -> Result<()> {
    let current = sidecar_delete_proof(path).with_context(|| {
        format!(
            "XMP merge succeeded but the sidecar cannot be revalidated for deletion; retained {}",
            path.display()
        )
    })?;
    if &current != expected {
        bail!(
            "XMP merge succeeded but the sidecar changed concurrently; retained {}",
            path.display()
        );
    }
    crate::io_utils::safe_remove_file(path).with_context(|| {
        format!(
            "XMP merge succeeded but the verified sidecar could not be deleted: {}",
            path.display()
        )
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ModernContainerProof {
    format: crate::image::format_detect::FormatKind,
    image_data_hash: String,
    payload_size: Option<u64>,
    width: Option<String>,
    height: Option<String>,
    frame_count: Option<String>,
    stable_metadata_hash: String,
    codec_feature_hash: Option<String>,
    has_xmp: bool,
}

fn proof_tag<'a>(record: &'a Map<String, Value>, suffix: &str) -> Option<&'a Value> {
    let suffix = suffix.to_ascii_lowercase();
    record.iter().find_map(|(key, value)| {
        let normalized = key.to_ascii_lowercase();
        (normalized == suffix || normalized.ends_with(&format!(":{suffix}"))).then_some(value)
    })
}

fn proof_value_text(value: &Value) -> Option<String> {
    let text = match value {
        Value::Null => return None,
        Value::String(text) => text.clone(),
        _ => canonical_proof_value(value),
    };
    let text = text.trim().to_string();
    (!text.is_empty()).then_some(text)
}

fn proof_has_xmp(record: &Map<String, Value>) -> bool {
    if let Some(value) = proof_tag(record, "hasxmp")
        && proof_value_text(value).is_some_and(|text| {
            text == "1" || text.eq_ignore_ascii_case("true") || text.eq_ignore_ascii_case("yes")
        })
    {
        return true;
    }
    record.iter().any(|(key, value)| {
        let normalized = key.to_ascii_lowercase();
        (normalized == "xmp"
            || normalized.starts_with("xmp:")
            || normalized.starts_with("xmp-")
            || normalized.contains(":xmp:")
            || normalized.contains(":xmp-"))
            && proof_value_text(value).is_some()
    })
}

fn modern_proof_key_ignored(key: &str) -> bool {
    let normalized = key.to_ascii_lowercase();
    normalized == "sourcefile"
        || normalized.starts_with("system:")
        || normalized.starts_with("macos:")
        || normalized.starts_with("exiftool:")
        || normalized.starts_with("composite:")
        || normalized.ends_with(":sourcefile")
        || normalized.ends_with(":filename")
        || normalized.ends_with(":filepath")
        || normalized.ends_with(":directory")
        || normalized.ends_with(":filesize")
        || normalized.contains("filemodifydate")
        || normalized.contains("fileaccessdate")
        || normalized.contains("filecreatedate")
        || normalized.contains("fileinodechangedate")
        || normalized.contains("processingtime")
        || normalized.contains("mediadataoffset")
        || normalized.contains("mediadatasize")
        || normalized.ends_with(":mediadata")
        || normalized == "imagedatahash"
        || normalized.ends_with(":imagedatahash")
        || normalized == "hasxmp"
        || normalized.ends_with(":hasxmp")
        || normalized == "xmp"
        || normalized.starts_with("xmp:")
        || normalized.starts_with("xmp-")
        || normalized.contains(":xmp:")
        || normalized.contains(":xmp-")
        || normalized.contains("xmlpacket")
}

fn canonical_proof_value(value: &Value) -> String {
    match value {
        Value::Null => "null".to_string(),
        Value::Bool(value) => value.to_string(),
        Value::Number(value) => value.to_string(),
        Value::String(value) => {
            serde_json::to_string(value).unwrap_or_else(|_| format!("{value:?}"))
        }
        Value::Array(values) => format!(
            "[{}]",
            values
                .iter()
                .map(canonical_proof_value)
                .collect::<Vec<_>>()
                .join(",")
        ),
        Value::Object(values) => {
            let mut keys = values.keys().collect::<Vec<_>>();
            keys.sort_unstable();
            format!(
                "{{{}}}",
                keys.into_iter()
                    .map(|key| {
                        let encoded_key =
                            serde_json::to_string(key).unwrap_or_else(|_| format!("{key:?}"));
                        format!("{encoded_key}:{}", canonical_proof_value(&values[key]))
                    })
                    .collect::<Vec<_>>()
                    .join(",")
            )
        }
    }
}

fn canonical_modern_proof_value(key: &str, value: &Value) -> Option<String> {
    let normalized = key.to_ascii_lowercase();
    if normalized == "file:filetype"
        && let Some(text) = proof_value_text(value)
    {
        if let Some(suffix) = text.strip_prefix("Extended WEBP") {
            return Some(format!("WEBP{suffix}"));
        }
        if text.starts_with("WEBP") {
            return Some(text);
        }
    }
    if normalized == "riff:webp_flags"
        && let Some(text) = proof_value_text(value)
    {
        let mut flags = text
            .split(',')
            .map(str::trim)
            .filter(|flag| !flag.eq_ignore_ascii_case("XMP") && !flag.is_empty())
            .collect::<Vec<_>>();
        flags.sort_unstable();
        return (!flags.is_empty()).then(|| flags.join(","));
    }
    Some(canonical_proof_value(value))
}

fn heif_archival_feature_hash_from_bytes(data: &[u8]) -> Result<String> {
    const CODEC_BOXES: [[u8; 4]; 3] = [*b"hvcC", *b"av1C", *b"vvcC"];
    const FEATURE_BOXES: [[u8; 4]; 15] = [
        *b"hvcC", *b"av1C", *b"vvcC", *b"colr", *b"pixi", *b"auxC", *b"dvcC", *b"dvvC", *b"mdcv",
        *b"clli", *b"clap", *b"pasp", *b"irot", *b"imir", *b"jumb",
    ];

    let mut hasher = blake3::Hasher::new();
    let mut has_codec = false;
    for box_type in FEATURE_BOXES {
        let payloads = crate::common_utils::find_all_box_data_recursive(data, box_type);
        has_codec |= CODEC_BOXES.contains(&box_type) && !payloads.is_empty();
        hasher.update(&box_type);
        hasher.update(
            &u64::try_from(payloads.len())
                .context("HEIF feature-box count does not fit u64")?
                .to_be_bytes(),
        );
        for payload in payloads {
            hasher.update(
                &u64::try_from(payload.len())
                    .context("HEIF feature-box length does not fit u64")?
                    .to_be_bytes(),
            );
            hasher.update(payload);
        }
    }
    anyhow::ensure!(
        has_codec,
        "HEIF archival proof found no supported codec configuration box"
    );
    Ok(hasher.finalize().to_hex().to_string())
}

fn heif_archival_feature_hash(path: &Path) -> Result<String> {
    let data = std::fs::read(path)
        .with_context(|| format!("failed to read HEIF archival proof {}", path.display()))?;
    heif_archival_feature_hash_from_bytes(&data)
}

fn jpeg_archival_feature_hash_from_bytes(data: &[u8]) -> Result<Option<String>> {
    let is_ultrahdr = crate::image_jpeg_analysis::is_ultra_hdr_jpeg(data);
    let has_mpf = crate::image_jpeg_analysis::find_mpf_segment(data).is_ok();
    if !is_ultrahdr {
        anyhow::ensure!(
            !has_mpf,
            "JPEG contains an MPF-linked secondary image that is not a proven UltraHDR gain map"
        );
        return Ok(None);
    }

    let payload = crate::image_jpeg_analysis::extract_ultrahdr_jpeg_payload(data)
        .map_err(anyhow::Error::msg)?;
    let params = crate::hdr::parse_gainmap_params_from_jpeg_xmp(data)?
        .ok_or_else(|| anyhow::anyhow!("UltraHDR JPEG has no readable gain-map parameters"))?;
    let mut hasher = blake3::Hasher::new();
    hasher.update(&payload.base_image.width().to_be_bytes());
    hasher.update(&payload.base_image.height().to_be_bytes());
    hasher.update(&payload.gainmap_image.width().to_be_bytes());
    hasher.update(&payload.gainmap_image.height().to_be_bytes());
    hasher.update(&payload.gainmap_jpeg);
    for value in [
        params.gain_map_max,
        params.gain_map_min,
        params.gamma,
        params.offset_sdr,
        params.offset_hdr,
    ] {
        hasher.update(&value.to_bits().to_be_bytes());
    }
    hasher.update(&[
        u8::from(params.use_base_color_space),
        u8::from(params.base_rendition_is_hdr),
    ]);
    Ok(Some(hasher.finalize().to_hex().to_string()))
}

fn jpeg_archival_feature_hash(path: &Path) -> Result<Option<String>> {
    let data = std::fs::read(path)
        .with_context(|| format!("failed to read JPEG archival proof {}", path.display()))?;
    jpeg_archival_feature_hash_from_bytes(&data)
}

const JP2_XMP_UUID: [u8; 16] = [
    0xbe, 0x7a, 0xcf, 0xcb, 0x97, 0xa9, 0x42, 0xe8, 0x9c, 0x71, 0x99, 0x94, 0x91, 0xe3, 0xaf, 0xac,
];

fn jp2_archival_feature_hash_from_bytes(data: &[u8]) -> Result<String> {
    let mut hasher = blake3::Hasher::new();
    let mut pos = 0_usize;
    let mut saw_signature = false;
    let mut saw_codestream = false;
    while pos < data.len() {
        anyhow::ensure!(data.len() - pos >= 8, "truncated JP2 box header");
        let size32 = u32::from_be_bytes(data[pos..pos + 4].try_into()?);
        let box_type: [u8; 4] = data[pos + 4..pos + 8].try_into()?;
        let (header_size, box_size) = match size32 {
            0 => (8_usize, data.len() - pos),
            1 => {
                anyhow::ensure!(data.len() - pos >= 16, "truncated JP2 extended box header");
                let size = usize::try_from(u64::from_be_bytes(data[pos + 8..pos + 16].try_into()?))
                    .context("JP2 extended box size does not fit usize")?;
                (16, size)
            }
            size => (
                8,
                usize::try_from(size).context("JP2 box size does not fit usize")?,
            ),
        };
        anyhow::ensure!(box_size >= header_size, "invalid JP2 box size");
        let next = pos
            .checked_add(box_size)
            .context("JP2 box boundary overflow")?;
        anyhow::ensure!(next <= data.len(), "JP2 box exceeds file boundary");
        let payload = &data[pos + header_size..next];
        let is_xmp = box_type == *b"uuid" && payload.starts_with(&JP2_XMP_UUID);
        if !is_xmp {
            hasher.update(&data[pos..next]);
        }
        saw_signature |= box_type == *b"jP  ";
        saw_codestream |= box_type == *b"jp2c";
        pos = next;
    }
    anyhow::ensure!(saw_signature, "JP2 archival proof found no signature box");
    anyhow::ensure!(saw_codestream, "JP2 archival proof found no codestream box");
    Ok(hasher.finalize().to_hex().to_string())
}

fn jp2_archival_feature_hash(path: &Path) -> Result<String> {
    let data = std::fs::read(path)
        .with_context(|| format!("failed to read JP2 archival proof {}", path.display()))?;
    jp2_archival_feature_hash_from_bytes(&data)
}

impl XmpMerger {
    #[must_use]
    pub const fn new(config: Config) -> Self {
        Self { config }
    }

    /// Check if exiftool is available on the system.
    ///
    /// # Errors
    /// Returns an error if exiftool is not found.
    pub fn check_exiftool() -> Result<()> {
        if !ExiftoolBuilder::check_available() {
            bail!("ExifTool not found. Install with: brew install exiftool");
        }
        Ok(())
    }

    /// Find all XMP files in a directory.
    ///
    /// # Errors
    /// Returns an error if directory traversal fails.
    pub fn find_xmp_files(&self, dir: &Path) -> Result<Vec<PathBuf>> {
        let mut xmp_files = Vec::new();

        for entry in WalkDir::new(dir).follow_links(false) {
            let entry = match entry {
                Ok(entry) => entry,
                Err(err) => {
                    crate::media_conversion_gate::delivery_metadata_batch_audit(
                        "delivery_metadata_xmp",
                        crate::infra::static_logs::messages::MSG_XMP_UNREADABLE
                            .replace("{}", &err.to_string()),
                    );
                    continue;
                }
            };

            let path = entry.path();
            if entry.file_type().is_file()
                && path
                    .extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("xmp"))
            {
                xmp_files.push(path.to_path_buf());
            }
        }

        Ok(xmp_files)
    }

    fn read_parent_paths(parent: &Path) -> Option<Vec<PathBuf>> {
        let entries = match std::fs::read_dir(parent) {
            Ok(entries) => entries,
            Err(err) => {
                crate::media_conversion_gate::delivery_metadata_path_audit(
                    "delivery_metadata_xmp",
                    parent,
                    format!(
                        "XMP Audit: Failed to read directory {}: {}",
                        parent.display(),
                        err
                    ),
                );
                return None;
            }
        };

        let mut paths = Vec::new();
        for entry in entries {
            match entry {
                Ok(entry) => paths.push(entry.path()),
                Err(err) => {
                    crate::media_conversion_gate::delivery_metadata_path_audit(
                        "delivery_metadata_xmp",
                        parent,
                        format!(
                            "XMP Audit: Failed to read directory entry in {}: {}",
                            parent.display(),
                            err
                        ),
                    );
                }
            }
        }

        Some(paths)
    }

    fn extract_xmp_metadata(xmp_path: &Path, _log_level: LogLevel) -> Result<XmpFile> {
        let xmp_data = std::fs::read(xmp_path)
            .with_context(|| format!("Failed to read XMP file: {}", xmp_path.display()))?;

        let mut xmp_info = XmpFile {
            path: xmp_path.to_path_buf(),
            document_id: None,
            derived_from: None,
            source: None,
        };

        let mut reader = quick_xml::reader::Reader::from_reader(xmp_data.as_slice());
        reader.config_mut().trim_text(true);

        loop {
            match reader.read_event() {
                Ok(Event::Start(e) | Event::Empty(e)) => {
                    for attr in e.attributes() {
                        let attr = attr.with_context(|| {
                            format!("Failed to parse XMP attribute in {}", xmp_path.display())
                        })?;
                        let local_name = attr.key.local_name();
                        let name_ref = local_name.as_ref();

                        let target = if name_ref.as_bytes().windows(10).any(|w| w == b"DocumentID")
                        {
                            Some("DocumentID")
                        } else if name_ref.as_bytes().windows(11).any(|w| w == b"DerivedFrom") {
                            Some("DerivedFrom")
                        } else if name_ref.as_bytes().windows(6).any(|w| w == b"Source") {
                            Some("Source")
                        } else {
                            None
                        };
                        let Some(target) = target else {
                            continue;
                        };

                        let unescaped = attr
                            .normalized_value(quick_xml::XmlVersion::Implicit1_0)
                            .with_context(|| {
                            format!(
                                "Failed to normalize XMP attribute {target} in {}",
                                xmp_path.display()
                            )
                        })?;
                        let val_str = unescaped.as_ref();
                        match target {
                            "DocumentID" => xmp_info.document_id = Some(val_str.to_string()),
                            "DerivedFrom" => xmp_info.derived_from = Some(val_str.to_string()),
                            "Source" => xmp_info.source = Some(val_str.to_string()),
                            _ => unreachable!("target enumerated above"),
                        }
                    }
                }
                Ok(Event::Eof) => break,
                Err(err) => {
                    bail!(
                        "Failed to parse XMP XML from {}: {}",
                        xmp_path.display(),
                        err
                    );
                }
                _ => (),
            }
        }

        // Fallback to exiftool only if native parsing found nothing
        if xmp_info.document_id.is_none()
            && xmp_info.derived_from.is_none()
            && xmp_info.source.is_none()
        {
            let mut command = crate::ExiftoolBuilder::new()
                .arg("-charset")
                .arg("filename=utf8")
                .arg("-api")
                .arg("windowsunicode=1")
                .arg("-api")
                .arg("LargeFileSupport=1")
                .arg("-s3")
                .arg("-DocumentID")
                .arg("-DerivedFrom")
                .arg("-Source")
                .arg("-OriginalDocumentID")
                .arg(exiftool_path_arg(xmp_path).as_ref())
                .build();
            let command_line = crate::common_utils::format_command_for_audit(&command);
            let output = command.output().context("Failed to run exiftool")?;
            crate::infra::logging::log_captured_process_output(
                &command_line,
                output.status,
                &String::from_utf8_lossy(&output.stdout),
                &String::from_utf8_lossy(&output.stderr),
            );

            if !output.status.success() {
                let diagnostic = crate::infra::logging::combined_tool_output(
                    &String::from_utf8_lossy(&output.stdout),
                    &String::from_utf8_lossy(&output.stderr),
                );
                bail!(
                    crate::infra::static_logs::messages::MSG_XMP_EXTRACT_FAIL
                        .replacen("{}", &xmp_path.display().to_string(), 1)
                        .replacen(
                            "{}",
                            if diagnostic.is_empty() {
                                "no diagnostic output"
                            } else {
                                diagnostic.as_str()
                            },
                            1,
                        )
                );
            }

            let stdout = String::from_utf8_lossy(&output.stdout);
            let lines: Vec<&str> = stdout.lines().collect();

            xmp_info.document_id = lines
                .first()
                .map(std::string::ToString::to_string)
                .filter(|s| !s.is_empty());
            xmp_info.derived_from = lines
                .get(1)
                .map(std::string::ToString::to_string)
                .filter(|s| !s.is_empty());
            xmp_info.source = lines
                .get(2)
                .map(std::string::ToString::to_string)
                .filter(|s| !s.is_empty());
        }

        Ok(xmp_info)
    }

    fn is_uuid_filename(name: &str) -> bool {
        let parts: Vec<&str> = name.split('-').collect();
        if parts.len() != 5 {
            return false;
        }
        let expected_lens = [8, 4, 4, 4, 12];
        parts
            .iter()
            .zip(expected_lens.iter())
            .all(|(part, &len)| part.len() == len && part.chars().all(|c| c.is_ascii_hexdigit()))
    }

    fn find_direct_match(xmp_path: &Path) -> Option<PathBuf> {
        let xmp_str = xmp_path.to_string_lossy();
        if xmp_str.to_lowercase().ends_with(".xmp") {
            let base = &xmp_str[..xmp_str.len() - 4];
            let base_path = PathBuf::from(base);
            if is_regular_non_symlink(&base_path) {
                return Some(base_path);
            }
        }
        None
    }

    fn find_same_name_different_ext(xmp_path: &Path) -> Option<PathBuf> {
        let parent = xmp_path.parent()?;
        let xmp_stem_raw = xmp_path.file_stem()?.to_string_lossy().to_lowercase();

        let xmp_root_stem = crate::media_conversion_gate::path_stem_root_segment(&xmp_stem_raw);

        for path in Self::read_parent_paths(parent)? {
            if !is_regular_non_symlink(&path) {
                continue;
            }

            let file_stem_raw = match path.file_stem() {
                Some(s) => s.to_string_lossy().to_lowercase(),
                None => continue,
            };

            let file_root_stem =
                crate::media_conversion_gate::path_stem_root_segment(&file_stem_raw);

            let ext = match path.extension() {
                Some(e) => e.to_string_lossy().to_lowercase(),
                None => continue,
            };

            if (file_stem_raw == xmp_stem_raw || file_root_stem == xmp_root_stem)
                && is_potential_media(&ext)
            {
                return Some(path);
            }
        }
        None
    }

    fn find_case_insensitive(xmp_path: &Path) -> Option<PathBuf> {
        let parent = xmp_path.parent()?;
        let xmp_stem = xmp_path.file_stem()?.to_string_lossy().to_lowercase();

        for path in Self::read_parent_paths(parent)? {
            if !is_regular_non_symlink(&path) {
                continue;
            }

            let file_stem = match path.file_stem() {
                Some(stem) => stem.to_string_lossy().to_lowercase(),
                None => continue,
            };
            let ext = match path.extension() {
                Some(e) => e.to_string_lossy().to_lowercase(),
                None => continue,
            };

            if file_stem == xmp_stem && is_potential_media(&ext) {
                return Some(path);
            }
        }
        None
    }

    fn find_fuzzy_match(xmp_path: &Path) -> Option<PathBuf> {
        let parent = xmp_path.parent()?;
        let stem = xmp_path.file_stem()?.to_string_lossy();

        let normalized_stem = Self::normalize_filename(&stem);
        let root_normalized_stem =
            Self::normalize_filename(crate::media_conversion_gate::path_stem_root_segment(&stem));

        if normalized_stem.is_empty() {
            return None;
        }

        for path in Self::read_parent_paths(parent)? {
            if !is_regular_non_symlink(&path) {
                continue;
            }

            let ext = match path.extension() {
                Some(e) => e.to_string_lossy().to_lowercase(),
                None => continue,
            };

            if !is_potential_media(&ext) {
                continue;
            }

            let file_stem = match path.file_stem() {
                Some(stem) => stem.to_string_lossy(),
                None => continue,
            };
            let normalized_file = Self::normalize_filename(&file_stem);
            let root_normalized_file = Self::normalize_filename(
                crate::media_conversion_gate::path_stem_root_segment(&file_stem),
            );

            if normalized_file == normalized_stem || root_normalized_file == root_normalized_stem {
                return Some(path);
            }
        }
        None
    }

    fn normalize_filename(name: &str) -> String {
        name.chars()
            .filter(|c| c.is_alphanumeric())
            .collect::<String>()
            .to_lowercase()
    }

    fn find_by_xmp_reference_scan(xmp_path: &Path) -> Option<PathBuf> {
        let parent = xmp_path.parent()?;
        let xmp_filename = xmp_path.file_name()?.to_string_lossy();

        for path in Self::read_parent_paths(parent)? {
            if !is_regular_non_symlink(&path) {
                continue;
            }

            let ext = match path.extension() {
                Some(e) => e.to_string_lossy().to_lowercase(),
                None => continue,
            };

            if !is_potential_media(&ext) {
                continue;
            }

            let mut command = crate::ExiftoolBuilder::new()
                .arg("-s3")
                .arg("-SidecarForExtension")
                .arg("-XMPFileRef")
                .arg(exiftool_path_arg(&path).as_ref())
                .build();
            let command_line = crate::common_utils::format_command_for_audit(&command);
            let output = match command.output() {
                Ok(output) => {
                    let stdout_summary = format!(
                        "<candidate metadata stdout omitted: {} bytes>",
                        output.stdout.len()
                    );
                    crate::infra::logging::log_captured_process_output(
                        &command_line,
                        output.status,
                        &stdout_summary,
                        &String::from_utf8_lossy(&output.stderr),
                    );
                    if !output.status.success() {
                        crate::media_conversion_gate::delivery_metadata_path_audit(
                            "delivery_metadata_xmp",
                            &path,
                            format!(
                                "XMP Audit: Sidecar failure for {}: status {}",
                                path.display(),
                                output.status
                            ),
                        );
                        continue;
                    }
                    output
                }
                Err(err) => {
                    crate::media_conversion_gate::delivery_metadata_path_audit(
                        "delivery_metadata_xmp",
                        &path,
                        format!("XMP Audit: Sidecar error for {}: {err}", path.display()),
                    );
                    continue;
                }
            };

            let stdout = String::from_utf8_lossy(&output.stdout);
            if stdout.contains(&*xmp_filename) {
                return Some(path);
            }
        }
        None
    }

    fn find_partial_match(xmp_path: &Path) -> Option<PathBuf> {
        let parent = xmp_path.parent()?;
        let stem = xmp_path.file_stem()?.to_string_lossy();

        if stem.len() < 4 {
            return None;
        }

        for path in Self::read_parent_paths(parent)? {
            if !is_regular_non_symlink(&path) {
                continue;
            }

            let ext = match path.extension() {
                Some(e) => e.to_string_lossy().to_lowercase(),
                None => continue,
            };

            if !is_potential_media(&ext) {
                continue;
            }

            let file_stem = match path.file_stem() {
                Some(stem) => stem.to_string_lossy(),
                None => continue,
            };

            if file_stem.contains(&*stem) || stem.contains(&*file_stem) {
                let shorter = std::cmp::min(stem.len(), file_stem.len());
                let longer = std::cmp::max(stem.len(), file_stem.len());
                if shorter * 100 / longer >= 70 {
                    return Some(path);
                }
            }
        }
        None
    }

    fn find_in_subdirectories(xmp_path: &Path) -> Option<PathBuf> {
        let parent = xmp_path.parent()?;
        let stem = xmp_path.file_stem()?.to_string_lossy();

        for entry in WalkDir::new(parent).max_depth(2) {
            let entry = match entry {
                Ok(entry) => entry,
                Err(err) => {
                    crate::media_conversion_gate::delivery_metadata_path_audit(
                        "delivery_metadata_xmp",
                        xmp_path,
                        format!(
                            "XMP Audit: Failed to enter subdirectory near {}: {err}",
                            xmp_path.display(),
                        ),
                    );
                    continue;
                }
            };

            let path = entry.path();
            if !is_regular_non_symlink(path) || path == xmp_path {
                continue;
            }

            let ext = match path.extension() {
                Some(e) => e.to_string_lossy().to_lowercase(),
                None => continue,
            };

            if !is_potential_media(&ext) {
                continue;
            }

            let file_stem = match path.file_stem() {
                Some(stem) => stem.to_string_lossy(),
                None => continue,
            };
            if file_stem.to_lowercase() == stem.to_lowercase() {
                return Some(path.to_path_buf());
            }
        }
        None
    }

    fn find_by_xmp_metadata(xmp_path: &Path, xmp_info: &XmpFile) -> Option<PathBuf> {
        let parent = xmp_path.parent()?;

        if let Some(ref derived) = xmp_info.derived_from
            && !derived.contains("uuid:")
        {
            let candidate = parent.join(derived);
            if candidate.is_file() {
                return Some(candidate);
            }
        }

        if let Some(ref source) = xmp_info.source {
            let candidate = parent.join(source);
            if candidate.is_file() {
                return Some(candidate);
            }
        }

        None
    }

    fn find_by_document_id(&self, xmp_path: &Path, xmp_info: &XmpFile) -> Option<PathBuf> {
        let parent = xmp_path.parent()?;
        let xmp_doc_id = xmp_info.document_id.as_ref()?;

        let stem = xmp_path.file_stem()?.to_string_lossy();
        if !Self::is_uuid_filename(&stem) {
            return None;
        }

        if matches!(self.config.log_level, LogLevel::Verbose) {
            crate::log_detail!(
                &crate::infra::static_logs::messages::MSG_XMP_DOC_ID_SCAN.replace("{}", xmp_doc_id)
            );
        }

        for path in Self::read_parent_paths(parent)? {
            if !is_regular_non_symlink(&path) {
                continue;
            }

            let ext = match path.extension() {
                Some(ext) => ext.to_string_lossy().to_lowercase(),
                None => continue,
            };
            if !is_potential_media(&ext) {
                continue;
            }

            let mut command = crate::ExiftoolBuilder::new()
                .arg("-s3")
                .arg("-DocumentID")
                .arg(exiftool_path_arg(&path).as_ref())
                .build();
            let command_line = crate::common_utils::format_command_for_audit(&command);
            let output = match command.output() {
                Ok(output) => {
                    let stdout_summary = format!(
                        "<candidate DocumentID stdout omitted: {} bytes>",
                        output.stdout.len()
                    );
                    crate::infra::logging::log_captured_process_output(
                        &command_line,
                        output.status,
                        &stdout_summary,
                        &String::from_utf8_lossy(&output.stderr),
                    );
                    if !output.status.success() {
                        crate::media_conversion_gate::delivery_metadata_path_audit(
                            "delivery_metadata_xmp",
                            &path,
                            format!(
                                "XMP Audit: Failed to extract DocumentID from {}: status {}",
                                path.display(),
                                output.status
                            ),
                        );
                        continue;
                    }
                    output
                }
                Err(err) => {
                    crate::media_conversion_gate::delivery_metadata_path_audit(
                        "delivery_metadata_xmp",
                        &path,
                        format!("XMP Audit: DocumentID error for {}: {err}", path.display()),
                    );
                    continue;
                }
            };

            let media_doc_id = String::from_utf8_lossy(&output.stdout).trim().to_string();

            if !media_doc_id.is_empty() && media_doc_id == *xmp_doc_id {
                if matches!(self.config.log_level, LogLevel::Verbose) {
                    crate::log_detail!(
                        &crate::infra::static_logs::messages::MSG_XMP_MATCH_FOUND
                            .replace("{}", &path.display().to_string())
                    );
                }
                return Some(path);
            }
        }

        None
    }

    /// Find the media file corresponding to an XMP file.
    ///
    /// # Errors
    /// Returns an error if searching fails.
    // Rationale: This function handles complex, sequential initialization or
    // business logic where further fragmentation would hinder readability and
    // maintainability.
    pub fn find_media_file(&self, xmp_path: &Path) -> Result<(Option<PathBuf>, String)> {
        if matches!(self.config.log_level, LogLevel::Verbose) {
            crate::log_info!(
                crate::infra::static_logs::messages::LABEL_XMP,
                &crate::infra::static_logs::messages::MSG_XMP_FIND_MATCH
                    .replace("{}", &xmp_path.display().to_string())
            );
        }

        if let Some(media) = Self::find_direct_match(xmp_path) {
            if matches!(self.config.log_level, LogLevel::Verbose) {
                crate::log_detail!(
                    &crate::infra::static_logs::messages::MSG_XMP_STRATEGY_1
                        .replace("{}", &media.display().to_string())
                );
            }

            return Ok((Some(media), "direct_match".to_string()));
        }

        if let Some(media) = Self::find_same_name_different_ext(xmp_path) {
            if matches!(self.config.log_level, LogLevel::Verbose) {
                crate::log_detail!(
                    &crate::infra::static_logs::messages::MSG_XMP_STRATEGY_2
                        .replace("{}", &media.display().to_string())
                );
            }
            return Ok((Some(media), "same_name".to_string()));
        }

        if let Some(media) = Self::find_case_insensitive(xmp_path) {
            if matches!(self.config.log_level, LogLevel::Verbose) {
                crate::log_info!(
                    crate::infra::static_logs::messages::LABEL_XMP,
                    &crate::infra::static_logs::messages::MSG_XMP_STRATEGY_2_5
                        .replace("{}", &media.display().to_string())
                );
            }
            return Ok((Some(media), "case_insensitive".to_string()));
        }

        let xmp_info = Self::extract_xmp_metadata(xmp_path, self.config.log_level)?;

        if let Some(media) = Self::find_by_xmp_metadata(xmp_path, &xmp_info) {
            if matches!(self.config.log_level, LogLevel::Verbose) {
                crate::log_info!(
                    crate::infra::static_logs::messages::LABEL_XMP,
                    &crate::infra::static_logs::messages::MSG_XMP_STRATEGY_3
                        .replace("{}", &media.display().to_string())
                );
            }
            return Ok((Some(media), "xmp_metadata".to_string()));
        }

        if let Some(media) = self.find_by_document_id(xmp_path, &xmp_info) {
            if matches!(self.config.log_level, LogLevel::Verbose) {
                crate::log_info!(
                    crate::infra::static_logs::messages::LABEL_XMP,
                    &crate::infra::static_logs::messages::MSG_XMP_STRATEGY_4
                        .replace("{}", &media.display().to_string())
                );
            }
            return Ok((Some(media), "document_id".to_string()));
        }

        if let Some(media) = Self::find_fuzzy_match(xmp_path) {
            if matches!(self.config.log_level, LogLevel::Verbose) {
                crate::log_detail!(
                    &crate::infra::static_logs::messages::MSG_XMP_STRATEGY_5
                        .replace("{}", &media.display().to_string())
                );
            }
            return Ok((Some(media), "fuzzy_match".to_string()));
        }

        if let Some(media) = Self::find_by_xmp_reference_scan(xmp_path) {
            if matches!(self.config.log_level, LogLevel::Verbose) {
                crate::log_info!(
                    crate::infra::static_logs::messages::LABEL_XMP,
                    &crate::infra::static_logs::messages::MSG_XMP_STRATEGY_6
                        .replace("{}", &media.display().to_string())
                );
            }
            return Ok((Some(media), "xmp_ref_scan".to_string()));
        }

        if let Some(media) = Self::find_partial_match(xmp_path) {
            if matches!(self.config.log_level, LogLevel::Verbose) {
                crate::log_info!(
                    crate::infra::static_logs::messages::LABEL_XMP,
                    &crate::infra::static_logs::messages::MSG_XMP_STRATEGY_7
                        .replace("{}", &media.display().to_string())
                );
            }
            return Ok((Some(media), "partial_match".to_string()));
        }

        if let Some(media) = Self::find_in_subdirectories(xmp_path) {
            if matches!(self.config.log_level, LogLevel::Verbose) {
                crate::log_info!(
                    crate::infra::static_logs::messages::LABEL_XMP,
                    &crate::infra::static_logs::messages::MSG_XMP_STRATEGY_8
                        .replace("{}", &media.display().to_string())
                );
            }
            return Ok((Some(media), "subdirectory".to_string()));
        }

        if matches!(self.config.log_level, LogLevel::Verbose) {
            crate::media_conversion_gate::delivery_metadata_batch_audit(
                "delivery_metadata_xmp",
                crate::infra::static_logs::messages::MSG_XMP_NO_MATCH,
            );
        }
        Ok((None, "no_match".to_string()))
    }

    /// Capture the semantic state of a modern container before or after a
    /// metadata-only write. The proof deliberately excludes only fields that
    /// `ExifTool` is expected to change when a box/chunk is inserted (file paths,
    /// timestamps, offsets and XMP itself); codec payload, dimensions, frame
    /// count and all other reported container properties remain covered.
    fn capture_modern_container_proof(
        path: &Path,
        format: crate::image::format_detect::FormatKind,
    ) -> Result<ModernContainerProof> {
        let mut builder = crate::ExiftoolBuilder::new();
        builder
            .arg("-j")
            .arg("-G1")
            .arg("-a")
            .arg("-s")
            .arg("-u")
            .arg("-U")
            .arg("-api")
            .arg("RequestAll=3")
            .arg("-all")
            .arg("-ImageDataHash")
            .arg("-ImageWidth")
            .arg("-ImageHeight")
            .arg("-FrameCount")
            .arg("-HasXMP")
            .arg(safe_path_arg(path).as_ref());
        let mut command = builder.build();
        let output = crate::convert::process_runner::ManagedProcess::spawn_captured(&mut command)?
            .wait_liveness_timeout(
                Duration::from_secs(120),
                Duration::from_secs(300),
                "modern container metadata proof",
            )?;
        crate::infra::logging::log_captured_process_output(
            &output.command_line,
            output.status,
            &output.stdout,
            &output.stderr,
        );
        if !output.status.success() {
            let diagnostic =
                crate::infra::logging::combined_tool_output(&output.stdout, &output.stderr);
            bail!(
                "ExifTool modern-container proof failed with {}: {}",
                output.status,
                if diagnostic.is_empty() {
                    "no diagnostic output"
                } else {
                    diagnostic.as_str()
                }
            );
        }

        let document: Value = serde_json::from_str(&output.stdout)
            .context("ExifTool modern-container proof returned invalid JSON")?;
        let record = document
            .as_array()
            .and_then(|items| items.first())
            .and_then(Value::as_object)
            .ok_or_else(|| anyhow::anyhow!("ExifTool modern-container proof returned no record"))?;
        let image_data_hash = proof_tag(record, "imagedatahash")
            .and_then(proof_value_text)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| anyhow::anyhow!("ExifTool proof did not report ImageDataHash"))?;
        let width = proof_tag(record, "imagewidth").and_then(proof_value_text);
        let height = proof_tag(record, "imageheight").and_then(proof_value_text);
        let frame_count = proof_tag(record, "framecount").and_then(proof_value_text);
        let has_xmp = proof_has_xmp(record);
        let mut stable = Vec::new();
        for (key, value) in record {
            if !modern_proof_key_ignored(key)
                && let Some(value) = canonical_modern_proof_value(key, value)
            {
                stable.push((key.as_str(), value));
            }
        }
        stable.sort_by(|left, right| left.0.cmp(right.0));
        let mut stable_bytes = Vec::new();
        for (key, value) in stable {
            stable_bytes.extend_from_slice(key.as_bytes());
            stable_bytes.push(0);
            stable_bytes.extend_from_slice(value.as_bytes());
            stable_bytes.push(0xff);
        }
        let stable_metadata_hash = blake3::hash(&stable_bytes).to_hex().to_string();
        let codec_feature_hash = match format {
            crate::image::format_detect::FormatKind::Jpeg => jpeg_archival_feature_hash(path)?,
            crate::image::format_detect::FormatKind::Avif => Some(
                crate::image::fast_img::avif_codec_feature_hash(path)
                    .map_err(|error| anyhow::anyhow!(error.to_string()))?,
            ),
            crate::image::format_detect::FormatKind::Heic
            | crate::image::format_detect::FormatKind::Heif => {
                Some(heif_archival_feature_hash(path)?)
            }
            crate::image::format_detect::FormatKind::WebP => {
                Some(crate::image_formats::webp::archival_feature_hash(path)?)
            }
            crate::image::format_detect::FormatKind::Jp2 => Some(jp2_archival_feature_hash(path)?),
            _ => None,
        };
        // ISOBMFF stores XMP as an item in `mdat`; its aggregate mdat size must grow.
        // ImageDataHash is the exact primary-image payload proof for these formats.
        let payload_size = match format {
            crate::image::format_detect::FormatKind::Avif
            | crate::image::format_detect::FormatKind::Heic
            | crate::image::format_detect::FormatKind::Heif => None,
            _ => Some(
                crate::image::static_payload::measure_as(path, format)
                    .context("failed to measure immutable modern-container payload")?,
            ),
        };

        Ok(ModernContainerProof {
            format,
            image_data_hash,
            payload_size,
            width,
            height,
            frame_count,
            stable_metadata_hash,
            codec_feature_hash,
            has_xmp,
        })
    }

    /// Merge a sidecar into AVIF/HEIC/WebP/JP2 using the format-aware `ExifTool`
    /// writer, then commit only after a before/after proof succeeds. This keeps
    /// the positive native path for modern containers while retaining a strict
    /// fail-closed outcome for writers that drop auxiliary or unknown data.
    fn merge_modern_xmp_with_proof(
        xmp_path: &Path,
        media_path: &Path,
        format: crate::image::format_detect::FormatKind,
    ) -> Result<()> {
        Self::check_exiftool()?;
        let source_identity = source_file_identity(media_path)?;
        let source_hash = crate::common_utils::calculate_blake3_hash(media_path)
            .context("failed to hash modern container before XMP merge")?;
        let xmp_hash = crate::common_utils::calculate_blake3_hash(xmp_path)
            .context("failed to hash XMP sidecar before modern merge")?;
        let before = Self::capture_modern_container_proof(media_path, format)?;
        let parent = crate::media_conversion_gate::output_parent_or_dot(media_path);
        let staged = crate::media_conversion_gate::delivery_named_tempfile_in_parent_or_err(
            "modern_xmp_native_merge",
            parent,
            ".mfb-modern-xmp-",
            ".tmp",
        )?;
        let staged_path = staged.into_temp_path();
        let copied = std::fs::copy(media_path, &staged_path)
            .context("failed to stage modern container for native XMP merge")?;
        if copied != source_identity.len {
            bail!(
                "modern container staging length mismatch: expected={} actual={}",
                source_identity.len,
                copied
            );
        }
        let staged_hash = crate::common_utils::calculate_blake3_hash(&staged_path)
            .context("failed to hash staged modern container")?;
        if staged_hash != source_hash {
            bail!("modern container changed while staging native XMP merge");
        }

        let mut builder = crate::ExiftoolBuilder::new();
        builder
            .tags_from_file(xmp_path)
            .arg("-XMP:all")
            .preserve_date()
            .overwrite_original()
            .arg(safe_path_arg(&staged_path).as_ref());
        let mut command = builder.build();
        let output = crate::convert::process_runner::ManagedProcess::spawn_captured(&mut command)?
            .wait_liveness_timeout(
                Duration::from_secs(120),
                Duration::from_secs(300),
                "modern container native XMP merge",
            )?;
        crate::infra::logging::log_captured_process_output(
            &output.command_line,
            output.status,
            &output.stdout,
            &output.stderr,
        );
        if !output.status.success() {
            let diagnostic =
                crate::infra::logging::combined_tool_output(&output.stdout, &output.stderr);
            bail!(
                "native modern-container XMP merge failed with {}: {}",
                output.status,
                if diagnostic.is_empty() {
                    "no diagnostic output"
                } else {
                    diagnostic.as_str()
                }
            );
        }

        let after = Self::capture_modern_container_proof(&staged_path, format)?;
        if after.format != before.format
            || after.image_data_hash != before.image_data_hash
            || after.width != before.width
            || after.height != before.height
            || after.frame_count != before.frame_count
            || after.stable_metadata_hash != before.stable_metadata_hash
            || after.codec_feature_hash != before.codec_feature_hash
            || !after.has_xmp
            || before.payload_size != after.payload_size
        {
            bail!(
                "native modern-container XMP merge failed archival proof; original media and sidecar retained; before={before:?}; after={after:?}"
            );
        }
        let current_hash = crate::common_utils::calculate_blake3_hash(media_path)
            .context("failed to re-hash modern container before commit")?;
        let current_identity = source_file_identity(media_path)?;
        let current_xmp_hash = crate::common_utils::calculate_blake3_hash(xmp_path)
            .context("failed to re-hash XMP sidecar before commit")?;
        if current_hash != source_hash || current_identity != source_identity {
            bail!("modern container changed concurrently during native XMP merge");
        }
        if current_xmp_hash != xmp_hash {
            bail!("XMP sidecar changed concurrently during native modern merge");
        }
        let metadata_report =
            crate::metadata::preserve_filesystem_for_delivery(media_path, &staged_path)?;
        if matches!(
            metadata_report.xattr,
            crate::metadata::MetadataLayerOutcome::PartialAudit
        ) || matches!(
            metadata_report.timestamps,
            crate::metadata::MetadataLayerOutcome::PartialAudit
        ) {
            bail!("modern-container filesystem metadata proof was partial");
        }
        crate::io_utils::sync_committed_file_and_parent(&staged_path)?;
        staged_path.persist(media_path).map_err(|error| {
            anyhow::anyhow!(
                "failed to atomically commit native modern-container XMP merge: {}",
                error.error
            )
        })?;
        crate::io_utils::sync_committed_file_and_parent(media_path)
            .context("failed to flush native modern-container XMP merge")?;
        crate::log_info!(
            crate::infra::static_logs::messages::LABEL_XMP,
            &format!(
                "Native XMP merge committed with payload and container proof: {}",
                media_path.display()
            )
        );
        Ok(())
    }

    /// Merge XMP metadata into a media file.
    ///
    /// # Errors
    /// Returns an error if merging fails.
    pub fn merge_xmp(&self, xmp_path: &Path, media_path: &Path) -> Result<()> {
        if Self::merge_jxl_xmp_overlay(xmp_path, media_path)? {
            return Ok(());
        }
        let format = crate::image::format_detect::detect_true_format(media_path).map_err(|error| {
            anyhow::anyhow!(
                "refusing XMP merge because destination format cannot be proved for {}: {error}",
                media_path.display()
            )
        })?;
        if let Some(reason) = protected_container_reason(media_path, format)? {
            bail!(
                "refusing destructive XMP rewrite for {} because it contains a protected {reason}; media and sidecar retained so protected container structure is not invalidated or discarded",
                media_path.display()
            );
        }
        if matches!(
            format,
            crate::image::format_detect::FormatKind::Jpeg
                | crate::image::format_detect::FormatKind::Avif
                | crate::image::format_detect::FormatKind::Heic
                | crate::image::format_detect::FormatKind::Heif
                | crate::image::format_detect::FormatKind::WebP
                | crate::image::format_detect::FormatKind::Jp2
        ) {
            return Self::merge_modern_xmp_with_proof(xmp_path, media_path, format);
        }
        if xmp_rewrite_requires_immutable_container(format) {
            bail!(
                "refusing destructive XMP rewrite for {format:?} {}; media and sidecar retained because a generic metadata writer cannot prove preservation of HDR/auxiliary relationships, provenance data, unknown container structures, and codec bytes",
                media_path.display()
            );
        }
        Self::check_exiftool()?;
        match self.merge_xmp_core(xmp_path, media_path) {
            Ok(()) => Ok(()),
            Err(e) => {
                let err_str = e.to_string();
                let hint = crate::extract_suggested_extension(&err_str);

                if let Some(ref h) = hint {
                    crate::log_info!(
                        crate::infra::static_logs::messages::LABEL_XMP,
                        &format!("ExifTool suggests content is: {h}")
                    );
                }

                self.merge_xmp_fallback(xmp_path, media_path, hint.as_deref())
            }
        }
    }

    fn merge_jxl_xmp_overlay(xmp_path: &Path, media_path: &Path) -> Result<bool> {
        use crate::image::jxl_utils::JpegReconstructionEligibility;
        use crate::metadata::MetadataLayerOutcome;

        let format =
            crate::image::format_detect::detect_true_format(media_path).map_err(|error| {
                anyhow::anyhow!(
                    "failed to identify media before XMP merge for {}: {error}",
                    media_path.display()
                )
            })?;
        if format != crate::image::format_detect::FormatKind::Jxl {
            return Ok(false);
        }
        if !is_jxl_container(media_path)? {
            bail!(
                "refusing XMP merge for raw JPEG XL codestream {}; append-only metadata overlays require a JPEG XL container so codec and JBRD bytes remain immutable",
                media_path.display()
            );
        }

        let eligibility = crate::image::jxl_utils::probe_jpeg_reconstruction_eligibility(
            media_path,
        )
        .map_err(|error| {
            anyhow::anyhow!(
                "refusing JXL XMP merge without a reconstruction-state proof for {}: {error}",
                media_path.display()
            )
        })?;
        let baseline_jpeg = if matches!(&eligibility, JpegReconstructionEligibility::Exact) {
            Some(reconstruct_jpeg_to_temp(media_path)?)
        } else {
            None
        };
        let source_identity = source_file_identity(media_path)?;
        let source_hash =
            crate::common_utils::calculate_blake3_hash(media_path).with_context(|| {
                format!(
                    "failed to hash JXL before XMP merge: {}",
                    media_path.display()
                )
            })?;
        let source_size = source_identity.len;
        let parent = crate::media_conversion_gate::output_parent_or_dot(media_path);
        let staged = crate::media_conversion_gate::delivery_named_tempfile_in_parent_or_err(
            "jxl_xmp_overlay",
            parent,
            ".mfb-jxl-xmp-",
            ".tmp",
        )?;
        let staged_path = staged.into_temp_path();
        let copied = std::fs::copy(media_path, &staged_path)?;
        if copied != source_size {
            bail!(
                "JXL staging copy changed length before XMP merge for {}: expected={source_size} actual={copied}",
                media_path.display()
            );
        }

        if !crate::metadata::append_xmp_overlay_to_jxl(xmp_path, &staged_path)? {
            crate::log_info!(
                crate::infra::static_logs::messages::LABEL_XMP,
                &format!(
                    "JXL XMP overlay already current; no container rewrite: {}",
                    media_path.display()
                )
            );
            return Ok(true);
        }

        if let Some(baseline_jpeg) = baseline_jpeg.as_ref() {
            crate::image::fast_img::verify_jxl_roundtrip_integrity(
                baseline_jpeg.path(),
                &staged_path,
            )
            .map_err(|error| {
                anyhow::anyhow!(
                    "refusing XMP overlay because exact JPEG reconstruction changed for {}: {error}",
                    media_path.display()
                )
            })?;
        } else {
            let staged_eligibility =
                crate::image::jxl_utils::probe_jpeg_reconstruction_eligibility(&staged_path)
                    .map_err(|error| {
                        anyhow::anyhow!(
                            "JXL became unreadable while staging XMP overlay for {}: {error}",
                            media_path.display()
                        )
                    })?;
            let classification_preserved = matches!(
                (&eligibility, &staged_eligibility),
                (
                    JpegReconstructionEligibility::PixelOnly,
                    JpegReconstructionEligibility::PixelOnly
                ) | (
                    JpegReconstructionEligibility::AdvertisedButRejected { .. },
                    JpegReconstructionEligibility::AdvertisedButRejected { .. }
                )
            );
            if !classification_preserved {
                bail!(
                    "JXL reconstruction state changed while staging XMP overlay for {}",
                    media_path.display()
                );
            }
        }

        let metadata_report =
            crate::metadata::preserve_filesystem_for_delivery(media_path, &staged_path)?;
        if matches!(metadata_report.xattr, MetadataLayerOutcome::PartialAudit)
            || matches!(
                metadata_report.timestamps,
                MetadataLayerOutcome::PartialAudit
            )
        {
            bail!(
                "filesystem metadata preservation was partial while staging XMP overlay for {}",
                media_path.display()
            );
        }
        let current_hash =
            crate::common_utils::calculate_blake3_hash(media_path).with_context(|| {
                format!(
                    "failed to re-hash JXL before committing XMP overlay: {}",
                    media_path.display()
                )
            })?;
        if current_hash != source_hash {
            bail!(
                "JXL changed concurrently during XMP merge; original retained: {}",
                media_path.display()
            );
        }
        let current_identity = source_file_identity(media_path)?;
        if current_identity != source_identity {
            bail!(
                "JXL file identity changed concurrently during XMP merge; original retained: {}",
                media_path.display()
            );
        }
        std::fs::File::open(&staged_path)
            .with_context(|| format!("failed to open staged JXL {}", staged_path.display()))?
            .sync_all()
            .with_context(|| format!("failed to flush staged JXL {}", staged_path.display()))?;
        staged_path.persist(media_path).map_err(|error| {
            anyhow::anyhow!(
                "failed to atomically commit XMP overlay to {}: {}",
                media_path.display(),
                error.error
            )
        })?;
        crate::io_utils::sync_committed_file_and_parent(media_path).with_context(|| {
            format!(
                "failed to durably commit XMP overlay to {}",
                media_path.display()
            )
        })?;
        let reconstructed_jpeg_hash = baseline_jpeg
            .as_ref()
            .map(|jpeg| crate::common_utils::calculate_blake3_hash(jpeg.path()))
            .transpose()
            .context("failed to hash exact JPEG reconstruction for XMP audit")?;
        crate::metadata::audit_jxl_overlay_reconstruction_proof(
            media_path,
            reconstructed_jpeg_hash.as_deref(),
        )?;
        crate::log_info!(
            crate::infra::static_logs::messages::LABEL_XMP,
            &format!(
                "JXL XMP overlay committed without rewriting codec/JBRD bytes; reconstruction state verified: {}",
                media_path.display()
            )
        );
        Ok(true)
    }

    fn merge_xmp_core(&self, xmp_path: &Path, media_path: &Path) -> Result<()> {
        let original_timestamps = Self::get_file_timestamps(media_path);
        let xmp_timestamps = Self::get_file_timestamps(xmp_path);

        let xmp_file = std::fs::File::open(xmp_path)
            .with_context(|| format!("Failed to open XMP file: {}", xmp_path.display()))?;

        let mut builder = crate::ExiftoolBuilder::new();
        builder
            .use_stdin()
            .preserve_date()
            .arg("-charset")
            .arg("filename=utf8")
            .arg("-api")
            .arg("windowsunicode=1")
            .arg("-api")
            .arg("LargeFileSupport=1")
            .tags_from_file("-")
            .arg("-all:all")
            .unsafe_tags()
            .arg("-FileModifyDate<FileModifyDate")
            .arg(safe_path_arg(media_path).as_ref());

        if matches!(self.config.overwrite_mode, OverwriteMode::Original) {
            builder.overwrite_original();
        }

        let mut cmd = builder.build();
        cmd.stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());

        let mut child = cmd
            .spawn()
            .context("Failed to spawn exiftool merge process")?;

        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| anyhow::anyhow!("Failed to open stdin for exiftool"))?;

        let mut reader = std::io::BufReader::new(xmp_file);
        std::io::copy(&mut reader, &mut stdin).context("Failed to stream XMP to exiftool stdin")?;
        drop(stdin); // Close stdin to signal EOF

        let output = child
            .wait_with_output()
            .context("Failed to wait for exiftool merge")?;

        let command_line = crate::common_utils::format_command_for_audit(&cmd);
        crate::infra::logging::log_captured_process_output(
            &command_line,
            output.status,
            &String::from_utf8_lossy(&output.stdout),
            &String::from_utf8_lossy(&output.stderr),
        );

        if !output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            let diagnostic = crate::infra::logging::combined_tool_output(&stdout, &stderr);
            bail!(
                "ExifTool merge failed with {}: {}",
                output.status,
                if diagnostic.is_empty() {
                    "no diagnostic output"
                } else {
                    diagnostic.as_str()
                }
            );
        }

        if self.config.preserve_timestamps {
            Self::restore_timestamps(media_path, original_timestamps, xmp_timestamps);
        }

        Ok(())
    }

    fn merge_xmp_fallback(
        &self,
        xmp_path: &Path,
        media_path: &Path,
        hint_ext: Option<&str>,
    ) -> Result<()> {
        let xmp_filename =
            crate::media_conversion_gate::path_file_name_or_empty(xmp_path, "xmp_merger:fallback");

        let detected_ext = match hint_ext {
            None => crate::common_utils::detect_real_extension(media_path)
                .map(std::string::ToString::to_string),
            Some(hint) => Some(hint.to_string()),
        };

        let implied_ext = if xmp_filename.to_lowercase().ends_with(".xmp") {
            let stem = &xmp_filename[..xmp_filename.len() - 4];
            Path::new(stem).extension().and_then(|e| e.to_str())
        } else {
            None
        };

        let target_ext = match detected_ext {
            Some(v) => Some(v),
            None => implied_ext.map(std::string::ToString::to_string),
        };

        let Some(original_ext) = target_ext else {
            return self.merge_xmp_core(xmp_path, media_path);
        };

        let current_ext =
            crate::media_conversion_gate::path_extension_lowercase_or_empty_unchecked(media_path);

        if original_ext.eq_ignore_ascii_case(&current_ext) {
            return self.merge_xmp_core(xmp_path, media_path);
        }

        crate::log_info!(
            crate::infra::static_logs::messages::LABEL_XMP,
            &format!(
                "Merge failed, attempting fallback: Temporary rename to .{original_ext} for \
                 merge..."
            )
        );

        let temp_path = media_path.with_extension(&original_ext);

        if temp_path.exists() {
            crate::media_conversion_gate::delivery_metadata_batch_audit(
                "delivery_metadata_xmp",
                format!(
                    "Fallback aborted: Temporary target {path} already exists",
                    path = temp_path.display()
                ),
            );
            return self.merge_xmp_core(xmp_path, media_path);
        }

        std::fs::rename(media_path, &temp_path)
            .context("Fallback: Failed to rename for temporary merge")?;

        let merge_result = self.merge_xmp_core(xmp_path, &temp_path);

        if let Err(e) = std::fs::rename(&temp_path, media_path) {
            crate::media_conversion_gate::delivery_metadata_path_audit(
                "delivery_metadata_xmp",
                media_path,
                format!(
                    "CRITICAL: Failed to restore filename from {src} to {dst}: {e}",
                    src = temp_path.display(),
                    dst = media_path.display()
                ),
            );
            bail!("Critical: Failed to restore filename after fallback merge");
        }

        match merge_result {
            Ok(()) => {
                crate::log_info!(
                    crate::infra::static_logs::messages::LABEL_XMP,
                    "Fallback merge successful"
                );
                Ok(())
            }
            Err(e) => {
                crate::media_conversion_gate::delivery_metadata_batch_audit(
                    "delivery_metadata_xmp",
                    format!("Fallback merge failed: {e}"),
                );
                Err(e)
            }
        }
    }

    fn get_file_timestamps(path: &Path) -> Option<(filetime::FileTime, filetime::FileTime)> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            match std::fs::metadata(path) {
                Ok(meta) => {
                    let atime = filetime::FileTime::from_unix_time(meta.atime(), 0);
                    let mtime = filetime::FileTime::from_unix_time(meta.mtime(), 0);
                    return Some((atime, mtime));
                }
                Err(err) => {
                    crate::media_conversion_gate::delivery_metadata_path_audit(
                        "xmp_timestamp_probe",
                        path,
                        format!("failed to read source timestamps before XMP merge: {err}"),
                    );
                }
            }
        }
        #[cfg(not(unix))]
        {
            match std::fs::metadata(path) {
                Ok(meta) => match meta.modified() {
                    Ok(modified) => {
                        let mtime = filetime::FileTime::from_system_time(modified);
                        return Some((mtime, mtime));
                    }
                    Err(err) => {
                        crate::media_conversion_gate::delivery_metadata_path_audit(
                            "xmp_timestamp_probe",
                            path,
                            format!("failed to read source modified time before XMP merge: {err}"),
                        );
                    }
                },
                Err(err) => {
                    crate::media_conversion_gate::delivery_metadata_path_audit(
                        "xmp_timestamp_probe",
                        path,
                        format!("failed to read source timestamps before XMP merge: {err}"),
                    );
                }
            }
        }
        None
    }

    fn restore_timestamps(
        media_path: &Path,
        original: Option<(filetime::FileTime, filetime::FileTime)>,
        _xmp: Option<(filetime::FileTime, filetime::FileTime)>,
    ) {
        if let Some((atime, mtime)) = original
            && let Err(e) = filetime::set_file_times(media_path, atime, mtime)
        {
            crate::media_conversion_gate::delivery_metadata_path_audit(
                "delivery_metadata_xmp",
                media_path,
                format!(
                    "Failed to restore timestamp for {asset}: {e}",
                    asset = media_path.display()
                ),
            );
        }
    }

    #[must_use]
    pub fn process_xmp(&self, xmp_path: &Path) -> MergeResult {
        let (media_path, strategy) = match self.find_media_file(xmp_path) {
            Ok((path, strat)) => (path, strat),
            Err(e) => {
                return MergeResult {
                    xmp_path: xmp_path.to_path_buf(),
                    media_path: None,
                    success: false,
                    message: format!("Error finding media: {e}"),
                    match_strategy: None,
                };
            }
        };

        let Some(media) = media_path else {
            return MergeResult {
                xmp_path: xmp_path.to_path_buf(),
                media_path: None,
                success: false,
                message: "No matching media file found".to_string(),
                match_strategy: Some(strategy),
            };
        };

        let delete_proof = if self.config.delete_xmp_after_merge {
            match sidecar_delete_proof(xmp_path) {
                Ok(proof) => Some(proof),
                Err(error) => {
                    return MergeResult {
                        xmp_path: xmp_path.to_path_buf(),
                        media_path: Some(media),
                        success: false,
                        message: format!(
                            "Refusing merge with requested sidecar cleanup because its preflight proof failed: {error:#}"
                        ),
                        match_strategy: Some(strategy),
                    };
                }
            }
        } else {
            None
        };

        match self.merge_xmp(xmp_path, &media) {
            Ok(()) => {
                if let Some(proof) = delete_proof.as_ref()
                    && let Err(error) = delete_unchanged_sidecar(xmp_path, proof)
                {
                    return MergeResult {
                        xmp_path: xmp_path.to_path_buf(),
                        media_path: Some(media),
                        success: false,
                        message: format!(
                            "Merge committed, but verified sidecar cleanup was refused: {error:#}"
                        ),
                        match_strategy: Some(strategy),
                    };
                }

                MergeResult {
                    xmp_path: xmp_path.to_path_buf(),
                    media_path: Some(media),
                    success: true,
                    message: "Merged successfully".to_string(),
                    match_strategy: Some(strategy),
                }
            }
            Err(e) => MergeResult {
                xmp_path: xmp_path.to_path_buf(),
                media_path: Some(media),
                success: false,
                message: format!("Merge failed: {e}"),
                match_strategy: Some(strategy),
            },
        }
    }

    /// Process a directory for XMP merging.
    ///
    /// # Errors
    /// Returns an error if processing fails.
    pub fn process_directory(&self, dir: &Path) -> Result<Vec<MergeResult>> {
        let xmp_files = self.find_xmp_files(dir)?;
        let mut results = Vec::with_capacity(xmp_files.len());

        for xmp_path in xmp_files {
            let result = self.process_xmp(&xmp_path);
            results.push(result);
        }

        Ok(results)
    }
}

#[derive(Debug, Default)]
pub struct MergeSummary {
    pub total: usize,
    pub success: usize,
    pub failed: usize,
    pub skipped: usize,
    pub strategies: HashMap<String, usize>,
}

impl MergeSummary {
    #[must_use]
    pub fn from_results(results: &[MergeResult]) -> Self {
        let mut summary = Self {
            total: results.len(),
            ..Default::default()
        };

        for result in results {
            if result.success {
                summary.success += 1;
            } else if result.media_path.is_none() {
                summary.skipped += 1;
            } else {
                summary.failed += 1;
            }

            if let Some(ref strategy) = result.match_strategy {
                *summary.strategies.entry(strategy.clone()).or_insert(0) += 1;
            }
        }

        summary
    }
}

/// Merge XMP for a copied file (input relative).
///
/// # Errors
/// Returns an error if merging fails.
pub fn merge_xmp_for_copied_file(input: &Path, dest: &Path) -> Result<bool> {
    let stem = crate::media_conversion_gate::path_file_stem_or_empty(input, "xmp_merger:copied");
    let ext = crate::media_conversion_gate::path_extension_lowercase_or_empty_unchecked(input);
    let parent = crate::media_conversion_gate::path_parent_or_dot(input);

    let ext_lower = ext.to_lowercase();
    let xmp_candidates = [
        parent.join(format!("{stem}.xmp")),
        parent.join(format!("{stem}.{ext}.xmp")),
        parent.join(format!("{stem}.{ext_lower}.xmp")),
        parent.join(format!("{stem}.XMP")),
    ];

    for xmp_path in &xmp_candidates {
        if is_regular_non_symlink(xmp_path) {
            if crate::progress_mode::is_verbose_mode() {
                crate::log_info!(
                    crate::infra::static_logs::messages::LABEL_XMP,
                    &format!("Found XMP sidecar: {path}", path = xmp_path.display())
                );
            }

            let config = Config {
                delete_xmp_after_merge: false,
                overwrite_mode: OverwriteMode::Original,
                // file_copier applies timestamps once after merge via apply_file_timestamps.
                preserve_timestamps: false,
                log_level: LogLevel::Quiet,
            };

            let merger = XmpMerger::new(config);

            crate::progress_mode::xmp_merge_attempt();
            match merger.merge_xmp(xmp_path, dest) {
                Ok(()) => {
                    crate::progress_mode::xmp_merge_success();
                }
                Err(e) => {
                    crate::progress_mode::xmp_merge_failure(&e.to_string());
                    bail!("Failed to merge XMP: {e}");
                }
            }
            return Ok(true);
        }
    }

    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    use tempfile::TempDir;

    fn write_synthetic_hdr_gain_map_avif(root: &Path, output: &Path) -> Result<()> {
        let base = root.join("gain-map-base.png");
        let alternate = root.join("gain-map-hdr.png");
        image::RgbImage::from_fn(32, 24, |x, y| {
            image::Rgb([
                (x * 5 + y * 3 + 24).to_le_bytes()[0],
                (x * 3 + y * 7 + 32).to_le_bytes()[0],
                (x * 7 + y * 5 + 40).to_le_bytes()[0],
            ])
        })
        .save(&base)?;
        image::RgbImage::from_fn(32, 24, |x, y| {
            image::Rgb([
                (x * 7 + y * 5 + 96).to_le_bytes()[0],
                (x * 5 + y * 9 + 112).to_le_bytes()[0],
                (x * 9 + y * 7 + 128).to_le_bytes()[0],
            ])
        })
        .save(&alternate)?;

        let tool = crate::common_utils::resolve_tool_path("avifgainmaputil")
            .ok_or_else(|| anyhow::anyhow!("avifgainmaputil is unavailable"))?;
        let result = std::process::Command::new(tool)
            .arg("combine")
            .arg(&base)
            .arg(&alternate)
            .arg(output)
            .args([
                "--qcolor",
                "90",
                "--qgain-map",
                "90",
                "--speed",
                "8",
                "--cicp-base",
                "1/13/6",
                "--cicp-alternate",
                "9/16/9",
                "--clli-alternate",
                "1000,400",
            ])
            .output()?;
        anyhow::ensure!(
            result.status.success(),
            "synthetic HDR gain-map AVIF failed: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        Ok(())
    }

    fn gain_map_metadata(path: &Path) -> Result<String> {
        let tool = crate::common_utils::resolve_tool_path("avifgainmaputil")
            .ok_or_else(|| anyhow::anyhow!("avifgainmaputil is unavailable"))?;
        let result = std::process::Command::new(tool)
            .arg("printmetadata")
            .arg(path)
            .output()?;
        anyhow::ensure!(
            result.status.success(),
            "gain-map metadata probe failed: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        Ok(String::from_utf8(result.stdout)?)
    }

    #[test]
    fn generic_xmp_rewrite_is_blocked_for_archival_modern_containers() {
        use crate::image::format_detect::FormatKind;

        for format in [
            FormatKind::Jpeg,
            FormatKind::Jxl,
            FormatKind::Avif,
            FormatKind::Heic,
            FormatKind::Heif,
            FormatKind::WebP,
            FormatKind::Jp2,
            FormatKind::Unknown,
        ] {
            assert!(xmp_rewrite_requires_immutable_container(format));
        }
        for format in [FormatKind::Png, FormatKind::Tiff] {
            assert!(!xmp_rewrite_requires_immutable_container(format));
        }
    }

    #[test]
    fn jpeg_xmp_merge_preserves_primary_payload_and_rejects_unproven_mpf() -> Result<()> {
        let unproven_mpf = [
            0xff, 0xd8, 0xff, 0xe2, 0x00, 0x06, b'M', b'P', b'F', 0x00, 0xff, 0xd9,
        ];
        let error = jpeg_archival_feature_hash_from_bytes(&unproven_mpf)
            .expect_err("unclassified MPF JPEG must be retained rather than rewritten");
        assert!(error.to_string().contains("not a proven UltraHDR gain map"));

        if !ExiftoolBuilder::check_available() {
            return Ok(());
        }
        let temp = TempDir::new()?;
        let media = temp.path().join("archive.jpg");
        let sidecar = temp.path().join("archive.xmp");
        image::RgbImage::from_fn(24, 16, |x, y| {
            image::Rgb([
                (x * 7 + y * 3).to_le_bytes()[0],
                (x * 5 + y * 11).to_le_bytes()[0],
                (x * 13 + y * 2).to_le_bytes()[0],
            ])
        })
        .save(&media)?;
        fs::write(
            &sidecar,
            br#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"><rdf:Description xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:description><rdf:Alt><rdf:li xml:lang="x-default">JPEG archive proof</rdf:li></rdf:Alt></dc:description></rdf:Description></rdf:RDF></x:xmpmeta>"#,
        )?;
        let before = XmpMerger::capture_modern_container_proof(
            &media,
            crate::image::format_detect::FormatKind::Jpeg,
        )?;
        XmpMerger::new(Config::default()).merge_xmp(&sidecar, &media)?;
        let after = XmpMerger::capture_modern_container_proof(
            &media,
            crate::image::format_detect::FormatKind::Jpeg,
        )?;
        assert_eq!(after.image_data_hash, before.image_data_hash);
        assert_eq!(after.payload_size, before.payload_size);
        assert_eq!(after.codec_feature_hash, before.codec_feature_hash);
        assert!(after.has_xmp);
        Ok(())
    }

    #[test]
    fn modern_container_feature_hashes_cover_heif_auxiliary_and_jp2_non_xmp_boxes() {
        fn boxed(kind: [u8; 4], payload: &[u8]) -> Vec<u8> {
            let mut bytes = u32::try_from(payload.len() + 8)
                .expect("test box size fits u32")
                .to_be_bytes()
                .to_vec();
            bytes.extend_from_slice(&kind);
            bytes.extend_from_slice(payload);
            bytes
        }

        let mut heif = boxed(*b"hvcC", b"codec-config");
        heif.extend(boxed(*b"colr", b"nclx\0\t\0\x10\0\t"));
        heif.extend(boxed(*b"auxC", b"urn:com:apple:photo:2020:aux:hdrgainmap"));
        let heif_hash = heif_archival_feature_hash_from_bytes(&heif).unwrap();
        heif.extend(boxed(*b"xml ", b"mutable XMP overlay"));
        assert_eq!(
            heif_archival_feature_hash_from_bytes(&heif).unwrap(),
            heif_hash,
            "XMP metadata must not alter the HEIF codec/auxiliary feature proof"
        );
        let mut changed_heif = boxed(*b"hvcC", b"codec-config");
        changed_heif.extend(boxed(*b"colr", b"nclx\0\t\0\x10\0\t"));
        changed_heif.extend(boxed(*b"auxC", b"changed-auxiliary-role"));
        assert_ne!(
            heif_archival_feature_hash_from_bytes(&changed_heif).unwrap(),
            heif_hash
        );

        let mut jp2 = boxed(*b"jP  ", &[0x0d, 0x0a, 0x87, 0x0a]);
        jp2.extend(boxed(*b"jp2c", b"codestream"));
        let jp2_hash = jp2_archival_feature_hash_from_bytes(&jp2).unwrap();
        let mut xmp_payload = JP2_XMP_UUID.to_vec();
        xmp_payload.extend_from_slice(b"sidecar XMP");
        jp2.extend(boxed(*b"uuid", &xmp_payload));
        assert_eq!(
            jp2_archival_feature_hash_from_bytes(&jp2).unwrap(),
            jp2_hash,
            "JP2 XMP UUID boxes are mutable overlays"
        );
        let mut changed_jp2 = boxed(*b"jP  ", &[0x0d, 0x0a, 0x87, 0x0a]);
        changed_jp2.extend(boxed(*b"jp2c", b"changed-codestream"));
        assert_ne!(
            jp2_archival_feature_hash_from_bytes(&changed_jp2).unwrap(),
            jp2_hash
        );
    }

    #[test]
    fn modern_avif_xmp_merge_uses_native_writer_and_proof() -> Result<()> {
        if !ExiftoolBuilder::check_available() {
            return Ok(());
        }
        let temp_dir = TempDir::new().unwrap_or_else(|error| panic!("tempdir: {error}"));
        let media = temp_dir.path().join("fixture.avif");
        let xmp = temp_dir.path().join("fixture.xmp");
        fs::copy(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/metadata_clear_baseline.avif.fixture"),
            &media,
        )
        .unwrap_or_else(|error| panic!("copy AVIF fixture: {error}"));
        fs::write(
            &xmp,
            br#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"><rdf:Description xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:description><rdf:Alt><rdf:li xml:lang="x-default">native proof</rdf:li></rdf:Alt></dc:description></rdf:Description></rdf:RDF></x:xmpmeta>"#,
        )
        .unwrap_or_else(|error| panic!("write XMP fixture: {error}"));

        let merger = XmpMerger::new(Config::default());
        let before_hash = crate::common_utils::calculate_blake3_hash(&media)?;
        merger.merge_xmp(&xmp, &media)?;
        let after_hash = crate::common_utils::calculate_blake3_hash(&media)?;
        assert_ne!(before_hash, after_hash, "native merge must add XMP bytes");

        let mut builder = ExiftoolBuilder::new();
        builder.arg("-j").arg("-XMP-dc:Description").input(&media);
        let output = builder.build().output()?;
        assert!(
            output.status.success(),
            "ExifTool readback failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let readback = String::from_utf8_lossy(&output.stdout);
        assert!(
            readback.contains("native proof"),
            "native XMP value was not readable: {readback}"
        );

        let idempotent_hash = crate::common_utils::calculate_blake3_hash(&media)?;
        merger.merge_xmp(&xmp, &media)?;
        assert_eq!(
            idempotent_hash,
            crate::common_utils::calculate_blake3_hash(&media)?,
            "reapplying the same native XMP must be an idempotent no-op"
        );
        Ok(())
    }

    #[test]
    fn modern_webp_xmp_merge_preserves_archival_chunks() -> Result<()> {
        if !ExiftoolBuilder::check_available() {
            return Ok(());
        }
        let temp_dir = TempDir::new().unwrap_or_else(|error| panic!("tempdir: {error}"));
        let media = temp_dir.path().join("fixture.webp");
        let xmp = temp_dir.path().join("fixture.xmp");
        image::RgbImage::from_fn(24, 16, |x, y| {
            image::Rgb([
                (x * 7 + y * 3).to_le_bytes()[0],
                (x * 5 + y * 11).to_le_bytes()[0],
                (x * 13 + y * 2).to_le_bytes()[0],
            ])
        })
        .save(&media)?;
        fs::write(
            &xmp,
            br#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"><rdf:Description xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:description><rdf:Alt><rdf:li xml:lang="x-default">WebP archive proof</rdf:li></rdf:Alt></dc:description></rdf:Description></rdf:RDF></x:xmpmeta>"#,
        )?;

        let before = crate::image_formats::webp::archival_feature_hash(&media)?;
        XmpMerger::new(Config::default()).merge_xmp(&xmp, &media)?;
        assert_eq!(
            crate::image_formats::webp::archival_feature_hash(&media)?,
            before,
            "native WebP XMP merge changed a non-XMP archival chunk"
        );
        Ok(())
    }

    #[test]
    fn hdr_gain_map_survives_xmp_merge_and_meme_metadata_clear() -> Result<()> {
        for tool in ["avifgainmaputil", "avifdec", "exiftool"] {
            if crate::common_utils::resolve_tool_path(tool).is_none() {
                eprintln!("Skipping AVIF HDR archive matrix: {tool} is unavailable");
                return Ok(());
            }
        }

        let temp = TempDir::new()?;
        let media = temp.path().join("synthetic-hdr-gain-map.avif");
        let sidecar = temp.path().join("synthetic-hdr-gain-map.xmp");
        write_synthetic_hdr_gain_map_avif(temp.path(), &media)?;

        let gain_map_before = gain_map_metadata(&media)?;
        anyhow::ensure!(
            gain_map_before.contains("Alternate headroom: 4"),
            "fixture is not an HDR gain-map AVIF: {gain_map_before}"
        );
        let feature_hash_before = crate::image::fast_img::avif_codec_feature_hash(&media)?;
        let image_hash_before = crate::image::fast_img::image_data_sha256(&media)?;

        let initial_metadata = crate::ExiftoolBuilder::new()
            .arg("-XMP-dc:Title=MFB synthetic HDR original")
            .arg("-EXIF:ImageDescription=MFB synthetic HDR EXIF")
            .overwrite_original()
            .input(&media)
            .build()
            .output()?;
        anyhow::ensure!(
            initial_metadata.status.success(),
            "failed to seed synthetic AVIF metadata: {}",
            String::from_utf8_lossy(&initial_metadata.stderr)
        );
        assert_eq!(
            crate::image::fast_img::image_data_sha256(&media)?,
            image_hash_before,
            "seeding description metadata changed the AV1 primary image"
        );
        assert_eq!(
            crate::image::fast_img::avif_codec_feature_hash(&media)?,
            feature_hash_before,
            "seeding description metadata changed HDR/gain-map features"
        );

        fs::write(
            &sidecar,
            br#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"><rdf:Description xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:description><rdf:Alt><rdf:li xml:lang="x-default">MFB synthetic HDR sidecar</rdf:li></rdf:Alt></dc:description></rdf:Description></rdf:RDF></x:xmpmeta>"#,
        )?;
        let before = XmpMerger::capture_modern_container_proof(
            &media,
            crate::image::format_detect::FormatKind::Avif,
        )?;
        XmpMerger::new(Config::default()).merge_xmp(&sidecar, &media)?;
        let after = XmpMerger::capture_modern_container_proof(
            &media,
            crate::image::format_detect::FormatKind::Avif,
        )?;

        assert_eq!(after.image_data_hash, before.image_data_hash);
        assert_eq!(after.codec_feature_hash, before.codec_feature_hash);
        assert_eq!(after.stable_metadata_hash, before.stable_metadata_hash);
        assert_eq!(gain_map_metadata(&media)?, gain_map_before);
        assert!(
            sidecar.is_file(),
            "archive merge must retain the source sidecar"
        );

        let readback = crate::ExiftoolBuilder::new()
            .arg("-s3")
            .arg("-XMP-dc:Title")
            .arg("-XMP-dc:Description")
            .arg("-EXIF:ImageDescription")
            .input(&media)
            .build()
            .output()?;
        anyhow::ensure!(
            readback.status.success(),
            "AVIF metadata readback failed: {}",
            String::from_utf8_lossy(&readback.stderr)
        );
        let readback = String::from_utf8(readback.stdout)?;
        for expected in [
            "MFB synthetic HDR original",
            "MFB synthetic HDR sidecar",
            "MFB synthetic HDR EXIF",
        ] {
            assert!(
                readback.contains(expected),
                "AVIF archive merge lost {expected:?}: {readback}"
            );
        }

        let source_hash = crate::common_utils::calculate_blake3_hash(&media)?;
        let sanitized = temp.path().join("synthetic-hdr-sanitized.avif");
        crate::image::fast_img::prepare_existing_avif_meme_candidate(&media, &sanitized)?;
        assert_eq!(
            crate::common_utils::calculate_blake3_hash(&media)?,
            source_hash,
            "Meme Mode staging changed the source AVIF"
        );
        assert_eq!(
            crate::image::fast_img::image_data_sha256(&sanitized)?,
            image_hash_before,
            "Meme Mode metadata clear changed the AV1 primary image"
        );
        assert_eq!(
            crate::image::fast_img::avif_codec_feature_hash(&sanitized)?,
            after
                .codec_feature_hash
                .ok_or_else(|| anyhow::anyhow!("missing AVIF codec feature proof"))?,
            "Meme Mode metadata clear changed HDR/gain-map features"
        );
        assert_eq!(gain_map_metadata(&sanitized)?, gain_map_before);
        crate::metadata::verify_output_embedded_metadata(
            &media,
            &sanitized,
            crate::metadata::MetadataOutputPolicy::Clear,
        )?;
        Ok(())
    }

    #[test]
    fn protected_container_markers_block_generic_metadata_rewrite() {
        let temp_dir = TempDir::new().unwrap_or_else(|error| panic!("tempdir: {error}"));
        let jpeg = temp_dir.path().join("signed.jpg");
        let png = temp_dir.path().join("signed.png");
        fs::write(&jpeg, [0xFF, 0xD8, 0xFF, 0xEB, 0x00, 0x02, 0xFF, 0xD9])
            .unwrap_or_else(|error| panic!("JPEG fixture: {error}"));
        let mut png_bytes = b"\x89PNG\r\n\x1a\n".to_vec();
        png_bytes.extend_from_slice(&0_u32.to_be_bytes());
        png_bytes.extend_from_slice(b"caBX");
        png_bytes.extend_from_slice(&0_u32.to_be_bytes());
        fs::write(&png, png_bytes).unwrap_or_else(|error| panic!("PNG fixture: {error}"));

        assert!(jpeg_has_protected_app11(&jpeg).unwrap_or(false));
        assert!(png_has_c2pa_chunk(&png).unwrap_or(false));
    }

    #[test]
    fn test_is_uuid_filename() {
        assert!(XmpMerger::is_uuid_filename(
            "6cdf1517-be7d-4f85-b519-f4aeaac45fdd"
        ));
        assert!(XmpMerger::is_uuid_filename(
            "A1B2C3D4-E5F6-7890-ABCD-EF1234567890"
        ));
        assert!(!XmpMerger::is_uuid_filename("photo"));
        assert!(!XmpMerger::is_uuid_filename("photo-2024"));
        assert!(!XmpMerger::is_uuid_filename("123-456-789"));
    }

    #[test]
    fn test_find_xmp_files() {
        let temp_dir = TempDir::new().unwrap_or_else(|_| panic!("error"));
        let xmp1 = temp_dir.path().join("photo1.xmp");
        let xmp2 = temp_dir.path().join("photo2.jpg.xmp");
        let jpg = temp_dir.path().join("photo1.jpg");

        fs::write(&xmp1, "").unwrap_or_else(|_| panic!("error"));
        fs::write(&xmp2, "").unwrap_or_else(|_| panic!("error"));
        fs::write(&jpg, "").unwrap_or_else(|_| panic!("error"));

        let merger = XmpMerger::new(Config::default());
        let xmp_files = merger
            .find_xmp_files(temp_dir.path())
            .unwrap_or_else(|_| panic!("error"));

        assert_eq!(xmp_files.len(), 2);
    }

    #[test]
    fn test_direct_match_strategy() {
        let temp_dir = TempDir::new().unwrap_or_else(|_| panic!("error"));
        let jpg = temp_dir.path().join("photo.jpg");
        let xmp = temp_dir.path().join("photo.jpg.xmp");

        fs::write(&jpg, "fake jpg").unwrap_or_else(|_| panic!("error"));
        fs::write(&xmp, "fake xmp").unwrap_or_else(|_| panic!("error"));

        let _merger = XmpMerger::new(Config::default());
        let result = XmpMerger::find_direct_match(&xmp);

        assert!(result.is_some());
        assert_eq!(result.unwrap_or_else(|| panic!("error")), jpg);
    }

    #[test]
    fn test_same_name_different_ext_strategy() {
        let temp_dir = TempDir::new().unwrap_or_else(|_| panic!("error"));
        let jpg = temp_dir.path().join("photo.jpg");
        let xmp = temp_dir.path().join("photo.xmp");

        fs::write(&jpg, "fake jpg").unwrap_or_else(|_| panic!("error"));
        fs::write(&xmp, "fake xmp").unwrap_or_else(|_| panic!("error"));

        let result = XmpMerger::find_same_name_different_ext(&xmp);

        assert!(result.is_some());
        assert_eq!(result.unwrap_or_else(|| panic!("error")), jpg);
    }

    #[test]
    fn test_case_insensitive_match() {
        let temp_dir = TempDir::new().unwrap_or_else(|_| panic!("error"));
        let jpg = temp_dir.path().join("PHOTO.JPG");
        let xmp = temp_dir.path().join("photo.xmp");

        fs::write(&jpg, "fake jpg").unwrap_or_else(|_| panic!("error"));
        fs::write(&xmp, "fake xmp").unwrap_or_else(|_| panic!("error"));

        let result = XmpMerger::find_case_insensitive(&xmp);

        assert!(result.is_some());
    }

    #[test]
    fn test_fuzzy_match_special_chars() {
        let temp_dir = TempDir::new().unwrap_or_else(|_| panic!("error"));
        let jpg = temp_dir.path().join("photo (1).jpg");
        let xmp = temp_dir.path().join("photo(1).xmp");

        fs::write(&jpg, "fake jpg").unwrap_or_else(|_| panic!("error"));
        fs::write(&xmp, "fake xmp").unwrap_or_else(|_| panic!("error"));

        let result = XmpMerger::find_fuzzy_match(&xmp);

        assert!(result.is_some());
    }

    #[test]
    fn test_normalize_filename() {
        assert_eq!(XmpMerger::normalize_filename("Photo (1)"), "photo1");
        assert_eq!(
            XmpMerger::normalize_filename("IMG_2024-01-01"),
            "img20240101"
        );
        assert_eq!(XmpMerger::normalize_filename("test_file"), "testfile");
        assert_eq!(XmpMerger::normalize_filename("photo.test"), "phototest");
    }

    #[test]
    fn test_unicode_filename() {
        let temp_dir = TempDir::new().unwrap_or_else(|_| panic!("error"));
        let jpg = temp_dir.path().join("photo2024.jpg");
        let xmp = temp_dir.path().join("photo2024.xmp");

        fs::write(&jpg, "fake jpg").unwrap_or_else(|_| panic!("error"));
        fs::write(&xmp, "fake xmp").unwrap_or_else(|_| panic!("error"));

        let result = XmpMerger::find_same_name_different_ext(&xmp);

        assert!(result.is_some());
        assert_eq!(result.unwrap_or_else(|| panic!("error")), jpg);
    }

    #[test]
    fn test_spaces_in_filename() {
        let temp_dir = TempDir::new().unwrap_or_else(|_| panic!("error"));
        let jpg = temp_dir.path().join("my photo 2024.jpg");
        let xmp = temp_dir.path().join("my photo 2024.xmp");

        fs::write(&jpg, "fake jpg").unwrap_or_else(|_| panic!("error"));
        fs::write(&xmp, "fake xmp").unwrap_or_else(|_| panic!("error"));

        let result = XmpMerger::find_same_name_different_ext(&xmp);

        assert!(result.is_some());
        assert_eq!(result.unwrap_or_else(|| panic!("error")), jpg);
    }

    #[test]
    fn test_raw_format_match() {
        let temp_dir = TempDir::new().unwrap_or_else(|_| panic!("error"));
        let raw = temp_dir.path().join("DSC_0001.NEF");
        let xmp = temp_dir.path().join("DSC_0001.xmp");

        fs::write(&raw, "fake raw").unwrap_or_else(|_| panic!("error"));
        fs::write(&xmp, "fake xmp").unwrap_or_else(|_| panic!("error"));

        let merger = XmpMerger::new(Config::default());
        let (result, strategy) = merger
            .find_media_file(&xmp)
            .unwrap_or_else(|_| panic!("error"));

        assert!(result.is_some());
        assert!(strategy == "same_name" || strategy == "case_insensitive");
    }

    #[test]
    fn test_merge_xmp_mismatch_fallback() {
        if !crate::test_ci_contract::exiftool_available_or_ci_panic() {
            return;
        }

        let temp_dir = TempDir::new().unwrap_or_else(|_| panic!("error"));
        let jpg_path = temp_dir.path().join("mismatch.jpg");
        let xmp_path = temp_dir.path().join("mismatch.xmp");

        let png_data = [
            0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48,
            0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00,
            0x00, 0x90, 0x77, 0x53, 0xDE, 0x00, 0x00, 0x00, 0x0C, 0x49, 0x44, 0x41, 0x54, 0x08,
            0xD7, 0x63, 0xF8, 0xCF, 0xC0, 0x00, 0x00, 0x03, 0x01, 0x01, 0x00, 0x18, 0xDD, 0x8D,
            0xB0, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
        ];
        fs::write(&jpg_path, png_data).unwrap_or_else(|_| panic!("error"));

        let xmp_content = r"<?xpacket begin='﻿' id='W5M0MpCehiHzreSzNTczkc9d'?>
<x:xmpmeta xmlns:x='adobe:ns:meta/' x:xmptk='Image::ExifTool 12.00'>
<rdf:RDF xmlns:rdf='http://www.w3.org/1999/02/22-rdf-syntax-ns#'>
 <rdf:Description rdf:about=''
  xmlns:dc='http://purl.org/dc/elements/1.1/'>
  <dc:Description>
   <rdf:Alt>
    <rdf:li xml:lang='x-default'>Test Description</rdf:li>
   </rdf:Alt>
  </dc:Description>
 </rdf:Description>
</rdf:RDF>
</x:xmpmeta>
<?xpacket end='w'?>";
        fs::write(&xmp_path, xmp_content).unwrap_or_else(|_| panic!("error"));

        let config = Config {
            log_level: LogLevel::Verbose,
            ..Default::default()
        };
        let merger = XmpMerger::new(config);

        let result = merger.merge_xmp(&xmp_path, &jpg_path);

        if let Err(e) = &result {
            crate::media_conversion_gate::delivery_metadata_batch_audit(
                "delivery_metadata_xmp",
                format!("Merge failed with error: {e}"),
            );
        }
        assert!(result.is_ok(), "XMP merge failed for mismatched extension");

        assert!(jpg_path.exists());
        assert!(!jpg_path.with_extension("png").exists());
    }

    #[test]
    fn test_find_by_xmp_metadata_ignores_directory_candidate() {
        let temp_dir = TempDir::new().unwrap_or_else(|_| panic!("error"));
        let xmp_path = temp_dir.path().join("photo.xmp");
        let dir_candidate = temp_dir.path().join("photo.jpg");

        fs::write(&xmp_path, "fake xmp").unwrap_or_else(|_| panic!("error"));
        fs::create_dir(&dir_candidate).unwrap_or_else(|_| panic!("error"));

        let xmp_info = XmpFile {
            path: xmp_path.clone(),
            document_id: None,
            derived_from: Some("photo.jpg".to_string()),
            source: None,
        };

        let _merger = XmpMerger::new(Config::default());
        assert_eq!(XmpMerger::find_by_xmp_metadata(&xmp_path, &xmp_info), None);
    }

    #[test]
    fn test_merge_xmp_for_copied_file_ignores_directory_sidecar() {
        let temp_dir = TempDir::new().unwrap_or_else(|_| panic!("error"));
        let input = temp_dir.path().join("photo.jpg");
        let dest = temp_dir.path().join("copy.jpg");
        let xmp_dir = temp_dir.path().join("photo.xmp");

        fs::write(&input, "fake jpg").unwrap_or_else(|_| panic!("error"));
        fs::write(&dest, "fake jpg").unwrap_or_else(|_| panic!("error"));
        fs::create_dir(&xmp_dir).unwrap_or_else(|_| panic!("error"));

        let merged = merge_xmp_for_copied_file(&input, &dest).unwrap_or_else(|_| panic!("error"));
        assert!(!merged);
    }

    #[test]
    fn extract_xmp_metadata_malformed_xml_returns_error_not_empty_metadata() {
        let temp_dir = TempDir::new().unwrap_or_else(|_| panic!("error"));
        let malformed_xmp = temp_dir.path().join("malformed.xmp");
        std::fs::write(
            &malformed_xmp,
            br#"<x:xmpmeta><rdf:Description DocumentID="doc-1"></x:xmpmeta>"#,
        )
        .unwrap_or_else(|_| panic!("test setup error"));

        let err = XmpMerger::extract_xmp_metadata(&malformed_xmp, LogLevel::Quiet)
            .expect_err("malformed native XMP must fail closed before exiftool fallback");
        assert!(err.to_string().contains("XMP"), "unexpected error: {err}");
    }

    #[test]
    fn raw_jxl_xmp_merge_is_rejected_without_mutating_bytes() {
        let temp_dir = TempDir::new().unwrap_or_else(|_| panic!("error"));
        let jxl_path = temp_dir.path().join("raw.jxl");
        let xmp_path = temp_dir.path().join("raw.xmp");
        let original = vec![0xFF, 0x0A, 0x01, 0x02, 0x03];
        fs::write(&jxl_path, &original).unwrap_or_else(|_| panic!("error"));
        fs::write(&xmp_path, b"<x:xmpmeta xmlns:x='adobe:ns:meta/'/>")
            .unwrap_or_else(|_| panic!("error"));

        let error = XmpMerger::new(Config::default())
            .merge_xmp(&xmp_path, &jxl_path)
            .expect_err("raw JXL must not enter a rewriting metadata path");
        assert!(error.to_string().contains("raw JPEG XL codestream"));
        assert_eq!(
            fs::read(&jxl_path).unwrap_or_else(|_| panic!("error")),
            original
        );
    }

    #[test]
    fn test_extract_xmp_metadata_reports_exiftool_failure() {
        if !crate::test_ci_contract::exiftool_available_or_ci_panic() {
            return;
        }

        let temp_dir = TempDir::new().unwrap_or_else(|_| panic!("error"));
        let empty_xmp = temp_dir.path().join("empty.xmp");
        std::fs::write(&empty_xmp, "").unwrap_or_else(|_| panic!("test setup error"));
        let _merger = XmpMerger::new(Config::default());

        let err = XmpMerger::extract_xmp_metadata(&empty_xmp, LogLevel::Verbose)
            .err()
            .unwrap_or_else(|| anyhow::anyhow!("unknown error"));
        assert!(err.to_string().contains("Extraction failed"));
    }
}

#[cfg(test)]
mod xmp_jxl_apple_compat_contract {
    include!("../../tests/internal/xmp_jxl_apple_compat_contract.rs");
}
