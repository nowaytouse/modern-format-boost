use std::io::Write;

include!("../edge/gifs/synth_headless_gif.rs");

#[test]
fn headless_gif_regression_suite() {
    test_headless_gif_regression_frame_count_and_loop_intent();
}

fn test_headless_gif_regression_frame_count_and_loop_intent() {
    let gif_bytes = build_synthetic_headless_sticker_gif();
    let mut temp = tempfile::NamedTempFile::with_suffix(".gif").expect("temp headless gif file");
    temp.write_all(&gif_bytes)
        .expect("write synthetic headless gif");
    let dev_asset_path = temp.path();

    let scan = foundation::media_meta_utils::scan_gif_headers(dev_asset_path)
        .unwrap_or_else(|e| panic!("Failed to scan headless GIF: {e:?}"));

    assert_eq!(
        scan.frame_count, 7,
        "GIF scanner must identify exactly 7 frames based on Image Descriptors even without delays"
    );

    assert!(
        scan.duration_secs
            .is_none_or(foundation::float_compare::approx_zero_f64),
        "Headless GIF should correctly map to missing/zero duration"
    );

    let meta =
        foundation::loop_intent::LoopMeta::from_gif_path(dev_asset_path).unwrap_or_else(|| {
            panic!("LoopMeta from_gif_path must succeed for valid GIF without delays")
        });

    assert_eq!(
        meta.frame_count,
        Some(7),
        "LoopMeta frame count must strictly inherit the scanner's counted frames"
    );

    let verdict = foundation::loop_intent::evaluate_loop_tree(&meta, None).verdict;

    assert!(
        !verdict.reason().contains("Layer 1-A"),
        "Must NOT fall into Layer 1-A (single frame media) false positive! Actual reason: {}",
        verdict.reason()
    );
}
