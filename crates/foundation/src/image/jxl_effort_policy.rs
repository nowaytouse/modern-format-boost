//! Shared JPEG XL effort selection policy.
//!
//! The policy is intentionally centralized so JPEG bitstream encode and
//! direct JXL encode paths do not drift. Effort is an encoder policy, not a
//! quality-search axis: effort search selects one domain before quality search by default.
//!
//! ## Encoder tiers
//! Direct pixel encoding uses bounded search by default with these mode ceilings;
//! explicit runtime targets can select 1 through 11 (e11 needs the expert switch).
//!   - Normal   → e7  (`ultimate = false`)
//!   - Ultimate → e10 (`ultimate = true`)
//!
//! JPEG bitstream transcode is separate: full search compares e1 through e11.
//! Manual fixed/bounded modes retain their verified compatibility fallback;
//! full search never substitutes a different effort for an unavailable candidate.
//! Pixel e11 is explicit only, because it can be disproportionately slow.
//!
//! `archive` is a product mode, not a third encoder tier. Normalize it to the
//! Ultimate tier here so individual encode paths cannot drift:
//!
//! ```rust,ignore
//! encoder_effort_for_mode(ultimate, archive)
//! ```

use crate::constants;

/// Keep quality-parameter search separate from effort selection and source-quality inference.
#[must_use]
pub fn quality_search_enabled(default: bool) -> bool {
    crate::runtime_config::active()
        .map_or_else(crate::runtime_config::QualitySearch::default, |config| {
            config.img.quality_search
        })
        .enabled(default)
}

/// Compare bounded efforts at a fixed quality before entering one distance-search domain.
#[must_use]
pub fn effort_candidates(kind: JxlEffortContext, selected: u8) -> Vec<u8> {
    use crate::runtime_config::{EffortSearch, JpegEffortSearch};
    let config = crate::runtime_config::active();
    let jpeg_search = config.map_or_else(JpegEffortSearch::default, |config| {
        config.img.jpeg_effort_search
    });
    let adaptive = match kind {
        JxlEffortContext::DirectEncode => {
            config.map_or_else(EffortSearch::default, |config| config.img.jxl_effort_search)
                == EffortSearch::Adaptive
        }
        JxlEffortContext::JpegLosslessTranscode => jpeg_search == JpegEffortSearch::Adaptive,
    };
    if kind == JxlEffortContext::JpegLosslessTranscode && jpeg_search == JpegEffortSearch::Full {
        return (1..=11).collect();
    }
    if !adaptive {
        return vec![selected];
    }
    let domain: &[u8] = match kind {
        JxlEffortContext::DirectEncode => &[7, 8, 10],
        JxlEffortContext::JpegLosslessTranscode => &[1, 7, 8, 10, 11],
    };
    let mut efforts: Vec<u8> = domain
        .iter()
        .copied()
        .filter(|effort| *effort < selected)
        .collect();
    efforts.push(selected);
    efforts
}

/// Compare verified payloads at one immutable input/quality domain; ties retain lower effort.
/// A single candidate preserves the caller's existing verification and recovery path.
/// # Errors
/// Rejects invalid plans, changed inputs, I/O or verification failures without publishing a partial winner.
pub fn run_effort_search<T>(
    input: &std::path::Path,
    output: &std::path::Path,
    efforts: &[u8],
    mut run: impl FnMut(u8, &std::path::Path) -> anyhow::Result<T>,
    successful: impl Fn(&T) -> bool,
    mut verify_payload: impl FnMut(&std::path::Path) -> anyhow::Result<u64>,
) -> anyhow::Result<(T, u8)> {
    use anyhow::Context;
    anyhow::ensure!(
        !efforts.is_empty()
            && efforts.iter().all(|effort| (1..=11).contains(effort))
            && efforts.windows(2).all(|pair| pair[0] < pair[1]),
        "invalid effort search plan"
    );
    if let [effort] = efforts {
        return Ok((run(*effort, output)?, *effort));
    }
    let identity = crate::common_utils::calculate_blake3_hash(input)?;
    let parent = output.parent().context("effort search output parent")?;
    let scratch = tempfile::Builder::new()
        .prefix("mfb-effort-")
        .tempdir_in(parent)?;
    let mut best = None;
    for effort in efforts {
        let candidate = scratch.path().join(format!("e{effort}.jxl"));
        let started = std::time::Instant::now();
        let observation = run(*effort, &candidate)?;
        if !successful(&observation) {
            anyhow::ensure!(
                crate::common_utils::calculate_blake3_hash(input)? == identity,
                "effort search input changed during failed encoding"
            );
            crate::log_detail!(&format!(
                "Effort search stopped at requested e{effort}: encoder failure; no candidate selected"
            ));
            return Ok((observation, *effort));
        }
        anyhow::ensure!(
            std::fs::symlink_metadata(&candidate)?.file_type().is_file(),
            "effort search candidate is not a regular file"
        );
        let size = verify_payload(&candidate)?;
        anyhow::ensure!(size > 0, "effort search candidate has no media payload");
        anyhow::ensure!(
            crate::common_utils::calculate_blake3_hash(input)? == identity,
            "effort search input changed"
        );
        crate::log_detail!(&format!(
            "Effort search candidate requested e{effort}: payload={size}B, elapsed_ms={}",
            started.elapsed().as_millis()
        ));
        if best
            .as_ref()
            .is_none_or(|(_, _, previous_size, _)| size < *previous_size)
        {
            if let Some((_, _, _, previous)) = best.take() {
                std::fs::remove_file(previous).context("remove superseded effort candidate")?;
            }
            best = Some((observation, *effort, size, candidate));
        } else {
            std::fs::remove_file(candidate).context("remove larger effort candidate")?;
        }
    }
    let (observation, effort, size, candidate) =
        best.context("effort search has no verified candidate")?;
    anyhow::ensure!(
        crate::common_utils::calculate_blake3_hash(input)? == identity,
        "effort search input changed before selection"
    );
    std::fs::rename(candidate, output)
        .context("publish selected effort candidate to caller staging")?;
    crate::log_detail!(&format!(
        "Effort search selected requested e{effort}: payload={size}B, tried={} (permitted compatibility retries are logged separately; final delivery verification remains required)",
        efforts.len()
    ));
    Ok((observation, effort))
}

/// Compare every independent JPEG reconstruction effort using complete candidate files.
///
/// The run callback must finish identical archive metadata preparation for each effort.
/// Only contenders are decoded; all eleven encodes must complete before publication.
/// # Errors
/// Any incomplete candidate, failed reconstruction, changed input or changed winner
/// rejects the full search without publishing a partial minimum.
pub fn run_jpeg_full_effort_search<T>(
    input: &std::path::Path,
    output: &std::path::Path,
    mut run: impl FnMut(u8, &std::path::Path) -> anyhow::Result<T>,
    successful: impl Fn(&T) -> bool,
    mut verify_reconstruction: impl FnMut(&std::path::Path) -> anyhow::Result<()>,
) -> anyhow::Result<(T, u8)> {
    use anyhow::Context;
    anyhow::ensure!(
        input != output,
        "JPEG effort staging must not replace its source"
    );
    if output.try_exists()? {
        anyhow::ensure!(
            !crate::checkpoint::files_alias_same_inode(input, output)?,
            "JPEG effort staging aliases its source"
        );
    }
    let identity = crate::common_utils::calculate_blake3_hash(input)?;
    let scratch = tempfile::Builder::new()
        .prefix("mfb-jpeg-full-effort-")
        .tempdir_in(output.parent().context("JPEG full effort output parent")?)?;
    #[cfg(unix)]
    std::fs::set_permissions(
        scratch.path(),
        std::os::unix::fs::PermissionsExt::from_mode(0o700),
    )?;
    let result = (|| {
        let mut best = None;
        for effort in 1..=11 {
            // Pausing here would retain the file guard needed by the outer pause acknowledgment.
            anyhow::ensure!(
                !crate::batch_control::is_cancelled()?,
                "JPEG full effort search cancelled; no candidate selected"
            );
            let candidate = scratch.path().join(format!("e{effort}.jxl"));
            let started = std::time::Instant::now();
            let observation = run(effort, &candidate)
                .with_context(|| format!("JPEG full effort search incomplete at e{effort}"))?;
            anyhow::ensure!(
                successful(&observation),
                "JPEG full effort search incomplete: e{effort} encoding failed"
            );
            let metadata = std::fs::symlink_metadata(&candidate)
                .with_context(|| format!("JPEG full effort search incomplete: e{effort} output"))?;
            anyhow::ensure!(
                metadata.file_type().is_file() && metadata.len() > 0,
                "JPEG full effort search incomplete: e{effort} output is not a nonempty regular file"
            );
            anyhow::ensure!(
                !crate::checkpoint::files_alias_same_inode(input, &candidate)?,
                "JPEG full effort search incomplete: e{effort} candidate aliases its source"
            );
            let size = metadata.len();
            anyhow::ensure!(
                crate::common_utils::calculate_blake3_hash(input)? == identity,
                "JPEG full effort search incomplete: input changed after e{effort}"
            );
            let contender = best
                .as_ref()
                .is_none_or(|(_, _, best_size, _, _)| size < *best_size);
            if contender {
                let candidate_hash = crate::common_utils::calculate_blake3_hash(&candidate)?;
                verify_reconstruction(&candidate).with_context(|| {
                    format!("JPEG full effort search incomplete: e{effort} reconstruction")
                })?;
                anyhow::ensure!(
                    std::fs::symlink_metadata(&candidate)?.file_type().is_file()
                        && crate::common_utils::calculate_blake3_hash(&candidate)?
                            == candidate_hash,
                    "JPEG full effort search incomplete: e{effort} candidate changed during verification"
                );
                anyhow::ensure!(
                    crate::common_utils::calculate_blake3_hash(input)? == identity,
                    "JPEG full effort search incomplete: input changed during e{effort} verification"
                );
                if let Some((_, _, _, previous, _)) = best.take() {
                    std::fs::remove_file(previous)
                        .context("remove superseded JPEG effort candidate")?;
                }
                best = Some((observation, effort, size, candidate, candidate_hash));
            } else {
                std::fs::remove_file(candidate).context("remove larger JPEG effort candidate")?;
            }
            crate::log_detail!(&format!(
                "JPEG full effort candidate e{effort}: file={size}B, reconstruction_verified={contender}, elapsed_ms={}",
                started.elapsed().as_millis()
            ));
        }
        let (observation, effort, size, candidate, verified_hash) =
            best.context("JPEG full effort search has no verified candidate")?;
        anyhow::ensure!(
            !crate::batch_control::is_cancelled()?,
            "JPEG full effort search cancelled before publication"
        );
        anyhow::ensure!(
            crate::common_utils::calculate_blake3_hash(input)? == identity,
            "JPEG full effort search incomplete: input changed before selection"
        );
        let metadata = std::fs::symlink_metadata(&candidate)?;
        anyhow::ensure!(
            metadata.file_type().is_file()
                && metadata.len() == size
                && crate::common_utils::calculate_blake3_hash(&candidate)? == verified_hash,
            "JPEG full effort search incomplete: verified winner changed before selection"
        );
        std::fs::rename(candidate, output).context("publish complete JPEG effort search winner")?;
        crate::log_detail!(&format!(
            "JPEG full effort selected e{effort}: file={size}B, covered=11/11, equivalent_efforts_skipped=0, winning_file_reused=true; final delivery verification remains required"
        ));
        Ok((observation, effort))
    })();
    let cleanup = scratch
        .close()
        .context("clean owned JPEG effort search scratch");
    match (result, cleanup) {
        (result, Ok(())) => result,
        (Ok(_), Err(error)) => Err(error),
        (Err(error), Err(cleanup)) => {
            Err(error.context(format!("scratch cleanup also failed: {cleanup:#}")))
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JxlEffortPlan {
    Single(u8),
}

impl JxlEffortPlan {
    #[must_use]
    pub const fn effort(self) -> u8 {
        match self {
            Self::Single(effort) => effort,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JxlEffortContext {
    DirectEncode,
    JpegLosslessTranscode,
}

#[must_use]
pub fn encoder_effort(ultimate: bool) -> u8 {
    direct_encoder_effort(constants::jxl_effort_for_mode(ultimate))
}

/// Resolve a direct pixel encode effort without changing distance or verification.
/// The default is caller-specific: mode effort for most sources, e10 for PNG.
#[must_use]
pub fn direct_encoder_effort(default: u8) -> u8 {
    resolve_direct_effort(
        crate::infra::runtime_config::active().and_then(|config| config.img.jxl_effort),
        default,
    )
}

fn resolve_direct_effort(configured: Option<u8>, default: u8) -> u8 {
    configured.unwrap_or(default)
}

/// Map product flags onto the two encoder tiers.
#[must_use]
pub const fn effective_ultimate(ultimate: bool, archive: bool) -> bool {
    ultimate || archive
}

/// Resolve product flags and select one of the two encoder tiers.
#[must_use]
pub fn encoder_effort_for_mode(ultimate: bool, archive: bool) -> u8 {
    encoder_effort(effective_ultimate(ultimate, archive))
}

/// Build the encoder-effort plan from product flags without exposing their
/// normalization to every encode path.
#[must_use]
pub fn effort_plan_for_mode(
    kind: JxlEffortContext,
    ultimate: bool,
    archive: bool,
) -> Vec<JxlEffortPlan> {
    effort_plan(kind, effective_ultimate(ultimate, archive))
}

/// Build the encoder-effort plan.
///
/// The final-domain plan intentionally contains one item. Explicit effort search
/// selects that domain separately; distance exploration stays within that effort.
/// JPEG bitstream transcode is deliberately isolated
/// from direct pixel encoding because its e11 implementation has different
/// performance characteristics.
#[must_use]
pub fn effort_plan(kind: JxlEffortContext, ultimate: bool) -> Vec<JxlEffortPlan> {
    let effort = match kind {
        JxlEffortContext::DirectEncode => encoder_effort(ultimate),
        JxlEffortContext::JpegLosslessTranscode => jpeg_transcode_effort(
            crate::infra::runtime_config::active().map(|config| config.img.jpeg_effort),
        ),
    };
    vec![JxlEffortPlan::Single(effort)]
}

fn jpeg_transcode_effort(configured: Option<u8>) -> u8 {
    configured.unwrap_or(constants::JXL_EXPERIMENTAL_LOSSLESS_EFFORT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jpeg_full_search_covers_all_efforts_and_reuses_nonmonotonic_minimum() -> anyhow::Result<()> {
        let root = tempfile::tempdir()?;
        let source = root.path().join("source");
        let output = root.path().join("output");
        std::fs::write(&source, b"synthetic JPEG input")?;
        let mut encoded = Vec::new();
        let mut verified = Vec::new();
        let sizes = [95, 95, 90, 90, 90, 91, 89, 85, 70, 72, 72];
        let (observation, effort) = run_jpeg_full_effort_search(
            &source,
            &output,
            |effort, candidate| {
                encoded.push(effort);
                std::fs::write(candidate, vec![effort; sizes[usize::from(effort - 1)]])?;
                assert!(std::fs::read_dir(candidate.parent().unwrap())?.count() <= 2);
                Ok(effort)
            },
            |_| true,
            |candidate| {
                verified.push(std::fs::read(candidate)?[0]);
                Ok(())
            },
        )?;
        assert_eq!(encoded, (1..=11).collect::<Vec<_>>());
        assert_eq!(verified, [1, 3, 7, 8, 9]);
        assert_eq!((observation, effort), (9, 9));
        assert_eq!(std::fs::read(&output)?, vec![9; 70]);
        assert_eq!(std::fs::read_dir(root.path())?.count(), 2);
        let mut decodes = 0;
        let ((), effort) = run_jpeg_full_effort_search(
            &source,
            &output,
            |effort, candidate| {
                std::fs::write(candidate, vec![effort; 20])?;
                Ok(())
            },
            |()| true,
            |_| {
                decodes += 1;
                Ok(())
            },
        )?;
        assert_eq!(effort, 1, "same-size ties retain lower effort");
        assert_eq!(
            decodes, 1,
            "larger/equal later candidates need no reconstruction"
        );
        Ok(())
    }

    #[test]
    fn jpeg_full_search_rejects_incomplete_or_changed_candidates_without_publishing()
    -> anyhow::Result<()> {
        for cause in [
            "launch",
            "failed",
            "missing",
            "empty",
            "directory",
            "verify",
            "source",
            "candidate",
            "winner",
        ] {
            let root = tempfile::tempdir()?;
            let source = root.path().join("source");
            let output = root.path().join("output");
            std::fs::write(&source, b"synthetic JPEG input")?;
            std::fs::write(&output, b"previous staging")?;
            let winner = std::cell::RefCell::new(None::<std::path::PathBuf>);
            let result = run_jpeg_full_effort_search(
                &source,
                &output,
                |effort, candidate| {
                    if effort == 3 && cause == "launch" {
                        anyhow::bail!("synthetic encoder failure");
                    }
                    if effort == 3 && cause == "missing" {
                        return Ok(true);
                    }
                    if effort == 3 && cause == "directory" {
                        std::fs::create_dir(candidate)?;
                        return Ok(true);
                    }
                    let size = if effort == 1 {
                        30
                    } else if effort == 2 {
                        20
                    } else {
                        40
                    };
                    std::fs::write(
                        candidate,
                        vec![
                            effort;
                            if effort == 3 && cause == "empty" {
                                0
                            } else {
                                size
                            }
                        ],
                    )?;
                    if effort == 10 && cause == "source" {
                        std::fs::write(&source, b"changed JPEG input")?;
                    }
                    if effort == 10 && cause == "winner" {
                        std::fs::write(winner.borrow().as_ref().unwrap(), vec![b'z'; 20])?;
                    }
                    Ok(!(effort == 3 && cause == "failed"))
                },
                |success| *success,
                |candidate| {
                    if candidate.ends_with("e2.jxl") && cause == "verify" {
                        anyhow::bail!("reconstruction mismatch");
                    }
                    if candidate.ends_with("e2.jxl") && cause == "candidate" {
                        std::fs::write(candidate, vec![b'z'; 20])?;
                    }
                    *winner.borrow_mut() = Some(candidate.to_path_buf());
                    Ok(())
                },
            );
            assert!(
                result.is_err(),
                "{cause} must not publish a partial minimum"
            );
            assert_eq!(std::fs::read(&output)?, b"previous staging");
            assert_eq!(
                std::fs::read_dir(root.path())?.count(),
                2,
                "{cause}: owned scratch leaked"
            );
        }
        let root = tempfile::tempdir()?;
        let source = root.path().join("source");
        std::fs::write(&source, b"synthetic JPEG input")?;
        assert!(
            run_jpeg_full_effort_search(&source, &source, |_, _| Ok(()), |()| true, |_| Ok(()))
                .is_err()
        );
        assert_eq!(std::fs::read(&source)?, b"synthetic JPEG input");
        Ok(())
    }

    #[test]
    fn jpeg_full_search_defers_pause_until_transaction_end_and_honors_cancellation()
    -> anyhow::Result<()> {
        use anyhow::Context;
        const CHILD: &str = "MFB_TEST_JPEG_FULL_CONTROL_BOUNDARY";
        if std::env::var_os(CHILD).is_some() {
            let control = std::path::PathBuf::from(
                std::env::var_os("MFB_BATCH_CONTROL_FILE").context("isolated control path")?,
            );
            let root = control.parent().context("isolated control parent")?;
            let source = root.join("source");
            let output = root.join("output");
            let ack = root.join("control.ack");
            std::fs::write(&source, b"synthetic JPEG input")?;
            let transaction = crate::batch_control::begin_transaction()?;
            let mut encoded = Vec::new();
            run_jpeg_full_effort_search(
                &source,
                &output,
                |effort, candidate| {
                    encoded.push(effort);
                    if effort == 1 {
                        std::fs::write(&control, "paused\npause-at-file-boundary")?;
                    }
                    std::fs::write(candidate, vec![effort; 20])?;
                    assert!(
                        !ack.try_exists()?,
                        "pause acknowledged inside a file transaction"
                    );
                    Ok(())
                },
                |()| true,
                |_| Ok(()),
            )?;
            assert_eq!(encoded, (1..=11).collect::<Vec<_>>());
            assert!(!ack.try_exists()?);
            drop(transaction);
            let previous = std::fs::read(&output)?;
            for cancel_effort in [2, 11] {
                std::fs::write(&control, "running\nstart-next-file")?;
                let _transaction = crate::batch_control::begin_transaction()?;
                let mut encoded = Vec::new();
                let result = run_jpeg_full_effort_search(
                    &source,
                    &output,
                    |effort, candidate| {
                        encoded.push(effort);
                        std::fs::write(candidate, vec![effort; 10])?;
                        if effort == cancel_effort {
                            std::fs::write(&control, "cancelled\ncancel-current-file")?;
                        }
                        Ok(())
                    },
                    |()| true,
                    |_| Ok(()),
                );
                assert!(
                    result.is_err(),
                    "cancellation must reject the current winner"
                );
                assert_eq!(encoded, (1..=cancel_effort).collect::<Vec<_>>());
                assert_eq!(std::fs::read(&output)?, previous);
                assert!(!ack.try_exists()?);
            }
            return Ok(());
        }
        let root = tempfile::tempdir()?;
        let control = root.path().join("control");
        #[cfg(unix)]
        std::fs::set_permissions(
            root.path(),
            std::os::unix::fs::PermissionsExt::from_mode(0o700),
        )?;
        std::fs::write(&control, "running\nisolated-file-transaction")?;
        let name = format!(
            "{}::jpeg_full_search_defers_pause_until_transaction_end_and_honors_cancellation",
            module_path!()
        );
        let (_, name) = name.split_once("::").context("test module path")?;
        let mut command = std::process::Command::new(std::env::current_exe()?);
        command
            .args(["--exact", name])
            .env(CHILD, "1")
            .env("MFB_BATCH_CONTROL_FILE", control)
            .env("MFB_HOME_ROOT", root.path())
            .env("MFB_LOG_DIR", root.path().join("logs"));
        let output = crate::process_runner::ManagedProcess::spawn_captured(&mut command)?
            .wait_liveness_timeout(
                std::time::Duration::from_secs(5),
                std::time::Duration::from_secs(5),
                "JPEG effort search file transaction pause/cancel regression",
            )?;
        anyhow::ensure!(
            output.status.success(),
            "isolated JPEG effort control regression failed: {} {}",
            output.stdout,
            output.stderr
        );
        Ok(())
    }

    #[test]
    fn installed_jpeg_full_search_keeps_all_efforts_independent_of_fixed_value()
    -> anyhow::Result<()> {
        use crate::runtime_config::{EffortSearch, JpegEffortSearch, RuntimeConfig};
        use anyhow::Context;
        const CHILD: &str = "MFB_TEST_INSTALLED_JPEG_FULL_SEARCH";
        if std::env::var_os(CHILD).is_some() {
            assert!(crate::runtime_config::active().is_none());
            assert_eq!(
                effort_candidates(JxlEffortContext::JpegLosslessTranscode, 1),
                (1..=11).collect::<Vec<_>>()
            );
            assert_eq!(
                effort_candidates(JxlEffortContext::DirectEncode, 9),
                [7, 8, 9]
            );
            assert!(quality_search_enabled(false));
            let mut config = RuntimeConfig::default();
            config.img.jpeg_effort_search = JpegEffortSearch::Full;
            config.img.jxl_effort_search = EffortSearch::Adaptive;
            crate::runtime_config::install(config)?;
            for selected in [1, 5, 9, 11] {
                assert_eq!(
                    effort_candidates(JxlEffortContext::JpegLosslessTranscode, selected),
                    (1..=11).collect::<Vec<_>>()
                );
            }
            assert_eq!(
                effort_candidates(JxlEffortContext::DirectEncode, 9),
                [7, 8, 9]
            );
            return Ok(());
        }
        let name = format!(
            "{}::installed_jpeg_full_search_keeps_all_efforts_independent_of_fixed_value",
            module_path!()
        );
        let (_, name) = name.split_once("::").context("test module path")?;
        let root = tempfile::tempdir()?;
        let status = std::process::Command::new(std::env::current_exe()?)
            .args(["--exact", name])
            .env(CHILD, "1")
            .env("MFB_HOME_ROOT", root.path())
            .env("MFB_LOG_DIR", root.path().join("logs"))
            .status()?;
        anyhow::ensure!(
            status.success(),
            "isolated JPEG full search config test failed"
        );
        Ok(())
    }

    #[test]
    fn effort_search_selects_verified_payload_and_preserves_prior_output_on_failure()
    -> anyhow::Result<()> {
        let root = tempfile::tempdir()?;
        let source = root.path().join("source");
        let output = root.path().join("output");
        std::fs::write(&source, b"synthetic opaque input")?;
        let (observed, effort) = run_effort_search(
            &source,
            &output,
            &[7, 8, 10],
            |effort, candidate| {
                assert!(std::fs::read_dir(candidate.parent().unwrap())?.count() <= 1);
                std::fs::write(candidate, vec![b'x'; if effort == 7 { 40 } else { 20 }])?;
                Ok(effort)
            },
            |_| true,
            |candidate| Ok(std::fs::metadata(candidate)?.len()),
        )?;
        assert_eq!((observed, effort), (8, 8));
        assert_eq!(std::fs::metadata(&output)?.len(), 20);
        let prior = std::fs::read(&output)?;
        for cause in ["verification", "mutation", "launch"] {
            std::fs::write(&source, b"synthetic opaque input")?;
            let result = run_effort_search(
                &source,
                &output,
                &[7, 8],
                |effort, candidate| {
                    if effort == 8 && cause == "launch" {
                        anyhow::bail!("synthetic launch failure");
                    }
                    std::fs::write(candidate, b"opaque candidate")?;
                    if effort == 8 && cause == "mutation" {
                        std::fs::write(&source, b"changed source")?;
                    }
                    Ok(effort)
                },
                |_| true,
                |candidate| {
                    if cause == "verification" {
                        anyhow::bail!("synthetic verification failure");
                    }
                    Ok(std::fs::metadata(candidate)?.len())
                },
            );
            assert!(result.is_err(), "{cause} must not publish a partial winner");
            assert_eq!(std::fs::read(&output)?, prior);
        }
        let (success, _) = run_effort_search(
            &source,
            &output,
            &[7, 8],
            |_, _| Ok(false),
            |value| *value,
            |_| unreachable!(),
        )?;
        assert!(!success);
        assert_eq!(std::fs::read(&output)?, prior);
        let remaining = std::fs::read_dir(root.path())?.collect::<std::io::Result<Vec<_>>>()?;
        assert_eq!(remaining.len(), 2, "owned search staging leaked");
        assert!(!crate::runtime_config::QualitySearch::Fixed.enabled(true));
        assert!(crate::runtime_config::QualitySearch::Adaptive.enabled(false));
        assert!(crate::runtime_config::QualitySearch::ModeDefault.enabled(true));
        assert!(!crate::runtime_config::QualitySearch::ModeDefault.enabled(false));
        Ok(())
    }

    fn efforts(plan: &[JxlEffortPlan]) -> Vec<u8> {
        plan.iter().map(|item| item.effort()).collect()
    }

    #[test]
    fn normal_tier_produces_e7() {
        assert_eq!(encoder_effort(false), constants::JXL_DEFAULT_EFFORT);
        let plan = effort_plan(JxlEffortContext::DirectEncode, false);
        assert_eq!(efforts(&plan), vec![constants::JXL_DEFAULT_EFFORT]);
        let plan = effort_plan(JxlEffortContext::JpegLosslessTranscode, false);
        assert_eq!(
            efforts(&plan),
            vec![constants::JXL_EXPERIMENTAL_LOSSLESS_EFFORT]
        );
    }

    #[test]
    fn ultimate_tier_produces_e10() {
        assert_eq!(encoder_effort(true), constants::JXL_ULTIMATE_EFFORT);
        let plan = effort_plan(JxlEffortContext::DirectEncode, true);
        assert_eq!(efforts(&plan), vec![constants::JXL_ULTIMATE_EFFORT]);
        let plan = effort_plan(JxlEffortContext::JpegLosslessTranscode, true);
        assert_eq!(
            efforts(&plan),
            vec![constants::JXL_EXPERIMENTAL_LOSSLESS_EFFORT]
        );
    }

    #[test]
    fn archive_normalizes_to_ultimate_in_policy() {
        let cases = [
            (false, false, false), // Normal
            (false, true, true),   // Ultimate
            (true, false, true),   // archive → Ultimate
            (true, true, true),    // archive+ultimate → Ultimate
        ];
        for (archive, ultimate, expected_ultimate) in cases {
            let effective = effective_ultimate(ultimate, archive);
            assert_eq!(effective, expected_ultimate);
            assert_eq!(
                encoder_effort_for_mode(ultimate, archive),
                encoder_effort(effective)
            );
        }
    }

    #[test]
    fn effort_plan_produces_single_item() {
        assert_eq!(
            effort_plan(JxlEffortContext::DirectEncode, false).len(),
            1,
            "effort plan must always contain exactly one item"
        );
        assert_eq!(
            effort_plan(JxlEffortContext::DirectEncode, true).len(),
            1,
            "effort plan must always contain exactly one item"
        );
        assert_eq!(
            efforts(&effort_plan_for_mode(
                JxlEffortContext::DirectEncode,
                false,
                true,
            )),
            vec![constants::JXL_ULTIMATE_EFFORT]
        );
    }

    #[test]
    fn jpeg_effort_uses_configured_value_or_legacy_default() {
        assert_eq!(jpeg_transcode_effort(None), 11);
        assert_eq!(jpeg_transcode_effort(Some(1)), 1);
        assert_eq!(jpeg_transcode_effort(Some(9)), 9);
        assert_eq!(jpeg_transcode_effort(Some(10)), 10);
    }

    #[test]
    fn direct_effort_override_preserves_default_domains_and_jpeg_effort() {
        for default in [
            constants::JXL_DEFAULT_EFFORT,
            constants::JXL_ULTIMATE_EFFORT,
        ] {
            assert_eq!(resolve_direct_effort(None, default), default);
            for configured in 1..=11 {
                assert_eq!(resolve_direct_effort(Some(configured), default), configured);
                assert_eq!(jpeg_transcode_effort(None), 11);
            }
        }
        assert_eq!(
            resolve_direct_effort(None, crate::image::png_validation::PNG_LOSSLESS_JXL_EFFORT),
            10
        );
    }

    #[test]
    fn installed_direct_effort_reaches_encoder_arguments_without_touching_jpeg()
    -> anyhow::Result<()> {
        use crate::builder_base::ToolBuilder;
        use anyhow::Context;
        const CHILD: &str = "MFB_TEST_INSTALLED_DIRECT_EFFORT";
        let child_effort = match std::env::var(CHILD) {
            Ok(value) => Some(value),
            Err(std::env::VarError::NotPresent) => None,
            Err(error) => return Err(anyhow::anyhow!("read isolated effort fixture: {error}")),
        };
        if let Some(value) = child_effort {
            let effort = crate::infra::runtime_config::parse_direct_jxl_effort(&value)
                .map_err(anyhow::Error::msg)?;
            let mut config = crate::infra::runtime_config::RuntimeConfig::default();
            config.img.jxl_effort = Some(effort);
            config.img.jpeg_effort = 9;
            config.img.jxl_effort_search = crate::runtime_config::EffortSearch::Adaptive;
            config.img.jpeg_effort_search = crate::runtime_config::JpegEffortSearch::Adaptive;
            config.tools.paths.insert(
                "cjxl".into(),
                std::path::PathBuf::from(
                    std::env::var_os("MFB_HOME_ROOT").context("isolated test root")?,
                )
                .join("missing-cjxl"),
            );
            crate::infra::runtime_config::install(config)?;
            assert_eq!(
                effort_candidates(JxlEffortContext::DirectEncode, effort),
                [7, 8, 10]
                    .into_iter()
                    .filter(|candidate| *candidate < effort)
                    .chain(std::iter::once(effort))
                    .collect::<Vec<_>>()
            );
            assert_eq!(
                effort_candidates(JxlEffortContext::JpegLosslessTranscode, 9),
                vec![1, 7, 8, 9]
            );
            for (ultimate, archive) in [(false, false), (true, false), (false, true)] {
                assert_eq!(encoder_effort_for_mode(ultimate, archive), effort);
                assert_eq!(
                    efforts(&effort_plan_for_mode(
                        JxlEffortContext::DirectEncode,
                        ultimate,
                        archive
                    )),
                    vec![effort]
                );
                assert_eq!(
                    efforts(&effort_plan_for_mode(
                        JxlEffortContext::JpegLosslessTranscode,
                        ultimate,
                        archive
                    )),
                    vec![9]
                );
            }
            let png_effort =
                direct_encoder_effort(crate::image::png_validation::PNG_LOSSLESS_JXL_EFFORT);
            assert_eq!(png_effort, effort);
            let mut builder = crate::CjxlBuilder::new();
            builder.distance(0.0).effort(png_effort);
            let command = builder.build();
            let args: Vec<_> = command.get_args().collect();
            let expected = effort.to_string();
            assert!(
                args.windows(2).any(
                    |pair| pair[0] == constants::JXL_ARG_EFFORT && pair[1] == expected.as_str()
                )
            );
            assert!(
                args.windows(2)
                    .any(|pair| pair[0] == constants::JXL_ARG_DISTANCE && pair[1] == "0")
            );
            assert_eq!(
                args.contains(&std::ffi::OsStr::new(
                    constants::JXL_ARG_ALLOW_EXPERT_OPTIONS
                )),
                effort == 11
            );
            return Ok(());
        }
        let name = format!(
            "{}::installed_direct_effort_reaches_encoder_arguments_without_touching_jpeg",
            module_path!()
        );
        let (_, name) = name.split_once("::").context("test module path")?;
        let root = tempfile::tempdir()?;
        for effort in ["1", "7", "8", "9", "10", "11"] {
            let status = std::process::Command::new(std::env::current_exe()?)
                .args(["--exact", name])
                .env(CHILD, effort)
                .env("MFB_HOME_ROOT", root.path())
                .env("MFB_LOG_DIR", root.path().join("logs"))
                .status()?;
            anyhow::ensure!(status.success(), "isolated effort {effort} test failed");
        }
        Ok(())
    }
}
