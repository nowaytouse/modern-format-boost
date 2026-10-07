#![no_main]

use arbitrary::{Arbitrary, Unstructured};
use foundation::image_detection::PrecisionMetadata;
use foundation::image_quality_detector::analyze_image_quality;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let mut u = Unstructured::new(data);

    // 1. Generate mocking dimensions (constrained to avoid OOM)
    let Ok(width) = u.int_in_range(1..=256) else {
        return;
    };
    let Ok(height) = u.int_in_range(1..=256) else {
        return;
    };

    // 2. Generate other parameters
    let Ok(file_size) = u64::arbitrary(&mut u) else {
        return;
    };
    let Ok(format) = String::arbitrary(&mut u) else {
        return;
    };
    let Ok(frame_count) = u32::arbitrary(&mut u) else {
        return;
    };

    // 3. Extract RGBA data
    // Width * Height * 4 bytes
    let Some(width_usize) = foundation::numeric_cast::u32_to_usize_strict(width, "fuzz_width")
    else {
        return;
    };
    let Some(height_usize) = foundation::numeric_cast::u32_to_usize_strict(height, "fuzz_height")
    else {
        return;
    };
    let Some(rgba_len) = width_usize
        .checked_mul(height_usize)
        .and_then(|pixels| pixels.checked_mul(4))
    else {
        return;
    };
    let Ok(rgba_data) = u.bytes(rgba_len) else {
        return;
    };

    // 4. Run analysis
    let _ = analyze_image_quality(
        width,
        height,
        rgba_data,
        file_size,
        &format,
        Some(frame_count),
        PrecisionMetadata::default(),
    );
});
