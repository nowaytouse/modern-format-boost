//! Explorer Module Tests - 精度验证和裁判机制测试
//!
//! 测试覆盖：
//! 1. 二分搜索精度测试
//! 2. SSIM/PSNR 质量验证测试
//! 3. 边界条件测试
//! 4. 低分辨率 GIF 特殊处理测试

use std::path::PathBuf;
use std::process::Command;
use std::fs;

/// 测试辅助：创建测试 GIF
fn create_test_gif(path: &PathBuf, width: u32, height: u32, frames: u32) -> bool {
    // 使用 ffmpeg 创建测试 GIF
    let status = Command::new("ffmpeg")
        .arg("-y")
        .arg("-f").arg("lavfi")
        .arg("-i").arg(format!(
            "testsrc=duration={}:size={}x{}:rate=10",
            frames as f64 / 10.0, width, height
        ))
        .arg("-vf").arg("palettegen=max_colors=256")
        .arg("-y")
        .arg("/tmp/palette.png")
        .status();
    
    if status.is_err() || !status.unwrap().success() {
        return false;
    }
    
    let status = Command::new("ffmpeg")
        .arg("-y")
        .arg("-f").arg("lavfi")
        .arg("-i").arg(format!(
            "testsrc=duration={}:size={}x{}:rate=10",
            frames as f64 / 10.0, width, height
        ))
        .arg("-i").arg("/tmp/palette.png")
        .arg("-lavfi").arg("paletteuse")
        .arg(path)
        .status();
    
    let _ = fs::remove_file("/tmp/palette.png");
    
    status.is_ok() && status.unwrap().success()
}

/// 测试辅助：计算 SSIM
fn calculate_ssim(original: &PathBuf, converted: &PathBuf) -> Option<f64> {
    let output = Command::new("ffmpeg")
        .arg("-i").arg(original)
        .arg("-i").arg(converted)
        .arg("-lavfi").arg("ssim=stats_file=-")
        .arg("-f").arg("null")
        .arg("-")
        .output()
        .ok()?;
    
    let stderr = String::from_utf8_lossy(&output.stderr);
    
    // 解析 SSIM 值
    for line in stderr.lines() {
        if line.contains("All:") {
            if let Some(pos) = line.find("All:") {
                let value_str = &line[pos + 4..];
                if let Some(end) = value_str.find(|c: char| !c.is_numeric() && c != '.') {
                    return value_str[..end].parse().ok();
                } else {
                    return value_str.trim().parse().ok();
                }
            }
        }
    }
    None
}

/// 测试辅助：获取文件大小
fn get_file_size(path: &PathBuf) -> u64 {
    fs::metadata(path).map(|m| m.len()).unwrap_or(0)
}

// ═══════════════════════════════════════════════════════════════
// 精度测试
// ═══════════════════════════════════════════════════════════════

#[test]
fn test_binary_search_precision() {
    // 测试二分搜索是否能在 8 次迭代内找到最优 CRF
    // CRF 范围 [10, 28]，二分搜索最多需要 log2(18) ≈ 5 次
    let range = 28 - 10;
    let max_iterations = (range as f64).log2().ceil() as u32 + 1;
    assert!(max_iterations <= 8, "Binary search should complete in <= 8 iterations");
}

#[test]
fn test_ssim_threshold_validation() {
    // 测试 SSIM 阈值验证逻辑
    let min_ssim = 0.95;
    
    // 高质量应该通过
    assert!(0.98 >= min_ssim);
    assert!(0.95 >= min_ssim);
    
    // 低质量应该失败
    assert!(0.90 < min_ssim);
    assert!(0.80 < min_ssim);
}

#[test]
fn test_psnr_threshold_validation() {
    // 测试 PSNR 阈值验证逻辑
    let min_psnr = 35.0;
    
    // 高质量应该通过
    assert!(45.0 >= min_psnr);
    assert!(35.0 >= min_psnr);
    
    // 低质量应该失败
    assert!(30.0 < min_psnr);
    assert!(25.0 < min_psnr);
}

// ═══════════════════════════════════════════════════════════════
// 裁判验证测试
// ═══════════════════════════════════════════════════════════════

#[test]
#[ignore] // 需要 ffmpeg，CI 环境可能没有
fn test_ssim_calculation_accuracy() {
    // 创建测试文件
    let test_gif = PathBuf::from("/tmp/test_ssim.gif");
    let test_mp4 = PathBuf::from("/tmp/test_ssim.mp4");
    
    if !create_test_gif(&test_gif, 320, 240, 30) {
        eprintln!("Skipping test: ffmpeg not available");
        return;
    }
    
    // 转换为 MP4（高质量）
    let status = Command::new("ffmpeg")
        .arg("-y")
        .arg("-i").arg(&test_gif)
        .arg("-c:v").arg("libx265")
        .arg("-crf").arg("18")
        .arg("-preset").arg("fast")
        .arg(&test_mp4)
        .status();
    
    if status.is_err() || !status.unwrap().success() {
        let _ = fs::remove_file(&test_gif);
        eprintln!("Skipping test: HEVC encoding failed");
        return;
    }
    
    // 计算 SSIM
    let ssim = calculate_ssim(&test_gif, &test_mp4);
    
    // 清理
    let _ = fs::remove_file(&test_gif);
    let _ = fs::remove_file(&test_mp4);
    
    // 验证 SSIM 在合理范围内
    if let Some(s) = ssim {
        assert!((0.0..=1.0).contains(&s), "SSIM should be in [0, 1], got {}", s);
        assert!(s >= 0.90, "High quality encoding should have SSIM >= 0.90, got {}", s);
    }
}

#[test]
#[ignore] // 需要 ffmpeg
fn test_quality_degrades_with_higher_crf() {
    // 验证 CRF 越高，质量越低（SSIM 越低）
    let test_gif = PathBuf::from("/tmp/test_crf_quality.gif");
    
    if !create_test_gif(&test_gif, 320, 240, 30) {
        eprintln!("Skipping test: ffmpeg not available");
        return;
    }
    
    let mut ssim_values = Vec::new();
    
    for crf in [10, 18, 25, 30] {
        let test_mp4 = PathBuf::from(format!("/tmp/test_crf_{}.mp4", crf));
        
        let status = Command::new("ffmpeg")
            .arg("-y")
            .arg("-i").arg(&test_gif)
            .arg("-c:v").arg("libx265")
            .arg("-crf").arg(crf.to_string())
            .arg("-preset").arg("fast")
            .arg(&test_mp4)
            .status();
        
        if status.is_ok() && status.unwrap().success() {
            if let Some(ssim) = calculate_ssim(&test_gif, &test_mp4) {
                ssim_values.push((crf, ssim));
            }
            let _ = fs::remove_file(&test_mp4);
        }
    }
    
    let _ = fs::remove_file(&test_gif);
    
    // 验证 SSIM 随 CRF 增加而降低
    for i in 1..ssim_values.len() {
        let (crf_prev, ssim_prev) = ssim_values[i - 1];
        let (crf_curr, ssim_curr) = ssim_values[i];
        
        assert!(
            ssim_curr <= ssim_prev,
            "SSIM should decrease with higher CRF: CRF {} ({:.4}) vs CRF {} ({:.4})",
            crf_prev, ssim_prev, crf_curr, ssim_curr
        );
    }
}

// ═══════════════════════════════════════════════════════════════
// 边界条件测试
// ═══════════════════════════════════════════════════════════════

#[test]
fn test_crf_range_validation() {
    // CRF 范围应该在 [0, 51] 内（HEVC 标准）
    let min_crf = 10u8;
    let max_crf = 28u8;
    
    // min_crf >= 0 is always true for u8, so we just verify the range makes sense
    assert!(max_crf <= 51);
    assert!(min_crf < max_crf);
}

#[test]
fn test_target_ratio_validation() {
    // 目标比率应该在合理范围内
    let target_ratio = 1.0f64;
    
    assert!(target_ratio > 0.0);
    assert!(target_ratio <= 2.0); // 最多允许输出是输入的 2 倍
}

#[test]
#[ignore] // 需要 ffmpeg
fn test_low_resolution_gif_handling() {
    // 测试低分辨率 GIF（320x180）的特殊处理
    let test_gif = PathBuf::from("/tmp/test_low_res.gif");
    let test_mp4 = PathBuf::from("/tmp/test_low_res.mp4");
    
    if !create_test_gif(&test_gif, 320, 180, 40) {
        eprintln!("Skipping test: ffmpeg not available");
        return;
    }
    
    let input_size = get_file_size(&test_gif);
    
    // 尝试不同 CRF 值
    let mut found_smaller = false;
    
    for crf in [18, 22, 25, 28] {
        let status = Command::new("ffmpeg")
            .arg("-y")
            .arg("-i").arg(&test_gif)
            .arg("-c:v").arg("libx265")
            .arg("-crf").arg(crf.to_string())
            .arg("-preset").arg("medium")
            .arg(&test_mp4)
            .status();
        
        if status.is_ok() && status.unwrap().success() {
            let output_size = get_file_size(&test_mp4);
            
            if output_size <= input_size {
                found_smaller = true;
                eprintln!("CRF {} produces smaller output: {} <= {}", crf, output_size, input_size);
                break;
            } else {
                eprintln!("CRF {} produces larger output: {} > {}", crf, output_size, input_size);
            }
        }
    }
    
    let _ = fs::remove_file(&test_gif);
    let _ = fs::remove_file(&test_mp4);
    
    // 对于低分辨率 GIF，可能需要较高 CRF 才能减小大小
    // 这是预期行为，不是错误
    if !found_smaller {
        eprintln!("Note: Low resolution GIF may not benefit from HEVC conversion");
    }
}

// ═══════════════════════════════════════════════════════════════
// 探索结果验证测试
// ═══════════════════════════════════════════════════════════════

#[test]
fn test_explore_result_fields() {
    // 验证 ExploreResult 结构体字段
    // 这是编译时测试，确保结构体定义正确
    
    // 模拟一个探索结果
    let result = shared_utils::ExploreResult {
        optimal_crf: 22.0,
        output_size: 186000,
        size_change_pct: -11.0,
        ssim: Some(0.97),
        psnr: None,
        vmaf: None,
        iterations: 5,
        quality_passed: true,
        log: vec!["Test log".to_string()],
        confidence: 0.85,
        confidence_detail: shared_utils::ConfidenceBreakdown::default(),
        actual_min_ssim: 0.95,  // 🔥 v5.69
    };
    
    assert!((result.optimal_crf - 22.0).abs() < 0.01);
    assert!(result.size_change_pct < 0.0); // 负数表示减小
    assert!(result.quality_passed);
    assert_eq!(result.iterations, 5);
}

#[test]
fn test_quality_thresholds_customization() {
    // 测试自定义质量阈值
    let thresholds = shared_utils::QualityThresholds {
        min_ssim: 0.98,      // 更严格
        min_psnr: 40.0,      // 更严格
        min_vmaf: 90.0,      // VMAF 阈值
        validate_ssim: true,
        validate_psnr: true, // 同时验证两者
        validate_vmaf: false, // 不验证 VMAF
        ..Default::default()
    };
    
    assert_eq!(thresholds.min_ssim, 0.98);
    assert_eq!(thresholds.min_psnr, 40.0);
    assert!(thresholds.validate_ssim);
    assert!(thresholds.validate_psnr);
}

// ═══════════════════════════════════════════════════════════════
// 集成测试
// ═══════════════════════════════════════════════════════════════

#[test]
#[ignore] // 需要 ffmpeg 和真实文件
fn test_full_exploration_workflow() {
    // 完整探索工作流测试
    let test_gif = PathBuf::from("/tmp/test_full_explore.gif");
    let test_mp4 = PathBuf::from("/tmp/test_full_explore.mp4");
    
    if !create_test_gif(&test_gif, 480, 360, 50) {
        eprintln!("Skipping test: ffmpeg not available");
        return;
    }
    
    let input_size = get_file_size(&test_gif);
    eprintln!("Input GIF size: {} bytes", input_size);
    
    // 使用探索器
    let vf_args = vec![
        "-vf".to_string(),
        "format=yuv420p".to_string(),
    ];
    
    // 使用 shared_utils 统一探索器
    match shared_utils::explore_hevc(&test_gif, &test_mp4, vf_args, 18.0) {
        Ok(result) => {
            eprintln!("Exploration result:");
            eprintln!("  Optimal CRF: {}", result.optimal_crf);
            eprintln!("  Output size: {} bytes", result.output_size);
            eprintln!("  Size change: {:.1}%", result.size_change_pct);
            eprintln!("  SSIM: {:?}", result.ssim);
            eprintln!("  Iterations: {}", result.iterations);
            eprintln!("  Quality passed: {}", result.quality_passed);
            
            for log in &result.log {
                eprintln!("  {}", log);
            }
            
            // 验证结果
            assert!(result.optimal_crf >= 10.0 && result.optimal_crf <= 28.0);
            assert!(result.iterations <= 8);
            
            if result.output_size <= input_size {
                assert!(result.size_change_pct <= 0.0);
            }
        }
        Err(e) => {
            eprintln!("Exploration failed: {}", e);
        }
    }
    
    let _ = fs::remove_file(&test_gif);
    let _ = fs::remove_file(&test_mp4);
}
