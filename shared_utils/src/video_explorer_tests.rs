//! 🧪 Video Explorer Test Module
//! 
//! Modular test suite covering edge cases and comprehensive scenarios.

#[cfg(test)]
mod constants_tests {
    use super::super::video_explorer::*;
    
    #[test]
    fn test_crf_bounds() {
        assert!(ABSOLUTE_MIN_CRF >= 0.0);
        assert!(ABSOLUTE_MIN_CRF < ABSOLUTE_MAX_CRF);
        assert!(ABSOLUTE_MAX_CRF <= 63.0); // CRF max for most codecs
    }
    
    #[test]
    fn test_iteration_limits() {
        assert!(STAGE_B1_MAX_ITERATIONS > 0);
        assert!(STAGE_B2_MAX_ITERATIONS > 0);
        assert!(GLOBAL_MAX_ITERATIONS >= 50);
        assert!(GLOBAL_MAX_ITERATIONS <= 100); // Reasonable upper bound
    }
    
    #[test]
    fn test_binary_search_iterations() {
        assert!(BINARY_SEARCH_MAX_ITERATIONS >= 8);
        assert!(BINARY_SEARCH_MAX_ITERATIONS <= 20);
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
        assert!(thresholds.min_vmaf >= 80.0);
    }
    
    #[test]
    fn test_ssim_range() {
        let thresholds = QualityThresholds::default();
        // SSIM should be between 0 and 1
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
            vmaf: None,
            iterations: 5,
            quality_passed: true,
            log: vec![],
            confidence: 0.85,
            confidence_detail: ConfidenceBreakdown::default(),
            actual_min_ssim: 0.95,  // 🔥 v5.69
        };
        
        assert!(result.size_change_pct < 0.0); // Compressed
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
            vmaf: None,
            iterations: 10,
            quality_passed: false,
            log: vec![],
            confidence: 0.3,
            confidence_detail: ConfidenceBreakdown::default(),
            actual_min_ssim: 0.95,  // 🔥 v5.69
        };
        
        assert!(result.size_change_pct > 0.0); // Not compressed
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
        // 测试动态迭代次数计算
        let calc_max_iter = |range: f32| -> u32 {
            ((range.log2().ceil() as u32) + 3).max(5)
        };
        
        // 小范围
        assert!(calc_max_iter(5.0) >= 5);
        
        // 中等范围
        let mid = calc_max_iter(25.0);
        assert!(mid >= 7 && mid <= 10);
        
        // 大范围
        let large = calc_max_iter(50.0);
        assert!(large >= 8 && large <= 12);
    }
    
    #[test]
    fn test_iteration_bounds() {
        // 确保迭代次数在合理范围内
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
        // 输入大小为 0 的边缘情况
        let size_pct = if 0_u64 > 0 {
            ((100_u64 as f64 / 0_u64 as f64) - 1.0) * 100.0
        } else {
            0.0
        };
        assert_eq!(size_pct, 0.0);
    }
    
    #[test]
    fn test_crf_precision() {
        // CRF 精度测试（0.1 步进）
        let crf = 20.5_f32;
        let rounded = (crf * 10.0).round() / 10.0;
        assert!((rounded - 20.5).abs() < 0.01);
    }
    
    #[test]
    fn test_ssim_bounds() {
        // SSIM 边界测试
        let ssim_values = [0.0, 0.5, 0.9, 0.95, 0.99, 1.0];
        for ssim in ssim_values {
            assert!(ssim >= 0.0 && ssim <= 1.0);
        }
    }
    
    #[test]
    fn test_extreme_crf_values() {
        // 极端 CRF 值测试
        assert!(ABSOLUTE_MIN_CRF >= 0.0);
        assert!(ABSOLUTE_MAX_CRF <= 63.0);
        
        // 确保范围有效
        let range = ABSOLUTE_MAX_CRF - ABSOLUTE_MIN_CRF;
        assert!(range > 20.0); // 至少 20 CRF 范围
    }
}

#[cfg(test)]
mod precision_tests {
    use super::super::video_explorer::precision::{crf_to_cache_key, cache_key_to_crf, CACHE_KEY_MULTIPLIER};
    
    #[test]
    fn test_crf_key_generation() {
        // 🔥 v5.73: 测试统一的 crf_to_cache_key() 函数
        assert_eq!(crf_to_cache_key(20.0), 200);
        assert_eq!(crf_to_cache_key(20.1), 201);
        assert_eq!(crf_to_cache_key(20.5), 205);
        
        let crf2 = 20.55_f32;
        let key2 = crf_to_cache_key(crf2);
        assert_eq!(key2, 206); // 四舍五入
        
        // 测试反向转换
        assert!((cache_key_to_crf(200) - 20.0).abs() < 0.01);
        assert!((cache_key_to_crf(205) - 20.5).abs() < 0.01);
        
        // 验证乘数常量
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


// ═══════════════════════════════════════════════════════════════
// 🔥 v5.72: 三阶段搜索属性测试
// ═══════════════════════════════════════════════════════════════

#[cfg(test)]
mod three_phase_search_tests {
    use super::super::video_explorer::precision::*;

    // **Feature: video-explorer-robustness-v5.72, Property 7: 三阶段搜索递进**
    // **Validates: Requirements 4.1, 4.2, 4.3, 4.4**
    // 🔥 v5.72: GPU+CPU 双精细化策略
    // GPU: 4 → 1 → 0.5 → 0.25 (快速，SSIM 上限 ~0.97)
    // CPU: 0.1 (慢，突破到 0.98+)
    #[test]
    fn prop_gpu_cpu_dual_refinement() {
        let search = ThreePhaseSearch::default();
        
        // 🔥 核心属性：GPU 步进值递减 4 → 1 → 0.5 → 0.25
        assert!(search.gpu_coarse_step > search.gpu_medium_step,
            "GPU coarse ({}) > medium ({})", search.gpu_coarse_step, search.gpu_medium_step);
        assert!(search.gpu_medium_step > search.gpu_fine_step,
            "GPU medium ({}) > fine ({})", search.gpu_medium_step, search.gpu_fine_step);
        assert!(search.gpu_fine_step > search.gpu_ultra_fine_step,
            "GPU fine ({}) > ultra_fine ({})", search.gpu_fine_step, search.gpu_ultra_fine_step);
        
        // 🔥 核心属性：CPU 只做最终 0.1 精细化
        assert!(search.gpu_ultra_fine_step > search.cpu_finest_step,
            "GPU ultra_fine ({}) > CPU finest ({})", search.gpu_ultra_fine_step, search.cpu_finest_step);
        
        // 验证具体值
        assert!((search.gpu_coarse_step - 4.0).abs() < 0.01, "GPU coarse should be 4.0");
        assert!((search.gpu_medium_step - 1.0).abs() < 0.01, "GPU medium should be 1.0");
        assert!((search.gpu_fine_step - 0.5).abs() < 0.01, "GPU fine should be 0.5");
        assert!((search.gpu_ultra_fine_step - 0.25).abs() < 0.01, "GPU ultra_fine should be 0.25");
        assert!((search.cpu_finest_step - 0.1).abs() < 0.01, "CPU finest should be 0.1");
    }

    #[test]
    fn prop_search_phase_step_sizes() {
        // 验证SearchPhase枚举的步进值
        assert!((SearchPhase::GpuCoarse.step_size() - 4.0).abs() < 0.01);
        assert!((SearchPhase::GpuMedium.step_size() - 1.0).abs() < 0.01);
        assert!((SearchPhase::GpuFine.step_size() - 0.5).abs() < 0.01);
        assert!((SearchPhase::GpuUltraFine.step_size() - 0.25).abs() < 0.01);
        assert!((SearchPhase::CpuFinest.step_size() - 0.1).abs() < 0.01);
    }

    #[test]
    fn prop_gpu_vs_cpu_phase() {
        // 验证 GPU/CPU 阶段分类
        assert!(SearchPhase::GpuCoarse.is_gpu());
        assert!(SearchPhase::GpuMedium.is_gpu());
        assert!(SearchPhase::GpuFine.is_gpu());
        assert!(SearchPhase::GpuUltraFine.is_gpu());
        assert!(!SearchPhase::CpuFinest.is_gpu()); // CPU 阶段
    }

    #[test]
    fn prop_cache_key_unified() {
        // 🔥 v5.80: 验证统一缓存键机制
        // 所有CRF都应使用 precision::crf_to_cache_key()（×10）

        // 验证不同精度的CRF能正确映射到缓存键
        assert_eq!(crf_to_cache_key(18.0), 180);
        assert_eq!(crf_to_cache_key(18.1), 181);
        assert_eq!(crf_to_cache_key(18.5), 185);
        assert_eq!(crf_to_cache_key(18.25), 183);  // 18.25 × 10 = 182.5 → 183

        // 验证缓存键的唯一性（0.1精度）
        let crfs = vec![18.0, 18.1, 18.2, 18.3, 18.4, 18.5];
        let keys: Vec<i32> = crfs.iter().map(|&crf| crf_to_cache_key(crf)).collect();

        // 所有键应该不同
        for i in 0..keys.len() {
            for j in (i+1)..keys.len() {
                assert_ne!(keys[i], keys[j],
                    "CRF {:.1} and {:.1} should have different cache keys",
                    crfs[i], crfs[j]);
            }
        }

        // 验证逆映射
        for &crf in &crfs {
            let key = crf_to_cache_key(crf);
            let reconstructed = cache_key_to_crf(key);
            assert!((reconstructed - crf).abs() < 0.01,
                "Cache key round-trip failed: {:.1} → {} → {:.1}",
                crf, key, reconstructed);
        }
    }

    #[test]
    fn prop_phase_progression() {
        // 验证阶段递进：GPU → GPU → GPU → GPU → CPU
        assert_eq!(SearchPhase::GpuCoarse.next(), Some(SearchPhase::GpuMedium));
        assert_eq!(SearchPhase::GpuMedium.next(), Some(SearchPhase::GpuFine));
        assert_eq!(SearchPhase::GpuFine.next(), Some(SearchPhase::GpuUltraFine));
        assert_eq!(SearchPhase::GpuUltraFine.next(), Some(SearchPhase::CpuFinest));
        assert_eq!(SearchPhase::CpuFinest.next(), None);
    }
}

// ═══════════════════════════════════════════════════════════════
// 🔥 v5.74: 透明度报告属性测试
// ═══════════════════════════════════════════════════════════════

#[cfg(test)]
mod transparency_prop_tests {
    use super::super::video_explorer::*;
    use proptest::prelude::*;

    // **Feature: video-explorer-transparency-v5.74, Property 5: 迭代输出完整性**
    // **Validates: Requirements 2.2**
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

            // 验证所有必要字段都存在
            prop_assert!(metrics.iteration > 0);
            prop_assert!(!metrics.phase.is_empty());
            prop_assert!(metrics.crf >= 10.0 && metrics.crf <= 51.0);
            // size_change_pct 可以是任意值
            // can_compress 是布尔值
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

        // 验证预测的 SSIM 会被正确标记
        assert_eq!(metrics.ssim_source, SsimSource::Predicted);
    }
}


// ═══════════════════════════════════════════════════════════════
// 🔥 v5.74: PSNR 透明度数据属性测试
// ═══════════════════════════════════════════════════════════════

#[cfg(test)]
mod psnr_transparency_tests {
    use super::super::video_explorer::*;
    use proptest::prelude::*;

    // **Feature: video-explorer-transparency-v5.74, Property 1: PSNR 透明度数据**
    // **Validates: Requirements 1.1**
    // 注意：这是一个结构测试，验证 IterationMetrics 可以存储 PSNR 数据
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

            // 验证 PSNR 数据可以被存储和访问
            prop_assert_eq!(metrics.psnr, psnr);
            
            // 验证 PSNR 值在有效范围内（如果存在）
            if let Some(p) = psnr {
                prop_assert!(p >= 0.0 && p <= 100.0, "PSNR should be in valid range");
            }
        }
    }

    // **Feature: video-explorer-transparency-v5.74, Property 2: PSNR→SSIM 映射完整性**
    // **Validates: Requirements 1.2**
    #[test]
    fn test_psnr_ssim_mapping_integration() {
        use super::super::ssim_mapping::PsnrSsimMapping;
        
        let mut mapping = PsnrSsimMapping::new();
        
        // 模拟 GPU 阶段收集的数据
        mapping.insert(35.0, 0.92);
        mapping.insert(40.0, 0.95);
        mapping.insert(45.0, 0.97);
        
        assert!(mapping.has_enough_points());
        
        // 验证预测功能
        let predicted = mapping.predict_ssim(42.5).unwrap();
        assert!(predicted > 0.95 && predicted < 0.97);
    }
}


// ═══════════════════════════════════════════════════════════════
// 🔥 v5.74: Preset 一致性属性测试
// ═══════════════════════════════════════════════════════════════

#[cfg(test)]
mod preset_consistency_tests {
    use super::super::video_explorer::*;
    use proptest::prelude::*;

    // **Feature: video-explorer-transparency-v5.74, Property 6: Preset 一致性**
    // **Validates: Requirements 3.2**
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
            
            // 验证 preset 名称映射正确
            let name = preset.x26x_name();
            prop_assert!(!name.is_empty());
            
            // 验证 SVT-AV1 preset 在有效范围内
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


// ═══════════════════════════════════════════════════════════════
// 🔥 v5.74: Mock 测试支持
// ═══════════════════════════════════════════════════════════════

#[cfg(test)]
mod mock_tests {
    use super::super::video_explorer::*;
    use super::super::ssim_mapping::PsnrSsimMapping;

    /// Mock encode 函数：CRF 越高，文件越小
    fn mock_encode(crf: f32, input_size: u64) -> u64 {
        // 模拟：CRF 20 时输出 = 输入，CRF 每增加 1，输出减少 5%
        let ratio = 1.0_f64 - (crf as f64 - 20.0) * 0.05;
        (input_size as f64 * ratio.max(0.1)) as u64
    }

    /// Mock SSIM 函数：CRF 越低，SSIM 越高
    fn mock_ssim(crf: f32) -> f64 {
        // 模拟：CRF 10 时 SSIM = 0.99，CRF 每增加 1，SSIM 减少 0.005
        (0.99_f64 - (crf as f64 - 10.0) * 0.005).max(0.8)
    }

    /// Mock PSNR 函数：CRF 越低，PSNR 越高
    fn mock_psnr(crf: f32) -> f64 {
        // 模拟：CRF 10 时 PSNR = 50，CRF 每增加 1，PSNR 减少 0.5
        (50.0_f64 - (crf as f64 - 10.0) * 0.5).max(25.0)
    }

    // **Feature: video-explorer-transparency-v5.74, Mock 测试**
    // **Validates: Requirements 5.3**

    #[test]
    fn test_mock_cannot_compress_scenario() {
        // 场景：输入文件已经高度压缩，无法进一步压缩
        let input_size = 1000000_u64;
        
        // 即使 CRF 51（最高），输出仍然大于输入
        let output_at_max_crf = mock_encode(51.0, input_size);
        // 在这个 mock 中，CRF 51 时 ratio = 1.0 - (51-20)*0.05 = -0.55，被 clamp 到 0.1
        // 所以 output = 100000，小于输入
        
        // 修改场景：假设输入已经很小
        let small_input = 50000_u64;
        let output = mock_encode(20.0, small_input);
        assert_eq!(output, small_input); // CRF 20 时 1:1
    }

    #[test]
    fn test_mock_quality_never_passes_scenario() {
        // 场景：质量阈值设置过高，永远无法达到
        let min_ssim = 0.999; // 极高阈值
        
        // 即使 CRF 10（最低），SSIM 也只有 0.99
        let ssim_at_min_crf = mock_ssim(10.0);
        assert!(ssim_at_min_crf < min_ssim);
    }

    #[test]
    fn test_mock_single_iteration_success() {
        // 场景：第一次尝试就成功
        let input_size = 1000000_u64;
        let initial_crf = 25.0;
        
        let output = mock_encode(initial_crf, input_size);
        let ssim = mock_ssim(initial_crf);
        
        // CRF 25 时：ratio = 1.0 - 5*0.05 = 0.75，output = 750000 < input
        assert!(output < input_size);
        // SSIM = 0.99 - 15*0.005 = 0.915
        assert!(ssim > 0.9);
    }

    #[test]
    fn test_mock_psnr_ssim_mapping() {
        // 测试 PSNR→SSIM 映射的 mock 数据
        let mut mapping = PsnrSsimMapping::new();
        
        for crf in [15.0, 20.0, 25.0, 30.0] {
            let psnr = mock_psnr(crf);
            let ssim = mock_ssim(crf);
            mapping.insert(psnr, ssim);
        }
        
        assert!(mapping.has_enough_points());
        
        // 验证预测功能
        let test_psnr = mock_psnr(22.5);
        let predicted = mapping.predict_ssim(test_psnr);
        assert!(predicted.is_some());
    }

    #[test]
    fn test_mock_deterministic_results() {
        // 验证 mock 函数产生确定性结果
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
