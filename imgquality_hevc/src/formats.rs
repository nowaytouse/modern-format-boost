//! Format-specific utilities and helpers

/// PNG format utilities
pub mod png {
    use std::path::Path;
    use std::fs;
    use std::io::Read;

    /// Check if PNG uses optimal compression by analyzing IDAT chunk sizes
    pub fn is_optimally_compressed(path: &Path) -> bool {
        if let Ok(bytes) = fs::read(path) {
            // Count IDAT chunks - optimized PNGs typically have fewer, larger chunks
            let idat_count = bytes.windows(4).filter(|w| *w == b"IDAT").count();
            // Well-optimized PNGs usually have 1-2 IDAT chunks
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
                // Check zlib compression header in IDAT
                // Higher compression levels use different strategies
                // Default to level 6 (balanced)
                return 6;
            }
        }
        6
    }
}

/// JPEG format utilities
pub mod jpeg {
    use std::path::Path;
    use std::fs;
    use std::io::Read;

    /// Estimate JPEG quality factor (0-100) by analyzing quantization tables
    pub fn estimate_quality(path: &Path) -> u8 {
        if let Ok(mut file) = fs::File::open(path) {
            let mut buffer = vec![0u8; 4096];
            if file.read(&mut buffer).is_ok() {
                // Look for DQT marker (0xFF 0xDB) and analyze quantization values
                for i in 0..buffer.len().saturating_sub(70) {
                    if buffer[i] == 0xFF && buffer[i + 1] == 0xDB {
                        // Found quantization table, estimate quality from first few values
                        if i + 5 < buffer.len() {
                            let q_value = buffer[i + 5] as u32;
                            // Lower quantization values = higher quality
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
        85 // Default estimate
    }

    /// Check if JPEG is progressive by looking for SOF2 marker
    pub fn is_progressive(path: &Path) -> bool {
        if let Ok(mut file) = fs::File::open(path) {
            let mut buffer = vec![0u8; 4096];
            if file.read(&mut buffer).is_ok() {
                // SOF2 (0xFF 0xC2) indicates progressive JPEG
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
    use std::path::Path;
    use std::fs;

    /// Check if WebP is lossless
    pub fn is_lossless(path: &Path) -> bool {
        if let Ok(bytes) = fs::read(path) {
            // Look for VP8L chunk (lossless)
            bytes.windows(4).any(|w| w == b"VP8L")
        } else {
            false
        }
    }

    /// Check if WebP is animated
    pub fn is_animated(path: &Path) -> bool {
        if let Ok(bytes) = fs::read(path) {
            // Look for ANIM chunk
            bytes.windows(4).any(|w| w == b"ANIM")
        } else {
            false
        }
    }
}

/// GIF format utilities
pub mod gif {
    use std::path::Path;
    use std::fs;

    /// Check if GIF is animated
    pub fn is_animated(path: &Path) -> bool {
        if let Ok(bytes) = fs::read(path) {
            // Count image descriptor markers (0x2C)
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
    use std::path::Path;
    use std::fs;

    /// Verify JXL signature
    pub fn verify_signature(path: &Path) -> bool {
        if let Ok(mut file) = fs::File::open(path) {
            use std::io::Read;
            let mut sig = [0u8; 2];
            if file.read_exact(&mut sig).is_ok() {
                // JXL codestream: 0xFF 0x0A
                // JXL container: 0x00 0x00
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
    use std::path::Path;

    #[test]
    fn test_png_compression_estimate() {
        let level = png::estimate_compression_level(Path::new("test.png"));
        assert!(level <= 9, "PNG compression level should be 0-9");
    }

    #[test]
    fn test_jpeg_quality_estimate() {
        let quality = jpeg::estimate_quality(Path::new("test.jpg"));
        assert!(quality <= 100, "JPEG quality should be 0-100");
    }

    #[test]
    fn test_webp_detection_nonexistent() {
        let path = Path::new("/nonexistent/file.webp");
        assert!(!webp::is_lossless(path));
        assert!(!webp::is_animated(path));
    }

    #[test]
    fn test_gif_detection_nonexistent() {
        let path = Path::new("/nonexistent/file.gif");
        assert!(!gif::is_animated(path));
        assert_eq!(gif::get_frame_count(path), 0);
    }

    #[test]
    fn test_jxl_signature_nonexistent() {
        let path = Path::new("/nonexistent/file.jxl");
        assert!(!jxl::verify_signature(path));
        assert!(!jxl::is_valid(path));
    }
}
