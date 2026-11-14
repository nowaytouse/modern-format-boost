//! KNN pgvector feature encoding for loop samples.
//!
//! Absent optional KNN dimensions use
//! `media_conversion_gate::knn_absent_feature_component()` (L2 sparse origin at
//! 0.0 — **not** a measured quality score; never confuse with PSNR/SSIM/embed
//! slots).

use anyhow::Result;

use crate::database::{FeatureMap, SampleRow};

fn feature_std(stats_map: &FeatureMap, feature: &str) -> Option<f64> {
    stats_map
        .stats
        .get(feature)
        .map(|s| s.std_dev.max(crate::constants::KNN_VECTOR_MIN_STD_DEV))
}

fn feature_weight(stats_map: &FeatureMap, feature: &str) -> f64 {
    match crate::media_conversion_gate::db_feature_weight_optional(
        stats_map.stats.get(feature).and_then(|s| s.weight),
        feature,
        "KNN vector feature_weight",
    ) {
        Some(weight) => weight,
        // L2 sparse origin: inactive dimension, not `KNN_VECTOR_DEFAULT_FEATURE_WEIGHT` corpus
        // stats.
        None => crate::media_conversion_gate::knn_absent_feature_component(),
    }
}

pub(crate) fn sample_frame_density_and_gap(sample: &SampleRow) -> Option<(f64, f64)> {
    let fc = sample.frame_count?;
    if fc == 0 {
        return None;
    }
    let dur = sample.duration_secs?;
    let density = crate::numeric_cast::u64_to_f64(fc)
        / dur.max(crate::constants::KNN_VECTOR_MIN_DURATION_FOR_DENSITY);
    let gap = dur / crate::numeric_cast::u64_to_f64(fc);
    Some((density, gap))
}

pub(crate) fn sample_loop_affinity(sample: &SampleRow) -> f64 {
    let sample_audio_score = if sample.flags.streams.is_native_gif {
        1.0_f64
    } else {
        crate::constants::KNN_VECTOR_DEFAULT_AUDIO_SCORE
    };
    let baseline_fps = crate::constants::KNN_VECTOR_BASELINE_FPS;
    let sample_fps_score = match sample.fps {
        None => crate::media_conversion_gate::knn_absent_feature_component(),
        Some(fps_val) => (1.0_f64
            - crate::database::normalize_log_ratio(
                fps_val.max(crate::constants::KNN_VECTOR_FPS_MIN_LIMIT),
                baseline_fps,
                crate::constants::KNN_VECTOR_FPS_NORMALIZATION_SCALE,
            ))
        .clamp(0.0_f64, 1.0_f64),
    };

    match (sample.loop_frequency, sample.cadence_score) {
        (Some(loop_freq), Some(cadence)) => sample_fps_score
            .mul_add(
                crate::constants::KNN_VECTOR_LAFFIN_FPS_WEIGHT,
                loop_freq
                    .mul_add(
                        crate::constants::KNN_VECTOR_LAFFIN_FREQ_WEIGHT,
                        cadence * crate::constants::KNN_VECTOR_LAFFIN_CADENCE_WEIGHT,
                    )
                    .mul_add(
                        crate::constants::KNN_VECTOR_LAFFIN_AUDIO_WEIGHT,
                        sample_audio_score,
                    ),
            )
            .clamp(0.0, 1.0),
        _ => crate::media_conversion_gate::knn_absent_feature_component(),
    }
}

/// Compute a 261-dimensional pgvector encoding for a sample using
/// pre-calculated std deviations. This precisely bakes the weights and
/// normalization terms from the old dynamically computed KNN
/// into an L2-compatible vector, allowing `PostgreSQL`'s HNSW index to do the
/// heavy lifting!
pub(crate) fn calculate_continuous_features(
    sample: &SampleRow,
    stats_map: &FeatureMap,
) -> Option<(f64, f64, f64, f64, f64, f64, f64, f64)> {
    let sample_pixels = (f64::from(sample.width) * f64::from(sample.height)).max(1.0);
    let fc = sample.frame_count?;
    let dur = sample.duration_secs?;
    let (sample_frame_density, sample_frame_gap) = sample_frame_density_and_gap(sample)?;

    Some((
        sample_pixels / feature_std(stats_map, "pixels")?
            * feature_weight(stats_map, "pixels").sqrt(),
        dur / feature_std(stats_map, "duration")? * feature_weight(stats_map, "duration").sqrt(),
        crate::numeric_cast::u64_to_f64(fc) / feature_std(stats_map, "frame_count")?
            * feature_weight(stats_map, "frame_count").sqrt(),
        crate::numeric_cast::u64_to_f64(sample.file_size_bytes)
            / feature_std(stats_map, "file_size_bytes")?
            * feature_weight(stats_map, "file_size_bytes").sqrt(),
        sample_frame_density / feature_std(stats_map, "density")?
            * feature_weight(stats_map, "density").sqrt(),
        sample_frame_gap / feature_std(stats_map, "gap")? * feature_weight(stats_map, "gap").sqrt(),
        sample.temporal_bpp / feature_std(stats_map, "temporal_bpp")?
            * feature_weight(stats_map, "temporal_bpp").sqrt(),
        sample.spatial_bpp / feature_std(stats_map, "spatial_bpp")?
            * feature_weight(stats_map, "spatial_bpp").sqrt(),
    ))
}

pub(crate) fn calculate_discrete_features(
    sample: &SampleRow,
    stats_map: &FeatureMap,
) -> Option<(f64, f64, f64, f64, f64, f64, f64)> {
    let sample_webp_ratio =
        crate::numeric_cast::option_f64_strict(sample.webp_compression_ratio, "sample_webp_ratio");
    let v_wratio = match sample_webp_ratio {
        Some(v) => {
            v / feature_std(stats_map, "webp_ratio")?
                * feature_weight(stats_map, "webp_ratio").sqrt()
        }
        None => crate::media_conversion_gate::knn_absent_feature_component(),
    };

    let sample_loop_freq =
        crate::numeric_cast::option_f64_strict(sample.loop_frequency, "sample_loop_freq");
    let v_lfreq = match sample_loop_freq {
        Some(v) => {
            v / feature_std(stats_map, "loop_freq")? * feature_weight(stats_map, "loop_freq").sqrt()
        }
        None => crate::media_conversion_gate::knn_absent_feature_component(),
    };

    let sample_cadence =
        crate::numeric_cast::option_f64_strict(sample.cadence_score, "sample_cadence");
    let v_cadence = match sample_cadence {
        Some(v) => {
            v / feature_std(stats_map, "cadence")? * feature_weight(stats_map, "cadence").sqrt()
        }
        None => crate::media_conversion_gate::knn_absent_feature_component(),
    };

    let sample_payload_var = crate::numeric_cast::option_f64_strict(
        sample.frame_payload_variation,
        "sample_payload_var",
    );
    let v_payload = match sample_payload_var {
        Some(v) => {
            v / feature_std(stats_map, "payload_var")?
                * feature_weight(stats_map, "payload_var").sqrt()
        }
        None => crate::media_conversion_gate::knn_absent_feature_component(),
    };

    let sample_delay_var =
        crate::numeric_cast::option_f64_strict(sample.frame_delay_variation, "sample_delay_var");
    let v_delay = match sample_delay_var {
        Some(v) => {
            v / feature_std(stats_map, "delay_var")? * feature_weight(stats_map, "delay_var").sqrt()
        }
        None => crate::media_conversion_gate::knn_absent_feature_component(),
    };

    let sample_aspect =
        crate::numeric_cast::option_f64_strict(sample.aspect_ratio, "sample_aspect");
    let v_aspect = match sample_aspect {
        Some(v) => {
            v / feature_std(stats_map, "aspect")? * feature_weight(stats_map, "aspect").sqrt()
        }
        None => crate::media_conversion_gate::knn_absent_feature_component(),
    };

    let v_pal = match sample.palette_size {
        None => crate::media_conversion_gate::knn_absent_feature_component(),
        Some(v) => (f64::from(v) / 256.0_f64) * feature_weight(stats_map, "p_depth").sqrt(),
    };

    Some((
        v_wratio, v_lfreq, v_cadence, v_payload, v_delay, v_aspect, v_pal,
    ))
}

pub(crate) fn calculate_categorical_features(
    sample: &SampleRow,
) -> (f64, f64, f64, f64, f64, f64, f64) {
    let cat = |val: bool, w: f64| {
        if val {
            w.sqrt() / 2.0_f64
        } else {
            -w.sqrt() / 2.0_f64
        }
    };

    (
        cat(
            sample.flags.meme.is_meme_platform,
            crate::constants::KNN_VECTOR_CAT_MEME_WEIGHT,
        ),
        cat(
            sample.flags.meme.is_human_semantic_name,
            crate::constants::KNN_VECTOR_CAT_NAME_WEIGHT,
        ),
        cat(
            sample.flags.streams.is_native_gif,
            crate::constants::KNN_VECTOR_CAT_NATIVE_WEIGHT,
        ),
        cat(
            sample.flags.source.is_high_value_source,
            crate::constants::KNN_VECTOR_CAT_HIGH_VALUE_WEIGHT,
        ),
        cat(
            sample.flags.streams.has_transparency,
            crate::constants::KNN_VECTOR_CAT_TRANS_WEIGHT,
        ),
        cat(
            sample.flags.color.has_embedded_icc,
            crate::constants::KNN_VECTOR_CAT_ICC_WEIGHT / 2.0_f64,
        ),
        cat(
            sample.flags.color.has_complex_color_profile,
            crate::constants::KNN_VECTOR_CAT_COMPLEX_WEIGHT / 2.0_f64,
        ),
    )
}

pub(crate) fn calculate_extended_features(
    sample: &SampleRow,
    stats_map: &FeatureMap,
) -> Option<(f64, f64, f64, f64, f64, f64, f64, f64, f64)> {
    let v_laffin = sample_loop_affinity(sample) / feature_std(stats_map, "loop_affin")?
        * feature_weight(stats_map, "loop_affin").sqrt();

    let v_pdepth = match sample.palette_depth {
        Some(v) => {
            v / feature_std(stats_map, "p_depth")? * feature_weight(stats_map, "p_depth").sqrt()
        }
        None => crate::media_conversion_gate::knn_absent_feature_component(),
    };
    let v_mgini = match sample.motion_gini {
        Some(v) => {
            v / feature_std(stats_map, "m_gini")? * feature_weight(stats_map, "m_gini").sqrt()
        }
        None => crate::media_conversion_gate::knn_absent_feature_component(),
    };
    let v_bskew = match sample.block_skew {
        Some(v) => {
            v / feature_std(stats_map, "b_skew")? * feature_weight(stats_map, "b_skew").sqrt()
        }
        None => crate::media_conversion_gate::knn_absent_feature_component(),
    };
    let v_tflat = match sample.temporal_flatness {
        Some(v) => {
            v / feature_std(stats_map, "t_flat")? * feature_weight(stats_map, "t_flat").sqrt()
        }
        None => crate::media_conversion_gate::knn_absent_feature_component(),
    };
    let v_lclose = match sample.loop_closure_score {
        Some(v) => {
            v / feature_std(stats_map, "l_close")? * feature_weight(stats_map, "l_close").sqrt()
        }
        None => crate::media_conversion_gate::knn_absent_feature_component(),
    };
    let v_mperiod = match sample.motion_periodicity {
        Some(v) => {
            v / feature_std(stats_map, "m_period")? * feature_weight(stats_map, "m_period").sqrt()
        }
        None => crate::media_conversion_gate::knn_absent_feature_component(),
    };
    let v_tjitter = match sample.temporal_jitter {
        Some(v) => {
            v / feature_std(stats_map, "t_jitter")? * feature_weight(stats_map, "t_jitter").sqrt()
        }
        None => crate::media_conversion_gate::knn_absent_feature_component(),
    };

    let v_directory_meme = match sample.directory_loop_intent_score {
        None => crate::media_conversion_gate::knn_absent_feature_component(),
        Some(v) => v * feature_weight(stats_map, "dir_meme").sqrt(),
    };

    Some((
        v_laffin,
        v_pdepth,
        v_mgini,
        v_bskew,
        v_tflat,
        v_lclose,
        v_mperiod,
        v_tjitter,
        v_directory_meme,
    ))
}

pub(crate) fn calculate_new_temporal_file_features(
    sample: &SampleRow,
    stats_map: &FeatureMap,
) -> Result<(f64, f64, f64, f64, f64), String> {
    let v_max_fd = match sample.max_frame_delay {
        Some(v) => {
            v / feature_std(stats_map, "max_fd")
                .ok_or_else(|| "missing feature_std for max_fd".to_string())?
                * feature_weight(stats_map, "max_fd").sqrt()
        }
        None => crate::media_conversion_gate::knn_absent_feature_component(),
    };

    let v_min_fd = match sample.min_frame_delay {
        Some(v) => {
            v / feature_std(stats_map, "min_fd")
                .ok_or_else(|| "missing feature_std for min_fd".to_string())?
                * feature_weight(stats_map, "min_fd").sqrt()
        }
        None => crate::media_conversion_gate::knn_absent_feature_component(),
    };

    let v_audio_dur = match sample.audio_duration_secs {
        Some(v) => {
            v / feature_std(stats_map, "audio_dur")
                .ok_or_else(|| "missing feature_std for audio_dur".to_string())?
                * feature_weight(stats_map, "audio_dur").sqrt()
        }
        None => crate::media_conversion_gate::knn_absent_feature_component(),
    };

    let v_path_depth = {
        let v = f64::from(sample.path_depth);
        v / feature_std(stats_map, "path_depth")
            .ok_or_else(|| "missing feature_std for path_depth".to_string())?
            * feature_weight(stats_map, "path_depth").sqrt()
    };

    let v_density = {
        let v = sample.filename_numeric_density;
        v / feature_std(stats_map, "num_density")
            .ok_or_else(|| "missing feature_std for num_density".to_string())?
            * feature_weight(stats_map, "num_density").sqrt()
    };

    Ok((v_max_fd, v_min_fd, v_audio_dur, v_path_depth, v_density))
}

pub(crate) fn compute_sample_vector(
    sample: &SampleRow,
    stats_map: &FeatureMap,
) -> Result<Vec<f32>> {
    let (v_pix, v_dur, v_frm, v_fsize, v_dens, v_gap, v_temporal_bpp, v_spatial_bpp) =
        calculate_continuous_features(sample, stats_map).ok_or_else(|| {
            anyhow::anyhow!(
                "continuous features unavailable (frame_count/duration_secs missing or \
                 feature_std absent)"
            )
        })?;

    let (v_wratio, v_lfreq, v_cadence, v_payload, v_delay, v_aspect, v_pal) =
        calculate_discrete_features(sample, stats_map)
            .ok_or_else(|| anyhow::anyhow!("discrete features unavailable (feature_std absent)"))?;

    let (v_meme, v_name, v_native, v_hv, v_trans, v_icc, v_complex) =
        calculate_categorical_features(sample);

    let (
        v_laffin,
        v_pdepth,
        v_mgini,
        v_bskew,
        v_tflat,
        v_lclose,
        v_mperiod,
        v_tjitter,
        v_directory_meme,
    ) = calculate_extended_features(sample, stats_map)
        .ok_or_else(|| anyhow::anyhow!("extended features unavailable (feature_std absent)"))?;

    let (v_max_fd, v_min_fd, v_audio_dur, v_path_depth, v_density) =
        calculate_new_temporal_file_features(sample, stats_map).map_err(|err| {
            crate::media_conversion_gate::delivery_db_batch_audit(
                "delivery_db_vector",
                format!("LoopIntent vector feature normalization failed: {err}"),
            );
            anyhow::anyhow!(err)
        })?;

    let mut vec = vec![0.0_f32; 261];
    vec[0] = crate::numeric_cast::f64_to_f32_lossy(v_pix);
    vec[1] = crate::numeric_cast::f64_to_f32_lossy(v_dur);
    vec[2] = crate::numeric_cast::f64_to_f32_lossy(v_frm);
    vec[3] = crate::numeric_cast::f64_to_f32_lossy(v_fsize);
    vec[4] = crate::numeric_cast::f64_to_f32_lossy(v_dens);
    vec[5] = crate::numeric_cast::f64_to_f32_lossy(v_gap);
    vec[6] = crate::numeric_cast::f64_to_f32_lossy(v_temporal_bpp);
    vec[7] = crate::numeric_cast::f64_to_f32_lossy(v_spatial_bpp);
    vec[8] = crate::numeric_cast::f64_to_f32_lossy(v_wratio);
    vec[9] = crate::numeric_cast::f64_to_f32_lossy(v_lfreq);
    vec[10] = crate::numeric_cast::f64_to_f32_lossy(v_laffin);
    vec[11] = crate::numeric_cast::f64_to_f32_lossy(v_cadence);
    vec[12] = crate::numeric_cast::f64_to_f32_lossy(v_payload);
    vec[13] = crate::numeric_cast::f64_to_f32_lossy(v_delay);
    vec[14] = crate::numeric_cast::f64_to_f32_lossy(v_aspect);
    vec[15] = crate::numeric_cast::f64_to_f32_lossy(v_pal);
    vec[16] = crate::numeric_cast::f64_to_f32_lossy(v_pdepth);
    vec[17] = crate::numeric_cast::f64_to_f32_lossy(v_mgini);
    vec[18] = crate::numeric_cast::f64_to_f32_lossy(v_bskew);
    vec[19] = crate::numeric_cast::f64_to_f32_lossy(v_tflat);
    vec[20] = crate::numeric_cast::f64_to_f32_lossy(v_lclose);
    vec[21] = crate::numeric_cast::f64_to_f32_lossy(v_mperiod);
    vec[22] = crate::numeric_cast::f64_to_f32_lossy(v_tjitter);
    vec[23] = crate::numeric_cast::f64_to_f32_lossy(v_directory_meme);
    vec[24] = crate::numeric_cast::f64_to_f32_lossy(v_meme);
    vec[25] = crate::numeric_cast::f64_to_f32_lossy(v_name);
    vec[26] = crate::numeric_cast::f64_to_f32_lossy(v_native);
    vec[27] = crate::numeric_cast::f64_to_f32_lossy(v_hv);
    vec[28] = crate::numeric_cast::f64_to_f32_lossy(v_trans);
    vec[29] = crate::numeric_cast::f64_to_f32_lossy(v_icc);
    vec[30] = crate::numeric_cast::f64_to_f32_lossy(v_complex);
    vec[31] = crate::numeric_cast::f64_to_f32_lossy(v_max_fd);
    vec[32] = crate::numeric_cast::f64_to_f32_lossy(v_min_fd);
    vec[33] = crate::numeric_cast::f64_to_f32_lossy(v_audio_dur);
    vec[34] = crate::numeric_cast::f64_to_f32_lossy(v_path_depth);
    vec[35] = crate::numeric_cast::f64_to_f32_lossy(v_density);

    // ── Authentic Physical Signal Mapping (225 Advanced CV dimensions) ──
    // Maps the mathematically rigorous CBIR features: Color Moments, DCT, HOG, etc.
    if let Some(physics) = &sample.physics_225 {
        crate::real_physics::encode_normalized_physics_225(&mut vec, 36, physics);
    }

    Ok(vec)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::{FeatureMap, SampleFlags};

    #[test]
    fn test_compute_sample_vector_normalizes_signed_physics_tail() {
        let mut physics = vec![0.0_f32; 225];
        physics[0] = 0.25;
        physics[1] = 0.25;
        physics[2] = -10.0;
        physics[3] = 10.0;
        physics[24] = 0.0;

        let sample = SampleRow {
            width: 320,
            height: 240,
            duration_secs: Some(2.0),
            frame_count: Some(20),
            file_size_bytes: 24_000,
            fps: Some(10.0),
            temporal_bpp: 0.5,
            spatial_bpp: 0.5,
            flags: SampleFlags::default(),
            palette_size: Some(128),
            frame_payload_variation: Some(0.2),
            frame_delay_variation: Some(0.1),
            aspect_ratio: Some(320.0 / 240.0),
            loop_frequency: Some(0.8),
            cadence_score: Some(0.7),
            directory_loop_intent_score: Some(0.0),
            palette_depth: Some(8.0),
            motion_gini: Some(0.3),
            block_skew: Some(0.1),
            temporal_flatness: Some(0.2),
            loop_closure_score: Some(0.9),
            motion_periodicity: Some(0.6),
            temporal_jitter: Some(0.1),
            webp_compression_ratio: Some(1.0),
            max_frame_delay: None,
            min_frame_delay: None,
            audio_duration_secs: None,
            path_depth: 0,
            filename_numeric_density: 0.0,
            physics_225: Some(physics),
        };

        let vec = compute_sample_vector(&sample, &FeatureMap::mock()).expect("vector should build"); // audited: db module unit-test fixture assertion; not production DB runtime path
        assert_eq!(vec.len(), 261);
        // New temporal features are None/0 → normalized to 0.0
        assert!((vec[31] - 0.0).abs() < 1.0e-6, "max_frame_delay=None → 0.0");
        assert!((vec[32] - 0.0).abs() < 1.0e-6, "min_frame_delay=None → 0.0");
        assert!((vec[33] - 0.0).abs() < 1.0e-6, "audio_duration=None → 0.0");
        assert!((vec[34] - 0.0).abs() < 1.0e-6, "path_depth=0 → 0.0");
        assert!((vec[35] - 0.0).abs() < 1.0e-6, "num_density=0.0 → 0.0");
        // Physics starts at index 36: physics[0]=0.25 → vec[36]
        assert!((vec[36] - 0.25).abs() < 1.0e-6, "physics[0]=0.25");
        // physics[24]=0.0 normalized to 0.5 → vec[60]
        assert!((vec[60] - 0.5).abs() < 1.0e-6, "physics[24]=0.0 → 0.5");
    }
}
