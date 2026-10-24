//! Append-only JPEG restoration evidence and post-deletion receipts.

use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, bail, ensure};

pub const MANIFEST_NAME: &str = ".mfb_restore_jpeg_manifest.tsv";
const HEADER: &str = "# MFB_RESTORE_JPEG_MANIFEST_V3\nsource_rel_hex\toutput_rel_hex\tsource_jxl_blake3\treconstruction_jpeg_blake3\trestored_jpeg_blake3\txmp_rel_hex\txmp_blake3\tverified_unix_seconds\tmfb_version\tdjxl_version_hex\tsource_deleted\n";
const DELETED: &str = "# MFB_RESTORE_JPEG_DELETED\t";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestoreJpegManifestRecord {
    pub source_rel: String,
    pub output_rel: String,
    pub source_blake3: String,
    pub output_blake3: String,
    pub xmp_rel: Option<String>,
    pub xmp_blake3: Option<String>,
    pub source_deleted: bool,
}

fn decode_hex(value: &str) -> anyhow::Result<String> {
    ensure!(value.len().is_multiple_of(2), "invalid odd-length hex");
    let bytes = value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let high = char::from(pair[0]).to_digit(16).context("invalid hex")?;
            let low = char::from(pair[1]).to_digit(16).context("invalid hex")?;
            u8::try_from(high * 16 + low).context("invalid hex byte")
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    String::from_utf8(bytes).context("invalid UTF-8")
}

fn encode_hex(value: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    value
        .bytes()
        .flat_map(|byte| {
            [
                char::from(HEX[usize::from(byte >> 4)]),
                char::from(HEX[usize::from(byte & 15)]),
            ]
        })
        .collect()
}

fn safe_relative(value: &str) -> bool {
    !value.is_empty()
        && !Path::new(value).is_absolute()
        && Path::new(value)
            .components()
            .all(|part| matches!(part, Component::Normal(_)))
}

fn parse_record(line: &str) -> anyhow::Result<RestoreJpegManifestRecord> {
    let parts = line.split('\t').collect::<Vec<_>>();
    ensure!(
        matches!(parts.len(), 5 | 7 | 11 | 12),
        "expected 5, 7, 11, or legacy 12 TSV fields, got {}",
        parts.len()
    );
    let (reconstruction, output, xmp, xmp_hash, deleted) = match parts.len() {
        11 | 12 => (
            parts[3],
            parts[4],
            parts[5],
            parts[6],
            parts[parts.len() - 1],
        ),
        7 => (parts[3], parts[3], parts[4], parts[5], parts[6]),
        _ => (parts[3], parts[3], "", "", parts[4]),
    };
    if parts.len() >= 11 {
        ensure!(
            reconstruction == output,
            "reconstruction and restored JPEG hashes differ"
        );
        ensure!(
            parts[7].parse::<u64>().is_ok_and(|value| value > 0),
            "verified_unix_seconds must be a positive integer"
        );
        ensure!(!parts[8].trim().is_empty(), "missing MFB version");
        let version = decode_hex(parts[9]).context("invalid djxl version hex/UTF-8")?;
        ensure!(!version.trim().is_empty(), "missing djxl version");
        if parts.len() == 12 && !parts[10].is_empty() {
            decode_hex(parts[10]).context("invalid Photos UUID hex/UTF-8")?;
        }
    }
    let source_rel = decode_hex(parts[0]).context("invalid source_rel hex/UTF-8 field")?;
    let output_rel = decode_hex(parts[1]).context("invalid output_rel hex/UTF-8 field")?;
    let (xmp_rel, xmp_blake3) = if xmp.is_empty() && xmp_hash.is_empty() {
        (None, None)
    } else {
        ensure!(
            !xmp.is_empty() && !xmp_hash.trim().is_empty(),
            "XMP path and hash fields must both be present"
        );
        (
            Some(decode_hex(xmp).context("invalid xmp_rel hex/UTF-8 field")?),
            Some(xmp_hash.to_string()),
        )
    };
    ensure!(
        safe_relative(&source_rel)
            && safe_relative(&output_rel)
            && xmp_rel.as_deref().is_none_or(safe_relative),
        "manifest paths must be safe relative paths"
    );
    ensure!(
        !parts[2].trim().is_empty() && !output.trim().is_empty(),
        "missing manifest hash field"
    );
    let source_deleted = match deleted {
        "true" => true,
        "false" => false,
        _ => bail!("source_deleted must be true or false"),
    };
    Ok(RestoreJpegManifestRecord {
        source_rel,
        output_rel,
        source_blake3: parts[2].to_string(),
        output_blake3: output.to_string(),
        xmp_rel,
        xmp_blake3,
        source_deleted,
    })
}

/// Parse existing proof rows and replay only exactly bound deletion receipts.
/// Invalid or incomplete records are returned as errors, never silently repaired.
#[must_use]
pub fn parse_manifest(content: &str) -> (Vec<RestoreJpegManifestRecord>, Vec<String>) {
    let mut records: Vec<RestoreJpegManifestRecord> = Vec::new();
    let mut sources = HashMap::<String, usize>::new();
    let mut outputs = HashMap::<String, usize>::new();
    let mut errors = Vec::new();
    for (index, raw) in content.split_inclusive('\n').enumerate() {
        let result = (|| -> anyhow::Result<()> {
            ensure!(
                raw.ends_with('\n'),
                "incomplete manifest tail; final record has no newline"
            );
            let line = raw.trim();
            if let Some(fields) = line.strip_prefix(DELETED) {
                let fields = fields.split('\t').collect::<Vec<_>>();
                ensure!(
                    fields.len() == 4,
                    "deletion receipt requires four identity fields"
                );
                let source = decode_hex(fields[0]).context("invalid deletion receipt source")?;
                let output = decode_hex(fields[1]).context("invalid deletion receipt output")?;
                let record = sources
                    .get(&source)
                    .and_then(|&slot| records.get_mut(slot))
                    .context("deletion receipt has no preceding unique proof")?;
                ensure!(!record.source_deleted, "duplicate deletion receipt");
                ensure!(
                    record.output_rel == output
                        && record.source_blake3 == fields[2]
                        && record.output_blake3 == fields[3],
                    "deletion receipt identity does not match proof"
                );
                record.source_deleted = true;
                return Ok(());
            }
            if let Some(fields) = line.strip_prefix("# MFB_RESTORE_JPEG_ATTENTION\t") {
                let (source, reason) = fields
                    .split_once('\t')
                    .context("invalid restoration metadata attention")?;
                let source =
                    decode_hex(source).context("invalid restoration metadata attention")?;
                let reason =
                    decode_hex(reason).context("invalid restoration metadata attention")?;
                ensure!(
                    safe_relative(&source) && !reason.trim().is_empty(),
                    "invalid restoration metadata attention"
                );
                bail!("unresolved restoration metadata for {source}: {reason}");
            }
            if let Some(version) = line.strip_prefix("# MFB_RESTORE_JPEG_MANIFEST_V") {
                ensure!(
                    matches!(version, "1" | "2" | "3"),
                    "unsupported restoration manifest version"
                );
                return Ok(());
            }
            if line.is_empty() || line.starts_with("source_rel_hex\toutput_rel_hex\t") {
                return Ok(());
            }
            ensure!(
                !line.starts_with("# MFB_RESTORE_JPEG_"),
                "unknown restoration manifest event"
            );
            if line.starts_with('#') {
                return Ok(());
            }
            let record = parse_record(line)?;
            ensure!(
                !sources.contains_key(&record.source_rel),
                "duplicate restoration manifest source"
            );
            ensure!(
                !outputs.contains_key(&record.output_rel),
                "duplicate restoration manifest output"
            );
            sources.insert(record.source_rel.clone(), records.len());
            outputs.insert(record.output_rel.clone(), records.len());
            records.push(record);
            Ok(())
        })();
        if let Err(error) = result {
            errors.push(format!("line {}: {error:#}", index + 1));
        }
    }
    (records, errors)
}

/// Read restoration evidence without accepting symlinks or non-files.
#[must_use]
pub fn load_manifest(root: &Path) -> (Vec<RestoreJpegManifestRecord>, Vec<String>) {
    let path = root.join(MANIFEST_NAME);
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let opened = (|| -> anyhow::Result<String> {
        let mut file = options.open(&path)?;
        ensure!(
            file.metadata()?.is_file()
                && !std::fs::symlink_metadata(&path)?.file_type().is_symlink(),
            "restore manifest is not a regular non-symlink file"
        );
        let mut content = String::new();
        file.read_to_string(&mut content)?;
        Ok(content)
    })();
    match opened {
        Ok(content) => parse_manifest(&content),
        Err(error)
            if error
                .downcast_ref::<std::io::Error>()
                .is_some_and(|error| error.kind() == std::io::ErrorKind::NotFound) =>
        {
            (Vec::new(), Vec::new())
        }
        Err(error) => (
            Vec::new(),
            vec![format!("restore manifest unreadable: {path:?}: {error:#}")],
        ),
    }
}

/// Own one manifest until the run ends; append failures permanently disable further writes.
#[derive(Debug)]
pub struct RestoreJpegManifestWriter {
    file: File,
    path: PathBuf,
    length: u64,
    records: HashMap<String, RestoreJpegManifestRecord>,
    outputs: HashMap<String, String>,
    poisoned: bool,
}

impl RestoreJpegManifestWriter {
    /// Open and lock existing evidence without replacing it.
    ///
    /// # Errors
    /// Rejects occupied, malformed, replaced, or non-regular manifests.
    pub fn open(root: &Path) -> anyhow::Result<Self> {
        std::fs::create_dir_all(root)?;
        let path = root.join(MANIFEST_NAME);
        let mut options = OpenOptions::new();
        options.read(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
        }
        let (mut file, created) = match options.clone().create_new(true).open(&path) {
            Ok(file) => (file, true),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => (
                options
                    .open(&path)
                    .context("open existing restore manifest")?,
                false,
            ),
            Err(error) => return Err(error).context("create restore manifest"),
        };
        ensure!(
            file.metadata()?.is_file(),
            "restore manifest is not a regular file"
        );
        file.try_lock().map_err(|error| {
            anyhow::anyhow!("restore manifest is occupied or cannot be locked: {error}")
        })?;
        let mut content = String::new();
        file.read_to_string(&mut content)?;
        ensure!(
            created
                || content
                    .lines()
                    .any(|line| line.starts_with("source_rel_hex\toutput_rel_hex\t")),
            "existing restore manifest has no recognized header; refusing to replace it"
        );
        let (records, errors) = parse_manifest(&content);
        ensure!(
            errors.is_empty(),
            "restore manifest is invalid: {}",
            errors.join("; ")
        );
        let length = file.metadata()?.len();
        let mut writer = Self {
            file,
            path,
            length,
            outputs: records
                .iter()
                .map(|record| (record.output_rel.clone(), record.source_rel.clone()))
                .collect(),
            records: records
                .into_iter()
                .map(|record| (record.source_rel.clone(), record))
                .collect(),
            poisoned: false,
        };
        if created {
            writer.append(HEADER)?;
        }
        // A previous run may have left a complete visible row after a failed durability barrier.
        writer.check_owned()?;
        writer.file.sync_all()?;
        crate::io_utils::sync_parent_directory(&writer.path)?;
        writer.check_owned()?;
        Ok(writer)
    }

    fn ensure_owned(&self) -> anyhow::Result<()> {
        ensure!(
            !self.poisoned,
            "restore manifest writer is poisoned after a persistence failure"
        );
        let opened = self.file.metadata()?;
        let current = std::fs::symlink_metadata(&self.path)?;
        ensure!(
            current.is_file() && !current.file_type().is_symlink(),
            "restore manifest path is no longer a regular file"
        );
        ensure!(
            opened.len() == self.length && current.len() == self.length,
            "restore manifest length changed outside the owning writer"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            ensure!(
                opened.dev() == current.dev() && opened.ino() == current.ino(),
                "restore manifest path was replaced"
            );
        }
        Ok(())
    }

    fn check_owned(&mut self) -> anyhow::Result<()> {
        let result = self.ensure_owned();
        if result.is_err() {
            self.poisoned = true;
        }
        result
    }

    fn append(&mut self, content: &str) -> anyhow::Result<()> {
        let result = (|| {
            self.check_owned()?;
            self.file.seek(SeekFrom::End(0))?;
            self.file.write_all(content.as_bytes())?;
            self.file.sync_all()?;
            self.length = self
                .length
                .checked_add(u64::try_from(content.len())?)
                .context("manifest length overflow")?;
            self.ensure_owned()
        })();
        if result.is_err() {
            self.poisoned = true;
        }
        result
    }

    /// Persist one new proof, or reuse an identical retained-source proof.
    ///
    /// # Errors
    /// Rejects duplicate identities, changed proof fields, and failed durability barriers.
    pub fn record_proof(&mut self, line: &str) -> anyhow::Result<()> {
        self.check_owned()?;
        ensure!(
            line.ends_with('\n')
                && line.lines().count() == 1
                && line.trim_end().split('\t').count() == 11,
            "new restoration proof must be one complete V3 row"
        );
        let (mut records, errors) = parse_manifest(line);
        ensure!(
            errors.is_empty() && records.len() == 1,
            "invalid restoration proof: {}",
            errors.join("; ")
        );
        let record = records.remove(0);
        ensure!(
            !record.source_deleted,
            "new restoration proof cannot claim source deletion"
        );
        if let Some(existing) = self.records.get(&record.source_rel) {
            ensure!(
                existing == &record,
                "existing restoration proof conflicts with current source or output"
            );
            return Ok(());
        }
        ensure!(
            !self.outputs.contains_key(&record.output_rel),
            "restoration output is already owned by another proof"
        );
        self.append(line)?;
        self.outputs
            .insert(record.output_rel.clone(), record.source_rel.clone());
        self.records.insert(record.source_rel.clone(), record);
        Ok(())
    }

    /// Persist a completion receipt only after the caller actually removed the proved source.
    ///
    /// # Errors
    /// Rejects missing, duplicate, or unpersistable deletion receipts.
    pub fn record_deleted(&mut self, source: &str) -> anyhow::Result<()> {
        let record = self
            .records
            .get(source)
            .context("deletion receipt has no durable restoration proof")?;
        ensure!(!record.source_deleted, "duplicate deletion receipt");
        let line = format!(
            "{DELETED}{}\t{}\t{}\t{}\n",
            encode_hex(&record.source_rel),
            encode_hex(&record.output_rel),
            record.source_blake3,
            record.output_blake3
        );
        self.append(&line)?;
        self.records
            .get_mut(source)
            .context("restoration proof disappeared")?
            .source_deleted = true;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn proof(source: &str, output: &str) -> String {
        format!(
            "{}\t{}\tsource-hash\tjpeg-hash\tjpeg-hash\t\t\t1\t0.12.0\t646a786c\tfalse\n",
            encode_hex(source),
            encode_hex(output)
        )
    }

    #[test]
    fn restoration_manifest_appends_linearly_and_preserves_prior_proofs() -> anyhow::Result<()> {
        let root = tempfile::tempdir()?;
        let first = proof("a.jxl", "a.jpg");
        let path = root.path().join(MANIFEST_NAME);
        {
            let mut writer = RestoreJpegManifestWriter::open(root.path())?;
            writer.record_proof(&first)?;
            let before = std::fs::read(&path)?;
            writer.record_deleted("a.jxl")?;
            assert!(std::fs::read(&path)?.starts_with(&before));
        }
        let before = std::fs::read(&path)?;
        let mut writer = RestoreJpegManifestWriter::open(root.path())?;
        writer.record_proof(&proof("b.jxl", "b.jpg"))?;
        let length = writer.length;
        writer.record_proof(&proof("b.jxl", "b.jpg"))?;
        assert_eq!(writer.length, length);
        assert!(std::fs::read(&path)?.starts_with(&before));
        let (records, errors) = load_manifest(root.path());
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(records.len(), 2);
        assert!(records[0].source_deleted);
        assert!(!records[1].source_deleted);
        assert!(writer.record_proof(&proof("b.jxl", "changed.jpg")).is_err());
        assert!(writer.record_proof(&proof("c.jxl", "b.jpg")).is_err());
        Ok(())
    }

    #[test]
    fn restoration_manifest_rejects_duplicate_unbound_and_torn_receipts() {
        let row = proof("a.jxl", "a.jpg");
        let receipt = format!("{DELETED}612e6a786c\t612e6a7067\tsource-hash\tjpeg-hash\n");
        for invalid in [
            receipt.clone(),
            format!("{row}{receipt}{receipt}"),
            format!("{row}{}", receipt.replace("jpeg-hash", "other-hash")),
            format!("{row}{}", receipt.trim_end()),
            format!("{row}{row}"),
            format!("# MFB_RESTORE_JPEG_MANIFEST_V99\n{row}"),
        ] {
            assert!(!parse_manifest(&invalid).1.is_empty(), "{invalid}");
        }
    }

    #[test]
    fn restoration_manifest_rejects_lock_and_external_truncation() -> anyhow::Result<()> {
        let root = tempfile::tempdir()?;
        let mut writer = RestoreJpegManifestWriter::open(root.path())?;
        assert!(RestoreJpegManifestWriter::open(root.path()).is_err());
        writer.record_proof(&proof("a.jxl", "a.jpg"))?;
        OpenOptions::new()
            .write(true)
            .open(root.path().join(MANIFEST_NAME))?
            .set_len(0)?;
        assert!(writer.record_deleted("a.jxl").is_err());
        assert!(writer.poisoned);
        assert!(writer.record_proof(&proof("b.jxl", "b.jpg")).is_err());
        Ok(())
    }

    #[test]
    fn restoration_manifest_write_failure_retains_proof_and_poison_is_permanent()
    -> anyhow::Result<()> {
        let root = tempfile::tempdir()?;
        let path = root.path().join(MANIFEST_NAME);
        let mut writer = RestoreJpegManifestWriter::open(root.path())?;
        writer.record_proof(&proof("a.jxl", "a.jpg"))?;
        let before = std::fs::read(&path)?;
        writer.file = File::open(&path)?;
        assert!(writer.record_deleted("a.jxl").is_err());
        assert!(writer.poisoned);
        assert_eq!(std::fs::read(&path)?, before);
        let (records, errors) = load_manifest(root.path());
        assert!(errors.is_empty(), "{errors:?}");
        assert!(!records[0].source_deleted);
        assert!(writer.record_proof(&proof("b.jxl", "b.jpg")).is_err());
        std::fs::OpenOptions::new()
            .append(true)
            .open(&path)?
            .write_all(b"# MFB_RESTORE_JPEG_DEL")?;
        assert!(!load_manifest(root.path()).1.is_empty());
        assert!(RestoreJpegManifestWriter::open(root.path()).is_err());
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn restoration_manifest_rejects_replaced_path_and_symlink() -> anyhow::Result<()> {
        let root = tempfile::tempdir()?;
        let mut writer = RestoreJpegManifestWriter::open(root.path())?;
        writer.record_proof(&proof("a.jxl", "a.jpg"))?;
        let path = root.path().join(MANIFEST_NAME);
        std::fs::rename(&path, root.path().join("previous.tsv"))?;
        std::fs::write(&path, std::fs::read(root.path().join("previous.tsv"))?)?;
        assert!(writer.record_deleted("a.jxl").is_err());
        assert!(writer.poisoned);
        drop(writer);
        std::fs::remove_file(&path)?;
        std::os::unix::fs::symlink(root.path().join("previous.tsv"), &path)?;
        assert!(RestoreJpegManifestWriter::open(root.path()).is_err());
        assert!(!load_manifest(root.path()).1.is_empty());
        Ok(())
    }
}
