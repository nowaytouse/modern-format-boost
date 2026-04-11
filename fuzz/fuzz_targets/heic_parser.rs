#![no_main]

use libfuzzer_sys::fuzz_target;
use shared_utils::image_heic_analysis::{detect_heic_is_lossless, extract_xmp_from_heic_data};
use std::path::Path;

fuzz_target!(|data: &[u8]| {
    // 1. Stress-test the box-scanning and lossless detection logic
    // This involves find_box_data_recursive which is prone to infinite loops or recursion depth issues
    let _ = detect_heic_is_lossless(data, Path::new("fuzz_input.heic"));

    // 2. Stress-test XMP extraction
    let _ = extract_xmp_from_heic_data(data);
});
