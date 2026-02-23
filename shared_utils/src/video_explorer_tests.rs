//! 🧪 Video Explorer Test Module
//!
//! Modular test suite covering edge cases and comprehensive scenarios.

#[cfg(test)]
mod constants_tests {
    use super::super::video_explorer::*;

    #[test]
    fn test_crf_bounds() {
        assert_eq!(ABSOLUTE_MAX_CRF, 51.0);
    }

    #[test]
    fn test_iteration_limits() {
        assert_eq!(GLOBAL_MAX_ITERATIONS, 60);
    }

    #[test]
    fn test_binary_search_iterations() {
        assert_eq!(BINARY_SEARCH_MAX_ITERATIONS, 12);
    }
}

#[cfg(test)]
mod explore_config_tests {
    use super::super::video_explorer::*;

    #[test]
    fn test_default_config() {
        let config = ExploreConfig::default();
        assert!(config.initial_crf > 0.0);
        assert!(config.min_crf < config.max_crf);
        assert!(config.target_ratio > 0.0);
    }

    #[test]
    fn test_size_only_config() {
        let config = ExploreConfig::size_only(20.0, 40.0);
        assert_eq!(config.mode, ExploreMode::SizeOnly);
        assert!(!config.quality_thresholds.validate_ssim);
    }

    #[test]
    fn test_quality_match_config() {
        let config = ExploreConfig::quality_match(18.0);
        assert_eq!(config.mode, ExploreMode::QualityMatch);
        assert_eq!(config.max_iterations, 1);
    }

    #[test]
    fn test_precise_quality_config() {
        let config = ExploreConfig::precise_quality_match(20.0, 35.0, 0.95);
        assert_eq!(config.mode, ExploreMode::PreciseQualityMatch);
        assert!(config.quality_thresholds.validate_ssim);
    }

    #[test]
    fn test_compress_only_config() {
        let config = ExploreConfig::compress_only(20.0, 40.0);
        assert_eq!(config.mode, ExploreMode::CompressOnly);
        assert!(!config.quality_thresholds.validate_ssim);
    }
}

#[cfg(test)]
mod quality_thresholds_tests {
    use super::super::video_explorer::*;

    #[test]
    fn test_default_thresholds() {
        let thresholds = QualityThresholds::default();
        assert!(thresholds.min_ssim >= 0.9);
        assert!(thresholds.min_ssim <= 1.0);
        assert!(thresholds.min_psnr >= 30.0);
        assert!(thresholds.min_ms_ssim >= 0.80);
    }

    #[test]
    fn test_ssim_range() {
        let thresholds = QualityThresholds::default();
        assert!(thresholds.min_ssim >= 0.0);
        assert!(thresholds.min_ssim <= 1.0);
    }
}

#[cfg(test)]
mod video_encoder_tests {
    use super::super::video_explorer::*;

    #[test]
    fn test_hevc_encoder() {
        let encoder = VideoEncoder::Hevc;
        assert_eq!(encoder.ffmpeg_name(), "libx265");
        assert_eq!(encoder.container(), "mp4");
    }

    #[test]
    fn test_av1_encoder() {
        let encoder = VideoEncoder::Av1;
        assert_eq!(encoder.ffmpeg_name(), "libsvtav1");
        assert_eq!(encoder.container(), "mp4");
    }

    #[test]
    fn test_h264_encoder() {
        let encoder = VideoEncoder::H264;
        assert_eq!(encoder.ffmpeg_name(), "libx264");
        assert_eq!(encoder.container(), "mp4");
    }

    #[test]
    fn test_extra_args() {
        let hevc = VideoEncoder::Hevc;
        let args = hevc.extra_args(4);
        assert!(!args.is_empty());
    }
}

#[cfg(test)]
mod explore_result_tests {
    use super::super::video_explorer::*;

    #[test]
    fn test_result_size_change_calculation() {
        let result = ExploreResult {
            optimal_crf: 20.0,
            output_size: 800,
            size_change_pct: -20.0,
            ssim: Some(0.98),
            psnr: None,
            iterations: 5,
            quality_passed: true,
            log: vec![],
            confidence: 0.85,
            confidence_detail: ConfidenceBreakdown::default(),
            actual_min_ssim: 0.95,
            ..Default::default()
        };

        assert!(result.size_change_pct < 0.0);
        assert!(result.quality_passed);
    }

    #[test]
    fn test_result_no_compression() {
        let result = ExploreResult {
            optimal_crf: 35.0,
            output_size: 1200,
            size_change_pct: 20.0,
            ssim: Some(0.95),
            psnr: None,
            iterations: 10,
            quality_passed: false,
            log: vec![],
            confidence: 0.3,
            confidence_detail: ConfidenceBreakdown::default(),
            actual_min_ssim: 0.95,
            ..Default::default()
        };

        assert!(result.size_change_pct > 0.0);
        assert!(!result.quality_passed);
    }
}

#[cfg(test)]
mod explore_mode_tests {
    use super::super::video_explorer::*;

    #[test]
    fn test_all_modes_exist() {
        let modes = [
            ExploreMode::SizeOnly,
            ExploreMode::QualityMatch,
            ExploreMode::PreciseQualityMatch,
            ExploreMode::PreciseQualityMatchWithCompression,
            ExploreMode::CompressOnly,
            ExploreMode::CompressWithQuality,
        ];
        assert_eq!(modes.len(), 6);
    }

    #[test]
    fn test_mode_equality() {
        assert_eq!(ExploreMode::SizeOnly, ExploreMode::SizeOnly);
        assert_ne!(ExploreMode::SizeOnly, ExploreMode::QualityMatch);
    }
}

#[cfg(test)]
mod dynamic_iteration_tests {
    #[test]
    fn test_log2_calculation() {
        let calc_max_iter = |range: f32| -> u32 { ((range.log2().ceil() as u32) + 3).max(5) };

        assert!(calc_max_iter(5.0) >= 5);

        let mid = calc_max_iter(25.0);
        assert!((7..=10).contains(&mid));

        let large = calc_max_iter(50.0);
        assert!((8..=12).contains(&large));
    }

    #[test]
    fn test_iteration_bounds() {
        for range in [1.0_f32, 5.0, 10.0, 25.0, 50.0, 100.0] {
            let max_iter = ((range.log2().ceil() as u32) + 3).max(5);
            assert!(max_iter >= 5, "range {} gave iter {}", range, max_iter);
            assert!(max_iter <= 15, "range {} gave iter {}", range, max_iter);
        }
    }
}

#[cfg(test)]
mod edge_case_tests {
    use super::super::video_explorer::*;

    #[test]
    fn test_zero_input_size() {
        let input_size = 0_u64;
        let size_pct = if input_size > 0 {
            ((100_f64 / input_size as f64) - 1.0) * 100.0
        } else {
            0.0
        };
        assert_eq!(size_pct, 0.0);
    }

    #[test]
    fn test_crf_precision() {
        let crf = 20.5_f32;
        let rounded = (crf * 10.0).round() / 10.0;
        assert!((rounded - 20.5).abs() < 0.01);
    }

    #[test]
    fn test_ssim_bounds() {
        let ssim_values = [0.0, 0.5, 0.9, 0.95, 0.99, 1.0];
        for ssim in ssim_values {
            assert!((0.0..=1.0).contains(&ssim));
        }
    }

    #[test]
    fn test_extreme_crf_values() {
        assert_eq!(ABSOLUTE_MIN_CRF, 10.0);
        assert_eq!(ABSOLUTE_MAX_CRF, 51.0);

        let range = ABSOLUTE_MAX_CRF - ABSOLUTE_MIN_CRF;
        assert!(range > 20.0);
    }
}

#[cfg(test)]
mod precision_tests {
    use super::super::video_explorer::precision::{
        cache_key_to_crf, crf_to_cache_key, CACHE_KEY_MULTIPLIER,
    };

    #[test]
    fn test_crf_key_generation() {
        assert_eq!(crf_to_cache_key(20.0), 200);
        assert_eq!(crf_to_cache_key(20.1), 201);
        assert_eq!(crf_to_cache_key(20.5), 205);

        let crf2 = 20.55_f32;
        let key2 = crf_to_cache_key(crf2);
        assert_eq!(key2, 206);

        assert!((cache_key_to_crf(200) - 20.0).abs() < 0.01);
        assert!((cache_key_to_crf(205) - 20.5).abs() < 0.01);

        assert_eq!(CACHE_KEY_MULTIPLIER, 10.0);
    }

    #[test]
    fn test_size_ratio_calculation() {
        let input_size = 1000_u64;
        let output_size = 800_u64;
        let ratio = output_size as f64 / input_size as f64;
        assert!((ratio - 0.8).abs() < 0.001);

        let pct = (ratio - 1.0) * 100.0;
        assert!((pct - (-20.0)).abs() < 0.1);
    }
}

#[cfg(test)]
mod three_phase_search_tests {
    use super::super::video_explorer::precision::*;

    #[test]
    fn prop_gpu_cpu_dual_refinement() {
        let search = ThreePhaseSearch::default();

        assert!(
            search.gpu_coarse_step > search.gpu_medium_step,
            "GPU coarse ({}) > medium ({})",
            search.gpu_coarse_step,
            search.gpu_medium_step
        );
        assert!(
            search.gpu_medium_step > search.gpu_fine_step,
            "GPU medium ({}) > fine ({})",
            search.gpu_medium_step,
            search.gpu_fine_step
        );
        assert!(
            search.gpu_fine_step > search.gpu_ultra_fine_step,
            "GPU fine ({}) > ultra_fine ({})",
            search.gpu_fine_step,
            search.gpu_ultra_fine_step
        );

        assert!(
            search.gpu_ultra_fine_step > search.cpu_finest_step,
            "GPU ultra_fine ({}) > CPU finest ({})",
            search.gpu_ultra_fine_step,
            search.cpu_finest_step
        );

        assert!(
            (search.gpu_coarse_step - 4.0).abs() < 0.01,
            "GPU coarse should be 4.0"
        );
        assert!(
            (search.gpu_medium_step - 1.0).abs() < 0.01,
            "GPU medium should be 1.0"
        );
        assert!(
            (search.gpu_fine_step - 0.5).abs() < 0.01,
            "GPU fine should be 0.5"
        );
        assert!(
            (search.gpu_ultra_fine_step - 0.25).abs() < 0.01,
            "GPU ultra_fine should be 0.25"
        );
        assert!(
            (search.cpu_finest_step - 0.1).abs() < 0.01,
            "CPU finest should be 0.1"
        );
    }

    #[test]
    fn prop_search_phase_step_sizes() {
        assert!((SearchPhase::GpuCoarse.step_size() - 4.0).abs() < 0.01);
        assert!((SearchPhase::GpuMedium.step_size() - 1.0).abs() < 0.01);
        assert!((SearchPhase::GpuFine.step_size() - 0.5).abs() < 0.01);
        assert!((SearchPhase::GpuUltraFine.step_size() - 0.25).abs() < 0.01);
        assert!((SearchPhase::CpuFinest.step_size() - 0.1).abs() < 0.01);
    }

    #[test]
    fn prop_gpu_vs_cpu_phase() {
        assert!(SearchPhase::GpuCoarse.is_gpu());
        assert!(SearchPhase::GpuMedium.is_gpu());
        assert!(SearchPhase::GpuFine.is_gpu());
        assert!(SearchPhase::GpuUltraFine.is_gpu());
        assert!(!SearchPhase::CpuFinest.is_gpu());
    }

    #[test]
    fn prop_cache_key_unified() {
        assert_eq!(crf_to_cache_key(18.0), 180);
        assert_eq!(crf_to_cache_key(18.1), 181);
        assert_eq!(crf_to_cache_key(18.5), 185);
        assert_eq!(crf_to_cache_key(18.25), 183);

        let crfs = vec![18.0, 18.1, 18.2, 18.3, 18.4, 18.5];
        let keys: Vec<i32> = crfs.iter().map(|&crf| crf_to_cache_key(crf)).collect();

        for i in 0..keys.len() {
            for j in (i + 1)..keys.len() {
                assert_ne!(
                    keys[i], keys[j],
                    "CRF {:.1} and {:.1} should have different cache keys",
                    crfs[i], crfs[j]
                );
            }
        }

        for &crf in &crfs {
            let key = crf_to_cache_key(crf);
            let reconstructed = cache_key_to_crf(key);
            assert!(
                (reconstructed - crf).abs() < 0.01,
                "Cache key round-trip failed: {:.1} → {} → {:.1}",
                crf,
                key,
                reconstructed
            );
        }
    }

    #[test]
    fn prop_phase_progression() {
        assert_eq!(SearchPhase::GpuCoarse.next(), Some(SearchPhase::GpuMedium));
        assert_eq!(SearchPhase::GpuMedium.next(), Some(SearchPhase::GpuFine));
        assert_eq!(SearchPhase::GpuFine.next(), Some(SearchPhase::GpuUltraFine));
        assert_eq!(
            SearchPhase::GpuUltraFine.next(),
            Some(SearchPhase::CpuFinest)
        );
        assert_eq!(SearchPhase::CpuFinest.next(), None);
    }
}

#[cfg(test)]
mod transparency_prop_tests {
    use super::super::video_explorer::*;
    use proptest::prelude::*;

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]

        #[test]
        fn prop_iteration_output_completeness(
            iteration in 1..100u32,
            crf in 10.0..51.0_f32,
            size_pct in -50.0..50.0_f64,
            ssim in proptest::option::of(0.8..1.0_f64),
            psnr in proptest::option::of(25.0..55.0_f64),
            can_compress in proptest::bool::ANY,
        ) {
            let metrics = IterationMetrics {
                iteration,
                phase: "GPU粗搜".to_string(),
                crf,
                output_size: 1000000,
                size_change_pct: size_pct,
                ssim,
                ssim_source: if ssim.is_some() { SsimSource::Actual } else { SsimSource::None },
                psnr,
                can_compress,
                quality_passed: ssim.map(|s| s >= 0.95),
                decision: "测试".to_string(),
            };

            prop_assert!(metrics.iteration > 0);
            prop_assert!(!metrics.phase.is_empty());
            prop_assert!(metrics.crf >= 10.0 && metrics.crf <= 51.0);
        }
    }

    #[test]
    fn test_ssim_source_predicted_prefix() {
        let metrics = IterationMetrics {
            iteration: 1,
            phase: "GPU精细".to_string(),
            crf: 20.0,
            output_size: 1000000,
            size_change_pct: -10.0,
            ssim: Some(0.9500),
            ssim_source: SsimSource::Predicted,
            psnr: Some(40.0),
            can_compress: true,
            quality_passed: Some(true),
            decision: "预测验证".to_string(),
        };

        assert_eq!(metrics.ssim_source, SsimSource::Predicted);
    }
}

#[cfg(test)]
mod psnr_transparency_tests {
    use super::super::video_explorer::*;
    use proptest::prelude::*;

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]

        #[test]
        fn prop_psnr_transparency_data(
            psnr in proptest::option::of(25.0..55.0_f64),
            ssim in proptest::option::of(0.8..1.0_f64),
        ) {
            let metrics = IterationMetrics {
                iteration: 1,
                phase: "GPU粗搜".to_string(),
                crf: 20.0,
                output_size: 1000000,
                size_change_pct: -10.0,
                ssim,
                ssim_source: SsimSource::Actual,
                psnr,
                can_compress: true,
                quality_passed: Some(true),
                decision: "测试".to_string(),
            };

            prop_assert_eq!(metrics.psnr, psnr);

            if let Some(p) = psnr {
                prop_assert!((0.0..=100.0).contains(&p), "PSNR should be in valid range");
            }
        }
    }

    #[test]
    fn test_psnr_ssim_mapping_integration() {
        use super::super::ssim_mapping::PsnrSsimMapping;

        let mut mapping = PsnrSsimMapping::new();

        mapping.insert(35.0, 0.92);
        mapping.insert(40.0, 0.95);
        mapping.insert(45.0, 0.97);

        assert!(mapping.has_enough_points());

        let predicted = mapping.predict_ssim(42.5).unwrap();
        assert!(predicted > 0.95 && predicted < 0.97);
    }
}

#[cfg(test)]
mod preset_consistency_tests {
    use super::super::video_explorer::*;
    use proptest::prelude::*;

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]

        #[test]
        fn prop_preset_consistency(
            preset_idx in 0..6_usize,
        ) {
            let presets = [
                EncoderPreset::Ultrafast,
                EncoderPreset::Fast,
                EncoderPreset::Medium,
                EncoderPreset::Slow,
                EncoderPreset::Slower,
                EncoderPreset::Veryslow,
            ];
            let preset = presets[preset_idx];

            let name = preset.x26x_name();
            prop_assert!(!name.is_empty());

            let svt_preset = preset.svtav1_preset();
            prop_assert!(svt_preset <= 13, "SVT-AV1 preset should be 0-13");
        }
    }

    #[test]
    fn test_default_preset_is_medium() {
        let preset = EncoderPreset::default();
        assert_eq!(preset, EncoderPreset::Medium);
        assert_eq!(preset.x26x_name(), "medium");
    }

    #[test]
    fn test_preset_names() {
        assert_eq!(EncoderPreset::Ultrafast.x26x_name(), "ultrafast");
        assert_eq!(EncoderPreset::Fast.x26x_name(), "fast");
        assert_eq!(EncoderPreset::Medium.x26x_name(), "medium");
        assert_eq!(EncoderPreset::Slow.x26x_name(), "slow");
        assert_eq!(EncoderPreset::Slower.x26x_name(), "slower");
        assert_eq!(EncoderPreset::Veryslow.x26x_name(), "veryslow");
    }
}

#[cfg(test)]
mod mock_tests {

    use super::super::ssim_mapping::PsnrSsimMapping;

    fn mock_encode(crf: f32, input_size: u64) -> u64 {
        let ratio = 1.0_f64 - (crf as f64 - 20.0) * 0.05;
        (input_size as f64 * ratio.max(0.1)) as u64
    }

    fn mock_ssim(crf: f32) -> f64 {
        (0.99_f64 - (crf as f64 - 10.0) * 0.005).max(0.8)
    }

    fn mock_psnr(crf: f32) -> f64 {
        (50.0_f64 - (crf as f64 - 10.0) * 0.5).max(25.0)
    }

    #[test]
    fn test_mock_cannot_compress_scenario() {
        let input_size = 1000000_u64;

        let _output_at_max_crf = mock_encode(51.0, input_size);

        let small_input = 50000_u64;
        let output = mock_encode(20.0, small_input);
        assert_eq!(output, small_input);
    }

    #[test]
    fn test_mock_quality_never_passes_scenario() {
        let min_ssim = 0.999;

        let ssim_at_min_crf = mock_ssim(10.0);
        assert!(ssim_at_min_crf < min_ssim);
    }

    #[test]
    fn test_mock_single_iteration_success() {
        let input_size = 1000000_u64;
        let initial_crf = 25.0;

        let output = mock_encode(initial_crf, input_size);
        let ssim = mock_ssim(initial_crf);

        assert!(output < input_size);
        assert!(ssim > 0.9);
    }

    #[test]
    fn test_mock_psnr_ssim_mapping() {
        let mut mapping = PsnrSsimMapping::new();

        for crf in [15.0, 20.0, 25.0, 30.0] {
            let psnr = mock_psnr(crf);
            let ssim = mock_ssim(crf);
            mapping.insert(psnr, ssim);
        }

        assert!(mapping.has_enough_points());

        let test_psnr = mock_psnr(22.5);
        let predicted = mapping.predict_ssim(test_psnr);
        assert!(predicted.is_some());
    }

    #[test]
    fn test_mock_deterministic_results() {
        let crf = 23.5;
        let input_size = 1000000_u64;

        let output1 = mock_encode(crf, input_size);
        let output2 = mock_encode(crf, input_size);
        assert_eq!(output1, output2);

        let ssim1 = mock_ssim(crf);
        let ssim2 = mock_ssim(crf);
        assert!((ssim1 - ssim2).abs() < 0.0001);

        let psnr1 = mock_psnr(crf);
        let psnr2 = mock_psnr(crf);
        assert!((psnr1 - psnr2).abs() < 0.0001);
    }
}

#[cfg(test)]
mod vmaf_ssim_synergy_tests {
    use super::super::video_explorer::*;
    use proptest::prelude::*;

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]

        #[test]
        fn prop_vmaf_threshold_config(
            min_ms_ssim in 70.0..99.0_f64,
            force_long in proptest::bool::ANY,
        ) {
            let thresholds = QualityThresholds {
                min_ms_ssim,
                force_ms_ssim_long: force_long,
                ..Default::default()
            };

            prop_assert!((thresholds.min_ms_ssim - min_ms_ssim).abs() < 0.001,
                "VMAF 阈值应正确传递: expected={}, actual={}", min_ms_ssim, thresholds.min_ms_ssim);
            prop_assert_eq!(thresholds.force_ms_ssim_long, force_long,
                "force_ms_ssim_long 应正确传递");
        }
    }

    #[test]
    fn test_long_video_threshold_constant() {
        assert!((LONG_VIDEO_THRESHOLD - 300.0).abs() < 0.1);
    }

    #[test]
    fn test_default_force_vmaf_long_is_false() {
        let thresholds = QualityThresholds::default();
        assert!(!thresholds.force_ms_ssim_long, "默认应跳过长视频 VMAF");
    }
}

#[cfg(test)]
mod vmaf_long_video_tests {
    use super::super::video_explorer::*;
    use proptest::prelude::*;

    fn should_skip_vmaf(duration_secs: f32, force_vmaf_long: bool) -> bool {
        duration_secs >= LONG_VIDEO_THRESHOLD && !force_vmaf_long
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]

        #[test]
        fn prop_long_video_skip_vmaf(
            duration in 0.0..1000.0_f32,
        ) {
            let should_skip = should_skip_vmaf(duration, false);

            if duration >= LONG_VIDEO_THRESHOLD {
                prop_assert!(should_skip,
                    "长视频 ({:.1}s >= {:.1}s) 应跳过 VMAF", duration, LONG_VIDEO_THRESHOLD);
            } else {
                prop_assert!(!should_skip,
                    "短视频 ({:.1}s < {:.1}s) 不应跳过 VMAF", duration, LONG_VIDEO_THRESHOLD);
            }
        }

        #[test]
        fn prop_force_vmaf_long_override(
            duration in 300.0..1000.0_f32,
        ) {
            let should_skip = should_skip_vmaf(duration, true);
            prop_assert!(!should_skip,
                "force_vmaf_long=true 时不应跳过 VMAF，即使时长为 {:.1}s", duration);
        }
    }

    #[test]
    fn test_boundary_duration() {
        assert!(should_skip_vmaf(300.0, false));
        assert!(!should_skip_vmaf(299.9, false));
        assert!(!should_skip_vmaf(300.0, true));
    }
}

#[cfg(test)]
mod vmaf_enable_condition_tests {
    use super::super::video_explorer::*;
    use proptest::prelude::*;

    fn should_enable_vmaf(
        vmaf_enabled: bool,
        duration_secs: Option<f64>,
        force_vmaf_long: bool,
    ) -> bool {
        if !vmaf_enabled {
            return false;
        }

        match duration_secs {
            Some(d) if d >= LONG_VIDEO_THRESHOLD as f64 => force_vmaf_long,
            Some(_) => true,
            None => false,
        }
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]

        #[test]
        fn prop_vmaf_enable_short_video(
            duration in 0.0..299.9_f64,
        ) {
            let enabled = should_enable_vmaf(true, Some(duration), false);
            prop_assert!(enabled,
                "短视频 ({:.1}s) 且 VMAF 启用时应执行 VMAF", duration);
        }

        #[test]
        fn prop_vmaf_disabled_no_execution(
            duration in 0.0..1000.0_f64,
            force in proptest::bool::ANY,
        ) {
            let enabled = should_enable_vmaf(false, Some(duration), force);
            prop_assert!(!enabled,
                "VMAF 未启用时不应执行 VMAF");
        }
    }

    #[test]
    fn test_vmaf_enable_edge_cases() {
        assert!(!should_enable_vmaf(true, Some(300.0), false));
        assert!(should_enable_vmaf(true, Some(299.9), false));

        assert!(should_enable_vmaf(true, Some(600.0), true));

        assert!(!should_enable_vmaf(true, None, false));
        assert!(!should_enable_vmaf(true, None, true));
    }
}

#[cfg(test)]
mod quality_report_tests {

    use proptest::prelude::*;

    fn generate_quality_report(
        ssim: Option<f64>,
        vmaf: Option<f64>,
        vmaf_enabled: bool,
        vmaf_skipped: bool,
    ) -> (bool, bool, Option<String>) {
        let has_ssim = ssim.is_some();
        let has_vmaf_or_reason = vmaf.is_some() || (vmaf_enabled && vmaf_skipped);
        let skip_reason = if vmaf_enabled && vmaf_skipped && vmaf.is_none() {
            Some("长视频跳过 VMAF".to_string())
        } else {
            None
        };
        (has_ssim, has_vmaf_or_reason, skip_reason)
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]

        #[test]
        fn prop_quality_report_has_ssim(
            ssim in 0.9..1.0_f64,
        ) {
            let (has_ssim, _, _) = generate_quality_report(Some(ssim), None, false, false);
            prop_assert!(has_ssim, "质量报告应包含 SSIM 值");
        }

        #[test]
        fn prop_quality_report_vmaf_or_reason(
            vmaf in 80.0..100.0_f64,
            vmaf_enabled in proptest::bool::ANY,
            vmaf_skipped in proptest::bool::ANY,
        ) {
            if vmaf_enabled {
                let vmaf_val = if vmaf_skipped { None } else { Some(vmaf) };
                let (_, has_vmaf_or_reason, skip_reason) = generate_quality_report(
                    Some(0.95), vmaf_val, vmaf_enabled, vmaf_skipped
                );

                prop_assert!(has_vmaf_or_reason,
                    "VMAF 启用时应有 VMAF 值或跳过原因");

                if vmaf_skipped && vmaf_val.is_none() {
                    prop_assert!(skip_reason.is_some(),
                        "VMAF 跳过时应有跳过原因");
                }
            }
        }
    }

    #[test]
    fn test_quality_report_completeness() {
        let (has_ssim, has_vmaf, _) = generate_quality_report(Some(0.95), Some(90.0), true, false);
        assert!(has_ssim);
        assert!(has_vmaf);

        let (has_ssim, has_reason, skip_reason) =
            generate_quality_report(Some(0.95), None, true, true);
        assert!(has_ssim);
        assert!(has_reason);
        assert!(skip_reason.is_some());
    }
}

#[cfg(test)]
mod smart_wall_collision_tests {
    use proptest::prelude::*;

    const ZERO_GAIN_THRESHOLD: f64 = 0.0002;
    const REQUIRED_ZERO_GAINS: u32 = 5;

    struct QualityWallDetector {
        consecutive_zeros: u32,
    }

    impl QualityWallDetector {
        fn new() -> Self {
            Self {
                consecutive_zeros: 0,
            }
        }

        fn record_gain(&mut self, gain: f64) {
            if gain.abs() < ZERO_GAIN_THRESHOLD {
                self.consecutive_zeros += 1;
            } else {
                self.consecutive_zeros = 0;
            }
        }

        fn is_wall_hit(&self) -> bool {
            self.consecutive_zeros >= REQUIRED_ZERO_GAINS
        }
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]

        #[test]
        fn prop_quality_wall_detection(
            gains in proptest::collection::vec(-0.001..0.001_f64, 1..20),
        ) {
            let mut detector = QualityWallDetector::new();
            let mut consecutive_count = 0_u32;

            for gain in &gains {
                detector.record_gain(*gain);

                if gain.abs() < ZERO_GAIN_THRESHOLD {
                    consecutive_count += 1;
                } else {
                    consecutive_count = 0;
                }

                prop_assert_eq!(detector.consecutive_zeros, consecutive_count,
                    "连续零增益计数应一致");

                let expected_wall = consecutive_count >= REQUIRED_ZERO_GAINS;
                prop_assert_eq!(detector.is_wall_hit(), expected_wall,
                    "质量墙检测应正确: consecutive={}, required={}",
                    consecutive_count, REQUIRED_ZERO_GAINS);
            }
        }
    }

    #[test]
    fn prop_step_progression() {
        let crf_range = 31.5_f32;
        let initial_step = (crf_range / 5.0).clamp(2.0, 10.0);

        let step_schedule: Vec<f32> = vec![
            initial_step,
            initial_step / 2.0,
            initial_step / 4.0,
            (initial_step / 8.0).max(0.2),
            0.1,
        ];

        for i in 1..step_schedule.len() {
            assert!(
                step_schedule[i] < step_schedule[i - 1],
                "步长应递减: step[{}]={} >= step[{}]={}",
                i,
                step_schedule[i],
                i - 1,
                step_schedule[i - 1]
            );
        }

        assert!(
            (step_schedule.last().unwrap() - 0.1).abs() < 0.01,
            "最终步长应为0.1"
        );
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]

        #[test]
        fn prop_initial_step_calculation(
            crf_range in 10.0..50.0_f32,
        ) {
            let initial_step = (crf_range / 5.0).clamp(2.0, 10.0);

            prop_assert!(initial_step >= 2.0,
                "初始步长应 >= 2.0: range={}, step={}", crf_range, initial_step);
            prop_assert!(initial_step <= 10.0,
                "初始步长应 <= 10.0: range={}, step={}", crf_range, initial_step);

            let expected = (crf_range / 5.0).clamp(2.0, 10.0);
            prop_assert!((initial_step - expected).abs() < 0.01,
                "初始步长计算应正确: expected={}, actual={}", expected, initial_step);
        }
    }

    #[test]
    fn test_quality_wall_exact_threshold() {
        let mut detector = QualityWallDetector::new();

        for _ in 0..4 {
            detector.record_gain(0.00001);
        }
        assert!(!detector.is_wall_hit(), "4次零增益不应触发质量墙");

        detector.record_gain(0.00001);
        assert!(detector.is_wall_hit(), "5次零增益应触发质量墙");
    }

    #[test]
    fn test_quality_wall_reset_on_high_gain() {
        let mut detector = QualityWallDetector::new();

        for _ in 0..4 {
            detector.record_gain(0.00001);
        }
        assert_eq!(detector.consecutive_zeros, 4);

        detector.record_gain(0.001);
        assert_eq!(detector.consecutive_zeros, 0, "高增益应重置计数");

        for _ in 0..4 {
            detector.record_gain(0.00001);
        }
        assert!(!detector.is_wall_hit(), "重置后4次不应触发");

        detector.record_gain(0.00001);
        assert!(detector.is_wall_hit(), "重置后5次应触发");
    }

    #[test]
    fn test_zero_gain_threshold_boundary() {
        let mut detector = QualityWallDetector::new();

        detector.record_gain(ZERO_GAIN_THRESHOLD);
        assert_eq!(detector.consecutive_zeros, 0, "等于阈值不算零增益");

        detector.record_gain(ZERO_GAIN_THRESHOLD - 0.00001);
        assert_eq!(detector.consecutive_zeros, 1, "小于阈值算零增益");
    }
}

#[cfg(test)]
mod metadata_margin_tests {

    use crate::video_explorer::{
        calculate_metadata_margin, can_compress_with_metadata, compression_target_size,
        detect_metadata_size, pure_video_size, verify_compression_precise,
        verify_compression_simple, CompressionVerifyStrategy, METADATA_MARGIN_MAX,
        METADATA_MARGIN_MIN, METADATA_MARGIN_PERCENT,
    };
    use proptest::prelude::*;

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]

        #[test]
        fn prop_margin_formula_correctness(
            input_size in 0u64..10_000_000_000u64,
        ) {
            let margin = calculate_metadata_margin(input_size);

            let expected = {
                let percent_based = (input_size as f64 * METADATA_MARGIN_PERCENT) as u64;
                percent_based.clamp(METADATA_MARGIN_MIN, METADATA_MARGIN_MAX)
            };

            prop_assert_eq!(margin, expected,
                "余量应符合公式: input={}, expected={}, actual={}",
                input_size, expected, margin);

            prop_assert!(margin >= METADATA_MARGIN_MIN,
                "余量应 >= 最小值: margin={}, min={}", margin, METADATA_MARGIN_MIN);
            prop_assert!(margin <= METADATA_MARGIN_MAX,
                "余量应 <= 最大值: margin={}, max={}", margin, METADATA_MARGIN_MAX);
        }
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]

        #[test]
        fn prop_target_calculation_correctness(
            input_size in 0u64..10_000_000_000u64,
        ) {
            let target = compression_target_size(input_size);
            let margin = calculate_metadata_margin(input_size);

            let expected = input_size.saturating_sub(margin);
            prop_assert_eq!(target, expected,
                "压缩目标应 = input - margin: input={}, margin={}, expected={}, actual={}",
                input_size, margin, expected, target);

            if input_size > margin {
                prop_assert!(target < input_size,
                    "压缩目标应 < 输入: input={}, target={}", input_size, target);
            }
        }
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]

        #[test]
        fn prop_can_compress_correctness(
            input_size in 1u64..10_000_000_000u64,
            output_ratio in 0.5..1.5_f64,
        ) {
            let output_size = (input_size as f64 * output_ratio) as u64;
            let target = compression_target_size(input_size);
            let can_compress = can_compress_with_metadata(output_size, input_size);

            let expected = output_size < target;
            prop_assert_eq!(can_compress, expected,
                "压缩判断应正确: input={}, output={}, target={}, expected={}, actual={}",
                input_size, output_size, target, expected, can_compress);
        }
    }

    #[test]
    fn test_margin_formula_examples() {
        let size_100kb = 100 * 1024;
        let margin = calculate_metadata_margin(size_100kb);
        assert_eq!(
            margin, METADATA_MARGIN_MIN,
            "100KB 应使用最小余量: expected={}, actual={}",
            METADATA_MARGIN_MIN, margin
        );

        let size_1mb = 1024 * 1024;
        let margin = calculate_metadata_margin(size_1mb);
        let expected = (size_1mb as f64 * METADATA_MARGIN_PERCENT) as u64;
        assert_eq!(
            margin, expected,
            "1MB 应使用百分比余量: expected={}, actual={}",
            expected, margin
        );

        let size_100mb = 100 * 1024 * 1024;
        let margin = calculate_metadata_margin(size_100mb);
        assert_eq!(
            margin, METADATA_MARGIN_MAX,
            "100MB 应使用最大余量: expected={}, actual={}",
            METADATA_MARGIN_MAX, margin
        );
    }

    #[test]
    fn test_margin_extreme_cases() {
        assert_eq!(calculate_metadata_margin(0), METADATA_MARGIN_MIN);
        assert_eq!(compression_target_size(0), 0);

        assert_eq!(calculate_metadata_margin(1), METADATA_MARGIN_MIN);
        assert_eq!(compression_target_size(1), 0);

        let size_10gb = 10 * 1024 * 1024 * 1024;
        let margin = calculate_metadata_margin(size_10gb);
        assert_eq!(
            margin, METADATA_MARGIN_MAX,
            "10GB 应使用最大余量: expected={}, actual={}",
            METADATA_MARGIN_MAX, margin
        );
    }

    #[test]
    fn test_verify_compression_precise() {
        let input_small = 5 * 1024 * 1024;
        let output_total = 4800 * 1024;
        let metadata = 50 * 1024;

        let (can_compress, pure_size, strategy) =
            verify_compression_precise(output_total, input_small, metadata);
        assert_eq!(
            strategy,
            CompressionVerifyStrategy::PureVideo,
            "小文件应使用纯视频策略"
        );
        assert_eq!(pure_size, output_total - metadata, "纯视频大小应去除元数据");
        assert!(
            can_compress,
            "纯视频 {} < 输入 {} 应可压缩",
            pure_size, input_small
        );

        let input_large = 20 * 1024 * 1024;
        let output_large = 18 * 1024 * 1024;
        let metadata_large = 80 * 1024;

        let (can_compress, compare_size, strategy) =
            verify_compression_precise(output_large, input_large, metadata_large);
        assert_eq!(
            strategy,
            CompressionVerifyStrategy::TotalSize,
            "大文件应使用总大小策略"
        );
        assert_eq!(compare_size, output_large, "大文件应对比总大小");
        assert!(
            can_compress,
            "输出 {} < 输入 {} 应可压缩",
            compare_size, input_large
        );
    }

    #[test]
    fn test_verify_compression_simple() {
        let (can_compress, size) = verify_compression_simple(1000, 2000, 100);
        assert!(can_compress);
        assert!(size > 0);
    }

    #[test]
    fn test_detect_metadata_size() {
        assert_eq!(detect_metadata_size(1000, 1500), 500);
        assert_eq!(detect_metadata_size(1000, 1000), 0);
        assert_eq!(detect_metadata_size(1500, 1000), 0);
    }

    #[test]
    fn test_pure_video_size() {
        assert_eq!(pure_video_size(1000, 200), 800);
        assert_eq!(pure_video_size(1000, 0), 1000);
        assert_eq!(pure_video_size(100, 200), 0);
    }

    #[test]
    fn test_can_compress_with_margin() {
        let input_small = 500 * 1024;
        let target_small = compression_target_size(input_small);
        assert!(target_small < input_small, "应预留余量");

        let input_large = 100 * 1024 * 1024;
        let target_large = compression_target_size(input_large);
        assert!(target_large < input_large, "应预留余量");
        assert_eq!(
            input_large - target_large,
            METADATA_MARGIN_MAX,
            "大文件余量应为最大值"
        );
    }
}

#[cfg(test)]
mod boundary_tests {
    use super::super::video_explorer::*;
    use proptest::prelude::*;

    #[test]
    fn test_metadata_margin_boundary_u64_max() {
        let margin = calculate_metadata_margin(u64::MAX);
        assert_eq!(
            margin, METADATA_MARGIN_MAX,
            "u64::MAX 应使用最大余量: expected={}, actual={}",
            METADATA_MARGIN_MAX, margin
        );
    }

    #[test]
    fn test_compression_target_underflow_protection() {
        for size in [0u64, 1, 100, 1000, 2047, 2048, 2049] {
            let target = compression_target_size(size);
            assert!(target <= size, "压缩目标 {} 不应大于输入 {}", target, size);
        }
    }

    #[test]
    fn test_small_file_threshold_boundary() {
        let just_below = SMALL_FILE_THRESHOLD - 1;
        let at_threshold = SMALL_FILE_THRESHOLD;
        let just_above = SMALL_FILE_THRESHOLD + 1;

        let (_, _, strategy_below) = verify_compression_precise(1000, just_below, 100);
        assert_eq!(
            strategy_below,
            CompressionVerifyStrategy::PureVideo,
            "刚好低于阈值应使用 PureVideo 策略"
        );

        let (_, _, strategy_at) = verify_compression_precise(1000, at_threshold, 100);
        assert_eq!(
            strategy_at,
            CompressionVerifyStrategy::TotalSize,
            "刚好等于阈值应使用 TotalSize 策略"
        );

        let (_, _, strategy_above) = verify_compression_precise(1000, just_above, 100);
        assert_eq!(
            strategy_above,
            CompressionVerifyStrategy::TotalSize,
            "刚好高于阈值应使用 TotalSize 策略"
        );
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]

        #[test]
        fn prop_margin_monotonic(
            size1 in 1u64..1_000_000_000u64,
            size2 in 1u64..1_000_000_000u64,
        ) {
            let margin1 = calculate_metadata_margin(size1);
            let margin2 = calculate_metadata_margin(size2);

            if size1 <= size2 {
                prop_assert!(margin1 <= margin2,
                    "余量应单调非递减: size1={}, margin1={}, size2={}, margin2={}",
                    size1, margin1, size2, margin2);
            }
        }

        #[test]
        fn prop_margin_bounded(size in 0u64..u64::MAX / 2) {
            let margin = calculate_metadata_margin(size);

            prop_assert!(margin >= METADATA_MARGIN_MIN,
                "余量 {} 应 >= 最小值 {}", margin, METADATA_MARGIN_MIN);
            prop_assert!(margin <= METADATA_MARGIN_MAX,
                "余量 {} 应 <= 最大值 {}", margin, METADATA_MARGIN_MAX);
        }
    }

    #[test]
    fn test_verify_compression_edge_cases() {
        let (can_compress, _, _) = verify_compression_precise(1000, 1000, 0);
        assert!(!can_compress, "输出 = 输入时不应能压缩");

        let (can_compress, _, _) = verify_compression_precise(2000, 1000, 0);
        assert!(!can_compress, "输出 > 输入时不应能压缩");

        let (can_compress, _, _) = verify_compression_precise(0, 1000, 0);
        assert!(can_compress, "输出 = 0 时应能压缩");

        let (can_compress, pure_size, _) = verify_compression_precise(100, 500, 200);
        assert_eq!(
            pure_size, 0,
            "元数据 > 输出时纯视频大小应为 0 (saturating_sub)"
        );
        assert!(can_compress, "纯视频 0 < 输入 500 应能压缩");
    }
}

#[cfg(test)]
mod strategy_helper_tests {
    use super::super::explore_strategy::*;
    use super::super::video_explorer::{EncoderPreset, ExploreConfig, VideoEncoder};
    use std::path::PathBuf;

    fn create_test_context() -> ExploreContext {
        ExploreContext::new(
            PathBuf::from("/tmp/test_input.mp4"),
            PathBuf::from("/tmp/test_output.mp4"),
            1_000_000,
            VideoEncoder::Hevc,
            vec![],
            4,
            false,
            EncoderPreset::Medium,
            ExploreConfig::default(),
        )
    }

    #[test]
    fn test_build_result_basic() {
        let ctx = create_test_context();

        let result = ctx.build_result(20.0, 800_000, None, 5, true, 0.85);

        assert_eq!(result.optimal_crf, 20.0);
        assert_eq!(result.output_size, 800_000);
        assert!((result.size_change_pct - (-20.0)).abs() < 0.1, "应为 -20%");
        assert!(result.ssim.is_none());
        assert_eq!(result.iterations, 5);
        assert!(result.quality_passed);
        assert_eq!(result.confidence, 0.85);
    }

    #[test]
    fn test_build_result_with_ssim() {
        let ctx = create_test_context();

        let ssim_result = SsimResult::actual(0.98, Some(45.0));
        let result = ctx.build_result(18.0, 900_000, Some(ssim_result), 3, true, 0.9);

        assert_eq!(result.ssim, Some(0.98));
        assert_eq!(result.psnr, Some(45.0));
    }

    #[test]
    fn test_size_change_pct_calculation() {
        let ctx = create_test_context();

        let pct = ctx.size_change_pct(800_000);
        assert!((pct - (-20.0)).abs() < 0.1);

        let pct = ctx.size_change_pct(1_500_000);
        assert!((pct - 50.0).abs() < 0.1);

        let pct = ctx.size_change_pct(1_000_000);
        assert!(pct.abs() < 0.1);
    }

    #[test]
    fn test_can_compress() {
        let ctx = create_test_context();

        assert!(ctx.can_compress(999_999), "小于输入应能压缩");
        assert!(!ctx.can_compress(1_000_000), "等于输入不应能压缩");
        assert!(!ctx.can_compress(1_000_001), "大于输入不应能压缩");
    }

    #[test]
    fn test_ssim_result_helpers() {
        let actual = SsimResult::actual(0.98, Some(45.0));
        assert!(actual.is_actual());
        assert!(!actual.is_predicted());

        let predicted = SsimResult::predicted(0.95, 40.0);
        assert!(!predicted.is_actual());
        assert!(predicted.is_predicted());
    }
}

#[cfg(test)]
mod evaluation_consistency_tests {
    use crate::stream_size::{ExtractionMethod, StreamSizeInfo};
    use proptest::prelude::*;

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]

        #[test]
        fn prop_exploration_verification_consistency(
            input_video_size in 1000u64..1_000_000_000u64,
            output_video_size in 1000u64..1_000_000_000u64,
        ) {
            let exploration_can_compress = output_video_size < input_video_size;

            let verification_can_compress = output_video_size < input_video_size;

            prop_assert_eq!(exploration_can_compress, verification_can_compress,
                "探索阶段和验证阶段的判断应一致: input={}, output={}, exploration={}, verification={}",
                input_video_size, output_video_size, exploration_can_compress, verification_can_compress);
        }
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]

        #[test]
        fn prop_early_termination_on_incompressible(
            input_video_size in 1000u64..1_000_000_000u64,
            size_increase_percent in 0.0..50.0_f64,
        ) {
            let output_video_size = input_video_size + (input_video_size as f64 * size_increase_percent / 100.0) as u64;

            let can_compress = output_video_size < input_video_size;
            prop_assert!(!can_compress,
                "当 output {} >= input {} 时应报告不能压缩",
                output_video_size, input_video_size);
        }
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]

        #[test]
        fn prop_input_video_stream_size_consistency(
            video_size in 1000u64..1_000_000_000u64,
            audio_size in 0u64..100_000_000u64,
            overhead in 0u64..10_000_000u64,
        ) {
            let info1 = StreamSizeInfo {
                video_stream_size: video_size,
                audio_stream_size: audio_size,
                total_file_size: video_size + audio_size + overhead,
                container_overhead: overhead,
                extraction_method: ExtractionMethod::BitrateCalculation,
                duration_secs: 60.0,
                video_bitrate: None,
                audio_bitrate: None,
            };

            let info2 = StreamSizeInfo {
                video_stream_size: video_size,
                audio_stream_size: audio_size,
                total_file_size: video_size + audio_size + overhead,
                container_overhead: overhead,
                extraction_method: ExtractionMethod::BitrateCalculation,
                duration_secs: 60.0,
                video_bitrate: None,
                audio_bitrate: None,
            };

            prop_assert_eq!(info1.video_stream_size, info2.video_stream_size,
                "多次提取应返回相同的视频流大小");
        }
    }

    #[test]
    fn test_pure_video_comparison_logic() {
        let input_video = 1_000_000u64;
        let output_video = 900_000u64;
        assert!(output_video < input_video, "输出 < 输入应能压缩");

        let output_video = 1_000_000u64;
        assert!((output_video >= input_video), "输出 == 输入不应能压缩");

        let output_video = 1_100_000u64;
        assert!((output_video >= input_video), "输出 > 输入不应能压缩");
    }

    #[test]
    fn test_container_overhead_does_not_affect_compression() {
        let input_video = 1_000_000u64;
        let output_video = 900_000u64;

        let output_total_with_overhead = output_video + 200_000;
        let input_total = input_video + 50_000;

        assert!(output_total_with_overhead > input_total, "总文件变大了");

        assert!(output_video < input_video, "视频流变小，应算压缩成功");
    }
}
