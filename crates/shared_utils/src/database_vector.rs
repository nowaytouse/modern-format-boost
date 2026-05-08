use crate::database::{FeatureMap, SampleRow};

/// Compute a 31-dimensional pgvector encoding for a sample using pre-calculated std deviations.
/// This precisely bakes the weights and normalization terms from the old dynamically computed KNN
/// into an L2-compatible vector, allowing `PostgreSQL`'s HNSW index to do the heavy lifting!
pub(crate) fn calculate_continuous_features(
    sample: &SampleRow,
    stats_map: &FeatureMap,
) -> Option<(f64, f64, f64, f64, f64, f64, f64, f64)> {
    let get_std = |f: &str| stats_map.stats.get(f).map(|s| s.std_dev.max(1e-6));
    let get_w = |f: &str| {
        stats_map
            .stats
            .get(f)
            .and_then(|s| s.weight)
            .map(|w| w.max(0.01))
    };

    let sample_pixels = (f64::from(sample.width) * f64::from(sample.height)).max(1.0);
    let fc = sample.frame_count?;
    let dur = sample.duration_secs?;
    let sample_frame_density = crate::numeric_cast::u64_to_f64(fc) / dur.max(0.05);
    let sample_frame_gap = dur / crate::numeric_cast::u64_to_f64(fc.max(1));

    Some((
        sample_pixels / get_std("pixels")? * get_w("pixels")?.sqrt(),
        dur / get_std("duration")? * get_w("duration")?.sqrt(),
        crate::numeric_cast::u64_to_f64(fc) / get_std("frame_count")?
            * get_w("frame_count")?.sqrt(),
        crate::numeric_cast::u64_to_f64(sample.file_size_bytes) / get_std("file_size_bytes")?
            * get_w("file_size_bytes")?.sqrt(),
        sample_frame_density / get_std("density")? * get_w("density")?.sqrt(),
        sample_frame_gap / get_std("gap")? * get_w("gap")?.sqrt(),
        sample.temporal_bpp / get_std("temporal_bpp")? * get_w("temporal_bpp")?.sqrt(),
        sample.spatial_bpp / get_std("spatial_bpp")? * get_w("spatial_bpp")?.sqrt(),
    ))
}

pub(crate) fn calculate_discrete_features(
    sample: &SampleRow,
    stats_map: &FeatureMap,
) -> Option<(f64, f64, f64, f64, f64, f64, f64)> {
    let get_std = |f: &str| stats_map.stats.get(f).map(|s| s.std_dev.max(1e-6));
    let get_w = |f: &str| {
        stats_map
            .stats
            .get(f)
            .and_then(|s| s.weight)
            .map(|w| w.max(0.01))
    };

    let sample_webp_ratio =
        crate::numeric_cast::option_f64_strict(sample.webp_compression_ratio, "sample_webp_ratio")?;
    let v_wratio = sample_webp_ratio / get_std("webp_ratio")? * get_w("webp_ratio")?.sqrt();

    let v_lfreq =
        crate::numeric_cast::option_f64_strict(sample.loop_frequency, "sample_loop_freq")?
            / get_std("loop_freq")?
            * get_w("loop_freq")?.sqrt();
    let v_cadence = crate::numeric_cast::option_f64_strict(sample.cadence_score, "sample_cadence")?
        / get_std("cadence")?
        * get_w("cadence")?.sqrt();
    let v_payload = crate::numeric_cast::option_f64_strict(
        sample.frame_payload_variation,
        "sample_payload_var",
    )? / get_std("payload_var")?
        * get_w("payload_var")?.sqrt();
    let v_delay =
        crate::numeric_cast::option_f64_strict(sample.frame_delay_variation, "sample_delay_var")?
            / get_std("delay_var")?
            * get_w("delay_var")?.sqrt();

    let v_aspect = crate::numeric_cast::option_f64_strict(sample.aspect_ratio, "sample_aspect")?
        / get_std("aspect")?
        * get_w("aspect")?.sqrt();
    let v_pal = (sample.palette_size.map(f64::from)? / 256.0_f64) * get_w("p_depth")?.sqrt();

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
            sample.is_meme_platform,
            crate::constants::KNN_VECTOR_CAT_MEME_WEIGHT,
        ),
        cat(
            sample.is_human_semantic_name,
            crate::constants::KNN_VECTOR_CAT_NAME_WEIGHT,
        ),
        cat(
            sample.is_native_gif,
            crate::constants::KNN_VECTOR_CAT_NATIVE_WEIGHT,
        ),
        cat(
            sample.is_high_value_source,
            crate::constants::KNN_VECTOR_CAT_HIGH_VALUE_WEIGHT,
        ),
        cat(
            sample.has_transparency,
            crate::constants::KNN_VECTOR_CAT_TRANS_WEIGHT,
        ),
        cat(
            sample.has_embedded_icc,
            crate::constants::KNN_VECTOR_CAT_ICC_WEIGHT / 2.0_f64,
        ),
        cat(
            sample.has_complex_color_profile,
            crate::constants::KNN_VECTOR_CAT_COMPLEX_WEIGHT / 2.0_f64,
        ),
    )
}

pub(crate) fn calculate_extended_features(
    sample: &SampleRow,
    stats_map: &FeatureMap,
) -> Option<(f64, f64, f64, f64, f64, f64, f64, f64, f64)> {
    let get_std = |f: &str| stats_map.stats.get(f).map(|s| s.std_dev.max(1e-6));
    let get_w = |f: &str| {
        stats_map
            .stats
            .get(f)
            .and_then(|s| s.weight)
            .map(|w| w.max(0.01))
    };

    let sample_audio_score = if sample.is_native_gif {
        1.0_f64
    } else {
        crate::constants::KNN_VECTOR_DEFAULT_AUDIO_SCORE
    };
    let baseline_fps = crate::constants::KNN_VECTOR_BASELINE_FPS;
    let fps_val = sample.fps?;
    let sample_fps_score: f64 = (1.0_f64
        - crate::database::normalize_log_ratio(
            fps_val.max(1e-3),
            baseline_fps,
            crate::constants::KNN_VECTOR_FPS_NORMALIZATION_SCALE,
        ))
    .clamp(0.0_f64, 1.0_f64);

    let loop_freq = sample.loop_frequency?;
    let cadence = sample.cadence_score?;

    let sample_loop_affinity = sample_fps_score
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
        .clamp(0.0, 1.0);

    let v_laffin = sample_loop_affinity / get_std("loop_affin")? * get_w("loop_affin")?.sqrt();

    let v_pdepth = sample.palette_depth? / get_std("p_depth")? * get_w("p_depth")?.sqrt();
    let v_mgini = sample.motion_gini? / get_std("m_gini")? * get_w("m_gini")?.sqrt();
    let v_bskew = sample.block_skew? / get_std("b_skew")? * get_w("b_skew")?.sqrt();
    let v_tflat = sample.temporal_flatness? / get_std("t_flat")? * get_w("t_flat")?.sqrt();
    let v_lclose = sample.loop_closure_score? / get_std("l_close")? * get_w("l_close")?.sqrt();
    let v_mperiod = sample.motion_periodicity? / get_std("m_period")? * get_w("m_period")?.sqrt();
    let v_tjitter = sample.temporal_jitter? / get_std("t_jitter")? * get_w("t_jitter")?.sqrt();

    let v_directory_meme = sample.directory_loop_intent_score? * get_w("dir_meme")?.sqrt();

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

pub(crate) fn compute_sample_vector(
    sample: &SampleRow,
    stats_map: &FeatureMap,
) -> Option<Vec<f32>> {
    let (v_pix, v_dur, v_frm, v_fsize, v_dens, v_gap, v_temporal_bpp, v_spatial_bpp) =
        calculate_continuous_features(sample, stats_map)?;

    let (v_wratio, v_lfreq, v_cadence, v_payload, v_delay, v_aspect, v_pal) =
        calculate_discrete_features(sample, stats_map)?;

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
    ) = calculate_extended_features(sample, stats_map)?;

    Some(vec![
        crate::numeric_cast::f64_to_f32_lossy(v_pix),
        crate::numeric_cast::f64_to_f32_lossy(v_dur),
        crate::numeric_cast::f64_to_f32_lossy(v_frm),
        crate::numeric_cast::f64_to_f32_lossy(v_fsize),
        crate::numeric_cast::f64_to_f32_lossy(v_dens),
        crate::numeric_cast::f64_to_f32_lossy(v_gap),
        crate::numeric_cast::f64_to_f32_lossy(v_temporal_bpp),
        crate::numeric_cast::f64_to_f32_lossy(v_spatial_bpp),
        crate::numeric_cast::f64_to_f32_lossy(v_wratio),
        crate::numeric_cast::f64_to_f32_lossy(v_lfreq),
        crate::numeric_cast::f64_to_f32_lossy(v_laffin),
        crate::numeric_cast::f64_to_f32_lossy(v_cadence),
        crate::numeric_cast::f64_to_f32_lossy(v_payload),
        crate::numeric_cast::f64_to_f32_lossy(v_delay),
        crate::numeric_cast::f64_to_f32_lossy(v_aspect),
        crate::numeric_cast::f64_to_f32_lossy(v_pal),
        crate::numeric_cast::f64_to_f32_lossy(v_pdepth),
        crate::numeric_cast::f64_to_f32_lossy(v_mgini),
        crate::numeric_cast::f64_to_f32_lossy(v_bskew),
        crate::numeric_cast::f64_to_f32_lossy(v_tflat),
        crate::numeric_cast::f64_to_f32_lossy(v_lclose),
        crate::numeric_cast::f64_to_f32_lossy(v_mperiod),
        crate::numeric_cast::f64_to_f32_lossy(v_tjitter),
        crate::numeric_cast::f64_to_f32_lossy(v_directory_meme),
        crate::numeric_cast::f64_to_f32_lossy(v_meme),
        crate::numeric_cast::f64_to_f32_lossy(v_name),
        crate::numeric_cast::f64_to_f32_lossy(v_native),
        crate::numeric_cast::f64_to_f32_lossy(v_hv),
        crate::numeric_cast::f64_to_f32_lossy(v_trans),
        crate::numeric_cast::f64_to_f32_lossy(v_icc),
        crate::numeric_cast::f64_to_f32_lossy(v_complex),
    ])
}
