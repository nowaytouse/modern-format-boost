#![no_main]

use arbitrary::{Arbitrary, Unstructured};
use image::{DynamicImage, GrayImage, RgbImage};
use libfuzzer_sys::fuzz_target;
use shared_utils::hdr_synthesis::{synthesize_hdr, GainMapParams};

fuzz_target!(|data: &[u8]| {
    let mut u = Unstructured::new(data);

    // 1. Generate mocking Parameters
    let Ok(params) = GainMapParams::arbitrary(&mut u) else {
        return;
    };

    // 2. Generate mocking dimensions (constrained to avoid OOM)
    let Ok(width) = u.int_in_range(1..=128) else {
        return;
    };
    let Ok(height) = u.int_in_range(1..=128) else {
        return;
    };
    let Ok(needs_p3_conversion) = bool::arbitrary(&mut u) else {
        return;
    };

    // 3. Create mocking images
    // We use small images to keep the fuzzing loop tight
    let sdr_img = DynamicImage::ImageRgb8(RgbImage::new(width, height));
    let gain_img = DynamicImage::ImageLuma8(GrayImage::new(width, height));

    // 4. Run synthesis
    // We are looking for:
    // - NaNs in calculations
    // - Infinite loops
    // - Buffer overflows (should be caught by Rust but good to verify)
    // - Unexpected panics
    let _ = synthesize_hdr(&sdr_img, &gain_img, &params, needs_p3_conversion);
});
