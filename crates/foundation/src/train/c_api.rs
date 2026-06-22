use crate::scenario::ScenarioType;
use postgres::{Client, NoTls};
use std::ffi::{CStr, CString};
use std::os::raw::c_char;
use std::path::Path;
use std::sync::Mutex;
use std::time::Instant;

/// Last per-path ingestion diagnostic (for C API / Python callers to classify failures).
static LAST_INGEST_ERR: Mutex<Option<CString>> = Mutex::new(None);

fn set_last_ingest_error(msg: &str) {
    let msg = msg.trim();
    let mut guard = crate::media_conversion_gate::mutex_guard_or_recover(
        "training_c_api_last_error",
        LAST_INGEST_ERR.lock(),
    );
    if msg.is_empty() {
        *guard = None;
        return;
    }
    *guard = match CString::new(msg.to_string()) {
        Ok(c_msg) => Some(c_msg),
        Err(e) => {
            crate::media_conversion_gate::delivery_db_batch_audit(
                "training_c_api_last_error",
                format!("failed to store ingest error as CString: {e}; sanitizing NUL bytes"),
            );
            let sanitized = msg.replace('\0', "\\0");
            match CString::new(sanitized) {
                Ok(c_msg) => Some(c_msg),
                Err(sanitize_err) => {
                    crate::media_conversion_gate::delivery_db_batch_audit(
                        "training_c_api_last_error",
                        format!(
                            "failed to store sanitized ingest error as CString: {sanitize_err}"
                        ),
                    );
                    None
                }
            }
        }
    };
}

fn clear_last_ingest_error() {
    let mut guard = crate::media_conversion_gate::mutex_guard_or_recover(
        "training_c_api_last_error",
        LAST_INGEST_ERR.lock(),
    );
    *guard = None;
}

fn ingest_batch_fatal(code: i32, audit_tag: &'static str, msg: &str) -> i32 {
    set_last_ingest_error(msg);
    crate::media_conversion_gate::delivery_db_batch_audit(audit_tag, msg);
    code
}

/// Return the last ingest error as a NUL-terminated UTF-8 string (valid until the next ingest API call).
///
/// # Safety
///
/// The returned pointer is valid until the next ingest API call on this process.
/// It must not be freed by the caller.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mfb_last_ingest_error() -> *const c_char {
    static EMPTY: &[u8] = b"\0";
    let guard = crate::media_conversion_gate::mutex_guard_or_recover(
        "c_api_last_ingest_err",
        LAST_INGEST_ERR.lock(),
    );
    match guard.as_ref() {
        None => EMPTY.as_ptr().cast::<c_char>(),
        Some(c) => c.as_ptr(),
    }
}

fn normalize_loop_intent_label_override(label: &str) -> Result<Option<&'static str>, ()> {
    match label.trim().to_ascii_lowercase().as_str() {
        "" | "auto" | "heuristic" => Ok(None),
        "loop" | "video_loop" | "high" => Ok(Some("high")),
        "low" => Ok(Some("low")),
        "non_loop" | "video" => Ok(Some("video")),
        _ => Err(()),
    }
}

fn require_i32_from_u32(value: u32, field: &str) -> anyhow::Result<i32> {
    crate::numeric_cast::u32_to_i32_strict(value, field)
        .ok_or_else(|| anyhow::anyhow!("{field} value '{value}' out of i32 range"))
}

fn require_i64_from_u64(value: u64, field: &str) -> anyhow::Result<i64> {
    crate::numeric_cast::u64_to_i64_strict(value, field)
        .ok_or_else(|| anyhow::anyhow!("{field} value '{value}' out of i64 range"))
}

fn require_quality_score(score: Option<f32>) -> anyhow::Result<f32> {
    score.ok_or_else(|| anyhow::anyhow!("quality scenarios must pre-validate quality_score"))
}

#[unsafe(no_mangle)]
/// Batch-ingest media samples from a C caller into the multi-scenario schema.
///
/// # Safety
///
/// `conn_str_ptr`, `paths_ptr`, and `scenario_ptr` must each point to a valid
/// NUL-terminated C string for the duration of this call. `label_ptr` may be
/// null; when non-null it must also point to a valid NUL-terminated C string.
pub unsafe extern "C" fn ingest_media_samples_batch(
    conn_str_ptr: *const c_char,
    paths_ptr: *const c_char,
    label_ptr: *const c_char,
    scenario_ptr: *const c_char,
) -> i32 {
    if conn_str_ptr.is_null() || paths_ptr.is_null() || scenario_ptr.is_null() {
        return ingest_batch_fatal(
            -1,
            "ingest_batch_null_ptr",
            "null conn_str, paths, or scenario pointer",
        );
    }

    let conn_str = match unsafe { CStr::from_ptr(conn_str_ptr) }.to_str() {
        Ok(s) if !s.trim().is_empty() => s.to_owned(),
        _ => {
            return ingest_batch_fatal(
                -5,
                "ingest_batch_connstr",
                "invalid or empty PostgreSQL connection string",
            );
        }
    };
    let paths_str = match unsafe { CStr::from_ptr(paths_ptr) }.to_str() {
        Ok(s) => s.to_owned(),
        Err(_) => {
            return ingest_batch_fatal(-5, "ingest_batch_paths", "paths JSON is not valid UTF-8");
        }
    };
    let label_owned = if label_ptr.is_null() {
        String::new()
    } else {
        match unsafe { CStr::from_ptr(label_ptr) }.to_str() {
            Ok(s) => s.to_owned(),
            _ => {
                return ingest_batch_fatal(-5, "ingest_batch_label", "label is not valid UTF-8");
            }
        }
    };
    let scenario_name = match unsafe { CStr::from_ptr(scenario_ptr) }.to_str() {
        Ok(s) if !s.trim().is_empty() => s.to_owned(),
        _ => {
            return ingest_batch_fatal(-2, "ingest_batch_scenario", "empty scenario name");
        }
    };

    let scenario: ScenarioType = match scenario_name.parse() {
        Ok(s) => s,
        Err(_) => {
            return ingest_batch_fatal(
                -2,
                "ingest_batch_scenario",
                &format!("unsupported scenario '{scenario_name}'"),
            );
        }
    };

    let Ok(mut client) = Client::connect(&conn_str, NoTls) else {
        return ingest_batch_fatal(
            -3,
            "ingest_batch_connect",
            "PostgreSQL connect failed for ingest batch",
        );
    };
    if let Err(err) = crate::multi_scenario_db::init_multi_scenario_schema(&mut client) {
        let msg = format!("multi_scenario schema init failed: {err:#}");
        return ingest_batch_fatal(-4, "ingest_batch_schema", &msg);
    }

    let mut success_count = 0;
    let paths = crate::media_conversion_gate::ffi_ingest_path_list_or_delimited(paths_str.as_ref());

    if paths.is_empty() {
        return 0;
    }

    let total_paths = paths.len();
    let progress_on = crate::training_progress::ingest_progress_enabled();
    let progress_step = crate::training_progress::ingest_progress_step(total_paths);
    let batch_started = Instant::now();
    if progress_on {
        let label_log = if label_owned.is_empty() {
            "(auto)"
        } else {
            label_owned.as_str()
        };
        crate::training_progress::emit_ingest_progress_line(&format!(
            "[INGEST-RUST] batch_start scenario={scenario_name} label={label_log} n={total_paths}"
        ));
    }

    let quality_score = if scenario == ScenarioType::LoopIntent {
        None
    } else {
        let label = label_owned.trim();
        let Ok(quality_tier) = crate::scenario::QualityTier::parse_strict(label) else {
            return ingest_batch_fatal(
                -5,
                "ingest_batch_label",
                &format!("invalid quality label '{label}' for scenario {scenario_name}"),
            );
        };
        if scenario != ScenarioType::ImageQuality
            && crate::scenario::ImageQualityLabel::from_label(label).is_some()
        {
            return ingest_batch_fatal(
                -5,
                "ingest_batch_label",
                &format!("image_quality label '{label}' is not valid for scenario {scenario_name}"),
            );
        }
        Some(quality_tier.to_score())
    };

    let loop_label_override = if scenario == ScenarioType::LoopIntent {
        match normalize_loop_intent_label_override(label_owned.as_str()) {
            Ok(label) => label.map(ToOwned::to_owned),
            Err(()) => {
                return ingest_batch_fatal(
                    -5,
                    "ingest_batch_loop_label",
                    &format!("invalid loop_intent label override '{label_owned}'"),
                );
            }
        }
    } else {
        None
    };

    let loop_feature_map = if scenario == ScenarioType::LoopIntent {
        match crate::database::prepare_loop_training_feature_map(&mut client) {
            Ok(feature_map) => Some(feature_map),
            Err(err) => {
                let msg = format!("prepare_loop_training_feature_map failed: {err:#}");
                return ingest_batch_fatal(-4, "loop_ingest_feature_map_bootstrap", &msg);
            }
        }
    } else {
        None
    };

    for (path_index, path_raw) in paths.into_iter().enumerate() {
        let index_one_based = path_index + 1;
        if progress_on
            && crate::training_progress::should_emit_ingest_progress_tick(
                index_one_based,
                total_paths,
                progress_step,
            )
        {
            let path_preview = if path_raw.trim().is_empty() {
                "<empty>".to_string()
            } else {
                crate::training_progress::path_basename_for_log(Path::new(path_raw.trim()))
            };
            crate::training_progress::emit_ingest_progress_line(&format!(
                "[INGEST-RUST] progress {index_one_based}/{total_paths} file={path_preview} scenario={scenario_name} elapsed={}",
                crate::training_progress::format_elapsed_secs(batch_started.elapsed())
            ));
        }

        if path_raw.trim().is_empty() {
            set_last_ingest_error("empty path entry in C-API batch");
            continue;
        }
        let path = Path::new(path_raw.trim());
        if !path.is_file() {
            set_last_ingest_error(&format!("not a regular file: {}", path.display()));
            continue;
        }
        let stored_source_path = crate::common_utils::training_source_path_for(path);

        let Ok(blake3_hash) = crate::common_utils::calculate_blake3_hash_bytes(path) else {
            set_last_ingest_error(&format!("blake3 hash failed: {}", path.display()));
            continue;
        };

        let ingested = match scenario {
            ScenarioType::ImageQuality => {
                use crate::image_analyzer::analyze_image;
                (|| -> anyhow::Result<bool> {
                    let format = crate::image_detection::detect_format_from_bytes(path)?;
                    let (is_animated, _, _) =
                        crate::image_detection::detect_animation(path, &format)?;
                    if is_animated {
                        anyhow::bail!(
                            "Animated asset is not valid for image_quality scenario: {}",
                            path.display()
                        );
                    }

                    let analysis = analyze_image(path)?;
                    let embedding = crate::image_quality_db::get_quality_features(&analysis)?;
                    let width_i32 = require_i32_from_u32(analysis.width, "image_width")?;
                    let height_i32 = require_i32_from_u32(analysis.height, "image_height")?;
                    let file_size_i64 =
                        require_i64_from_u64(analysis.file_size, "image_file_size")?;
                    let quality_score = require_quality_score(quality_score)?;

                    let mut sample = crate::multi_scenario_db::ScenarioSample::new(
                        blake3_hash.clone(),
                        scenario,
                    )
                    .with_path(stored_source_path.to_string_lossy().to_string())
                    .with_label(label_owned.clone())
                    .with_embedding(embedding)
                    .with_dimensions(width_i32, height_i32)
                    .with_size(file_size_i64)
                    .with_format(analysis.format.clone())
                    .with_entropy(analysis.features.entropy)
                    .with_compression_ratio(analysis.features.compression_ratio)
                    .with_lossless(analysis.is_lossless)
                    .with_quality_score(quality_score);
                    sample.metadata = crate::image_quality_db::build_image_quality_ingest_metadata(
                        &analysis,
                        Some(label_owned.as_str()),
                        path,
                    )?;

                    crate::multi_scenario_db::ingest_image_quality_sample(&mut client, &sample)?;
                    Ok(true)
                })()
            }
            ScenarioType::AnimatedImageQuality => {
                use crate::animated_image_quality_features::AnimatedImageQualityFeatures;
                (|| -> anyhow::Result<bool> {
                    let features = AnimatedImageQualityFeatures::from_path(path)?;
                    let vec_data = features.to_embedding_vector();
                    let width_i32 = require_i32_from_u32(features.width, "animated_width")?;
                    let height_i32 = require_i32_from_u32(features.height, "animated_height")?;
                    let file_size_i64 =
                        require_i64_from_u64(features.file_size_bytes, "animated_file_size")?;
                    let frame_count_i64 = i64::from(features.frame_count);
                    let quality_score = require_quality_score(quality_score)?;
                    if vec_data.len() != scenario.embedding_dimension() {
                        anyhow::bail!(
                            "Animated-image embedding dim {} != expected {}",
                            vec_data.len(),
                            scenario.embedding_dimension()
                        );
                    }
                    let mut sample = crate::multi_scenario_db::ScenarioSample::new(
                        blake3_hash.clone(),
                        scenario,
                    )
                    .with_path(stored_source_path.to_string_lossy().to_string())
                    .with_format(features.format.as_str().to_string())
                    .with_embedding(pgvector::Vector::from(vec_data))
                    .with_dimensions(width_i32, height_i32)
                    .with_size(file_size_i64)
                    .with_frame_count(frame_count_i64)
                    .with_duration_secs(features.duration_secs)
                    .with_fps(features.fps)
                    .with_animation_smoothness(features.animation_smoothness)
                    .with_frame_delay_variation(features.frame_delay_variation)
                    .with_is_meme(features.content_flags.is_meme_suspected)
                    .with_quality_score(quality_score);
                    if let Some(palette_size) = features.palette_size {
                        sample = sample.with_palette_size(i32::from(palette_size));
                    }
                    if let Some(palette_depth) = features.palette_depth {
                        sample = sample.with_palette_depth(palette_depth);
                    }
                    sample.metadata = serde_json::json!({
                        "scenario_semantics": "animated_image_quality",
                        "storage_table": "animated_image_quality_samples",
                        "container_format": features.format.as_str(),
                        "has_alpha": features.render_flags.has_alpha,
                        "is_lossless": features.render_flags.is_lossless,
                        "reference_entropy": features.reference_entropy
                    });

                    crate::multi_scenario_db::ingest_animated_image_quality_sample(
                        &mut client,
                        &sample,
                    )?;
                    Ok(true)
                })()
            }
            ScenarioType::VideoQuality => {
                use crate::video_quality_features::VideoQualityFeatures;
                (|| -> anyhow::Result<bool> {
                    let features = VideoQualityFeatures::from_path(path)?;
                    let vec_data = features.to_embedding_vector();
                    let width_i32 = require_i32_from_u32(features.width, "video_width")?;
                    let height_i32 = require_i32_from_u32(features.height, "video_height")?;
                    let file_size_i64 =
                        require_i64_from_u64(features.file_size_bytes, "video_file_size")?;
                    let frame_count_i64 =
                        require_i64_from_u64(features.frame_count, "video_frame_count")?;
                    let quality_score = require_quality_score(quality_score)?;
                    if vec_data.len() != scenario.embedding_dimension() {
                        anyhow::bail!(
                            "Video embedding dim {} != expected {}",
                            vec_data.len(),
                            scenario.embedding_dimension()
                        );
                    }
                    let sample = crate::multi_scenario_db::ScenarioSample::new(
                        blake3_hash.clone(),
                        scenario,
                    )
                    .with_path(stored_source_path.to_string_lossy().to_string())
                    .with_embedding(pgvector::Vector::from(vec_data))
                    .with_dimensions(width_i32, height_i32)
                    .with_size(file_size_i64)
                    .with_frame_count(frame_count_i64)
                    .with_format(features.codec.clone())
                    .with_duration_secs(features.duration_secs)
                    .with_fps(features.fps)
                    .with_bitrate_mbps(features.bitrate_mbps)
                    .with_bit_depth_opt(features.bit_depth)
                    .with_has_audio(features.has_audio)
                    .with_is_variable_frame_rate(features.is_variable_frame_rate)
                    .with_is_hdr(features.is_hdr)
                    .with_motion_intensity(features.motion_intensity)
                    .with_temporal_stability(features.temporal_stability)
                    .with_quality_score(quality_score);

                    let mut sample = sample;
                    sample.metadata = serde_json::json!({
                        "scenario_semantics": "video_quality",
                        "container_codec": features.codec,
                        "bit_depth": features.bit_depth,
                        "has_audio": features.has_audio,
                        "is_variable_frame_rate": features.is_variable_frame_rate,
                        "is_hdr": features.is_hdr
                    });

                    crate::multi_scenario_db::ingest_video_quality_sample(&mut client, &sample)?;
                    Ok(true)
                })()
            }
            ScenarioType::LoopIntent => (|| -> anyhow::Result<bool> {
                let feature_map = loop_feature_map
                    .as_ref()
                    .ok_or_else(|| anyhow::anyhow!("LoopIntent feature map not initialized"))?;
                let sample = crate::database::build_loop_intent_scenario_sample(
                    path,
                    "c_api_batch",
                    loop_label_override.as_deref(),
                    blake3_hash.clone(),
                    feature_map,
                )?;
                crate::multi_scenario_db::ingest_loop_intent_sample(&mut client, &sample)?;
                Ok(true)
            })(),
        };

        match &ingested {
            Ok(true) => {
                success_count += 1;
                clear_last_ingest_error();
            }
            Ok(false) => {
                set_last_ingest_error(&format!(
                    "skipped: no loop_intent sample produced for {}",
                    path.display()
                ));
            }
            Err(e) => {
                set_last_ingest_error(&format!("{e:#}"));
            }
        }
    }

    if scenario == ScenarioType::LoopIntent
        && success_count > 0
        && let Err(err) = crate::database::refresh_loop_intent_feature_stats(&mut client)
    {
        let msg = format!("refresh_loop_intent_feature_stats failed after ingest: {err:#}");
        return ingest_batch_fatal(-4, "loop_ingest_stats_refresh", &msg);
    }

    if progress_on {
        let not_ok =
            total_paths.saturating_sub(crate::numeric_cast::i32_to_usize_sat(success_count));
        crate::training_progress::emit_ingest_progress_line(&format!(
            "[INGEST-RUST] batch_done scenario={scenario_name} ok={success_count} not_ok={not_ok} n={total_paths} elapsed={}",
            crate::training_progress::format_elapsed_secs(batch_started.elapsed())
        ));
    }

    success_count
}

fn probe_json_ptr(value: &serde_json::Value, api: &str) -> *mut c_char {
    crate::media_conversion_gate::ffi_probe_json_ptr_or_null(value, api)
}

/// Probe a static still image for training-tier geometry/entropy (same engine as ingest).
///
/// Returns a heap-allocated JSON string; free with [`mfb_free_string`].
/// On null/invalid path, returns JSON `{"ok":false,"error":"..."}`.
///
/// # Safety
///
/// `path_ptr` must be a valid NUL-terminated UTF-8 path for the duration of the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mfb_probe_static_still_image(path_ptr: *const c_char) -> *mut c_char {
    if path_ptr.is_null() {
        return probe_json_ptr(
            &serde_json::json!({
                "ok": false,
                "error": "null path pointer"
            }),
            "mfb_probe_static_still_image",
        );
    }
    let path_str = match unsafe { CStr::from_ptr(path_ptr) }.to_str() {
        Ok(s) if !s.trim().is_empty() => s,
        _ => {
            return probe_json_ptr(
                &serde_json::json!({
                    "ok": false,
                    "error": "invalid UTF-8 path"
                }),
                "mfb_probe_static_still_image",
            );
        }
    };

    if let Err(guard_err) =
        crate::training_entry_guard::assert_tier_probe_c_api_entry("mfb_probe_static_still_image")
    {
        return probe_json_ptr(
            &serde_json::json!({
                "ok": false,
                "error": format!("{guard_err:#}")
            }),
            "mfb_probe_static_still_image",
        );
    }

    let path = Path::new(path_str);
    if !path.is_file() {
        return probe_json_ptr(
            &serde_json::json!({
                "ok": false,
                "error": format!("not a regular file: {}", path.display())
            }),
            "mfb_probe_static_still_image",
        );
    }

    match crate::training_tier_audit::probe_static_still_image(path) {
        Ok(probe) => {
            let tier = &probe.tier;
            let resolved =
                crate::training_tier_audit::resolve_collect_tier_label(tier).map(|t| match t {
                    crate::training_tier_audit::AssignedTrainingTier::High => "high",
                    crate::training_tier_audit::AssignedTrainingTier::Low => "low",
                });
            probe_json_ptr(
                &serde_json::json!({
                    "ok": true,
                    "width": probe.width,
                    "height": probe.height,
                    "entropy": probe.entropy,
                    "format": probe.format,
                    "high_tier": tier.high_tier,
                    "low_tier": tier.low_tier,
                    "high_rule_hits": tier.high_rule_hits,
                    "low_rule_hits": tier.low_rule_hits,
                    "resolved_tier": resolved,
                    "ambiguous_both_tiers": tier.high_tier && tier.low_tier,
                    "entropy_engine": "rust_analyze_image",
                }),
                "mfb_probe_static_still_image",
            )
        }
        Err(e) => probe_json_ptr(
            &serde_json::json!({
                "ok": false,
                "error": format!("{e:#}")
            }),
            "mfb_probe_static_still_image",
        ),
    }
}

/// Probe loop vs non-loop training bucket (same heuristics as `loop_intent` ingest).
///
/// Returns heap-allocated JSON; free with [`mfb_free_string`].
///
/// # Safety
///
/// `path_ptr` must be a valid NUL-terminated UTF-8 path for the duration of the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mfb_probe_loop_intent(path_ptr: *const c_char) -> *mut c_char {
    if path_ptr.is_null() {
        return probe_json_ptr(
            &serde_json::json!({
                "ok": false,
                "error": "null path pointer"
            }),
            "mfb_probe_loop_intent",
        );
    }
    let path_str = match unsafe { CStr::from_ptr(path_ptr) }.to_str() {
        Ok(s) if !s.trim().is_empty() => s,
        _ => {
            return probe_json_ptr(
                &serde_json::json!({
                    "ok": false,
                    "error": "invalid UTF-8 path"
                }),
                "mfb_probe_loop_intent",
            );
        }
    };

    if let Err(guard_err) =
        crate::training_entry_guard::assert_tier_probe_c_api_entry("mfb_probe_loop_intent")
    {
        return probe_json_ptr(
            &serde_json::json!({
                "ok": false,
                "error": format!("{guard_err:#}")
            }),
            "mfb_probe_loop_intent",
        );
    }

    let path = Path::new(path_str);
    if !path.is_file() {
        return probe_json_ptr(
            &serde_json::json!({
                "ok": false,
                "error": format!("not a regular file: {}", path.display())
            }),
            "mfb_probe_loop_intent",
        );
    }

    match crate::database::probe_loop_training_balance(path) {
        Ok(probe) => probe_json_ptr(
            &serde_json::json!({
                "ok": true,
                "loss_tolerance": probe.loss_tolerance,
                "loop_intent": probe.loop_intent,
                "complexity": probe.complexity,
                "loop_frequency": probe.loop_frequency,
                "temporal_bpp": probe.temporal_bpp,
                "probe_engine": "rust_sample_from_path",
            }),
            "mfb_probe_loop_intent",
        ),
        Err(e) => probe_json_ptr(
            &serde_json::json!({
                "ok": false,
                "error": format!("{e:#}")
            }),
            "mfb_probe_loop_intent",
        ),
    }
}

/// Free a string returned by a probe C-API.
///
/// # Safety
///
/// `ptr` must be null or exactly one pointer returned by a probe C-API and not yet freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mfb_free_string(ptr: *mut c_char) {
    if ptr.is_null() {
        return;
    }
    let _ = unsafe { CString::from_raw(ptr) };
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use std::io::Write;

    struct EnvRestore {
        key: &'static str,
        value: Option<String>,
    }

    impl EnvRestore {
        fn set(key: &'static str, value: &'static str) -> Self {
            let previous = saved_env_value(key);
            unsafe {
                std::env::set_var(key, value);
            }
            Self {
                key,
                value: previous,
            }
        }

        fn remove(key: &'static str) -> Self {
            let previous = saved_env_value(key);
            unsafe {
                std::env::remove_var(key);
            }
            Self {
                key,
                value: previous,
            }
        }
    }

    fn saved_env_value(key: &str) -> Option<String> {
        match std::env::var(key) {
            Ok(value) => Some(value),
            Err(std::env::VarError::NotPresent) => None,
            Err(e) => {
                crate::media_conversion_gate::delivery_db_batch_audit(
                    "training_c_api_env",
                    format!("failed to read env {key} before override: {e}; restore will remove"),
                );
                None
            }
        }
    }

    impl Drop for EnvRestore {
        fn drop(&mut self) {
            if let Some(value) = self.value.as_deref() {
                unsafe {
                    std::env::set_var(self.key, value);
                }
            } else {
                unsafe {
                    std::env::remove_var(self.key);
                }
            }
        }
    }

    fn probe_value(ptr: *mut c_char) -> serde_json::Value {
        assert!(!ptr.is_null());
        let text = unsafe { CStr::from_ptr(ptr) }
            .to_string_lossy()
            .into_owned();
        unsafe {
            mfb_free_string(ptr);
        }
        serde_json::from_str(&text).unwrap_or_else(|err| panic!("invalid probe JSON: {err}"))
    }

    #[test]
    #[serial]
    fn loop_probe_rejects_unknown_invoker() {
        let _invoker = EnvRestore::set(crate::entry_guard::MFB_INVOKER_ENV, "python_api.py");
        let _training = EnvRestore::remove(crate::entry_guard::TRAINING_INVOKER_ENV);
        let mut fixture =
            tempfile::NamedTempFile::new().unwrap_or_else(|err| panic!("tempfile: {err}"));
        fixture
            .write_all(b"not a real media file")
            .unwrap_or_else(|err| panic!("write fixture: {err}"));
        let path =
            CString::new(fixture.path().to_string_lossy().into_owned()).unwrap_or_else(|err| {
                panic!("path CString: {err}");
            });

        let value = probe_value(unsafe { mfb_probe_loop_intent(path.as_ptr()) });

        assert_eq!(
            value.get("ok").and_then(serde_json::Value::as_bool),
            Some(false)
        );
        let error = value
            .get("error")
            .and_then(serde_json::Value::as_str)
            .map_or("", |message| message);
        assert!(error.contains("mfb_probe_loop_intent"));
        assert!(error.contains("unknown invoker"));
    }
}
