#![allow(unused_imports)]

use shared_utils::{
    log_anomaly, log_corruption, log_detail, log_failure, log_fatal, log_hint, log_ignore,
    log_skip, log_success,
};

use shared_utils::image_jpeg_analysis::extract_gainmap_from_jpeg;
use std::fs;
use std::path::Path;

fn main() {
    log_detail!("Running test...");
    test_real_hdr_file_extraction_final();
    log_detail!("✅ Test completed!");
}

fn test_real_hdr_file_extraction_final() {
    let path = Path::new("/Users/nyamiiko/Downloads/GitHub/modern_format_boost/debug/IMG_0413.JPG");
    if !path.exists() {
        return;
    }
    let data = fs::read(path).unwrap_or_else(|e| panic!("Failed to read image: {e:?}"));
    let result = extract_gainmap_from_jpeg(&data);
    match result {
        Ok((base, gain)) => {
            log_detail!("✅ REAL HDR FILE EXTRACTION SUCCESSFUL!");
            log_detail!(
                "   Base: {}x{}, Gain: {}x{}",
                base.width(),
                base.height(),
                gain.width(),
                gain.height(),
            );
        }
        Err(e) => {
            panic!("❌ FAILED ON REAL FILE: {e}");
        }
    }
}
