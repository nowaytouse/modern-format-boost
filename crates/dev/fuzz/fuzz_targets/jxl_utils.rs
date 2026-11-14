#![no_main]

use foundation::jxl_utils::{is_grayscale_icc_cjxl_error, is_icc_rounding_error};
use libfuzzer_sys::fuzz_target;

// We don't fuzz strip_jpeg_tail_to_temp directly here because it requires a
// file path. However, we can fuzz the error parsers which involve complex
// string matching.

fuzz_target!(|data: &[u8]| {
    match core::str::from_utf8(data) {
        Ok(s) => {
            let _ = is_icc_rounding_error(s);
            let _ = is_grayscale_icc_cjxl_error(s);
        }
        Err(_err) => {}
    }
});
