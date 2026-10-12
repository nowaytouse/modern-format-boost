//! Read-only launcher routing plans. A plan is not a conversion or delivery proof.

use crate::infra::drag_drop::{ProcessingFilter, ScanRoute, inspect_content};
use anyhow::{Context, Result, ensure};
use serde::Serialize;
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
}

impl PreflightReport {
    #[must_use]
    pub const fn new(plans: Vec<RoutingPlan>) -> Self {
        Self {
            schema_version: 1,
            scope: "launcher_routing_snapshot",
            read_only: true,
            execution_started: false,
            plans,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

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
        let document = serde_json::to_value(PreflightReport::new(vec![plan]))?;
        assert_eq!(document["schema_version"], 1);
        assert_eq!(document["read_only"], true);
        assert_eq!(document["execution_started"], false);
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
