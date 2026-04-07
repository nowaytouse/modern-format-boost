#![no_main]

use libfuzzer_sys::fuzz_target;
use shared_utils::image_quality_detector::analyze_image_quality;
use shared_utils::image_detection::PrecisionMetadata;
use arbitrary::{Arbitrary, Unstructured};

fuzz_target!(|data: &[u8]| {
    let mut u = Unstructured::new(data);

    // 1. Generate mocking dimensions (constrained to avoid OOM)
    let Ok(width) = u.int_in_range(1..=256) else { return; };
    let Ok(height) = u.int_in_range(1..=256) else { return; };
    
    // 2. Generate other parameters
    let Ok(file_size) = u64::arbitrary(&mut u) else { return; };
    let Ok(format) = String::arbitrary(&mut u) else { return; };
    let Ok(frame_count) = u32::arbitrary(&mut u) else { return; };
    
    // 3. Extract RGBA data
    // Width * Height * 4 bytes
    let rgba_len = (width as usize) * (height as usize) * 4;
    let Ok(rgba_data) = u.bytes(rgba_len) else { return; };

    // 4. Run analysis
    let _ = analyze_image_quality(
        width,
        height,
        rgba_data,
        file_size,
        &format,
        frame_count,
        PrecisionMetadata::default(),
    );
});
