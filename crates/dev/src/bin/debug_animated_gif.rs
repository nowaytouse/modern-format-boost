#![allow(unused_imports)]

use foundation::{
    log_anomaly, log_corruption, log_debug, log_detail, log_failure, log_fatal, log_hint,
    log_ignore, log_info, log_skip, log_success,
};

use foundation::media_meta_utils::scan_gif_headers;
use std::io::Write;
use tempfile::NamedTempFile;

fn create_test_animated_gif() -> Vec<u8> {
    let mut gif = Vec::new();
    gif.extend_from_slice(b"GIF87a");
    gif.extend_from_slice(&[0x01, 0x00, 0x01, 0x00]);
    gif.extend_from_slice(&[0x00, 0x00]);
    gif.extend_from_slice(&[0x00, 0x00, 0x00]);
    gif.extend_from_slice(&[0x00, 0x00]);

    // First frame
    gif.extend_from_slice(&[0x21, 0xF9, 0x04, 0x00, 0x0A, 0x00, 0x00, 0x00]);
    gif.extend_from_slice(&[
        0x2C, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x01, 0x00, 0x00, 0x00,
    ]);
    gif.extend_from_slice(&[0x02, 0x02, 0x44, 0x01, 0x00]);

    // Second frame
    gif.extend_from_slice(&[0x21, 0xF9, 0x04, 0x00, 0x0A, 0x00, 0x00, 0x00]);
    gif.extend_from_slice(&[
        0x2C, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x01, 0x00, 0x00, 0x00,
    ]);
    gif.extend_from_slice(&[0x02, 0x02, 0x44, 0x01, 0x00]);

    // Third frame
    gif.extend_from_slice(&[0x21, 0xF9, 0x04, 0x00, 0x0A, 0x00, 0x00, 0x00]);
    gif.extend_from_slice(&[
        0x2C, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x01, 0x00, 0x00, 0x00,
    ]);
    gif.extend_from_slice(&[0x02, 0x02, 0x44, 0x01, 0x00]);

    gif.extend_from_slice(&[0x3B]);
    gif
}

fn main() {
    let gif = create_test_animated_gif();
    log_detail!("GIF length: {}", gif.len());

    // Print complete GIF byte structure with key markers
    log_detail!("GIF structure with markers:");
    for (i, chunk) in gif.chunks(16).enumerate() {
        print!("{:04X}: ", i * 16);
        for &byte in chunk {
            print!("{byte:02X} ");
        }
        log_detail!();
    }

    // Manually mark key positions
    log_detail!("\nKey positions:");
    for (i, &byte) in gif.iter().enumerate() {
        if byte == 0x21 {
            log_detail!("Pos {i}: Extension (0x21)");
            if i + 1 < gif.len() {
                log_detail!("  Extension type: 0x{:02X}", gif[i + 1]);
            }
        } else if byte == 0x2C {
            log_detail!("Pos {i}: Image descriptor (0x2C) - FRAME!");
        } else if byte == 0x3B {
            log_detail!("Pos {i}: GIF trailer (0x3B)");
        }
    }

    let mut temp_file = NamedTempFile::new().expect("Failed to create temp file");
    temp_file
        .write_all(&gif)
        .expect("Failed to write animated GIF");

    let scan_result = scan_gif_headers(temp_file.path());
    match &scan_result {
        Ok(scan) => {
            log_detail!("\nScan result:");
            log_detail!("  frame_count: {:?}", scan.frame_count);
            log_detail!("  duration: {:?}", scan.duration_secs);
            log_detail!("  app_extensions: {:?}", scan.app_extensions);
            log_detail!("  loop_count: {:?}", scan.loop_count);
        }
        Err(e) => log_detail!("Scan error: {e:?}"),
    }
}
