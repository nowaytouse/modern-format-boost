#![allow(unused_imports)]

use shared_utils::{
    log_anomaly, log_corruption, log_detail, log_failure, log_fatal, log_hint, log_ignore,
    log_skip, log_success,
};

use shared_utils::image_jpeg_analysis::extract_xmp_from_jpeg_data;
use std::fs;

fn main() {
    let data = fs::read("debug/Ultra_HDR_Samples-main/Ultra_HDR_Samples-main/Originals/Ultra_HDR_Samples_Originals_01.jpg")
        .unwrap_or_else(|e| panic!("error: {e:?}"));
    if let Some(xmp) = extract_xmp_from_jpeg_data(&data) {
        log_detail!("XMP found (len: {}): \n{:?}", xmp.len(), xmp);
    } else {
        log_detail!("No XMP extracted!");
    }
}
