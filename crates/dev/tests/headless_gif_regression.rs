use std::path::{Path, PathBuf};

#[test]
fn headless_gif_regression_suite() {
    test_headless_gif_regression_frame_count_and_loop_intent();
}

fn test_headless_gif_regression_frame_count_and_loop_intent() {
    let dev_asset_path: PathBuf = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("edge")
        .join("gifs")
        .join("simulated_headless_sticker.gif");

    // 1. Verify the fallback logic in GifHeaderScan correctly catches payload size
    let scan = shared_utils::media_meta_utils::scan_gif_headers(&dev_asset_path)
        .unwrap_or_else(|e| panic!("Failed to scan headless GIF: {e:?}"));

    // There were 7 images inside the standard_7f.gif
    assert_eq!(
        scan.frame_count, 7,
        "GIF scanner must identify exactly 7 frames based on Image Descriptors even without delays"
    );

    // 2. Assure duration and delay evaluations are handled smoothly (duration might be ~0 or None)
    assert!(
        scan.duration_secs.is_none_or(|d| d == 0.0),
        "Headless GIF should correctly map to missing/zero duration"
    );

    // 3. Complete LoopMeta Pipeline checks
    // If the fast path executes, we expect it is parsed successfully and is considered a loop
    let meta =
        shared_utils::loop_intent::LoopMeta::from_gif_path(&dev_asset_path).unwrap_or_else(|| {
            panic!("LoopMeta from_gif_path must succeed for valid GIF without delays")
        });

    assert_eq!(
        meta.frame_count,
        Some(7),
        "LoopMeta frame count must strictly inherit the scanner's counted frames"
    );

    let verdict = shared_utils::loop_intent::evaluate_loop_tree(&meta, None).verdict;

    // It should hit Layer 1-B or another layer, but DEFINITELY not Layer 1-A.
    // Given the test asset has no loops explicitly, is silent, and short,
    // it will either trigger Layer 1-B2 or Layer 1-B.
    assert!(
        !verdict.reason().contains("Layer 1-A"),
        "Must NOT fall into Layer 1-A (single frame media) false positive! Actual reason: {}",
        verdict.reason()
    );
}
