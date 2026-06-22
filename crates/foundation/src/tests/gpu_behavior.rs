// GPU Acceleration Behavioral & Edge Case Probe
//
// Validates the resilience of smart sampling calculations, quality score bounds,
// dynamic search center estimations, and backoff heuristics under extreme conditions.

use crate::gpu_accel::{
    GpuType, SearchPhase, calculate_quality_score, calculate_smart_sample,
    estimate_cpu_search_center_dynamic, is_quality_better,
};
use std::path::Path;

#[test]
fn test_gpu_smart_sample_calculation_resilience() {
    if !crate::common_utils::is_command_available("ffmpeg") {
        eprintln!("skipping test_gpu_smart_sample_calculation_resilience: ffmpeg unavailable");
        return;
    }

    // Test with extreme duration (e.g. 10 hours)
    // Missing input falls back to uniform sampling once ffmpeg is available.
    let res = calculate_smart_sample(Path::new("dummy.mp4"), 36000.0, 10.0)
        .unwrap_or_else(|e| panic!("calculate_smart_sample failed with ffmpeg present: {e}"));
    assert!(res.actual_duration > 0.0);

    // In CI/Dummy environment, we expect either smart or uniform fallback
    assert!(
        res.strategy.contains("Smart sampling") || res.strategy.contains("Uniform sampling"),
        "Strategy should be either Smart or Uniform sampling, got: {}",
        res.strategy
    );

    if res.strategy.contains("Smart sampling") {
        assert!(res.sample_filter.contains("select="));
    } else {
        assert!(res.sample_filter.is_empty());
    }

    // Test with short duration (e.g. 5 seconds) - should return "Full video" and empty filter
    let res_short = calculate_smart_sample(Path::new("dummy.mp4"), 5.0, 10.0).unwrap();
    assert!(res_short.actual_duration > 0.0);
    assert!(res_short.sample_filter.is_empty());
    assert!(res_short.strategy.contains("Full video"));
}

#[test]
fn test_gpu_quality_score_edge_cases() {
    // Test perfect SSIM and high compression
    let score_perf = calculate_quality_score(1.0, 5000, 10000, SearchPhase::Gpu);
    assert!(score_perf.combined_score > 0.0);
    assert!(score_perf.ssim_meets(0.95));

    // Test terrible SSIM and negative compression (bloat)
    let score_bad = calculate_quality_score(0.5, 20000, 10000, SearchPhase::Cpu);
    assert!(score_bad.combined_score < score_perf.combined_score);
    assert!(!score_bad.ssim_meets(0.95));

    // Test quality comparison logic
    assert!(is_quality_better(&score_perf, &score_bad, 0.90));
}

#[test]
fn test_gpu_dynamic_search_center_estimation_bounds() {
    // Test estimation with various coarse results
    let base_crf = estimate_cpu_search_center_dynamic(26.0, GpuType::Nvidia, "h264", Some(0.5));
    assert!(base_crf > 26.0); // Should add offset

    let fallback_crf = estimate_cpu_search_center_dynamic(26.0, GpuType::Apple, "hevc", Some(0.1));
    assert!(fallback_crf > 26.0); // Should adjust based on potential

    let fallback_standard = estimate_cpu_search_center_dynamic(26.0, GpuType::None, "av1", None);
    assert!((fallback_standard - 26.0).abs() < f32::EPSILON); // No GPU offset
}
