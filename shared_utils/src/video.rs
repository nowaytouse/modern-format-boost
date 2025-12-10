//! Video Processing Utilities
//! 
//! Provides common video processing functionality:
//! - Dimension validation and correction for chroma subsampling
//! - FFmpeg filter generation
//! - Video format detection

/// YUV420 色度子采样要求宽度和高度都是偶数
/// 如果尺寸是奇数，需要裁剪或填充到偶数
/// 
/// 常见错误: "Picture height must be an integer multiple of the specified chroma subsampling"
/// 
/// # Arguments
/// * `width` - 原始宽度
/// * `height` - 原始高度
/// 
/// # Returns
/// * `(corrected_width, corrected_height, needs_correction)` - 修正后的尺寸和是否需要修正
pub fn ensure_even_dimensions(width: u32, height: u32) -> (u32, u32, bool) {
    let corrected_width = if width % 2 != 0 { width - 1 } else { width };
    let corrected_height = if height % 2 != 0 { height - 1 } else { height };
    let needs_correction = corrected_width != width || corrected_height != height;
    
    (corrected_width, corrected_height, needs_correction)
}

/// 生成 FFmpeg 视频滤镜字符串，用于修正奇数尺寸
/// 
/// 使用 crop 滤镜裁剪到偶数尺寸（比 pad 更好，避免添加黑边）
/// 
/// # Arguments
/// * `width` - 原始宽度
/// * `height` - 原始高度
/// 
/// # Returns
/// * `Option<String>` - 如果需要修正，返回滤镜字符串；否则返回 None
pub fn get_dimension_correction_filter(width: u32, height: u32) -> Option<String> {
    let (corrected_width, corrected_height, needs_correction) = ensure_even_dimensions(width, height);
    
    if needs_correction {
        // 使用 crop 滤镜裁剪到偶数尺寸
        // crop=w:h:x:y - 从中心裁剪
        Some(format!("crop={}:{}:0:0", corrected_width, corrected_height))
    } else {
        None
    }
}

/// 生成完整的 FFmpeg 视频滤镜链
/// 
/// 包含:
/// 1. 尺寸修正（如果需要）
/// 2. 像素格式转换（yuv420p）
/// 
/// # Arguments
/// * `width` - 原始宽度
/// * `height` - 原始高度
/// * `has_alpha` - 是否有 alpha 通道（如果有，需要先移除）
/// 
/// # Returns
/// * `String` - 完整的滤镜链
pub fn build_video_filter_chain(width: u32, height: u32, has_alpha: bool) -> String {
    let mut filters = Vec::new();
    
    // 1. 如果有 alpha 通道，先移除（用黑色背景）
    if has_alpha {
        filters.push("format=rgba,colorchannelmixer=aa=1.0,format=rgb24".to_string());
    }
    
    // 2. 尺寸修正
    if let Some(crop_filter) = get_dimension_correction_filter(width, height) {
        filters.push(crop_filter);
    }
    
    // 3. 像素格式转换（确保 yuv420p）
    filters.push("format=yuv420p".to_string());
    
    if filters.is_empty() {
        "format=yuv420p".to_string()
    } else {
        filters.join(",")
    }
}

/// 检查视频尺寸是否兼容 YUV420 色度子采样
pub fn is_yuv420_compatible(width: u32, height: u32) -> bool {
    width % 2 == 0 && height % 2 == 0
}

/// 获取 FFmpeg 尺寸修正参数
/// 
/// 返回用于 FFmpeg 命令的参数列表
/// 
/// # Arguments
/// * `width` - 原始宽度
/// * `height` - 原始高度
/// * `has_alpha` - 是否有 alpha 通道
/// 
/// # Returns
/// * `Vec<String>` - FFmpeg 参数列表（如 ["-vf", "crop=..."]）
pub fn get_ffmpeg_dimension_args(width: u32, height: u32, has_alpha: bool) -> Vec<String> {
    let filter_chain = build_video_filter_chain(width, height, has_alpha);
    vec!["-vf".to_string(), filter_chain]
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_ensure_even_dimensions_already_even() {
        let (w, h, needs) = ensure_even_dimensions(1920, 1080);
        assert_eq!(w, 1920);
        assert_eq!(h, 1080);
        assert!(!needs);
    }
    
    #[test]
    fn test_ensure_even_dimensions_odd_width() {
        let (w, h, needs) = ensure_even_dimensions(1921, 1080);
        assert_eq!(w, 1920);
        assert_eq!(h, 1080);
        assert!(needs);
    }
    
    #[test]
    fn test_ensure_even_dimensions_odd_height() {
        let (w, h, needs) = ensure_even_dimensions(1920, 1081);
        assert_eq!(w, 1920);
        assert_eq!(h, 1080);
        assert!(needs);
    }
    
    #[test]
    fn test_ensure_even_dimensions_both_odd() {
        let (w, h, needs) = ensure_even_dimensions(1921, 1081);
        assert_eq!(w, 1920);
        assert_eq!(h, 1080);
        assert!(needs);
    }
    
    #[test]
    fn test_get_dimension_correction_filter_no_correction() {
        let filter = get_dimension_correction_filter(1920, 1080);
        assert!(filter.is_none());
    }
    
    #[test]
    fn test_get_dimension_correction_filter_needs_correction() {
        let filter = get_dimension_correction_filter(1921, 1081);
        assert_eq!(filter, Some("crop=1920:1080:0:0".to_string()));
    }
    
    #[test]
    fn test_build_video_filter_chain_simple() {
        let chain = build_video_filter_chain(1920, 1080, false);
        assert_eq!(chain, "format=yuv420p");
    }
    
    #[test]
    fn test_build_video_filter_chain_with_correction() {
        let chain = build_video_filter_chain(1921, 1081, false);
        assert_eq!(chain, "crop=1920:1080:0:0,format=yuv420p");
    }
    
    #[test]
    fn test_build_video_filter_chain_with_alpha() {
        let chain = build_video_filter_chain(1920, 1080, true);
        assert_eq!(chain, "format=rgba,colorchannelmixer=aa=1.0,format=rgb24,format=yuv420p");
    }
    
    #[test]
    fn test_build_video_filter_chain_with_alpha_and_correction() {
        let chain = build_video_filter_chain(1921, 1081, true);
        assert_eq!(chain, "format=rgba,colorchannelmixer=aa=1.0,format=rgb24,crop=1920:1080:0:0,format=yuv420p");
    }
    
    #[test]
    fn test_is_yuv420_compatible() {
        assert!(is_yuv420_compatible(1920, 1080));
        assert!(!is_yuv420_compatible(1921, 1080));
        assert!(!is_yuv420_compatible(1920, 1081));
        assert!(!is_yuv420_compatible(1921, 1081));
    }
}
