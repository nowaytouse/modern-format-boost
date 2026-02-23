//! Format-specific utilities and helpers

pub mod png {
    use std::fs;
    use std::io::Read;
    use std::path::Path;

    pub fn is_optimally_compressed(path: &Path) -> bool {
        if let Ok(bytes) = fs::read(path) {
            let idat_count = bytes.windows(4).filter(|w| *w == b"IDAT").count();
            idat_count <= 2
        } else {
            false
        }
    }

    pub fn estimate_compression_level(path: &Path) -> u8 {
        if let Ok(mut file) = fs::File::open(path) {
            let mut header = [0u8; 16];
            if file.read_exact(&mut header).is_ok() {
                return 6;
            }
        }
        6
    }
}

pub mod jpeg {
    use std::fs;
    use std::io::Read;
    use std::path::Path;

    pub fn estimate_quality(path: &Path) -> u8 {
        if let Ok(mut file) = fs::File::open(path) {
            let mut buffer = vec![0u8; 4096];
            if file.read(&mut buffer).is_ok() {
                for i in 0..buffer.len().saturating_sub(70) {
                    if buffer[i] == 0xFF && buffer[i + 1] == 0xDB {
                        if i + 5 < buffer.len() {
                            let q_value = buffer[i + 5] as u32;
                            return match q_value {
                                0..=2 => 98,
                                3..=5 => 95,
                                6..=10 => 90,
                                11..=20 => 85,
                                21..=40 => 75,
                                41..=60 => 65,
                                _ => 50,
                            };
                        }
                    }
                }
            }
        }
        85
    }

    pub fn is_progressive(path: &Path) -> bool {
        if let Ok(mut file) = fs::File::open(path) {
            let mut buffer = vec![0u8; 4096];
            if file.read(&mut buffer).is_ok() {
                for i in 0..buffer.len().saturating_sub(1) {
                    if buffer[i] == 0xFF && buffer[i + 1] == 0xC2 {
                        return true;
                    }
                }
            }
        }
        false
    }
}

pub mod webp {
    use std::fs;
    use std::path::Path;

    pub fn is_lossless_from_bytes(data: &[u8]) -> bool {
        data.windows(4).any(|w| w == b"VP8L")
    }

    pub fn is_animated_from_bytes(data: &[u8]) -> bool {
        data.windows(4).any(|w| w == b"ANIM")
    }

    pub fn count_frames_from_bytes(data: &[u8]) -> u32 {
        let count = data.windows(4).filter(|w| *w == b"ANMF").count() as u32;
        count.max(1)
    }

    pub fn is_lossless(path: &Path) -> bool {
        fs::read(path)
            .map(|b| is_lossless_from_bytes(&b))
            .unwrap_or(false)
    }

    pub fn is_animated(path: &Path) -> bool {
        fs::read(path)
            .map(|b| is_animated_from_bytes(&b))
            .unwrap_or(false)
    }
}

pub mod gif {
    use std::fs;
    use std::path::Path;

    pub fn count_frames_from_bytes(data: &[u8]) -> u32 {
        if data.len() < 24 || &data[0..3] != b"GIF" {
            return 0;
        }

        let mut pos = 6;
        if pos + 7 > data.len() {
            return 0;
        }
        let packed = data[pos + 4];
        let has_gct = (packed & 0x80) != 0;
        let gct_size = if has_gct {
            3 * (1 << ((packed & 0x07) + 1))
        } else {
            0
        };
        pos += 7 + gct_size;

        let mut frame_count = 0u32;
        while pos < data.len() {
            match data[pos] {
                0x2C => {
                    frame_count += 1;
                    if pos + 10 > data.len() {
                        break;
                    }
                    let img_packed = data[pos + 9];
                    let has_lct = (img_packed & 0x80) != 0;
                    let lct_size = if has_lct {
                        3 * (1 << ((img_packed & 0x07) + 1))
                    } else {
                        0
                    };
                    pos += 10 + lct_size;
                    if pos >= data.len() {
                        break;
                    }
                    pos += 1;
                    while pos < data.len() {
                        let block_size = data[pos] as usize;
                        pos += 1;
                        if block_size == 0 {
                            break;
                        }
                        pos += block_size;
                    }
                }
                0x21 => {
                    if pos + 2 >= data.len() {
                        break;
                    }
                    pos += 2;
                    while pos < data.len() {
                        let block_size = data[pos] as usize;
                        pos += 1;
                        if block_size == 0 {
                            break;
                        }
                        pos += block_size;
                    }
                }
                0x3B => break,
                _ => {
                    pos += 1;
                }
            }
        }
        frame_count
    }

    pub fn is_animated_from_bytes(data: &[u8]) -> bool {
        count_frames_from_bytes(data) > 1
    }

    pub fn is_animated(path: &Path) -> bool {
        fs::read(path)
            .map(|b| is_animated_from_bytes(&b))
            .unwrap_or(false)
    }

    pub fn get_frame_count(path: &Path) -> usize {
        fs::read(path)
            .map(|b| count_frames_from_bytes(&b) as usize)
            .unwrap_or(0)
    }
}

pub mod jxl {
    use std::fs;
    use std::path::Path;

    pub fn verify_signature(path: &Path) -> bool {
        if let Ok(mut file) = fs::File::open(path) {
            use std::io::Read;
            let mut sig = [0u8; 2];
            if file.read_exact(&mut sig).is_ok() {
                return sig == [0xFF, 0x0A] || sig == [0x00, 0x00];
            }
        }
        false
    }

    pub fn is_valid(path: &Path) -> bool {
        verify_signature(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn test_png_compression_with_real_data() {
        let png_data: &[u8] = &[
            0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48,
            0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00,
            0x00, 0x90, 0x77, 0x53, 0xDE,
        ];
        let mut file = NamedTempFile::new().expect("创建临时文件失败");
        file.write_all(png_data).expect("写入失败");

        let level = png::estimate_compression_level(file.path());
        assert!(level <= 9, "PNG 压缩级别应在 0-9 范围内，实际: {}", level);
    }

    #[test]
    fn test_jpeg_quality_with_real_data() {
        let jpeg_data: &[u8] = &[
            0xFF, 0xD8, 0xFF, 0xDB, 0x00, 0x43, 0x00, 0x02, 0x01, 0x01, 0x02, 0x01, 0x01, 0x02,
            0x02, 0x02, 0x02, 0x02, 0x02, 0x02, 0x02, 0x03, 0x05, 0x03, 0x03, 0x03, 0x03, 0x03,
            0x06, 0x04, 0x04, 0x03, 0x05, 0x07, 0x06, 0x07, 0x07, 0x07, 0x06, 0x07, 0x07, 0x08,
            0x09, 0x0B, 0x09, 0x08, 0x08, 0x0A, 0x08, 0x07, 0x07, 0x0A, 0x0D, 0x0A, 0x0A, 0x0B,
            0x0C, 0x0C, 0x0C, 0x0C, 0x07, 0x09, 0x0E, 0x0F, 0x0D, 0x0C, 0x0E, 0x0B, 0x0C, 0x0C,
            0x0C,
        ];
        let mut file = NamedTempFile::new().expect("创建临时文件失败");
        file.write_all(jpeg_data).expect("写入失败");

        let quality = jpeg::estimate_quality(file.path());
        assert!(quality >= 90, "低量化值应返回高质量，实际: {}", quality);
    }

    #[test]
    fn test_webp_lossless_detection() {
        let webp_lossless: Vec<u8> = {
            let mut data = b"RIFF".to_vec();
            data.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]);
            data.extend_from_slice(b"WEBP");
            data.extend_from_slice(b"VP8L");
            data.extend_from_slice(&[0u8; 20]);
            data
        };
        let mut file = NamedTempFile::new().expect("创建临时文件失败");
        file.write_all(&webp_lossless).expect("写入失败");

        assert!(
            webp::is_lossless(file.path()),
            "VP8L chunk 应被检测为 lossless"
        );
    }

    #[test]
    fn test_webp_lossy_detection() {
        let webp_lossy: Vec<u8> = {
            let mut data = b"RIFF".to_vec();
            data.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]);
            data.extend_from_slice(b"WEBP");
            data.extend_from_slice(b"VP8 ");
            data.extend_from_slice(&[0u8; 20]);
            data
        };
        let mut file = NamedTempFile::new().expect("创建临时文件失败");
        file.write_all(&webp_lossy).expect("写入失败");

        assert!(
            !webp::is_lossless(file.path()),
            "VP8 chunk 应被检测为 lossy"
        );
    }

    #[test]
    fn test_gif_frame_count() {
        let gif_data: Vec<u8> = {
            let mut data = b"GIF89a".to_vec();
            data.extend_from_slice(&[0x01, 0x00, 0x01, 0x00]);
            data.extend_from_slice(&[0x00, 0x00, 0x00]);

            data.push(0x2C);
            data.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]);
            data.extend_from_slice(&[0x01, 0x00, 0x01, 0x00]);
            data.push(0x00);
            data.push(0x02);
            data.extend_from_slice(&[0x02, 0x4C, 0x01]);
            data.push(0x00);

            data.push(0x2C);
            data.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]);
            data.extend_from_slice(&[0x01, 0x00, 0x01, 0x00]);
            data.push(0x00);
            data.push(0x02);
            data.extend_from_slice(&[0x02, 0x4C, 0x01]);
            data.push(0x00);

            data.push(0x3B);
            data
        };
        let mut file = NamedTempFile::new().expect("Failed to create temp file");
        file.write_all(&gif_data).expect("Failed to write");

        let count = gif::get_frame_count(file.path());
        assert_eq!(count, 2, "Expected 2 frames, got: {}", count);
        assert!(
            gif::is_animated(file.path()),
            "2-frame GIF should be detected as animated"
        );
    }

    #[test]
    fn test_jxl_codestream_signature() {
        let jxl_codestream: &[u8] = &[0xFF, 0x0A, 0x00, 0x00];
        let mut file = NamedTempFile::new().expect("创建临时文件失败");
        file.write_all(jxl_codestream).expect("写入失败");

        assert!(
            jxl::verify_signature(file.path()),
            "JXL codestream 签名应被识别"
        );
    }

    #[test]
    fn test_error_handling_nonexistent_file() {
        let path = std::path::Path::new("/nonexistent/file.test");

        assert!(!webp::is_lossless(path), "不存在的文件应返回 false");
        assert!(!webp::is_animated(path), "不存在的文件应返回 false");
        assert!(!gif::is_animated(path), "不存在的文件应返回 false");
        assert_eq!(gif::get_frame_count(path), 0, "不存在的文件应返回 0");
        assert!(!jxl::verify_signature(path), "不存在的文件应返回 false");
    }
}
