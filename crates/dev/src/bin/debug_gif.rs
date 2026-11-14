#![allow(unused_imports)]

use foundation::{
    log_anomaly, log_corruption, log_debug, log_detail, log_failure, log_fatal, log_hint,
    log_ignore, log_info, log_skip, log_success,
};

use foundation::quality_matcher::SourceCodec;

fn main() {
    let truncated_gif = b"GIF87";
    log_detail!("Testing: {truncated_gif:?}");
    log_detail!("Length: {}", truncated_gif.len());

    for (i, &byte) in truncated_gif.iter().enumerate() {
        log_detail!("Byte {}: 0x{:02X} ('{}')", i, byte, byte as char);
    }

    let codec = SourceCodec::identify_by_header(truncated_gif);
    log_detail!("Identified as: {codec:?}");

    // Test against MPEG patterns
    log_detail!("Starts with 0x47: {}", truncated_gif.starts_with(&[0x47]));
    log_detail!(
        "Starts with [0x00, 0x00, 0x01, 0xBA]: {}",
        truncated_gif.starts_with(&[0x00, 0x00, 0x01, 0xBA]),
    );
}
