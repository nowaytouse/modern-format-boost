//! Inspect GIF originals in place or admit them to Photos custody with an exact byte hash.
#![expect(
    clippy::redundant_pub_crate,
    reason = "the worker exposes these helpers only within the binary crate"
)]
use super::fast_img_ignored::{IgnoredInventory, IgnoredReason};
use super::fast_img_result::ProbeFailures;
use super::{
    FastImgInputScope, FastImgRunOptions, FastImgSourceInventory, fast_img_file_failure_line,
};
use anyhow::{Context, Result};
use foundation::ModernLossyStaticCandidate;
use foundation::batch::AdmittedImageFile;
use foundation::fast_img_store::FastImgAdmissionPlan;
use foundation::image::format_detect::FormatKind;
use foundation::metadata::XmpSidecarIndex;
use foundation::pipeline::verification::fast_img_relative_key;
use foundation::pipeline::verification::{FastImgStageName, WorkingCopyMarker};
use std::collections::BTreeMap;
use std::path::Path;

pub(super) fn validate_selection(input: &Path) -> Result<()> {
    match std::fs::symlink_metadata(input) {
        Ok(metadata) => anyhow::ensure!(
            !metadata.file_type().is_symlink(),
            "select the original GIF file or folder directly; the selection is a symbolic link"
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
        Err(error) => return Err(error.into()),
    }
    Ok(())
}

/// Use the same full stream decoder for both local retention and Photos admission.
fn inspect_gifs<'a>(
    observed: &'a [AdmittedImageFile],
    mut classify: impl FnMut(&'a AdmittedImageFile, std::io::Result<bool>) -> Result<()>,
) -> Result<()> {
    let progress = foundation::progress::FileStageProgress::new(
        observed.len() as u64,
        foundation::progress::FileProgressStage::FastImgSourcePreparation,
    );
    for (index, source) in observed.iter().enumerate() {
        foundation::batch_control::checkpoint()?;
        classify(
            source,
            foundation::media_meta_utils::is_verified_gif(source.path()),
        )?;
        progress.set((index + 1) as u64);
    }
    progress.finish();
    Ok(())
}

pub(super) fn verify_local(options: FastImgRunOptions<'_>) -> Result<()> {
    validate_selection(options.input)?;
    let scope = FastImgInputScope::observe(options.input, options.recursive.0)?;
    anyhow::ensure!(
        options
            .planned_scope
            .is_none_or(|expected| expected == scope.token()),
        "GIF input selection changed after preview. Review the selection and start again."
    );
    let root = scope.src_root.clone();
    let (observed, witness) = scope.into_scan();
    let mut ignored = Vec::new();
    let mut skipped = 0;
    let mut failed = 0;
    if options.dry_run.0 {
        println!("[DRY RUN ] Verify original GIFs and keep them in place.");
    }
    inspect_gifs(&observed, |source, result| {
        let path = source.path();
        let rel = fast_img_relative_key(path, &root, "GIF original").map_err(anyhow::Error::msg)?;
        match result {
            Ok(true) => {
                skipped += 1;
                println!(
                    "[SKIP    ]   {}: valid GIF original retained in place",
                    rel.replace(char::is_control, "�")
                );
            }
            Ok(false) => ignored.push((path.to_owned(), IgnoredReason::OutsideCurrentPolicyScope)),
            Err(error) => {
                failed += 1;
                report_failure(path, &rel, &error.to_string());
            }
        }
        Ok(())
    })?;
    for source in &observed {
        source.verify_regular_file_unchanged()?;
    }
    witness.verify_unchanged()?;
    validate_selection(options.input)?;
    let expected_ignored = observed
        .len()
        .checked_sub(skipped + failed)
        .context("overlapping GIF local verification results")?;
    let ignored = IgnoredInventory::new(&root, ignored, expected_ignored)?;
    ignored.emit_markers_from_environment()?;
    super::fast_img_result::print_processor_counts(0, skipped, failed, ignored.count(), 0);
    anyhow::ensure!(
        failed == 0,
        "Local GIF verification failed for {failed} file(s); originals retained."
    );
    Ok(())
}

/// A GIF task with receipts for its entire immutable plan only needs to reverify its
/// existing Photos assets. A newer decoder may recognize additional files in the folder.
pub(super) fn pending_receipt_scope(
    marker: &WorkingCopyMarker,
) -> Result<Option<FastImgAdmissionPlan>> {
    if marker.strategy != "gif"
        || marker.stage != FastImgStageName::CleanupComplete
        || !marker.tier2_in_progress
        || !marker.tier2_cleanup_journal
        || marker.photos_library_path.is_none()
        || marker.tier2_imported_assets.is_empty()
    {
        return Ok(None);
    }
    let Some(plan) = foundation::fast_img_store::read_admission_plan(marker)? else {
        return Ok(None);
    };
    if plan.len() != marker.tier2_imported_assets.len() {
        return Ok(None);
    }
    // Loading the saved marker rejects duplicate receipt paths and UUIDs.
    for asset in &marker.tier2_imported_assets {
        anyhow::ensure!(
            asset
                .photos_uuid
                .as_ref()
                .is_some_and(|uuid| !uuid.is_empty())
                && plan
                    .get(&asset.rel_path)
                    .is_some_and(|entry| entry.matches_gif_receipt(asset)),
            "saved GIF Photos receipt differs from its original admission; sources retained"
        );
    }
    Ok(Some(plan))
}

pub(super) fn scan_originals(
    observed: &[AdmittedImageFile],
    root: &Path,
    saved_scope: Option<&FastImgAdmissionPlan>,
) -> Result<FastImgSourceInventory> {
    let mut verified = Vec::new();
    let mut ignored = Vec::new();
    let mut failures = ProbeFailures::new(0);
    let mut live_photos = foundation::live_photo::LivePhotoScan::default();
    let mut deferred = 0;
    inspect_gifs(observed, |source, result| {
        let path = source.path();
        // Files outside the old admission can neither expand this recovery nor add new failures.
        if let Some(saved) = saved_scope
            && !matches!(result, Ok(false))
        {
            let rel =
                fast_img_relative_key(path, root, "GIF original").map_err(anyhow::Error::msg)?;
            if !saved.contains_key(&rel) {
                ignored.push((path.to_owned(), IgnoredReason::OutsideSavedGifTask));
                deferred += 1;
                return Ok(());
            }
        }
        match result {
            Ok(false) => ignored.push((path.to_owned(), IgnoredReason::OutsideCurrentPolicyScope)),
            Ok(true) if live_photos.is_live(path)? => {
                ignored.push((path.to_owned(), IgnoredReason::LivePhotoPairMember));
            }
            Ok(true) => verified.push(source),
            Err(error) => {
                let rel = fast_img_relative_key(path, root, "GIF original")
                    .map_err(anyhow::Error::msg)?;
                record_failure(&mut failures, source, &rel, &error.to_string());
            }
        }
        Ok(())
    })?;
    let paths: Vec<_> = verified
        .iter()
        .map(|source| source.path().to_owned())
        .collect();
    let sidecars = XmpSidecarIndex::capture_xmp_and_aae(&paths)?;
    let mut originals = Vec::with_capacity(verified.len());
    for source in verified {
        foundation::batch_control::checkpoint()?;
        let path = source.path();
        let rel = fast_img_relative_key(path, root, "GIF original").map_err(anyhow::Error::msg)?;
        let admission = (|| -> Result<String> {
            anyhow::ensure!(
                sidecars.find_aae(path)?.is_none(),
                "GIF has an AAE edit companion without a Photos adjustment receipt; source and companion retained"
            );
            if let Some(xmp) = sidecars.find(path)? {
                foundation::metadata::validate_xmp_sidecar(&xmp)?;
            }
            Ok(source.content_blake3()?)
        })();
        match admission {
            Ok(blake3) => originals.push(ModernLossyStaticCandidate {
                path: path.to_owned(),
                rel_path: rel,
                format: FormatKind::Gif,
                blake3,
            }),
            Err(error) => record_failure(&mut failures, source, &rel, &error.to_string()),
        }
    }
    sidecars.verify_unchanged()?;
    live_photos.verify_unchanged()?;
    for source in observed {
        source.verify_regular_file_unchanged()?;
    }
    let expected_ignored = observed
        .len()
        .checked_sub(originals.len() + failures.total)
        .context("overlapping GIF source classifications")?;
    if deferred > 0 {
        println!(
            "[RESUME  ] {deferred} file(s) outside the saved GIF Photos task retained in place. After verified cleanup, start a new task (--no-resume) for the remaining files."
        );
    }
    println!(
        "[FASTVID ] selected {} verified GIF original(s) for Apple Photos",
        originals.len()
    );
    Ok(FastImgSourceInventory {
        ignored: IgnoredInventory::new(root, ignored, expected_ignored)?,
        modern_lossy_candidates: originals,
        modern_probe_failures: failures,
        source_files: Vec::new(),
        source_hashes: BTreeMap::new(),
        scan_failures: BTreeMap::new(),
        planned_encode_count: 0,
    })
}

fn record_failure(
    failures: &mut ProbeFailures,
    source: &AdmittedImageFile,
    rel: &str,
    reason: &str,
) {
    failures.total += 1;
    match source.content_blake3() {
        Ok(hash) => {
            failures.hashes.insert(rel.to_owned(), hash);
        }
        Err(error) => {
            tracing::warn!(target: "fast_vid", source_rel = rel, %error, "GIF failure hash unavailable");
        }
    }
    report_failure(source.path(), rel, reason);
}

fn report_failure(path: &Path, rel: &str, reason: &str) {
    super::log_file_failure_at_phase(
        "img",
        path,
        reason,
        None,
        super::FileFailurePhase::Admission,
    );
    println!("{}", fast_img_file_failure_line(rel, reason));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Cli, scan_fast_img_sources_with_policy};
    use clap::Parser;
    use std::fs;

    const HEADER: &[u8] = b"GIF89a\x01\x00\x01\x00\x80\x00\x00\x00\x00\x00\xff\xff\xff";
    const FRAME: &[u8] = b",\x00\x00\x00\x00\x01\x00\x01\x00\x00\x02\x02\x44\x01\x00";

    #[test]
    fn valid_gifs_alone_enter_original_delivery_without_any_encoder_probe() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let root = fs::canonicalize(dir.path())?;
        let single = [HEADER, FRAME, b";"].concat();
        let animated = [HEADER, FRAME, FRAME, b";"].concat();
        let inputs = [
            ("no-extension", single.as_slice()),
            ("animated.mp4", animated.as_slice()),
            ("original.GIF", single.as_slice()),
            ("movie.MOV", animated.as_slice()),
            ("still.jpeg", animated.as_slice()),
            ("web.webp", animated.as_slice()),
            ("unknown.anything", animated.as_slice()),
            ("disguised-x.xmp", animated.as_slice()),
            ("disguised-a.aae", animated.as_slice()),
            ("broken.gif", HEADER),
            ("pretend.gif", b"not an image".as_slice()),
            ("with-xmp.gif", single.as_slice()),
            ("with-xmp.xmp", b"<xmp/>".as_slice()),
            ("with-aae.gif", single.as_slice()),
            ("with-aae.AAE", b"<plist/>".as_slice()),
        ];
        let mut observed = Vec::new();
        for (name, contents) in inputs {
            let path = root.join(name);
            fs::write(&path, contents)?;
            observed.push(AdmittedImageFile::from_metadata(
                &path,
                &fs::symlink_metadata(&path)?,
            )?);
        }
        let inventory = scan_fast_img_sources_with_policy(&observed, &root, "gif", false, |_| {
            anyhow::bail!("unexpected Fast IMG format probe")
        })?;
        assert_eq!(
            (inventory.source_files.len(), inventory.planned_encode_count),
            (0, 0)
        );
        assert_eq!(inventory.ignored.count(), 3);
        assert_eq!(inventory.modern_probe_failures.total, 2);
        assert_eq!(inventory.modern_probe_failures.hashes.len(), 2);
        let plan = inventory.admission_plan(&root)?;
        assert_eq!(plan.len(), 10);
        assert!(matches!(
            &plan["with-xmp.gif"].xmp,
            Some(foundation::fast_img_store::FastImgAdmissionSidecar::Present { blake3, .. })
                if blake3 == &foundation::common_utils::blake3_hex(b"<xmp/>")
        ));
        for (name, contents) in inputs.iter().take(9) {
            let admitted = &plan[*name];
            assert_eq!(
                admitted.route,
                foundation::fast_img_store::FastImgAdmissionRoute::Original
            );
            assert_eq!(
                admitted.source_blake3,
                foundation::common_utils::blake3_hex(contents)
            );
        }
        assert_eq!(fs::read_dir(&root)?.count(), inputs.len());
        Ok(())
    }

    #[test]
    fn gif_worker_accepts_local_and_photos_but_rejects_encoder_settings() -> Result<()> {
        let args = ["img", "fast-img", "sample", "--strategy", "gif"];
        let cli = Cli::try_parse_from(args)?;
        crate::validate_command_strategy(&cli.command)?;
        for flags in [
            &["--output", "working"][..],
            &["--retry"],
            &["--retry-failed"],
            &["--delete-source"],
        ] {
            let local = Cli::try_parse_from(args.into_iter().chain(flags.iter().copied()))?;
            assert!(crate::validate_command_strategy(&local.command).is_err());
        }
        let cli = Cli::try_parse_from(args.into_iter().chain(["--shortest-path"]))?;
        crate::validate_command_strategy(&cli.command)?;
        let config = foundation::runtime_config::RuntimeConfig::default();
        let contract = crate::fast_img_processing_contract(&config, "gif", false, false)?;
        assert!(matches!(
            contract.encoding,
            crate::FastImgEncodingParameters::Gif
        ));
        assert!(contract.validate("jxl").is_err());
        for settings in [["--jpeg-effort", "6"], ["--avif-speed", "4"]] {
            let cli =
                Cli::try_parse_from(args.into_iter().chain(["--shortest-path"]).chain(settings))?;
            assert!(cli.policy.validate_fast_strategy("gif").is_err());
        }
        Ok(())
    }

    #[test]
    fn single_gif_never_admits_its_sibling_or_a_selected_link() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let root = fs::canonicalize(dir.path())?;
        let gif = [HEADER, FRAME, b";"].concat();
        let selected = root.join("selected");
        fs::write(&selected, &gif)?;
        fs::write(root.join("sibling.gif"), &gif)?;
        let scope = foundation::batch::fast_img_scope::FastImgInputScope::observe(&selected, true)?;
        assert!(!scope.selected_is_directory);
        let (observed, witness) = scope.into_scan();
        let inventory = scan_originals(&observed, &root, None)?;
        let plan = inventory.admission_plan(&root)?;
        assert_eq!(plan.keys().collect::<Vec<_>>(), ["selected"]);
        witness.verify_unchanged()?;
        #[cfg(unix)]
        {
            let link = root.join("shortcut.gif");
            std::os::unix::fs::symlink(&selected, &link)?;
            assert!(validate_selection(&link).is_err());
            assert_eq!(fs::read(&selected)?, gif);
        }
        Ok(())
    }
}
