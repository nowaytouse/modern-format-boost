include!("../edge/gifs/synth_webp.rs");

use foundation::quality_matcher::SourceCodec;

#[test]
fn webp_animated_classification_suite() -> anyhow::Result<()> {
    classify_animated_webp_even_without_vp8x_in_first_64_bytes()?;
    Ok(())
}

fn classify_animated_webp_even_without_vp8x_in_first_64_bytes() -> anyhow::Result<()> {
    let bytes = build_synthetic_animated_webp_without_vp8x_in_header();

    // Header-only classifier should see RIFF/WEBP but NOT VP8X => initial
    // WebpStatic. Content classifier must upgrade to WebpAnimated by scanning
    // for ANIM/ANMF markers.
    let header_codec = SourceCodec::identify_by_header(bytes.get(..64).ok_or_else(|| {
        anyhow::anyhow!(
            "Required byte slice missing (out of bounds) at index 64 with length {}",
            bytes.len()
        )
    })?);
    assert_eq!(header_codec, Some(SourceCodec::WebpStatic));

    // Write to a temp file to exercise identify_by_content (file-based deep
    // verification).
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("animated.webp");
    std::fs::write(&path, &bytes)?;

    let content_codec = SourceCodec::identify_by_content(&path)?;
    assert_eq!(content_codec, Some(SourceCodec::WebpAnimated));
    Ok(())
}
