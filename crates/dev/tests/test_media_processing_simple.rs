#[test]
fn basic_format_detection_suite() {
    test_basic_format_detection();
}

fn test_basic_format_detection() {
    // Test basic format detection with minimal data
    let jpeg_header = vec![0xFF, 0xD8, 0xFF];
    let png_header = vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
    let gif_header = b"GIF87a".to_vec();

    // Test JPEG detection
    let jpeg_codec = shared_utils::quality_matcher::SourceCodec::identify_by_header(&jpeg_header);
    assert!(matches!(
        jpeg_codec,
        Some(shared_utils::quality_matcher::SourceCodec::Jpeg)
    ));

    // Test PNG detection
    let png_codec = shared_utils::quality_matcher::SourceCodec::identify_by_header(&png_header);
    assert!(matches!(
        png_codec,
        Some(shared_utils::quality_matcher::SourceCodec::Png)
    ));

    // Test GIF detection
    let gif_codec = shared_utils::quality_matcher::SourceCodec::identify_by_header(&gif_header);
    assert!(matches!(
        gif_codec,
        Some(shared_utils::quality_matcher::SourceCodec::Gif)
    ));

    println!("✅ Basic format detection test passed!");
}
