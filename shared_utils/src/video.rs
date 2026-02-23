//! Video Processing Utilities
//!
//! Provides common video processing functionality:
//! - Dimension validation and correction for chroma subsampling
//! - FFmpeg filter generation
//! - Video format detection

pub fn ensure_even_dimensions(width: u32, height: u32) -> (u32, u32, bool) {
    let corrected_width = if !width.is_multiple_of(2) {
        width - 1
    } else {
        width
    };
    let corrected_height = if !height.is_multiple_of(2) {
        height - 1
    } else {
        height
    };
    let needs_correction = corrected_width != width || corrected_height != height;

    (corrected_width, corrected_height, needs_correction)
}

pub fn get_dimension_correction_filter(width: u32, height: u32) -> Option<String> {
    let (corrected_width, corrected_height, needs_correction) =
        ensure_even_dimensions(width, height);

    if needs_correction {
        Some(format!("crop={}:{}:0:0", corrected_width, corrected_height))
    } else {
        None
    }
}

pub fn build_video_filter_chain(width: u32, height: u32, has_alpha: bool) -> String {
    let mut filters = Vec::new();

    if has_alpha {
        // Composite on black background: premultiply multiplies RGB by alpha (R*A/255),
        // which is equivalent to compositing on black since black contributes 0.
        // This avoids exposing garbage RGB data in transparent pixels (common in PNG/WebP).
        filters.push("format=rgba,premultiply=inplace=1,format=rgb24".to_string());
    }

    if let Some(crop_filter) = get_dimension_correction_filter(width, height) {
        filters.push(crop_filter);
    }

    filters.push("format=yuv420p".to_string());

    if filters.is_empty() {
        "format=yuv420p".to_string()
    } else {
        filters.join(",")
    }
}

pub fn is_yuv420_compatible(width: u32, height: u32) -> bool {
    width.is_multiple_of(2) && height.is_multiple_of(2)
}

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
        assert_eq!(
            chain,
            "format=rgba,premultiply=inplace=1,format=rgb24,format=yuv420p"
        );
    }

    #[test]
    fn test_build_video_filter_chain_with_alpha_and_correction() {
        let chain = build_video_filter_chain(1921, 1081, true);
        assert_eq!(
            chain,
            "format=rgba,premultiply=inplace=1,format=rgb24,crop=1920:1080:0:0,format=yuv420p"
        );
    }

    #[test]
    fn test_is_yuv420_compatible() {
        assert!(is_yuv420_compatible(1920, 1080));
        assert!(!is_yuv420_compatible(1921, 1080));
        assert!(!is_yuv420_compatible(1920, 1081));
        assert!(!is_yuv420_compatible(1921, 1081));
    }
}
