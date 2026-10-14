//! Shared JPEG XL effort selection policy.
//!
//! The policy is intentionally centralized so JPEG bitstream encode and
//! direct JXL encode paths do not drift. Effort is an encoder policy, not a
//! quality-search axis: opt-in effort search selects one domain before quality search.
//!
//! ## Encoder tiers
//! Direct pixel encoding inherits two bounded production tiers unless
//! `img.jxl_effort` explicitly selects a production effort (7, 8 or 10):
//!   - Normal   → e7  (`ultimate = false`)
//!   - Ultimate → e10 (`ultimate = true`)
//!
//! JPEG bitstream transcode is a separate primitive. libjxl's e11 path is
//! optimized for lossless JPEG reconstruction and is selected for that context;
//! a tool that rejects the expert switch receives the explicit production
//! fallback. It must not leak into ordinary pixel encoding, where e11 can be
//! disproportionately slow.
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
        .map_or(default, |config| config.img.quality_search.enabled(default))
}

/// Bounded effort candidates; the selected fixed value is an upper bound in adaptive mode.
#[must_use]
pub fn effort_candidates(kind: JxlEffortContext, upper: u8) -> Vec<u8> {
    use crate::runtime_config::EffortSearch;
    let strategy = crate::runtime_config::active().map(|config| match kind {
        JxlEffortContext::DirectEncode => config.img.jxl_effort_search,
        JxlEffortContext::JpegLosslessTranscode => config.img.jpeg_effort_search,
    });
    if strategy != Some(EffortSearch::Adaptive) {
        return vec![upper];
    }
    let domain: &[u8] = match kind {
        JxlEffortContext::DirectEncode => &[7, 8, 10],
        JxlEffortContext::JpegLosslessTranscode => &[1, 7, 8, 10, 11],
    };
    let mut efforts: Vec<u8> = domain
        .iter()
        .copied()
        .filter(|effort| *effort < upper)
        .collect();
    efforts.push(upper);
    efforts
}

/// Compare verified payloads at one immutable input/quality domain, retaining the lower effort on ties.
/// A single candidate preserves the fixed path, including its caller's verification and fallback.
/// # Errors
/// Rejects invalid plans, changed inputs, I/O or verification failures; no partial winner is published.
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
            best = Some((observation, *effort, size, candidate));
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
    let configured = crate::infra::runtime_config::active();
    let default = if configured.is_some_and(|config| {
        config.img.jxl_effort_search == crate::runtime_config::EffortSearch::Adaptive
    }) {
        constants::JXL_ULTIMATE_EFFORT
    } else {
        default
    };
    resolve_direct_effort(configured.and_then(|config| config.img.jxl_effort), default)
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
/// selects that domain separately; quality exploration then varies distance at
/// one effort. JPEG bitstream transcode is deliberately isolated
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
    fn effort_search_selects_measured_payload_and_keeps_source_and_prior_output_on_failure()
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
            for configured in [7, 8, 10] {
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
            config.img.jpeg_effort_search = crate::runtime_config::EffortSearch::Adaptive;
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
                    .filter(|candidate| *candidate <= effort)
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
            return Ok(());
        }
        let name = format!(
            "{}::installed_direct_effort_reaches_encoder_arguments_without_touching_jpeg",
            module_path!()
        );
        let (_, name) = name.split_once("::").context("test module path")?;
        let root = tempfile::tempdir()?;
        for effort in ["7", "8", "10"] {
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
