use foundation::conversion::ConvertFlags;
use img::lossless_converter::ConvertOptions;
use img::lossless_converter::convert_jpeg_to_jxl;
use std::fs;
use tempfile::tempdir;

#[test]
fn truncated_jpeg_is_skipped_with_delivery_required() -> anyhow::Result<()> {
    // Setup isolated temp home for logs/ghost mode
    let tmp_dir = tempdir()?;
    let mfb_home = tmp_dir.path().join("mfb_home");
    // tests in this repo use an unsafe set_var pattern for test env setup
    unsafe { std::env::set_var("MFB_HOME_ROOT", &mfb_home) };
    foundation::init_ghost_mode()?;

    // Create a truncated JPEG (only SOI + APP0 marker bytes)
    let input = tmp_dir.path().join("truncated.jpg");
    fs::write(&input, [0xFFu8, 0xD8u8, 0xFFu8, 0xE0u8])?;

    // Require delivery so the irreversible transcode path emits a skipped result
    let options = ConvertOptions {
        flags: ConvertFlags::REQUIRE_OUTPUT_DELIVERY,
        ..Default::default()
    };

    let result = convert_jpeg_to_jxl(&input, &options, None)?;

    // Expect the task to be marked as skipped and message to indicate irreversible transcode
    assert!(result.skipped, "expected skipped result for truncated JPEG");
    assert!(result.success, "skipped TaskResult should be success=true");
    assert!(result.skip_reason.is_some(), "skip_reason should be set");
    assert!(
        result
            .message
            .contains("JPEG cannot be reversibly transcoded")
            || result.message.contains("transcode preflight rejected"),
        "unexpected skip message: {}",
        result.message
    );
    Ok(())
}
