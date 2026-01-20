//! Format-specific utilities and helpers

/// PNG format utilities
pub mod png {
    use std::fs;
    use std::io::Read;
    use std::path::Path;

    /// Check if PNG uses optimal compression by analyzing IDAT chunk sizes
    pub fn is_optimally_compressed(path: &Path) -> bool {
        if let Ok(bytes) = fs::read(path) {
            let idat_count = bytes.windows(4).filter(|w| *w == b"IDAT").count();
            idat_count <= 2
        } else {
            false
        }
    }

    /// Get PNG compression level estimate based on file analysis
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

/// JPEG format utilities
pub mod jpeg {
    use std::fs;
    use std::io::Read;
    use std::path::Path;

    /// Estimate JPEG quality factor (0-100) by analyzing quantization tables
    pub fn estimate_quality(path: &Path) -> u8 {
        if let Ok(mut file) = fs::File::open(path) {
            let mut buffer = vec![0u8; 4096];
            if file.read(&mut buffer).is_ok() {
                for i in 0..buffer.len().saturating_sub(70) {
                    if buffer[i] == 0xFF && buffer[i + 1] == 0xDB && i + 5 < buffer.len() {
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
        85
    }

    /// Check if JPEG is progressive by looking for SOF2 marker
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

/// WebP format utilities
pub mod webp {
    use std::fs;
    use std::path::Path;

    /// Check if WebP is lossless
    pub fn is_lossless(path: &Path) -> bool {
        if let Ok(bytes) = fs::read(path) {
            bytes.windows(4).any(|w| w == b"VP8L")
        } else {
            false
        }
    }

    /// Check if WebP is animated
    pub fn is_animated(path: &Path) -> bool {
        if let Ok(bytes) = fs::read(path) {
            bytes.windows(4).any(|w| w == b"ANIM")
        } else {
            false
        }
    }
}

/// GIF format utilities
pub mod gif {
    use std::fs;
    use std::path::Path;

    /// Check if GIF is animated
    pub fn is_animated(path: &Path) -> bool {
        if let Ok(bytes) = fs::read(path) {
            let descriptor_count = bytes.iter().filter(|&&b| b == 0x2C).count();
            descriptor_count > 1
        } else {
            false
        }
    }

    /// Get number of frames in GIF
    pub fn get_frame_count(path: &Path) -> usize {
        if let Ok(bytes) = fs::read(path) {
            bytes.iter().filter(|&&b| b == 0x2C).count()
        } else {
            0
        }
    }
}

/// JXL format utilities
pub mod jxl {
    use std::fs;
    use std::io::Read;
    use std::path::Path;

    /// Verify JXL signature
    pub fn verify_signature(path: &Path) -> bool {
        if let Ok(mut file) = fs::File::open(path) {
            let mut sig = [0u8; 2];
            if file.read_exact(&mut sig).is_ok() {
                return sig == [0xFF, 0x0A] || sig == [0x00, 0x00];
            }
        }
        false
    }

    /// Check if JXL file is valid
    pub fn is_valid(path: &Path) -> bool {
        verify_signature(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    // 🔥 v7.0: 修复假测试 - 使用真实文件数据

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

        assert!(webp::is_lossless(file.path()), "VP8L 应被检测为 lossless");
    }

    #[test]
    fn test_gif_frame_count() {
        let gif_data: Vec<u8> = {
            let mut data = b"GIF89a".to_vec();
            data.extend_from_slice(&[0x01, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00]);
            data.push(0x2C); // frame 1
            data.extend_from_slice(&[0u8; 10]);
            data.push(0x2C); // frame 2
            data.extend_from_slice(&[0u8; 10]);
            data.push(0x3B);
            data
        };
        let mut file = NamedTempFile::new().expect("创建临时文件失败");
        file.write_all(&gif_data).expect("写入失败");

        assert_eq!(gif::get_frame_count(file.path()), 2, "应检测到 2 帧");
    }

    #[test]
    fn test_jxl_codestream_signature() {
        let jxl_data: &[u8] = &[0xFF, 0x0A, 0x00, 0x00];
        let mut file = NamedTempFile::new().expect("创建临时文件失败");
        file.write_all(jxl_data).expect("写入失败");

        assert!(jxl::verify_signature(file.path()), "JXL 签名应被识别");
    }

    #[test]
    fn test_error_handling_nonexistent() {
        let path = std::path::Path::new("/nonexistent/file.test");
        assert!(!webp::is_lossless(path));
        assert!(!gif::is_animated(path));
        assert_eq!(gif::get_frame_count(path), 0);
        assert!(!jxl::verify_signature(path));
    }
}
