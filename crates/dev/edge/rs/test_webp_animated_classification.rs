#[path = "../gifs/synth_webp.rs"]
mod synth_webp;

use shared_utils::quality_matcher::SourceCodec;

#[test]
fn classify_animated_webp_even_without_vp8x_in_first_64_bytes() {
    let bytes = synth_webp::build_synthetic_animated_webp_without_vp8x_in_header();

    // Header-only classifier should see RIFF/WEBP but NOT VP8X => initial WebpStatic.
    // Content classifier must upgrade to WebpAnimated by scanning for ANIM/ANMF markers.
    let header_codec = SourceCodec::identify_by_header(&bytes[..64]);
    assert_eq!(header_codec, Some(SourceCodec::WebpStatic));

    // Write to a temp file to exercise identify_by_content (file-based deep verification).
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("synthetic.webp");
    std::fs::write(&path, &bytes).expect("write synthetic webp");

    let content_codec = SourceCodec::identify_by_content(&path);
    assert_eq!(content_codec, Some(SourceCodec::WebpAnimated));
}

