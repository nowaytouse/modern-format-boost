#![no_main]

use libfuzzer_sys::fuzz_target;
use shared_utils::jxl_utils::{is_icc_rounding_error, is_grayscale_icc_cjxl_error};

// We don't fuzz strip_jpeg_tail_to_temp directly here because it requires a file path.
// However, we can fuzz the error parsers which involve complex string matching.

fuzz_target!(|data: &[u8]| {
    if let Ok(s) = std::str::from_utf8(data) {
        let _ = is_icc_rounding_error(s);
        let _ = is_grayscale_icc_cjxl_error(s);
    }
});
