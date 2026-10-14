//! Read-only launcher routing plans. A plan is not a conversion or delivery proof.

use crate::infra::drag_drop::{ProcessingFilter, ScanRoute, inspect_content};
use anyhow::{Context, Result, ensure};
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Serialize)]
pub struct RoutingPlan {
    pub input: PathBuf,
    pub output_requested: Option<PathBuf>,
    pub in_place_requested: bool,
    pub files_discovered: usize,
    pub selected_images: usize,
    pub selected_videos: usize,
    pub filtered_media: usize,
    pub sidecars: usize,
    pub other_files: usize,
    pub selected_media_bytes: u64,
    pub entries: Vec<PlannedEntry>,
    pub relationships: Vec<RelationshipObservation>,
    pub storage: StorageObservation,
}

/// Destination filesystem capacity only, not a required-space estimate or reservation.
#[derive(Debug, Serialize)]
pub struct StorageObservation {
    pub status: &'static str,
    pub directory: Option<PathBuf>,
    pub available_bytes: Option<u64>,
    pub diagnostic: Option<String>,
}

impl Default for StorageObservation {
    fn default() -> Self {
        Self {
            status: "unknown",
            directory: None,
            available_bytes: None,
            diagnostic: Some("destination_not_bound".to_owned()),
        }
    }
}

impl StorageObservation {
    #[must_use]
    pub fn observe(directory: Result<PathBuf>) -> Self {
        Self::observe_with(directory, crate::infra::system_checks::get_disk_free_space)
    }

    fn observe_with(directory: Result<PathBuf>, probe: impl FnOnce(&Path) -> Result<u64>) -> Self {
        let observed = directory.and_then(|path| {
            let directory = std::fs::canonicalize(&path).with_context(|| {
                format!("resolve storage observation directory {}", path.display())
            })?;
            ensure!(
                std::fs::metadata(&directory)?.is_dir(),
                "storage observation path is not a directory: {}",
                directory.display()
            );
            let available_bytes = probe(&directory)?;
            Ok((directory, available_bytes))
        });
        match observed {
            Ok((directory, available_bytes)) => Self {
                status: "observed",
                directory: Some(directory),
                available_bytes: Some(available_bytes),
                diagnostic: None,
            },
            Err(error) => Self {
                status: "unknown",
                directory: None,
                available_bytes: None,
                diagnostic: Some(format!("storage observation failed: {error:#}")),
            },
        }
    }
}

#[derive(Debug, Serialize)]
pub struct PlannedEntry {
    pub relative_path: PathBuf,
    pub route: ScanRoute,
    pub selected: bool,
    pub bytes: u64,
    pub action: &'static str,
    pub reason: &'static str,
}

/// Filename candidates only; these observations never authorize delivery or cleanup.
#[derive(Debug, Serialize)]
pub struct RelationshipObservation {
    pub kind: &'static str,
    pub source_relative_path: PathBuf,
    pub match_basis: &'static str,
    pub status: &'static str,
    pub candidate_relative_paths: Vec<PathBuf>,
    pub candidate_count: usize,
    pub candidates_truncated: bool,
    pub verified: bool,
}

// Bound ambiguous groups without hiding their full candidate counts.
const MAX_RELATIONSHIP_CANDIDATES: usize = 64;

#[derive(Default)]
struct StemCandidates<'a> {
    media: Vec<&'a PlannedEntry>,
    image_count: usize,
    videos: Vec<&'a PlannedEntry>,
}

fn observe_relationship(
    kind: &'static str,
    source: &PlannedEntry,
    match_basis: &'static str,
    candidates: &[&PlannedEntry],
    ambiguous: bool,
) -> RelationshipObservation {
    RelationshipObservation {
        kind,
        source_relative_path: source.relative_path.clone(),
        match_basis,
        status: if candidates.is_empty() {
            "unmatched"
        } else if ambiguous || candidates.len() > 1 {
            "ambiguous"
        } else {
            "candidate"
        },
        candidate_relative_paths: candidates
            .iter()
            .take(MAX_RELATIONSHIP_CANDIDATES)
            .map(|entry| entry.relative_path.clone())
            .collect(),
        candidate_count: candidates.len(),
        candidates_truncated: candidates.len() > MAX_RELATIONSHIP_CANDIDATES,
        verified: false,
    }
}

fn filename_relationships(entries: &[PlannedEntry]) -> Vec<RelationshipObservation> {
    let mut by_filename = BTreeMap::new();
    let mut by_stem: BTreeMap<PathBuf, StemCandidates<'_>> = BTreeMap::new();
    for entry in entries {
        if matches!(entry.route, ScanRoute::Image | ScanRoute::Video) {
            by_filename.insert(entry.relative_path.as_path(), entry);
            let group = by_stem
                .entry(entry.relative_path.with_extension(""))
                .or_default();
            group.media.push(entry);
            if entry.route == ScanRoute::Image {
                group.image_count += 1;
            } else {
                group.videos.push(entry);
            }
        }
    }
    let mut observations = Vec::new();
    for entry in entries {
        let stem_path = entry.relative_path.with_extension("");
        match entry.route {
            ScanRoute::Sidecar => {
                let exact = by_filename.get(stem_path.as_path());
                if let Some(candidate) = exact {
                    observations.push(observe_relationship(
                        "xmp_filename",
                        entry,
                        "exact_filename",
                        std::slice::from_ref(candidate),
                        false,
                    ));
                } else {
                    let candidates = by_stem
                        .get(&stem_path)
                        .map_or(&[][..], |group| group.media.as_slice());
                    observations.push(observe_relationship(
                        "xmp_filename",
                        entry,
                        "same_directory_stem",
                        candidates,
                        false,
                    ));
                }
            }
            ScanRoute::Image => {
                if let Some(group) = by_stem.get(&stem_path)
                    && !group.videos.is_empty()
                {
                    observations.push(observe_relationship(
                        "live_photo_filename",
                        entry,
                        "same_directory_stem",
                        &group.videos,
                        group.image_count > 1,
                    ));
                }
            }
            ScanRoute::Video | ScanRoute::CopyOnly => {}
        }
    }
    observations
}

impl RoutingPlan {
    /// Uses the execution scanner's content, hidden-file, and link policies.
    /// Does not create output, logs, databases, locks, or launch workers.
    pub fn inspect(
        input: &Path,
        output: Option<&Path>,
        filter: ProcessingFilter,
        in_place: bool,
    ) -> Result<Self> {
        let scan = inspect_content(input, filter)?;
        let mut entries = Vec::with_capacity(scan.entries.len());
        for entry in scan.entries {
            entry.source.verify_unchanged().with_context(|| {
                format!(
                    "preflight source changed before plan publication: {}",
                    entry.source.path().display()
                )
            })?;
            let (action, reason) = match (entry.route, entry.selected) {
                (ScanRoute::Image, true) => ("submit_img", "content_routed_to_image_worker"),
                (ScanRoute::Video, true) => ("submit_vid", "content_routed_to_video_worker"),
                (ScanRoute::Image | ScanRoute::Video, false) => {
                    ("retain", "excluded_by_selected_media_filter")
                }
                (ScanRoute::Sidecar, _) => (
                    "preserve_sidecar",
                    "metadata_sidecar_not_a_separate_media_task",
                ),
                (ScanRoute::CopyOnly, _) if in_place => {
                    ("retain", "non_media_source_retained_in_place")
                }
                (ScanRoute::CopyOnly, _) => (
                    "copy_after_success",
                    "non_media_copy_is_conditional_on_successful_adjacent_processing",
                ),
            };
            entries.push(PlannedEntry {
                relative_path: entry.relative_path,
                route: entry.route,
                selected: entry.selected,
                bytes: entry.source.bytes(),
                action,
                reason,
            });
        }
        ensure!(
            entries.len() == scan.total_files,
            "preflight inventory is incomplete"
        );
        let filtered_media = entries
            .iter()
            .filter(|entry| {
                !entry.selected && matches!(entry.route, ScanRoute::Image | ScanRoute::Video)
            })
            .count();
        let accounted = [
            scan.img_count,
            scan.vid_count,
            filtered_media,
            scan.xmp_count,
            scan.other_count,
        ]
        .into_iter()
        .try_fold(0_usize, |total, value| total.checked_add(value))
        .context("preflight inventory count overflow")?;
        ensure!(
            accounted == scan.total_files,
            "preflight inventory categories do not reconcile"
        );
        let relationships = filename_relationships(&entries);
        Ok(Self {
            input: input.to_path_buf(),
            output_requested: output.map(Path::to_path_buf),
            in_place_requested: in_place,
            files_discovered: scan.total_files,
            selected_images: scan.img_count,
            selected_videos: scan.vid_count,
            filtered_media,
            sidecars: scan.xmp_count,
            other_files: scan.other_count,
            selected_media_bytes: scan.media_total_size,
            entries,
            relationships,
            storage: StorageObservation::default(),
        })
    }
}

#[derive(Debug, Serialize)]
pub struct PreflightReport {
    pub schema_version: u8,
    pub scope: &'static str,
    pub read_only: bool,
    pub execution_started: bool,
    pub plans: Vec<RoutingPlan>,
    pub workers: Vec<WorkerObservation>,
}

/// A readable executable-file observation, not an encoder capability or execution proof.
#[derive(Debug, Serialize)]
pub struct WorkerObservation {
    pub name: &'static str,
    pub path: PathBuf,
    pub status: &'static str,
    pub bytes: Option<u64>,
    pub executable_check: &'static str,
}

impl WorkerObservation {
    #[must_use]
    pub fn inspect(name: &'static str, path: PathBuf) -> Self {
        let mut observation = Self {
            name,
            path,
            status: "unreadable",
            bytes: None,
            executable_check: if cfg!(unix) {
                "unix_mode_bits"
            } else {
                "unsupported_platform"
            },
        };
        let metadata = match std::fs::metadata(&observation.path) {
            Ok(metadata) => metadata,
            Err(error) => {
                if error.kind() == std::io::ErrorKind::NotFound {
                    observation.status = "missing";
                }
                return observation;
            }
        };
        if !metadata.is_file() {
            observation.status = "not_regular_file";
            return observation;
        }
        let mut options = std::fs::OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            // A replacement FIFO must not hang a read-only preflight.
            options.custom_flags(libc::O_NONBLOCK);
        }
        let metadata = match options
            .open(&observation.path)
            .and_then(|file| file.metadata())
        {
            Ok(metadata) if metadata.is_file() => metadata,
            Ok(_) => {
                observation.status = "not_regular_file";
                return observation;
            }
            Err(_) => return observation,
        };
        observation.bytes = Some(metadata.len());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            observation.status = if metadata.permissions().mode() & 0o111 == 0 {
                "not_executable"
            } else {
                "available"
            };
        }
        #[cfg(not(unix))]
        {
            observation.status = "executable_unknown";
        }
        observation
    }
}

impl PreflightReport {
    #[must_use]
    pub const fn new(plans: Vec<RoutingPlan>, workers: Vec<WorkerObservation>) -> Self {
        Self {
            schema_version: 2,
            scope: "launcher_routing_snapshot",
            read_only: true,
            execution_started: false,
            plans,
            workers,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn storage_observation_preserves_known_zero_and_does_not_create_outputs() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let observed = StorageObservation::observe_with(Ok(temp.path().to_path_buf()), |path| {
            ensure!(
                path == fs::canonicalize(temp.path())?,
                "unexpected directory"
            );
            Ok(0)
        });
        assert_eq!(observed.status, "observed");
        assert_eq!(observed.available_bytes, Some(0));
        assert_eq!(observed.directory, Some(fs::canonicalize(temp.path())?));
        assert!(observed.diagnostic.is_none());
        assert_eq!(fs::read_dir(temp.path())?.count(), 0);
        let json = serde_json::to_value(observed)?;
        assert_eq!(json["available_bytes"], 0);
        assert!(json["diagnostic"].is_null());
        Ok(())
    }

    #[test]
    fn storage_observation_keeps_unbound_missing_non_directory_and_probe_errors_unknown()
    -> Result<()> {
        let temp = tempfile::tempdir()?;
        let missing = temp.path().join("missing");
        let file = temp.path().join("not-a-directory");
        fs::write(&file, b"synthetic")?;
        let called = std::cell::Cell::new(false);
        for directory in [
            Err(anyhow::anyhow!("unresolved destination")),
            Ok(missing.clone()),
            Ok(file),
        ] {
            let observation = StorageObservation::observe_with(directory, |_| {
                called.set(true);
                Ok(123)
            });
            assert_eq!(observation.status, "unknown");
            assert!(observation.directory.is_none());
            assert!(observation.available_bytes.is_none());
            assert!(
                observation
                    .diagnostic
                    .is_some_and(|value| !value.is_empty())
            );
        }
        assert!(!called.get());
        assert!(!missing.exists());
        let failed = StorageObservation::observe_with(Ok(temp.path().to_path_buf()), |_| {
            Err(anyhow::anyhow!("synthetic capacity probe failed"))
        });
        assert_eq!(failed.status, "unknown");
        assert!(failed.available_bytes.is_none());
        assert!(
            failed
                .diagnostic
                .is_some_and(|value| value.contains("synthetic capacity probe failed"))
        );
        let default = serde_json::to_value(StorageObservation::default())?;
        assert_eq!(default["status"], "unknown");
        assert_eq!(default["diagnostic"], "destination_not_bound");
        assert!(default["directory"].is_null());
        assert!(default["available_bytes"].is_null());
        assert_eq!(fs::read_dir(temp.path())?.count(), 1);
        Ok(())
    }

    fn planned(path: impl Into<PathBuf>, route: ScanRoute) -> PlannedEntry {
        PlannedEntry {
            relative_path: path.into(),
            route,
            selected: false,
            bytes: 1,
            action: "retain",
            reason: "synthetic_inventory",
        }
    }

    #[test]
    fn worker_observation_is_read_only_and_does_not_execute() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let path = temp.path().join("worker");
        assert_eq!(
            WorkerObservation::inspect("img", path.clone()).status,
            "missing"
        );
        assert_eq!(
            WorkerObservation::inspect("img", temp.path().to_path_buf()).status,
            "not_regular_file"
        );
        let script = b"#!/bin/sh\ntouch should-not-exist\n";
        fs::write(&path, script)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
            assert_eq!(
                WorkerObservation::inspect("img", path.clone()).status,
                "not_executable"
            );
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700))?;
            let observation = WorkerObservation::inspect("img", path.clone());
            assert_eq!(observation.status, "available");
            assert_eq!(observation.bytes, Some(u64::try_from(script.len())?));
            assert_eq!(observation.executable_check, "unix_mode_bits");
        }
        #[cfg(not(unix))]
        assert_eq!(
            WorkerObservation::inspect("img", path.clone()).status,
            "executable_unknown"
        );
        assert_eq!(fs::read(&path)?, script);
        assert_eq!(fs::read_dir(temp.path())?.count(), 1);
        Ok(())
    }

    #[test]
    fn sidecars_use_exact_names_before_same_directory_stems() -> Result<()> {
        let entries = vec![
            planned("one/photo.jpg", ScanRoute::Image),
            planned("one/photo.png", ScanRoute::Image),
            planned("one/photo.jpg.xmp", ScanRoute::Sidecar),
            planned("one/photo.xmp", ScanRoute::Sidecar),
            planned("one/Photo.xmp", ScanRoute::Sidecar),
            planned("two/photo.jpg", ScanRoute::Image),
            planned("two/photo.xmp", ScanRoute::Sidecar),
            planned("two/orphan.xmp", ScanRoute::Sidecar),
            planned("two/disguised.xmp", ScanRoute::Image),
            planned("two/notes.txt", ScanRoute::CopyOnly),
            planned("two/notes.xmp", ScanRoute::Sidecar),
            planned("one/\u{56fe}\u{50cf}\n\".jpg", ScanRoute::Image),
            planned("one/\u{56fe}\u{50cf}\n\".xmp", ScanRoute::Sidecar),
        ];
        let observations = filename_relationships(&entries);
        assert_eq!(observations.len(), 7);
        assert_eq!(observations[0].match_basis, "exact_filename");
        assert_eq!(observations[0].status, "candidate");
        assert_eq!(
            observations[0].candidate_relative_paths,
            [PathBuf::from("one/photo.jpg")]
        );
        assert_eq!(observations[1].status, "ambiguous");
        assert_eq!(observations[1].candidate_count, 2);
        assert_eq!(observations[2].status, "unmatched");
        assert_eq!(
            observations[3].candidate_relative_paths,
            [PathBuf::from("two/photo.jpg")]
        );
        assert_eq!(observations[4].status, "unmatched");
        assert_eq!(observations[5].status, "unmatched");
        assert_eq!(
            observations[6].candidate_relative_paths,
            [PathBuf::from("one/\u{56fe}\u{50cf}\n\".jpg")]
        );
        assert!(observations.iter().all(|observation| !observation.verified));
        assert!(
            entries
                .iter()
                .all(|entry| entry.action == "retain" && !entry.selected)
        );
        let document = serde_json::to_value(&observations)?;
        assert_eq!(
            document[6]["source_relative_path"],
            "one/\u{56fe}\u{50cf}\n\".xmp"
        );
        Ok(())
    }

    #[test]
    fn live_photo_candidates_require_routed_images_and_videos_and_remain_bounded() {
        let mut entries = vec![
            planned("one/pair.bin", ScanRoute::Image),
            planned("one/pair.weird", ScanRoute::Video),
            planned("one/pair.mov", ScanRoute::CopyOnly),
            planned("two/pair.mp4", ScanRoute::Video),
            planned("three/ambiguous.jpg", ScanRoute::Image),
            planned("three/ambiguous.png", ScanRoute::Image),
            planned("three/ambiguous.mov", ScanRoute::Video),
            planned("four/disguised.xmp", ScanRoute::Image),
            planned("four/disguised.mov", ScanRoute::Video),
            planned("five/many.jpg", ScanRoute::Image),
            planned("five/many.xmp", ScanRoute::Sidecar),
        ];
        for index in 0..=MAX_RELATIONSHIP_CANDIDATES {
            entries.push(planned(format!("five/many.clip{index}"), ScanRoute::Video));
        }
        let observations = filename_relationships(&entries);
        assert_eq!(observations.len(), 6);
        assert_eq!(observations[0].kind, "live_photo_filename");
        assert_eq!(observations[0].status, "candidate");
        assert_eq!(
            observations[0].candidate_relative_paths,
            [PathBuf::from("one/pair.weird")]
        );
        assert_eq!(observations[1].status, "ambiguous");
        assert_eq!(observations[2].status, "ambiguous");
        assert_eq!(observations[3].kind, "live_photo_filename");
        assert_eq!(
            observations[3].source_relative_path,
            PathBuf::from("four/disguised.xmp")
        );
        assert_eq!(
            observations[4].candidate_count,
            MAX_RELATIONSHIP_CANDIDATES + 1
        );
        assert_eq!(
            observations[5].candidate_count,
            MAX_RELATIONSHIP_CANDIDATES + 2
        );
        for observation in &observations[4..] {
            assert_eq!(observation.status, "ambiguous");
            assert_eq!(
                observation.candidate_relative_paths.len(),
                MAX_RELATIONSHIP_CANDIDATES
            );
            assert!(observation.candidates_truncated);
            assert!(!observation.verified);
        }
    }

    #[cfg(unix)]
    #[test]
    fn filename_relationships_do_not_merge_non_utf8_path_bytes() {
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt;

        let media = PathBuf::from(OsString::from_vec(b"dir/a\xff.jpg".to_vec()));
        let sidecar = PathBuf::from(OsString::from_vec(b"dir/a\xff.xmp".to_vec()));
        let entries = [
            planned(media.clone(), ScanRoute::Image),
            planned(
                PathBuf::from(OsString::from_vec(b"dir/a\xfe.jpg".to_vec())),
                ScanRoute::Image,
            ),
            planned(sidecar.clone(), ScanRoute::Sidecar),
        ];
        let observations = filename_relationships(&entries);
        assert_eq!(observations[0].candidate_relative_paths, [media]);
        assert_eq!(observations[0].source_relative_path, sidecar);
        assert!(serde_json::to_value(&observations).is_err());
    }

    #[test]
    fn routing_plan_uses_content_policy_and_is_explicit_about_conditional_actions() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let input = temp.path().join("input");
        let output = temp.path().join("output");
        fs::create_dir(&input)?;
        let jpeg = b"\xff\xd8\xff\xe0synthetic";
        fs::write(input.join("spoofed.bin"), jpeg)?;
        fs::write(input.join("metadata.xmp"), b"synthetic metadata")?;
        fs::write(input.join("notes.txt"), b"synthetic notes")?;
        fs::write(input.join(".hidden.jpg"), jpeg)?;
        let plan = RoutingPlan::inspect(&input, Some(&output), ProcessingFilter::Both, false)?;
        assert_eq!(
            (
                plan.files_discovered,
                plan.selected_images,
                plan.sidecars,
                plan.other_files
            ),
            (3, 1, 1, 1)
        );
        assert_eq!(plan.selected_media_bytes, u64::try_from(jpeg.len())?);
        assert_eq!(plan.entries[0].action, "preserve_sidecar");
        assert_eq!(plan.entries[1].action, "copy_after_success");
        assert_eq!(plan.entries[2].action, "submit_img");
        assert_eq!(plan.relationships.len(), plan.sidecars);
        assert_eq!(plan.relationships[0].status, "unmatched");
        let document = serde_json::to_value(PreflightReport::new(vec![plan], Vec::new()))?;
        assert_eq!(document["schema_version"], 2);
        assert_eq!(document["read_only"], true);
        assert_eq!(document["execution_started"], false);
        assert_eq!(document["plans"][0]["storage"]["status"], "unknown");
        assert!(document["plans"][0]["storage"]["available_bytes"].is_null());
        assert_eq!(fs::read(input.join("spoofed.bin"))?, jpeg);
        assert!(!output.exists());
        assert_eq!(fs::read_dir(temp.path())?.count(), 1);

        let filtered = RoutingPlan::inspect(&input, None, ProcessingFilter::VideosOnly, true)?;
        assert_eq!(filtered.entries[1].action, "retain");
        assert_eq!(
            filtered.entries[2].reason,
            "excluded_by_selected_media_filter"
        );
        assert_eq!(filtered.selected_images, 0);
        assert_eq!(filtered.filtered_media, 1);
        Ok(())
    }

    #[test]
    fn incomplete_plan_is_an_error_not_an_empty_success() -> Result<()> {
        let temp = tempfile::tempdir()?;
        assert!(
            RoutingPlan::inspect(
                &temp.path().join("missing"),
                None,
                ProcessingFilter::Both,
                false
            )
            .is_err()
        );
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(temp.path().join("missing"), temp.path().join("broken"))?;
            assert!(
                RoutingPlan::inspect(temp.path(), None, ProcessingFilter::Both, false).is_err()
            );
        }
        Ok(())
    }
}
