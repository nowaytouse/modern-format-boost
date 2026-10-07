use foundation::conversion::ConvertFlags;
use img::lossless_converter::ConvertOptions;
use img::lossless_converter::convert_jpeg_to_jxl;
use std::fs;
use tempfile::tempdir;

#[test]
fn truncated_jpeg_is_failed_with_delivery_required() -> anyhow::Result<()> {
    // Setup isolated temp home for logs/ghost mode
    let tmp_dir = tempdir()?;
    let mfb_home = tmp_dir.path().join("mfb_home");
    // tests in this repo use an unsafe set_var pattern for test env setup
    unsafe { std::env::set_var("MFB_HOME_ROOT", &mfb_home) };
    foundation::init_ghost_mode()?;

    // Create a truncated JPEG (only SOI + APP0 marker bytes)
    let input = tmp_dir.path().join("truncated.jpg");
    fs::write(&input, [0xFFu8, 0xD8u8, 0xFFu8, 0xE0u8])?;

    // Require delivery so the irreversible encode path returns a structured failure.
    let options = ConvertOptions {
        flags: ConvertFlags::REQUIRE_OUTPUT_DELIVERY,
        ..Default::default()
    };

    let result = convert_jpeg_to_jxl(&input, &options, None)?;

    assert!(!result.skipped, "corrupt media must not be marked skipped");
    assert!(!result.success, "corrupt media must be marked failed");
    assert_eq!(result.outcome(), foundation::conversion::Outcome::Failed);
    assert!(input.exists(), "failed conversion must retain its source");
    assert!(
        result.skip_reason.is_some(),
        "failure reason id should be set"
    );
    assert!(
        result
            .message
            .contains("JPEG cannot be byte-identically reconstructed"),
        "unexpected failure message: {}",
        result.message
    );
    assert!(
        result.message.contains("source remains unmodified"),
        "failure must state the source-retention guarantee: {}",
        result.message
    );
    Ok(())
}
