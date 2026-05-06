#[path = "edge/gifs/synth_webp.rs"]
mod synth_webp;

use shared_utils::quality_matcher::SourceCodec;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("运行 WebP 动画分类测试...");

    classify_animated_webp_even_without_vp8x_in_first_64_bytes()?;

    println!("✅ WebP 动画分类测试通过！");
    Ok(())
}

fn classify_animated_webp_even_without_vp8x_in_first_64_bytes(
) -> Result<(), Box<dyn std::error::Error>> {
    let bytes = synth_webp::build_synthetic_animated_webp_without_vp8x_in_header();

    // Header-only classifier should see RIFF/WEBP but NOT VP8X => initial WebpStatic.
    // Content classifier must upgrade to WebpAnimated by scanning for ANIM/ANMF markers.
    let header_codec = SourceCodec::identify_by_header(bytes.get(..64).ok_or_else(|| {
        anyhow::anyhow!(
            "Required byte slice missing (out of bounds) at index 64 with length {}",
            bytes.len()
        )
    })?);
    assert_eq!(header_codec, Some(SourceCodec::WebpStatic));

    // Write to a temp file to exercise identify_by_content (file-based deep verification).
    let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("temp dir: {e:?}"));
    let path = dir.path().join("animated.webp");
    std::fs::write(&path, &bytes).unwrap_or_else(|e| panic!("write synthetic webp: {e:?}"));

    let content_codec = SourceCodec::identify_by_content(&path);
    assert_eq!(content_codec, Some(SourceCodec::WebpAnimated));
    Ok(())
}
