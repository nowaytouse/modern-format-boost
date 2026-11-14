#![no_main]

use foundation::image_jpeg_analysis::extract_gainmap_from_jpeg;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // We are fuzzing the byte-level scanner for MPF segments and XMP metadata.
    // The goal is to ensure that even pathologically malformed JPEGs
    // do not cause out-of-bounds reads, infinite loops, or panics.
    let _ = extract_gainmap_from_jpeg(data);
});
