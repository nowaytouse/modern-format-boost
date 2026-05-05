#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "Unwrapping in test modules is idiomatic to ensure tests fail loudly and immediately on setup errors."
)]
use std::process::Command;
use tempfile::tempdir;

#[test]
fn test_cjxl_grayscale_icc_fallback() {
    use img::lossless_converter::{convert_to_jxl, ConvertOptions};

    // Use a temporary directory to avoid conflicts
    let tmp_dir = tempdir().expect("Failed to create temp dir");
    let input = tmp_dir.path().join("test_grayscale_icc.png");
    let output = tmp_dir.path().join("test_grayscale_icc.jxl");

    // 1. Create a large enough grayscale PNG (to avoid the 500KB "small PNG" skip)
    // Create 1024x1024 grayscale PNG with noise to ensure it's > 500KB
    let status = Command::new("magick")
        .args([
            "-size",
            "1024x1024",
            "xc:gray",
            "+noise",
            "random",
            input.to_str().unwrap(),
        ])
        .status()
        .unwrap_or_else(|e| panic!("magick failed to run: {e:?}"));
    assert!(status.success(), "Failed to create initial noise PNG");

    // 2. Assign an sRGB profile (RGB color space) to this grayscale image
    // This often triggers the libpng warning when cjxl reads it.
    let status = Command::new("magick")
        .args([
            input.to_str().unwrap(),
            "-colorspace",
            "sRGB",
            input.to_str().unwrap(),
        ])
        .status()
        .expect("Failed to run second magick command");
    assert!(status.success(), "Failed to modify PNG colorspace");

    // 3. Try to run the tool's conversion logic
    let options = ConvertOptions {
        flags: shared_utils::conversion::ConvertFlags::VERBOSE,
        ..Default::default()
    };

    let result = convert_to_jxl(&input, &options, 0.1, None);

    match result {
        Ok(_) => {
            println!("✅ Conversion succeeded (presumably via fallback)!");
            assert!(output.exists());

            // Verify it's a valid JXL
            let status = Command::new("jxlinfo")
                .arg(&output)
                .status()
                .unwrap_or_else(|e| panic!("jxlinfo failed to run: {e:?}"));
            assert!(status.success());
        }
        Err(e) => {
            panic!("❌ Conversion failed despite fallback logic: {e}");
        }
    }

    // tmp_dir is automatically cleaned up when dropped
}
