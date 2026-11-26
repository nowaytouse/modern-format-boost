//! Promote verified Fast IMG candidates without exposing unjournaled final names.
#![expect(
    clippy::redundant_pub_crate,
    reason = "Publication operations stay inside the CLI; public visibility conflicts with unreachable_pub"
)]

use super::{FastImgTranscodeProof, JpegExecutionAttempt, WorkingCopyMarker};
use anyhow::{Context, Result, ensure};
use foundation::fast_img_store::{self, PendingPublication};
use foundation::pipeline::verification::{
    validate_no_symlink_components, verify_regular_file_blake3,
};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, UNIX_EPOCH};

const PREFIX: &str = ".mfb-fast-img-publish-";

#[derive(Debug)]
pub(super) struct PublicationConflict;

impl std::fmt::Display for PublicationConflict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("cannot establish ownership of the occupied Fast IMG destination")
    }
}
impl std::error::Error for PublicationConflict {}

#[derive(Debug)]
pub(super) struct PublicationMissing;

impl std::fmt::Display for PublicationMissing {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("both the prepared Fast IMG candidate and its destination are absent")
    }
}
impl std::error::Error for PublicationMissing {}

#[derive(Serialize, Deserialize, PartialEq, Eq)]
struct Identity {
    volume: u64,
    file: u64,
    // Signed creation time, including dates before 1970 from preserved metadata.
    created: Option<(bool, Duration)>,
}

impl Identity {
    #[cfg(any(unix, windows))]
    fn read(metadata: &fs::Metadata) -> Result<Self> {
        #[cfg(unix)]
        let (volume, file) = {
            use std::os::unix::fs::MetadataExt;
            (metadata.dev(), metadata.ino())
        };
        #[cfg(windows)]
        let (volume, file) = {
            use std::os::windows::fs::MetadataExt;
            (
                u64::from(
                    metadata
                        .volume_serial_number()
                        .context("publication volume identity unavailable")?,
                ),
                metadata
                    .file_index()
                    .context("publication file identity unavailable")?,
            )
        };
        Ok(Self {
            volume,
            file,
            created: metadata
                .created()
                .ok()
                .map(|time| match time.duration_since(UNIX_EPOCH) {
                    Ok(duration) => (true, duration),
                    Err(before_epoch) => (false, before_epoch.duration()),
                }),
        })
    }

    #[cfg(not(any(unix, windows)))]
    fn read(_: &fs::Metadata) -> Result<Self> {
        anyhow::bail!("native file identity is unavailable for Fast IMG publication")
    }
}

#[derive(Clone, Copy, Serialize, Deserialize)]
enum StageLocation {
    Scratch,
    OutputParent,
}

impl StageLocation {
    fn base(self, working_copy: &Path) -> Result<PathBuf> {
        match self {
            Self::Scratch => Ok(foundation::process_lock::get_mfb_tmp_dir()?.canonicalize()?),
            Self::OutputParent => Ok(working_copy
                .parent()
                .context("working copy has no parent")?
                .canonicalize()?),
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Evidence<T = FastImgTranscodeProof> {
    location: StageLocation,
    directory: Identity,
    file: Identity,
    proof: T,
    attempt: Option<JpegExecutionAttempt>,
}

pub(super) fn new_stage(working_copy: &Path) -> Result<tempfile::TempDir> {
    let scratch = StageLocation::Scratch.base(working_copy)?;
    let volume = Identity::read(&fs::metadata(working_copy)?)?.volume;
    let base = if Identity::read(&fs::metadata(&scratch)?)?.volume == volume {
        scratch
    } else {
        StageLocation::OutputParent.base(working_copy)?
    };
    ensure!(
        Identity::read(&fs::metadata(&base)?)?.volume == volume,
        "private publication stage and destination require the same volume"
    );
    let mut builder = tempfile::Builder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        builder.permissions(fs::Permissions::from_mode(0o700));
    }
    Ok(builder.prefix(PREFIX).tempdir_in(base)?)
}

pub(super) fn prepare(
    marker: &WorkingCopyMarker,
    stage: tempfile::TempDir,
    proof: &FastImgTranscodeProof,
    attempt: Option<JpegExecutionAttempt>,
) -> Result<PendingPublication> {
    let root = stage.path();
    let relative = super::fast_img_checked_rel_path(&proof.out_rel)?;
    validate_no_symlink_components(root, &relative).map_err(anyhow::Error::msg)?;
    let path = root.join(relative);
    let meta = fs::symlink_metadata(&path)?;
    ensure!(
        meta.is_file(),
        "private publication candidate must be a regular file"
    );
    let location =
        if root.parent() == Some(StageLocation::Scratch.base(&marker.working_copy)?.as_path()) {
            StageLocation::Scratch
        } else {
            ensure!(
                root.parent()
                    == Some(
                        StageLocation::OutputParent
                            .base(&marker.working_copy)?
                            .as_path()
                    ),
                "publication stage outside the task's authorized locations"
            );
            StageLocation::OutputParent
        };
    foundation::io_utils::sync_committed_file_and_parent(&path)?;
    let record = PendingPublication {
        source_rel: proof.rel_key.clone(),
        source_blake3: proof.src_hash.clone(),
        output_rel: proof.out_rel.clone(),
        output_blake3: proof.out_hash.clone(),
        stage_name: root
            .file_name()
            .and_then(|name| name.to_str())
            .context("publication stage has no valid name")?
            .to_owned(),
        proof: serde_json::to_value(Evidence {
            location,
            directory: Identity::read(&fs::metadata(root)?)?,
            file: Identity::read(&meta)?,
            proof,
            attempt,
        })?,
    };
    fast_img_store::prepare_publication(marker, &record)?;
    let _owned_by_journal = stage.keep();
    Ok(record)
}

fn observed(path: &Path) -> io::Result<Option<fs::Metadata>> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => Ok(Some(metadata)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

struct Paths {
    root: PathBuf,
    staged: PathBuf,
    target: PathBuf,
}

fn resolve(marker: &WorkingCopyMarker, record: &PendingPublication) -> Result<(Evidence, Paths)> {
    let evidence: Evidence = serde_json::from_value(record.proof.clone())?;
    ensure!(
        evidence.proof.rel_key == record.source_rel
            && evidence.proof.out_rel == record.output_rel
            && evidence.proof.src_hash == record.source_blake3
            && evidence.proof.out_hash == record.output_blake3,
        "publication evidence does not match its recorded bindings"
    );
    let name = Path::new(&record.stage_name);
    ensure!(
        record.stage_name.starts_with(PREFIX)
            && name.components().count() == 1
            && name.file_name() == Some(name.as_os_str()),
        "invalid private publication stage name"
    );
    let root = evidence.location.base(&marker.working_copy)?.join(name);
    if let Some(meta) = observed(&root)? {
        ensure!(
            meta.is_dir() && Identity::read(&meta)? == evidence.directory,
            "private publication stage was replaced"
        );
    }
    let relative = super::fast_img_checked_rel_path(&record.output_rel)?;
    validate_no_symlink_components(&root, &relative).map_err(anyhow::Error::msg)?;
    if let Some(parent) = relative.parent() {
        validate_no_symlink_components(&marker.working_copy, parent).map_err(anyhow::Error::msg)?;
    }
    let staged = root.join(&relative);
    let target = marker.working_copy.join(relative);
    Ok((
        evidence,
        Paths {
            root,
            staged,
            target,
        },
    ))
}

fn owns(meta: &fs::Metadata, expected: &Identity) -> Result<bool> {
    Ok(meta.is_file() && Identity::read(meta)? == *expected)
}

pub(super) fn publish(
    marker: &WorkingCopyMarker,
    record: &PendingPublication,
) -> Result<(FastImgTranscodeProof, Option<JpegExecutionAttempt>)> {
    publish_candidate(marker, record, true)
}

pub(super) fn publish_new(
    marker: &WorkingCopyMarker,
    record: &PendingPublication,
) -> Result<(FastImgTranscodeProof, Option<JpegExecutionAttempt>)> {
    publish_candidate(marker, record, false)
}

pub(super) fn verify_published_identity(
    marker: &WorkingCopyMarker,
    record: &PendingPublication,
) -> Result<()> {
    let (evidence, paths) = resolve(marker, record)?;
    ensure!(
        owns(&fs::symlink_metadata(paths.target)?, &evidence.file)?,
        "published candidate was replaced before its final receipt"
    );
    Ok(())
}

fn publish_candidate(
    marker: &WorkingCopyMarker,
    record: &PendingPublication,
    replay: bool,
) -> Result<(FastImgTranscodeProof, Option<JpegExecutionAttempt>)> {
    let (evidence, paths) = resolve(marker, record)?;
    let staged = observed(&paths.staged)?;
    if let Some(meta) = &staged {
        ensure!(
            owns(meta, &evidence.file)?,
            "private publication candidate was replaced"
        );
    }
    let target = observed(&paths.target)?;
    if let Some(meta) = &target {
        if !owns(meta, &evidence.file)? {
            return Err(PublicationConflict.into());
        }
        // IDs may be recycled after the last link disappears. A pending intent
        // alone cannot claim a final-only file after an interrupted rename.
        if staged.is_none()
            && !marker
                .blake3_log
                .get(&record.source_rel)
                .is_some_and(|entry| {
                    entry.out == record.output_blake3
                        && entry.src == record.source_blake3
                        && entry.out_rel.as_ref() == Some(&record.output_rel)
                })
        {
            return Err(PublicationConflict.into());
        }
    } else {
        if staged.is_none() {
            return Err(PublicationMissing.into());
        }
        if replay {
            verify_regular_file_blake3(&paths.staged, &record.output_blake3)
                .context("private publication candidate bytes have changed")?;
        }
        foundation::ensure_parent_dir_exists(&paths.target)?;
        match fs::hard_link(&paths.staged, &paths.target) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                return Err(PublicationConflict.into());
            }
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::Unsupported | io::ErrorKind::PermissionDenied
                ) =>
            {
                // Preserve all metadata on filesystems which support a same-volume
                // no-replace rename but do not support hard links.
                let mut temp = tempfile::TempPath::try_from_path(paths.staged.clone())?;
                temp.disable_cleanup(true);
                if let Err(error) = temp.persist_noclobber(&paths.target) {
                    let error = error.error;
                    if error.kind() == io::ErrorKind::AlreadyExists {
                        return Err(PublicationConflict.into());
                    }
                    return Err(error.into());
                }
            }
            Err(error) => return Err(error.into()),
        }
    }
    let meta = fs::symlink_metadata(&paths.target)?;
    ensure!(
        owns(&meta, &evidence.file)?,
        "published Fast IMG file identity changed"
    );
    verify_regular_file_blake3(&paths.target, &record.output_blake3)
        .context("published Fast IMG file bytes changed")?;
    ensure!(
        owns(&fs::symlink_metadata(&paths.target)?, &evidence.file)?,
        "published Fast IMG file was replaced during verification"
    );
    foundation::io_utils::sync_parent_directory(&paths.target)?;
    Ok((evidence.proof, evidence.attempt))
}

pub(super) fn finish(marker: &WorkingCopyMarker, record: &PendingPublication) -> Result<()> {
    let (evidence, paths) = resolve(marker, record)?;
    if let Some(meta) = observed(&paths.staged)? {
        ensure!(
            owns(&meta, &evidence.file)?,
            "cannot retire replaced private publication candidate"
        );
        #[cfg(unix)]
        foundation::io_utils::remove_verified_fast_img_output_from_root(
            &paths.root,
            &paths.staged,
            &record.output_blake3,
            &fs::symlink_metadata(&paths.root)?,
            &meta,
        )?;
        #[cfg(not(unix))]
        {
            verify_regular_file_blake3(&paths.staged, &record.output_blake3)?;
            ensure!(
                owns(&fs::symlink_metadata(&paths.staged)?, &evidence.file)?,
                "private publication candidate changed before retirement"
            );
            fs::remove_file(&paths.staged)?;
        }
    }
    // Only remove the recorded empty directories, never recursively sweep a
    // shared scratch parent or another user's files.
    for path in paths
        .staged
        .ancestors()
        .skip(1)
        .take_while(|p| p.starts_with(&paths.root))
    {
        match fs::remove_dir(path) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    fast_img_store::forget_publication(marker, record)?;
    Ok(())
}

/// Commit a worker outcome and tell the progress reporter whether it completed.
pub(super) fn record_result(
    marker: &mut WorkingCopyMarker,
    result: super::FastImgJobResult,
    summary: &mut super::FastImgEncodeResultSummary,
    source_root: &Path,
    working_copy: &Path,
) -> Result<bool> {
    let attempt = super::fast_img_execution_attempt()?;
    let mut published = None;
    let result = match result {
        Ok(super::FastImgTranscodeOutcome::Converted(mut proof)) => {
            let stage = proof
                .stage
                .take()
                .context("verified output lacks private publication stage")?;
            let record = prepare(marker, stage, &proof, attempt.clone())?;
            match publish_new(marker, &record) {
                Ok((proof, _)) => {
                    published = Some(record);
                    Ok(super::FastImgTranscodeOutcome::Converted(Box::new(proof)))
                }
                Err(error)
                    if error.is::<PublicationConflict>()
                        && !super::fast_img_may_have_primary_photos_submission(marker) =>
                {
                    // Retire only our staged candidate. If interrupted before the
                    // failure receipt commits, normal planning can retry this input.
                    finish(marker, &record)?;
                    Err(super::FastImgTranscodeError {
                        rel_key: proof.rel_key,
                        src_hash: proof.src_hash,
                        reason: format!(
                            "{error}: {}; source and existing output retained",
                            record.output_rel
                        ),
                        reason_code: Some("output_name_conflict".into()),
                    })
                }
                Err(error) => return Err(error),
            }
        }
        other => other,
    };
    let completed = result.is_ok();
    super::fast_img_apply_encode_results_at_attempt(
        marker,
        [result],
        summary,
        source_root,
        working_copy,
        super::FileFailurePhase::EncodeOrPublish,
        attempt,
    )?;
    if let Some(record) = published {
        finish(marker, &record)?;
    }
    Ok(completed)
}
