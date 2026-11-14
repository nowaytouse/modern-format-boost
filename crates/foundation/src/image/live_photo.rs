//! Live Photo Detection Module
//!
//! Detects Apple Live Photos from same-stem still/MOV companions and verifies
//! their Apple Content Identifier whenever both files expose one.

use crate::batch::AdmittedImageFile;
use crate::builder_base::ToolBuilder;
use anyhow::Context;
use std::collections::HashMap;
#[cfg(unix)]
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{Duration, Instant};

// JXL is also protected as an archived still, not a claim of native Live playback.
const LIVE_STILL_EXTENSIONS: &[&str] =
    &["heic", "heif", "hif", "jpg", "jpeg", "jpe", "jfif", "jxl"];

fn live_member_extension(path: &Path) -> String {
    use crate::image::format_detect::{FormatKind, detect_true_format};

    match detect_true_format(path) {
        Ok(FormatKind::Jpeg) => "jpg".to_string(),
        Ok(FormatKind::Jxl) => "jxl".to_string(),
        Ok(FormatKind::Heic | FormatKind::Heif) => "heic".to_string(),
        Ok(FormatKind::Mov) => "mov".to_string(),
        // A missing/truncated probe must not undo conservative retention of
        // conventional same-stem pairs. This fallback only protects originals;
        // it never authorizes conversion or cleanup.
        _ => crate::media_conversion_gate::path_extension_lowercase_or_empty_unchecked(path),
    }
}

/// Check if a file is part of a Live Photo pair
///
/// Live Photos consist of:
/// - A HEIC/HEIF/HIF or JPEG image file (e.g., `IMG_1234.HEIC`)
/// - A companion MOV video file (e.g., `IMG_1234.MOV`)
///
/// Same-stem discovery is followed by Apple Content Identifier verification.
/// A proven mismatch is not a pair; missing or unreadable identity metadata is
/// retained conservatively so metadata loss cannot make a real pair unsafe.
fn directory_paths(
    parent: &Path,
) -> std::io::Result<impl Iterator<Item = std::io::Result<PathBuf>>> {
    let entries = std::fs::read_dir(parent).inspect_err(|error| {
        crate::media_conversion_gate::probe_image_format_audit(
            "live_photo_parent_read_failed",
            parent,
            format!("failed to inspect Live Photo companion directory: {error}"),
        );
    })?;
    Ok(entries.map(move |entry| {
        entry.map(|entry| entry.path()).inspect_err(|error| {
            crate::media_conversion_gate::probe_image_format_audit(
                "live_photo_directory_entry_failed",
                parent,
                format!("failed to read a Live Photo companion directory entry: {error}"),
            );
        })
    }))
}

fn regular_companions(
    paths: impl Iterator<Item = std::io::Result<PathBuf>>,
    stem: &std::ffi::OsStr,
    extensions: &[&str],
    mut observe: impl FnMut(&Path, &std::fs::Metadata) -> std::io::Result<()>,
) -> std::io::Result<Vec<PathBuf>> {
    let mut companions = Vec::new();
    for path in paths {
        let path = path?;
        let same_stem = crate::media_conversion_gate::path_file_stem_os_or_none(&path)
            .is_some_and(|candidate| candidate.eq_ignore_ascii_case(stem));
        if !same_stem {
            continue;
        }
        match std::fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.file_type().is_file() => {
                observe(&path, &metadata)?;
                if extensions.contains(&live_member_extension(&path).as_str()) {
                    companions.push(path);
                }
            }
            Ok(_) => {}
            Err(error) => {
                crate::media_conversion_gate::probe_image_format_audit(
                    "live_photo_companion_metadata_failed",
                    &path,
                    format!("failed to inspect a matching Live Photo companion: {error}"),
                );
                return Err(error);
            }
        }
    }
    companions.sort_unstable();
    Ok(companions)
}

fn parse_content_identifier_json(stdout: &[u8]) -> anyhow::Result<Option<String>> {
    let records: Vec<serde_json::Map<String, serde_json::Value>> =
        serde_json::from_slice(stdout).context("invalid ExifTool Live Photo identity JSON")?;
    anyhow::ensure!(
        records.len() == 1,
        "ExifTool returned {} Live Photo identity records for one file",
        records.len()
    );

    let mut identifier: Option<String> = None;
    for (key, value) in &records[0] {
        if key != "ContentIdentifier" && !key.ends_with(":ContentIdentifier") {
            continue;
        }
        let candidate = value
            .as_str()
            .context("ExifTool returned a non-string Apple Content Identifier")?
            .trim();
        if candidate.is_empty() {
            continue;
        }
        if let Some(existing) = identifier.as_deref() {
            anyhow::ensure!(
                existing.eq_ignore_ascii_case(candidate),
                "file contains conflicting Apple Content Identifiers"
            );
        } else {
            identifier = Some(candidate.to_string());
        }
    }
    Ok(identifier)
}

const CONTENT_IDENTIFIER_ARGS: [&str; 4] = [
    "-j",
    "-G1",
    "-Apple:ContentIdentifier",
    "-Keys:ContentIdentifier",
];

fn content_identifier(path: &Path) -> anyhow::Result<Option<String>> {
    let mut builder = crate::ExiftoolBuilder::new();
    builder.args(CONTENT_IDENTIFIER_ARGS).input(path);
    let output = builder
        .build()
        .output()
        .with_context(|| format!("failed to read Live Photo identity from {}", path.display()))?;
    read_content_identifier(path, &output)
}

fn read_content_identifier(path: &Path, output: &Output) -> anyhow::Result<Option<String>> {
    anyhow::ensure!(
        output.status.success(),
        "ExifTool could not read Live Photo identity from {}: {}",
        path.display(),
        String::from_utf8_lossy(&output.stderr).trim()
    );
    parse_content_identifier_json(&output.stdout)
}

/// Read-only evidence about two candidate files. It does not verify Photos
/// playback, successful import, or eligibility for cleanup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LivePhotoIdentifierEvidence {
    Match,
    Mismatch,
    Unavailable,
}

/// Verify identifier evidence only for actual JPEG/HEIF and QuickTime contents.
/// A missing identifier, probe error or timeout leaves the relationship unknown.
#[must_use]
pub fn inspect_identifier_pair(
    image: &Path,
    video: &Path,
    deadline: Instant,
) -> LivePhotoIdentifierEvidence {
    use crate::image::format_detect::{FormatKind, detect_regular_file_format};

    if deadline <= Instant::now()
        || !matches!(
            detect_regular_file_format(image),
            Ok(FormatKind::Jpeg | FormatKind::Heic | FormatKind::Heif)
        )
        || !matches!(detect_regular_file_format(video), Ok(FormatKind::Mov))
    {
        return LivePhotoIdentifierEvidence::Unavailable;
    }

    // A health smoke test would launch an unbounded ExifTool process before
    // the deadline. The actual read below checks execution within the budget.
    let Some(exiftool) =
        crate::common_utils::resolve_tool_path_with_health(crate::constants::TOOL_EXIFTOOL, |_| {
            true
        })
    else {
        return LivePhotoIdentifierEvidence::Unavailable;
    };
    let probe = |path: &Path| {
        let timeout = deadline
            .checked_duration_since(Instant::now())?
            .min(Duration::from_millis(500));
        let mut command = Command::new(&exiftool);
        // Disable executable Perl from ExifTool configuration. This must be
        // the first option, before either the read arguments or input path.
        command
            .args(["-config", ""])
            .args(CONTENT_IDENTIFIER_ARGS)
            .arg(crate::safe_path_arg(path).as_ref());
        let output = crate::process_runner::run_command_with_liveness_timeout(
            &mut command,
            timeout,
            timeout,
            "preflight Live Photo identifier",
        )
        .ok()?;
        read_content_identifier(path, &output).ok().flatten()
    };
    match probe(image) {
        Some(image_identifier) => {
            let video_identifier = probe(video);
            match content_identifiers_agree(Some(&image_identifier), video_identifier.as_deref()) {
                Some(true) => LivePhotoIdentifierEvidence::Match,
                Some(false) => LivePhotoIdentifierEvidence::Mismatch,
                None => LivePhotoIdentifierEvidence::Unavailable,
            }
        }
        None => LivePhotoIdentifierEvidence::Unavailable,
    }
}

fn content_identifiers_agree(left: Option<&str>, right: Option<&str>) -> Option<bool> {
    match (left, right) {
        (Some(left), Some(right)) => Some(left.trim().eq_ignore_ascii_case(right.trim())),
        _ => None,
    }
}

fn same_stem_pair_is_live(path: &Path, companion: &Path) -> bool {
    match (content_identifier(path), content_identifier(companion)) {
        (Ok(left), Ok(right)) => match content_identifiers_agree(left.as_deref(), right.as_deref())
        {
            Some(true) => true,
            Some(false) => {
                crate::media_conversion_gate::probe_image_format_audit(
                    "live_photo_identifier_mismatch",
                    path,
                    format!(
                        "same-stem companion {} has a different Apple Content Identifier; treating the files independently",
                        companion.display()
                    ),
                );
                false
            }
            None => {
                crate::media_conversion_gate::probe_image_format_audit(
                    "live_photo_identifier_unavailable",
                    path,
                    format!(
                        "Apple Content Identifier is absent from one or both same-stem files; conservatively retaining the pair with {}",
                        companion.display()
                    ),
                );
                true
            }
        },
        (Err(error), _) | (_, Err(error)) => {
            crate::media_conversion_gate::probe_image_format_audit(
                "live_photo_identifier_probe_failed",
                path,
                format!(
                    "Apple Content Identifier could not be verified; conservatively retaining same-stem pair with {}: {error}",
                    companion.display()
                ),
            );
            true
        }
    }
}

/// Protect a same-stem Apple still/MOV pair or a candidate whose companion
/// directory could not be verified. Inconclusive probes never authorize cleanup.
#[must_use]
pub fn is_live(path: &Path) -> bool {
    match find_live_companions(path) {
        Ok(companions) => !companions.is_empty(),
        // Discovery already audited the error; uncertainty only protects originals.
        Err(_) => true,
    }
}

/// Locate the companions protected by [`is_live`] for complete archive delivery.
///
/// # Errors
/// Returns an error if companion discovery is incomplete; callers must not
/// report the pair as delivered when only the requested member was copied.
pub fn find_live_companions(path: &Path) -> std::io::Result<Vec<PathBuf>> {
    find_live_companions_with(path, |parent, stem, extensions| {
        regular_companions(directory_paths(parent)?, stem, extensions, |_, _| Ok(()))
    })
}

fn find_live_companions_with(
    path: &Path,
    find: impl FnOnce(&Path, &std::ffi::OsStr, &[&str]) -> std::io::Result<Vec<PathBuf>>,
) -> std::io::Result<Vec<PathBuf>> {
    let ext_lower = live_member_extension(path);
    let Some(stem) = crate::media_conversion_gate::path_file_stem_os_or_none(path) else {
        return Ok(Vec::new());
    };
    let Some(parent) = path.parent() else {
        return Ok(Vec::new());
    };
    if stem.is_empty() {
        return Ok(Vec::new());
    }

    let companions = if LIVE_STILL_EXTENSIONS.contains(&ext_lower.as_str()) {
        find(parent, stem, &["mov"])
    } else if ext_lower == "mov" {
        find(parent, stem, LIVE_STILL_EXTENSIONS)
    } else {
        return Ok(Vec::new());
    };

    Ok(companions?
        .into_iter()
        .filter(|companion| same_stem_pair_is_live(path, companion))
        .collect())
}

/// Track directory entries and the same-stem files observed during classification.
/// Call [`Self::verify_unchanged`] before using the results.
#[derive(Default)]
pub struct LivePhotoScan {
    #[cfg(unix)]
    directories: HashMap<PathBuf, DirectoryScan>,
    companions: HashMap<PathBuf, AdmittedImageFile>,
}

impl LivePhotoScan {
    /// Inspect current metadata and content for any same-stem companions.
    ///
    /// # Errors
    /// Returns an error when directory discovery or companion inspection fails.
    pub fn is_live(&mut self, selected: &Path) -> std::io::Result<bool> {
        let companions = find_live_companions_with(selected, |parent, stem, extensions| {
            #[cfg(unix)]
            let paths = {
                let directory = match self.directories.entry(parent.to_path_buf()) {
                    std::collections::hash_map::Entry::Occupied(entry) => entry.into_mut(),
                    std::collections::hash_map::Entry::Vacant(entry) => {
                        entry.insert(DirectoryScan::read(parent)?)
                    }
                };
                directory
                    .paths_by_stem
                    .get(&stem.to_ascii_lowercase())
                    .into_iter()
                    .flatten()
                    .cloned()
                    .map(Ok)
            };
            #[cfg(not(unix))]
            let paths = directory_paths(parent)?;
            regular_companions(paths, stem, extensions, |path, metadata| {
                // A rejected format also affects whether the selected file is standalone.
                if path != selected && !self.companions.contains_key(path) {
                    self.companions.insert(
                        path.to_path_buf(),
                        AdmittedImageFile::from_metadata(path, metadata)?,
                    );
                }
                Ok(())
            })
        })?;
        Ok(!companions.is_empty())
    }

    /// # Errors
    /// Returns an error if an observed directory or companion changed or cannot be inspected.
    pub fn verify_unchanged(&self) -> std::io::Result<()> {
        #[cfg(unix)]
        for (parent, directory) in &self.directories {
            if DirectoryScan::read(parent)? != *directory {
                return Err(changed_directory_error(parent));
            }
        }
        for companion in self.companions.values() {
            companion.verify_regular_file_unchanged()?;
        }
        Ok(())
    }
}

#[cfg(unix)]
#[derive(PartialEq, Eq)]
struct DirectoryScan {
    stamp: DirectoryStamp,
    paths_by_stem: HashMap<OsString, Vec<PathBuf>>,
}

#[cfg(unix)]
impl DirectoryScan {
    fn read(parent: &Path) -> std::io::Result<Self> {
        let stamp = directory_stamp(parent)?;
        let mut paths_by_stem: HashMap<OsString, Vec<PathBuf>> = HashMap::new();
        for path in directory_paths(parent)? {
            let path = path?;
            if let Some(stem) = path.file_stem() {
                paths_by_stem
                    .entry(stem.to_ascii_lowercase())
                    .or_default()
                    .push(path);
            }
        }
        for paths in paths_by_stem.values_mut() {
            paths.sort_unstable();
        }
        if directory_stamp(parent)? != stamp {
            return Err(changed_directory_error(parent));
        }
        Ok(Self {
            stamp,
            paths_by_stem,
        })
    }
}

#[cfg(unix)]
type DirectoryStamp = (u64, u64, i64, i64, i64, i64);

#[cfg(unix)]
fn directory_stamp(parent: &Path) -> std::io::Result<DirectoryStamp> {
    use std::os::unix::fs::MetadataExt;

    let metadata = std::fs::metadata(parent)?;
    if !metadata.is_dir() {
        return Err(std::io::Error::other(format!(
            "Live Photo parent is not a directory: {}",
            parent.display()
        )));
    }
    Ok((
        metadata.dev(),
        metadata.ino(),
        metadata.ctime(),
        metadata.ctime_nsec(),
        metadata.mtime(),
        metadata.mtime_nsec(),
    ))
}

#[cfg(unix)]
fn changed_directory_error(parent: &Path) -> std::io::Error {
    std::io::Error::other(format!(
        "Directory changed during Live Photo discovery: {}; rescan required",
        parent.display()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use tempfile::TempDir;

    #[cfg(unix)]
    #[test]
    fn indexed_companions_match_live_scan_and_reject_changed_namespace() -> std::io::Result<()> {
        let root = TempDir::new()?;
        let still = root.path().join("img_0001.JPG");
        let single = root.path().join("img_0002.jpg");
        File::create(&still)?;
        File::create(&single)?;
        File::create(root.path().join("IMG_0001.MOV"))?;

        let mut scan = LivePhotoScan::default();
        assert!(scan.is_live(&still)?);
        assert!(!scan.is_live(&single)?);
        scan.verify_unchanged()?;

        File::create(root.path().join("IMG_0002.MOV"))?;
        assert!(is_live(&single));
        assert!(scan.verify_unchanged().is_err());
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn indexed_companions_reject_in_place_changes_to_unselected_file() -> std::io::Result<()> {
        use std::io::Write;

        let root = TempDir::new()?;
        let still = root.path().join("img_0001.JPG");
        let motion = root.path().join("IMG_0001.media");
        File::create(&still)?;
        let mut file = File::create(&motion)?;
        file.write_all(b"plain text input")?;
        let timestamp = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_600_000_000);
        file.set_times(std::fs::FileTimes::new().set_modified(timestamp))?;
        let original = file.metadata()?;
        let directory = DirectoryScan::read(root.path())?;

        let mut scan = LivePhotoScan::default();
        assert!(!scan.is_live(&still)?);
        scan.verify_unchanged()?;

        use std::io::Seek;
        file.rewind()?;
        file.write_all(b"\x00\x00\x00\x10ftypqt  \x00\x00\x00\x00")?;
        file.set_times(std::fs::FileTimes::new().set_modified(timestamp))?;
        let changed = file.metadata()?;
        assert_eq!(changed.len(), original.len());
        assert_eq!(changed.modified()?, original.modified()?);
        assert!(DirectoryScan::read(root.path())? == directory);
        assert!(is_live(&still));
        assert!(scan.verify_unchanged().is_err());
        Ok(())
    }

    #[test]
    fn test_live_photo_detection() {
        let temp_dir = TempDir::new().unwrap_or_else(|e| {
            unreachable!("CRITICAL: Live Photo test setup failed (error: {:?})", e)
        });
        let base_path = temp_dir.path();

        // Create a Live Photo pair
        let heic_path = base_path.join("IMG_1234.HEIC");
        let mov_path = base_path.join("IMG_1234.MOV");

        File::create(&heic_path).unwrap_or_else(|e| {
            unreachable!("CRITICAL: Live Photo test setup failed (error: {:?})", e)
        });
        File::create(&mov_path).unwrap_or_else(|e| {
            unreachable!("CRITICAL: Live Photo test setup failed (error: {:?})", e)
        });

        // Both files should be detected as Live Photo
        assert!(is_live(&heic_path));
        assert!(is_live(&mov_path));

        // Single HEIC without MOV should not be Live Photo
        let single_heic = base_path.join("IMG_5678.HEIC");
        File::create(&single_heic).unwrap_or_else(|e| {
            unreachable!("CRITICAL: Live Photo test setup failed (error: {:?})", e)
        });
        assert!(!is_live(&single_heic));

        // Single MOV without HEIC should not be Live Photo
        let single_mov = base_path.join("VID_9999.MOV");
        File::create(&single_mov).unwrap_or_else(|e| {
            unreachable!("CRITICAL: Live Photo test setup failed (error: {:?})", e)
        });
        assert!(!is_live(&single_mov));
    }

    #[test]
    fn test_case_insensitive() {
        let temp_dir = TempDir::new().unwrap_or_else(|e| {
            unreachable!("CRITICAL: Live Photo test setup failed (error: {:?})", e)
        });
        let base_path = temp_dir.path();

        // Test lowercase heic with uppercase MOV
        let heic_lower = base_path.join("IMG_0001.heic");
        let mov_upper = base_path.join("IMG_0001.MOV");

        File::create(&heic_lower).unwrap_or_else(|e| {
            unreachable!("CRITICAL: Live Photo test setup failed (error: {:?})", e)
        });
        File::create(&mov_upper).unwrap_or_else(|e| {
            unreachable!("CRITICAL: Live Photo test setup failed (error: {:?})", e)
        });

        assert!(is_live(&heic_lower));
        assert!(is_live(&mov_upper));
    }

    #[test]
    fn jpeg_and_mov_pair_is_a_live_photo() -> std::io::Result<()> {
        let temp_dir = TempDir::new()?;
        let jpeg = temp_dir.path().join("IMG_0002.JPG");
        let mov = temp_dir.path().join("IMG_0002.mov");
        File::create(&jpeg)?;
        File::create(&mov)?;

        assert!(is_live(&jpeg));
        assert!(is_live(&mov));
        Ok(())
    }

    #[test]
    fn content_identifier_parser_accepts_both_apple_groups_and_rejects_conflicts() {
        let apple = br#"[{"SourceFile":"still.heic","Apple:ContentIdentifier":" AABB-CCDD "}]"#;
        let quicktime = br#"[{"SourceFile":"motion.mov","Keys:ContentIdentifier":"aabb-ccdd"}]"#;
        assert_eq!(
            parse_content_identifier_json(apple).expect("Apple identifier JSON"),
            Some("AABB-CCDD".to_string())
        );
        assert_eq!(
            parse_content_identifier_json(quicktime).expect("QuickTime identifier JSON"),
            Some("aabb-ccdd".to_string())
        );
        assert_eq!(
            content_identifiers_agree(Some("AABB-CCDD"), Some("aabb-ccdd")),
            Some(true)
        );
        assert_eq!(
            content_identifiers_agree(Some("AABB-CCDD"), Some("different")),
            Some(false)
        );
        assert_eq!(content_identifiers_agree(Some("AABB-CCDD"), None), None);

        let conflicting = br#"[{"Apple:ContentIdentifier":"one","Keys:ContentIdentifier":"two"}]"#;
        assert!(parse_content_identifier_json(conflicting).is_err());
    }

    #[test]
    fn live_pair_protection_uses_content_identity_for_jpeg_aliases() -> anyhow::Result<()> {
        for extension in ["jpe", "jfif", "png", ""] {
            let root = TempDir::new()?;
            let still = root.path().join("img_0042").with_extension(extension);
            let motion = root.path().join("IMG_0042.MOV");
            image::RgbImage::from_pixel(8, 8, image::Rgb([40, 80, 120]))
                .save_with_format(&still, image::ImageFormat::Jpeg)?;
            File::create(&motion)?;
            assert_eq!(
                crate::image::format_detect::detect_true_format(&still)?,
                crate::image::format_detect::FormatKind::Jpeg
            );
            assert!(
                is_live(&still),
                "JPEG content with .{extension} bypassed protection"
            );
            assert!(
                is_live(&motion),
                "MOV could not discover the .{extension} JPEG companion"
            );
        }
        Ok(())
    }

    #[test]
    fn failed_companion_directory_probe_does_not_authorize_standalone_processing()
    -> std::io::Result<()> {
        let root = TempDir::new()?;
        let parent = root.path().join("unreadable-parent");
        File::create(&parent)?;
        assert!(
            is_live(&parent.join("IMG_0043.JPG")),
            "a failed directory probe must retain a possible Live Photo member"
        );
        Ok(())
    }

    // Darwin filesystems reject these filenames before the detector is reached.
    // The portable OsStr boundary is also tested without filesystem I/O in the gate.
    #[cfg(target_os = "linux")]
    #[test]
    fn live_pair_protection_preserves_non_utf8_stems() -> std::io::Result<()> {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;

        let root = TempDir::new()?;
        let still = root.path().join(OsStr::from_bytes(b"img_\xff.JPG"));
        let motion = root.path().join(OsStr::from_bytes(b"IMG_\xff.MOV"));
        let different = root.path().join("img_\u{fffd}.JPG");
        File::create(&still)?;
        File::create(&motion)?;
        File::create(&different)?;

        assert!(is_live(&still), "native still stem must match its MOV");
        assert!(is_live(&motion), "native MOV stem must match its still");
        assert!(
            !is_live(&different),
            "lossy text must not alias a native stem"
        );
        Ok(())
    }
}
