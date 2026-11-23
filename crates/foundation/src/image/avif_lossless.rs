//! Positive lossless evidence for a decoded static AVIF. A 4:4:4 codec
//! property alone says nothing about quantization inside the AV1 frame.

use crate::common_utils::{find_any_box_recursive, resolve_tool_path};
use crate::unified_error::{ImgQualityError, Result};
use std::{collections::BTreeMap, path::Path, process::Command, time::Duration};

/// Called after the strict still-container/avifdec checks and before the
/// post-probe source hash. Accept one image with optional straight alpha.
pub(super) fn proven_static_lossless(path: &Path) -> Result<bool> {
    let Some(ffmpeg) = resolve_tool_path(crate::constants::TOOL_FFMPEG) else {
        return Ok(false);
    };
    // An absent forensic decoder also yields Unknown from the caller.
    if resolve_tool_path(crate::constants::TOOL_AVIFDEC).is_none() {
        return Ok(false);
    }
    let Some(item_ids) =
        simple_plane_ids(&super::modern_lossy_static::read_static_probe_input(path)?)?
    else {
        return Ok(false);
    };
    // Select by HEIF item ID. Older demuxers which cannot expose an alpha
    // item individually cannot establish the required evidence.
    let mut scratch = tempfile::Builder::new();
    scratch.prefix("mfb-av1-");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        scratch.permissions(std::fs::Permissions::from_mode(0o700));
    }
    let scratch = scratch.tempdir()?;
    let mut extract = Command::new(&ffmpeg);
    extract
        .args([
            "-nostdin",
            "-hide_banner",
            "-v",
            "error",
            "-xerror",
            "-protocol_whitelist",
            "file",
            "-f",
            "mov",
            "-i",
        ])
        .arg(crate::safe_path_arg(path).as_ref());
    let mut streams = Vec::with_capacity(item_ids.len());
    for id in item_ids {
        let target = scratch.path().join(format!("{id}.obu"));
        extract
            .arg("-map")
            .arg(format!("0:i:{id}"))
            .args(["-c", "copy", "-f", "obu"])
            .arg(&target);
        streams.push(target);
    }
    let run = |command: &mut Command| {
        crate::process_runner::run_command_with_liveness_timeout(
            command,
            Duration::from_secs(15),
            Duration::from_secs(60),
            "AV1 lossless frame inspection",
        )
    };
    if !run(&mut extract)?.status.success() {
        return Ok(false);
    }
    for (index, stream) in streams.iter().enumerate() {
        // Inspect only the selected elementary stream. Container metadata and
        // filenames are never interpreted as evidence in the field parser.
        let output = run(Command::new(&ffmpeg)
            .args([
                "-nostdin",
                "-hide_banner",
                "-nostats",
                "-xerror",
                "-f",
                "obu",
                "-i",
            ])
            .arg(stream)
            .args([
                "-map",
                "0:v:0",
                "-c",
                "copy",
                "-bsf:v",
                "trace_headers",
                "-f",
                "null",
                "-",
            ]))?;
        if !output.status.success()
            || !frame_trace_lossless(&String::from_utf8_lossy(&output.stderr), index > 0)
        {
            return Ok(false);
        }
    }
    Ok(true)
}

fn simple_plane_ids(data: &[u8]) -> Result<Option<Vec<u32>>> {
    // Flattening geometric transforms, layers or a gain map into an RGB image
    // would make a different object, even when its primary frame was lossless.
    if [
        *b"clap", *b"imir", *b"irot", *b"a1lx", *b"lsel", *b"tmap", *b"moov",
    ]
    .iter()
    .any(|kind| find_any_box_recursive(data, *kind))
    {
        return Ok(None);
    }
    let context = super::image_heic_analysis::read_heif_context_with_project_limits(data)?;
    let primary = context
        .primary_image_handle()
        .map_err(|e| ImgQualityError::AnalysisError(format!("AVIF primary image: {e}")))?;
    if context.image_ids() != [primary.item_id()]
        || primary.number_of_thumbnails() != 0
        || primary.number_of_depth_images() != 0
        || primary.is_premultiplied_alpha()
    {
        return Ok(None);
    }
    // The container's nclx profile may override what the AV1 sequence says.
    // A limited range or non-identity RGB conversion is insufficient proof.
    if let Some(profile) = primary.color_profile_nclx()
        && (profile.full_range_flag() == 0
            || (profile.matrix_coefficients() != libheif_rs::MatrixCoefficients::RGB_GBR
                && !matches!(
                    primary.preferred_decoding_colorspace(),
                    Ok(libheif_rs::ColorSpace::Monochrome)
                )))
    {
        return Ok(None);
    }
    let mut item_ids = vec![primary.item_id()];
    let aux = primary.auxiliary_images(None);
    if aux.len() != usize::from(primary.has_alpha_channel()) {
        return Ok(None);
    }
    for plane in aux {
        if plane.auxiliary_type().ok().as_deref()
            != Some("urn:mpeg:mpegB:cicp:systems:auxiliary:alpha")
            || (plane.width(), plane.height()) != (primary.width(), primary.height())
        {
            return Ok(None);
        }
        item_ids.push(plane.item_id());
    }
    if !super::image_formats::avif::only_coded_planes(data, &item_ids) {
        return Ok(None);
    }
    Ok(Some(item_ids))
}

/// Conservative subset of AV1 `AllLossless`: a single still frame with no
/// quantizer deltas, segment overrides, super-resolution or synthesized grain.
/// All positive facts are mandatory. Unknown trace formats retain the source.
fn frame_trace_lossless(trace: &str, alpha: bool) -> bool {
    let mut fields = BTreeMap::new();
    let mut frames = 0;
    for line in trace.lines() {
        let Some((_, body)) = line
            .strip_prefix("[trace_headers @ ")
            .and_then(|line| line.split_once("] "))
        else {
            continue;
        };
        if body == "Frame Header" {
            frames += 1;
        }
        let Some((syntax, value)) = body.rsplit_once(" = ") else {
            continue;
        };
        let Some(name) = syntax.split_ascii_whitespace().nth(1) else {
            continue;
        };
        // The demuxer may repeat the sequence header as codec extradata.
        // A conflicting repeated field supplies no positive evidence.
        fields
            .entry(name)
            .and_modify(|prior| {
                if *prior != Some(value) {
                    *prior = None;
                }
            })
            .or_insert(Some(value));
    }
    let is = |name: &str, value: &str| fields.get(name) == Some(&Some(value));
    if frames != 1
        || !is("still_picture", "1")
        || !is("enable_superres", "0")
        || !is("film_grain_params_present", "0")
        || !is("base_q_idx", "0")
        || !is("segmentation_enabled", "0")
        || !is("delta_q_y_dc.delta_coded", "0")
    {
        return false;
    }
    if is("mono_chrome", "1") {
        return is("color_range", "1");
    }
    // BT.709 primaries + sRGB transfer + identity matrix imply full range
    // and no subsampling; AV1 omits the color_range bit in that branch.
    let implicit_rgb = is("color_primaries", "1")
        && is("transfer_characteristics", "13")
        && is("matrix_coefficients", "0");
    !alpha
        && is("matrix_coefficients", "0")
        && (implicit_rgb
            || (is("color_range", "1") && (is("seq_profile", "1") || is("subsampling_x", "0"))))
        && is("delta_q_u_dc.delta_coded", "0")
        && is("delta_q_u_ac.delta_coded", "0")
        && (is("separate_uv_delta_q", "0")
            || is("diff_uv_delta", "0")
            || (is("delta_q_v_dc.delta_coded", "0") && is("delta_q_v_ac.delta_coded", "0")))
}
