// Regression test: animated sources must never be scheduled for static JXL conversion.

#[test]
fn animated_source_never_suggests_jxl() {
    use img::{
        CompressionType, DetectedFormat, DetectionResult, ImageType, TargetFormat,
        determine_strategy,
    };

    let det = DetectionResult {
        file_path: "animated.gif".to_string(),
        format: DetectedFormat::GIF,
        image_type: ImageType::Animated,
        compression: CompressionType::Lossless,
        width: 640,
        height: 480,
        bit_depth: Some(8),
        has_alpha: false,
        file_size: 1024,
        frame_count: Some(10),
        fps: Some(10.0),
        duration: Some(1.0),
        estimated_quality: None,
        entropy: None,
        precision: foundation::image_detection::PrecisionMetadata::default(),
    };

    let strat = determine_strategy(&det).expect("determine_strategy failed");
    assert_eq!(
        strat.target,
        TargetFormat::NoConversion,
        "Animated sources must not suggest JXL"
    );
}
