use shared_utils::media_meta_utils::scan_gif_headers;
use std::io::Write;
use tempfile::NamedTempFile;

fn create_test_animated_gif() -> Vec<u8> {
    let mut gif = Vec::new();
    gif.extend_from_slice(b"GIF87a");
    gif.extend_from_slice(&[0x01, 0x00, 0x01, 0x00]);
    gif.extend_from_slice(&[0x80, 0x00]);
    gif.extend_from_slice(&[0x00, 0x00, 0x00]);
    gif.extend_from_slice(&[0x00, 0x00]);
    gif.extend_from_slice(&[0xFF, 0xFF, 0xFF]);
    gif.extend_from_slice(&[0x00, 0x00, 0x00]);

    // First frame
    gif.extend_from_slice(&[0x21, 0xF9, 0x04, 0x00, 0x0A, 0x00, 0x00, 0x00]);
    gif.extend_from_slice(&[
        0x2C, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x01, 0x00, 0x00, 0x00,
    ]);
    gif.extend_from_slice(&[0x02]);
    gif.extend_from_slice(&[0x02, 0x44, 0x01]);
    gif.extend_from_slice(&[0x00]);

    // Second frame
    gif.extend_from_slice(&[0x21, 0xF9, 0x04, 0x00, 0x0A, 0x00, 0x00, 0x00]);
    gif.extend_from_slice(&[
        0x2C, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x01, 0x00, 0x00, 0x00,
    ]);
    gif.extend_from_slice(&[0x02]);
    gif.extend_from_slice(&[0x02, 0x44, 0x01]);
    gif.extend_from_slice(&[0x00]);

    // Third frame
    gif.extend_from_slice(&[0x21, 0xF9, 0x04, 0x00, 0x0A, 0x00, 0x00, 0x00]);
    gif.extend_from_slice(&[
        0x2C, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x01, 0x00, 0x00, 0x00,
    ]);
    gif.extend_from_slice(&[0x02]);
    gif.extend_from_slice(&[0x02, 0x44, 0x01]);
    gif.extend_from_slice(&[0x00]);

    gif.extend_from_slice(&[0x3B]);
    gif
}

fn main() {
    let gif = create_test_animated_gif();
    println!("GIF length: {}", gif.len());

    // Manual parsing to verify frame count
    let mut frame_count = 0;
    let mut pos = 13; // Skip header

    // Skip global color table
    pos += 6; // 2 colors * 3 bytes = 6 bytes

    println!("Starting position after header and color table: {pos}");

    while pos < gif.len() {
        let byte = gif[pos];
        println!("Position {pos}: 0x{byte:02X}");

        if byte == 0x2C {
            frame_count += 1;
            println!("  Found frame {frame_count}!");
            pos += 11; // Skip image descriptor
            if pos < gif.len() {
                pos += 1; // Skip LZW min code size
                          // Skip image data
                if pos < gif.len() {
                    let data_size = gif[pos] as usize;
                    pos += 1;
                    pos += data_size;
                    if pos < gif.len() && gif[pos] == 0x00 {
                        pos += 1;
                    }
                }
            }
        } else if byte == 0x21 {
            // Extension block
            pos += 2;
            if pos < gif.len() {
                let ext_size = gif[pos] as usize;
                pos += 1;
                pos += ext_size;
                // Skip sub-blocks
                while pos < gif.len() && gif[pos] != 0x00 {
                    let sub_size = gif[pos] as usize;
                    pos += 1;
                    pos += sub_size;
                }
                if pos < gif.len() && gif[pos] == 0x00 {
                    pos += 1;
                }
            }
        } else if byte == 0x3B {
            break;
        } else {
            pos += 1;
        }
    }

    println!("Manual frame count: {frame_count}");

    // Use actual scan function
    let mut temp_file = NamedTempFile::new().expect("Failed to create temp file");
    temp_file.write_all(&gif).expect("Failed to write GIF");

    let scan_result = scan_gif_headers(temp_file.path());
    match &scan_result {
        Ok(scan) => {
            println!("Scan result frame_count: {:?}", scan.frame_count);
        }
        Err(e) => println!("Scan error: {e:?}"),
    }
}
