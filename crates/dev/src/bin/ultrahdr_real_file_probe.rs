#![allow(unused_imports)]

use foundation::{
    log_anomaly, log_corruption, log_debug, log_detail, log_failure, log_fatal, log_hint,
    log_ignore, log_info, log_skip, log_success,
};

use foundation::image_jpeg_analysis::extract_gainmap_from_jpeg;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

fn main() {
    log_detail!("Running test...");
    test_real_hdr_file_extraction_final();
    log_detail!(foundation::infra::static_logs::messages::VERIFICATION_COMPLETE);
}

fn probe_image_path() -> PathBuf {
    match env::var("MFB_ULTRAHDR_PROBE_IMAGE") {
        Ok(raw) => {
            let trimmed = raw.trim();
            if !trimmed.is_empty() {
                return PathBuf::from(trimmed);
            }
        }
        Err(env::VarError::NotPresent) => {}
        Err(err) => {
            log_detail!(&format!(
                "Skipping unreadable MFB_ULTRAHDR_PROBE_IMAGE env override: {err}"
            ));
        }
    }
    PathBuf::from("debug/IMG_0413.JPG")
}

fn test_real_hdr_file_extraction_final() {
    let path = probe_image_path();
    if !path.exists() {
        log_detail!(
            "Skipping UltraHDR probe: sample not found at {} (set MFB_ULTRAHDR_PROBE_IMAGE)",
            path.display()
        );
        return;
    }
    let data = fs::read(path).unwrap_or_else(|e| panic!("Failed to read image: {e:?}"));
    let result = extract_gainmap_from_jpeg(&data);
    match result {
        Ok((base, gain)) => {
            log_detail!(" REAL HDR FILE EXTRACTION SUCCESSFUL!");
            log_detail!(
                "Base: {}x{}, Gain: {}x{}",
                base.width(),
                base.height(),
                gain.width(),
                gain.height(),
            );
        }
        Err(e) => {
            panic!(" FAILED ON REAL FILE: {e}");
        }
    }
}
