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

    // 第一帧
    gif.extend_from_slice(&[0x21, 0xF9, 0x04, 0x00, 0x0A, 0x00, 0x00, 0x00]);
    gif.extend_from_slice(&[
        0x2C, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x01, 0x00, 0x00, 0x00,
    ]);
    gif.extend_from_slice(&[0x02]);
    gif.extend_from_slice(&[0x02, 0x44, 0x01]);
    gif.extend_from_slice(&[0x00]);

    // 第二帧
    gif.extend_from_slice(&[0x21, 0xF9, 0x04, 0x00, 0x0A, 0x00, 0x00, 0x00]);
    gif.extend_from_slice(&[
        0x2C, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x01, 0x00, 0x00, 0x00,
    ]);
    gif.extend_from_slice(&[0x02]);
    gif.extend_from_slice(&[0x02, 0x44, 0x01]);
    gif.extend_from_slice(&[0x00]);

    // 第三帧
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

    // 手动解析来验证帧数
    let mut frame_count = 0;
    let mut pos = 13; // 跳过头部

    // 跳过全局颜色表
    pos += 6; // 2种颜色 * 3字节 = 6字节

    println!("Starting position after header and color table: {pos}");

    while pos < gif.len() {
        let byte = gif[pos];
        println!("Position {pos}: 0x{byte:02X}");

        if byte == 0x2C {
            frame_count += 1;
            println!("  Found frame {frame_count}!");
            pos += 11; // 跳过图像描述符
            if pos < gif.len() {
                pos += 1; // 跳过LZW最小码长
                          // 跳过图像数据
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
            // 扩展块
            pos += 2;
            if pos < gif.len() {
                let ext_size = gif[pos] as usize;
                pos += 1;
                pos += ext_size;
                // 跳过子块
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

    // 使用实际的扫描函数
    let mut temp_file = NamedTempFile::new().expect("创建临时文件失败");
    temp_file.write_all(&gif).expect("写入GIF失败");

    let scan_result = scan_gif_headers(temp_file.path());
    match &scan_result {
        Ok(scan) => {
            println!("Scan result frame_count: {:?}", scan.frame_count);
        }
        Err(e) => println!("Scan error: {e:?}"),
    }
}
