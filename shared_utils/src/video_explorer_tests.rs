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
    fn prop_cache_multiplier_consistency() {
        // 验证缓存键乘数与步进值的一致性
        let search = ThreePhaseSearch::default();
        
        for phase in [SearchPhase::GpuFine, SearchPhase::GpuUltraFine, SearchPhase::CpuFinest] {
            let step = phase.step_size();
            let multiplier = phase.cache_multiplier();
            
            // 验证：step * multiplier 应该产生整数键
            let test_crf = 18.5_f32;
            let key = search.cache_key(test_crf, phase);
            let reconstructed = key as f32 / multiplier;
            
            // 重建的CRF应该是step的整数倍
            let diff = (reconstructed - test_crf).abs();
            assert!(diff <= step / 2.0,
                "Phase {:?}: Cache key reconstruction error {} > step/2 ({})",
                phase, diff, step / 2.0);
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
