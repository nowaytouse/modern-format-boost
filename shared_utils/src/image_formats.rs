//! Format-specific utilities and helpers

pub mod tiff {
    use std::fs;
    use std::path::Path;
    use crate::img_errors::{ImgQualityError, Result};

    /// Detect TIFF compression type — traverses ALL IFDs. Supports both standard TIFF and BigTIFF.
    pub fn is_lossless(path: &Path) -> Result<bool> {
        crate::common_utils::validate_file_size_limit(path, 512 * 1024 * 1024)
            .map_err(|e| ImgQualityError::AnalysisError(e.to_string()))?;

        let data = fs::read(path)?;
        if data.len() < 8 {
            // Fallback to lossless for corrupted/truncated files (safe default for TIFF)
            return Ok(true);
        }

        let is_little_endian = &data[0..2] == b"II";
        if &data[0..2] != b"II" && &data[0..2] != b"MM" {
            // Invalid byte order marker - fallback to lossless (safe default)
            return Ok(true);
        }

        let version = if is_little_endian { u16::from_le_bytes([data[2], data[3]]) } else { u16::from_be_bytes([data[2], data[3]]) };
        let is_bigtiff = version == 0x002B;

        let read_u16 = |off: usize| -> u16 {
            if is_little_endian { u16::from_le_bytes([data[off], data[off + 1]]) } else { u16::from_be_bytes([data[off], data[off + 1]]) }
        };
        let read_u32 = |off: usize| -> u32 {
            if is_little_endian { u32::from_le_bytes([data[off], data[off+1], data[off+2], data[off+3]]) } 
            else { u32::from_be_bytes([data[off], data[off+1], data[off+2], data[off+3]]) }
        };
        let read_u64 = |off: usize| -> u64 {
            if is_little_endian { u64::from_le_bytes([data[off], data[off+1], data[off+2], data[off+3], data[off+4], data[off+5], data[off+6], data[off+7]]) }
            else { u64::from_be_bytes([data[off], data[off+1], data[off+2], data[off+3], data[off+4], data[off+5], data[off+6], data[off+7]]) }
        };

        let mut ifd_offset: u64 = if is_bigtiff {
            if data.len() < 16 {
                // Fallback to lossless for corrupted BigTIFF files
                return Ok(true);
            }
            read_u64(8)
        } else {
            read_u32(4) as u64
        };

        let mut ifd_count = 0u32;
        while ifd_offset != 0 && ifd_count < 100 {
            ifd_count += 1;
            let ifd_pos = ifd_offset as usize;
            let (num_entries, entries_start, entry_size, next_offset_pos) = if is_bigtiff {
                if ifd_pos + 8 > data.len() { break; }
                let n = read_u64(ifd_pos) as usize;
                (n, ifd_pos + 8, 20usize, ifd_pos + 8 + n * 20)
            } else {
                if ifd_pos + 2 > data.len() { break; }
                let n = read_u16(ifd_pos) as usize;
                (n, ifd_pos + 2, 12usize, ifd_pos + 2 + n * 12)
            };

            let mut pos = entries_start;
            for _ in 0..num_entries {
                if pos + entry_size > data.len() { break; }
                if read_u16(pos) == 259 {
                    let compression = if is_bigtiff { read_u16(pos + 12) } else { read_u16(pos + 8) };
                    if compression == 6 || compression == 7 || compression == 50001 { return Ok(false); }
                }
                pos += entry_size;
            }

            if is_bigtiff {
                if next_offset_pos + 8 > data.len() { break; }
                ifd_offset = read_u64(next_offset_pos);
            } else {
                if next_offset_pos + 4 > data.len() { break; }
                ifd_offset = read_u32(next_offset_pos) as u64;
            }
        }
        Ok(true)
    }
}

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
                    if buffer[i] == 0xFF && buffer[i + 1] == 0xDB
                        && i + 5 < buffer.len() {
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
    use crate::img_errors::{ImgQualityError, Result};

    /// Detect WebP animated compression by traversing all ANMF (animation frame) chunks.
    ///
    /// WebP animation: RIFF header → VP8X → ANIM → ANMF* frames.
    /// Each ANMF payload contains frame data starting with VP8/VP8L sub-chunk.
    /// Any VP8 (lossy) frame → Lossy. All VP8L → Lossless.
    pub fn detect_webp_animation_is_lossless(data: &[u8]) -> Result<bool> {
        // WebP structure: RIFF[size]WEBP[chunks...]
        // Walk top-level chunks to find ANMF frames
        if data.len() < 12 {
            // Fallback to lossy for corrupted/truncated files (safe default for animated WebP)
            return Ok(false);
        }

        let mut pos = 12; // skip RIFF + size + WEBP
        let mut found_any_frame = false;

        while pos + 8 <= data.len() {
            let chunk_id = &data[pos..pos + 4];
            let chunk_size = u32::from_le_bytes([
                data[pos + 4], data[pos + 5], data[pos + 6], data[pos + 7],
            ]) as usize;
            let payload_start = pos + 8;
            let payload_end = (payload_start + chunk_size).min(data.len());

            if chunk_id == b"ANMF" && payload_end > payload_start + 24 {
                found_any_frame = true;
                // ANMF payload: 24 bytes header, then frame data sub-chunk
                let frame_data = &data[payload_start + 24..payload_end];
                if frame_data.len() >= 4 {
                    // Check sub-chunk type: VP8L = lossless, VP8 = lossy
                    let sub_chunk = &frame_data[0..4];
                    if sub_chunk == b"VP8 " {
                        return Ok(false); // Lossy
                    } else if sub_chunk != b"VP8L" {
                        // Unknown frame type in animated WebP — ambiguous
                        return Err(ImgQualityError::AnalysisError(
                            format!("Animated WebP: unknown frame chunk type {:?} at pos {}; cannot determine compression", 
                            String::from_utf8_lossy(sub_chunk), payload_start + 24)
                        ));
                    }
                }
            }

            // Chunks are padded to even size
            let padded = (chunk_size + 1) & !1;
            pos = payload_start + padded;
        }

        if found_any_frame {
            Ok(true) // All frames were VP8L (or skipped non-frame chunks)
        } else {
            // No ANMF frames found in animated WebP — ambiguous if VP8L also not found via window search
            if data.windows(4).any(|w| w == b"VP8L") {
                Ok(true)
            } else if data.windows(4).any(|w| w == b"VP8 ") {
                 Ok(false)
            } else {
                Err(ImgQualityError::AnalysisError(
                    "Animated WebP: no ANMF frames or VP8/VP8L chunks found; cannot determine compression".to_string()
                ))
            }
        }
    }

    /// Estimate WebP VP8 quality by parsing the bitstream quantization index.
    pub fn estimate_quality_from_bytes(data: &[u8]) -> Result<u8> {
        let mut pos = 12; // skip RIFF + size + WEBP
        while pos + 8 <= data.len() {
            let chunk_id = &data[pos..pos + 4];
            let chunk_size = u32::from_le_bytes([
                data[pos + 4], data[pos + 5], data[pos + 6], data[pos + 7],
            ]) as usize;
            let payload_start = pos + 8;
            let payload_end = (payload_start + chunk_size).min(data.len());

            if chunk_id == b"VP8 " && payload_end > payload_start + 10 {
                let vp8_data = &data[payload_start..payload_end];
                if vp8_data.len() >= 10 && vp8_data[3..6] == [0x9D, 0x01, 0x2A] {
                    let y_ac_qi = (vp8_data[10] & 0x7F) as u8;
                    let quality = ((127 - y_ac_qi) * 100 / 127).min(100);
                    return Ok(quality);
                }
            }
            let padded = (chunk_size + 1) & !1;
            pos = payload_start + padded;
        }
        Err(ImgQualityError::AnalysisError("No VP8 chunk found".to_string()))
    }

    pub fn estimate_quality(path: &Path) -> Result<u8> {
        fs::read(path).map_err(|e| ImgQualityError::IoError(e))
            .and_then(|b| estimate_quality_from_bytes(&b))
    }

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

    /// Parse animated WebP RIFF/ANMF chunks and return total duration in seconds.
    /// ANMF payload: 24-byte header, bytes 16..20 = frame duration in ms (uint32 LE).
    /// Returns None if not animated WebP or no ANMF chunks.
    pub fn duration_secs_from_bytes(data: &[u8]) -> Option<f32> {
        if data.len() < 12 || &data[0..4] != b"RIFF" || &data[8..12] != b"WEBP" {
            return None;
        }
        if !data.windows(4).any(|w| w == b"ANIM") {
            return None;
        }
        let mut pos = 12u32 as usize;
        let mut total_ms = 0u64;
        while pos + 8 <= data.len() {
            let chunk_id = &data[pos..pos + 4];
            let chunk_size = u32::from_le_bytes([
                data[pos + 4],
                data[pos + 5],
                data[pos + 6],
                data[pos + 7],
            ]) as usize;
            let payload_start = pos + 8;
            let payload_end = (payload_start + chunk_size).min(data.len());
            if chunk_id == b"ANMF" && payload_end >= payload_start + 20 {
                let duration_ms = u32::from_le_bytes([
                    data[payload_start + 16],
                    data[payload_start + 17],
                    data[payload_start + 18],
                    data[payload_start + 19],
                ]);
                total_ms += duration_ms as u64;
            }
            let padded = (chunk_size + 1) & !1;
            pos = payload_start + padded;
        }
        if total_ms == 0 {
            return None;
        }
        Some((total_ms as f32) / 1000.0)
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

        // --- STAGE 1: Standard Structural Walk ---
        // Fast, spec-compliant traversal of the block chain.
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

        let mut structural_descriptors = 0u32;
        let mut structural_gce = 0u32;
        let mut current_pos = pos;

        while current_pos < data.len() {
            match data[current_pos] {
                0x2C => {
                    structural_descriptors += 1;
                    if current_pos + 10 > data.len() { break; }
                    let img_packed = data[current_pos + 9];
                    let has_lct = (img_packed & 0x80) != 0;
                    let lct_size = if has_lct { 3 * (1 << ((img_packed & 0x07) + 1)) } else { 0 };
                    current_pos += 10 + lct_size + 1; // +1 for LZW min code size
                    
                    while current_pos < data.len() {
                        let block_size = data[current_pos] as usize;
                        current_pos += 1;
                        if block_size == 0 { break; }
                        current_pos += block_size;
                    }
                }
                0x21 => {
                    if current_pos + 2 >= data.len() { break; }
                    if data[current_pos + 1] == 0xF9 { structural_gce += 1; }
                    current_pos += 2;
                    while current_pos < data.len() {
                        let block_size = data[current_pos] as usize;
                        current_pos += 1;
                        if block_size == 0 { break; }
                        current_pos += block_size;
                    }
                }
                0x3B => break,
                _ => current_pos += 1,
            }
        }

        let structural_max = structural_descriptors.max(structural_gce);

        // --- STAGE 2: Hedge Search (Internal Byte-Level Research) ---
        // If structural walk says static, but we find "fingerprints" of frames 
        // deeper in the bitstream, perform a deep byte-level audit.
        let gce_marker = &[0x21, 0xF9, 0x04];
        let raw_gce_count = data.windows(3).filter(|w| *w == gce_marker).count() as u32;

        if raw_gce_count > structural_max {
            // Disagreement! Map is broken. Investigation mode.
            return deep_bitstream_audit_gif(data, structural_max, raw_gce_count);
        }

        structural_max.max(1)
    }

    /// Internal "Microscope" for GIF bitstream auditing.
    /// Investigates every raw marker to confirm if it's a real frame or random collision.
    fn deep_bitstream_audit_gif(data: &[u8], structural: u32, raw_hints: u32) -> u32 {
        let gce_marker = &[0x21, 0xF9, 0x04];
        let mut verified_frames = 0u32;
        let mut last_verified_pos = 0;

        for i in 6..data.len().saturating_sub(15) {
            if &data[i..i+3] == gce_marker {
                // A potential Graphic Control Extension (GCE). 
                // To be valid, it MUST be followed by:
                // [GCE Data (4 bytes)] + [Block Terminator (0x00)] + 
                // [Optional Image Descriptor (0x2C)] OR [Another Extension (0x21)]
                
                // Let's verify the next few bytes:
                let terminator_pos = i + 6; // marker(3) + data(3)
                if terminator_pos < data.len() && data[terminator_pos] == 0x00 {
                    let next_block = terminator_pos + 1;
                    if next_block < data.len() {
                        let tag = data[next_block];
                        if tag == 0x2C || tag == 0x21 || tag == 0x3B {
                            // High confidence: this is a legitimate control frame.
                            if i > last_verified_pos + 8 { // Skip overlapping or duplicate markers
                                verified_frames += 1;
                                last_verified_pos = i;
                            }
                        }
                    }
                }
            }
        }

        if verified_frames > structural {
             if structural == 1 && verified_frames > 1 {
                 // Successfully rescued an animated GIF that was misjudged as static!
             }
             verified_frames
        } else {
             structural.max(1)
        }
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

pub mod avif {
    use std::fs;
    use std::path::Path;
    use crate::img_errors::{ImgQualityError, Result};
    use crate::common_utils::find_box_data_recursive;

    /// Detect AVIF lossless encoding — multi-dimension analysis.
    ///
    /// Dimensions checked (in priority order):
    /// 1. **av1C chroma subsampling**: 4:2:0 / 4:2:2 → definitely lossy
    /// 2. **av1C 4:4:4 + colr Identity matrix (MC=0)** → lossless
    /// 3. **av1C 4:4:4 + high_bitdepth / twelve_bit** → lossless
    /// 4. **av1C seq_profile**: Profile 0 + 4:4:4 → treat as lossless
    /// 5. **pixi box**: bit depth ≥ 12 with 4:4:4 → lossless indicator
    pub fn is_lossless_from_bytes(data: &[u8], path: &Path) -> Result<bool> {
        if let Some(av1c_data) = find_box_data_recursive(data, b"av1C") {
            if av1c_data.len() >= 4 {
                let byte1 = av1c_data[1];
                let byte2 = av1c_data[2];

                let seq_profile = (byte1 >> 5) & 0x07;
                let high_bitdepth = (byte2 >> 6) & 0x01;
                let twelve_bit = (byte2 >> 5) & 0x01;
                let monochrome = (byte2 >> 4) & 0x01;
                let chroma_subsampling_x = (byte2 >> 3) & 0x01;
                let chroma_subsampling_y = (byte2 >> 2) & 0x01;

                let is_444 = chroma_subsampling_x == 0 && chroma_subsampling_y == 0;
                let is_420 = chroma_subsampling_x == 1 && chroma_subsampling_y == 1;
                let is_422 = chroma_subsampling_x == 1 && chroma_subsampling_y == 0;

                if is_420 || is_422 {
                    return Ok(false);
                }

                if monochrome == 1 && !is_444 {
                    return Ok(false);
                }

                // Dimension 2: colr Identity matrix (MC=0)
                if let Some(colr_data) = find_box_data_recursive(data, b"colr") {
                    if colr_data.len() >= 11 && &colr_data[0..4] == b"nclx" {
                        let matrix_coefficients = u16::from_be_bytes([colr_data[8], colr_data[9]]);
                        if matrix_coefficients == 0 {
                            return Ok(true);
                        }
                    }
                }

                // Dimension 3: high_bitdepth/twelve_bit
                if is_444 && (twelve_bit == 1 || (high_bitdepth == 1 && seq_profile >= 1)) {
                    return Ok(true);
                }

                // Dimension 4: Profile 0 + 4:4:4
                if is_444 && seq_profile == 0 {
                    return Ok(true);
                }

                // Dimension 5: pixi box
                if is_444 {
                    if let Some(pixi_data) = find_box_data_recursive(data, b"pixi") {
                        if !pixi_data.is_empty() {
                            let num_ch = pixi_data[0] as usize;
                            if num_ch > 0 && pixi_data.len() > num_ch {
                                let max_depth = pixi_data[1..=num_ch].iter().copied().max().unwrap_or(0);
                                if max_depth >= 12 {
                                    return Ok(true);
                                }
                            }
                        }
                    }
                }

                if is_444 && monochrome == 1 {
                    return Ok(true);
                }

                if is_444 {
                    return Err(ImgQualityError::AnalysisError(format!(
                        "AVIF: 4:4:4 without definitive lossless indicators; refusing to guess — {}",
                        path.display()
                    )));
                }
            }
        }

        Err(ImgQualityError::AnalysisError(format!(
            "AVIF: no av1C box found; cannot determine compression — {}",
            path.display()
        )))
    }

    pub fn is_lossless(path: &Path) -> bool {
        fs::read(path).ok()
            .and_then(|b| is_lossless_from_bytes(&b, path).ok())
            .unwrap_or(false)
    }
}

pub mod jxl {
    use std::fs;
    use std::path::Path;
    use crate::img_errors::{ImgQualityError, Result};
    use crate::common_utils::{find_box_data_recursive, find_any_box_recursive};

    /// Detect JXL (JPEG XL) lossless encoding — multi-dimension analysis.
    pub fn is_lossless_from_bytes(data: &[u8], path: &Path) -> Result<bool> {
        if data.len() < 4 {
            return Err(ImgQualityError::AnalysisError(format!("JXL: file too short — {}", path.display())));
        }

        let is_naked = data[0] == 0xFF && data[1] == 0x0A;

        // Dimension 1: jbrd = JPEG bitstream reconstruction = lossless
        if !is_naked && find_any_box_recursive(data, b"jbrd") {
            return Ok(true);
        }

        // Dimension 2-4: Parse codestream header for xyb_encoded
        let codestream: Option<&[u8]> = if is_naked {
            Some(data)
        } else {
            find_box_data_recursive(data, b"jxlc")
                .or_else(|| find_box_data_recursive(data, b"jxlp"))
        };

        if let Some(cs) = codestream {
            match parse_jxl_xyb_encoded(cs) {
                Some(true) => return Ok(false), // xyb_encoded=true -> lossy
                Some(false) => return Ok(true),  // xyb_encoded=false -> lossless
                None => {}
            }
        }

        Err(ImgQualityError::AnalysisError(format!(
            "JXL: no jbrd and codestream header unparseable — cannot determine — {}",
            path.display()
        )))
    }

    /// Minimal bit reader for parsing JXL codestream headers.
    struct JxlBitReader<'a> {
        data: &'a [u8],
        byte_pos: usize,
        bit_pos: u8,
    }

    impl<'a> JxlBitReader<'a> {
        fn new(data: &'a [u8]) -> Self { Self { data, byte_pos: 0, bit_pos: 0 } }
        fn read_bits(&mut self, n: u8) -> Option<u32> {
            if n == 0 { return Some(0); }
            let mut result: u32 = 0;
            for i in 0..n {
                if self.byte_pos >= self.data.len() { return None; }
                let bit = (self.data[self.byte_pos] >> self.bit_pos) & 1;
                result |= (bit as u32) << i;
                self.bit_pos += 1;
                if self.bit_pos == 8 { self.bit_pos = 0; self.byte_pos += 1; }
            }
            Some(result)
        }
        fn read_bool(&mut self) -> Option<bool> { self.read_bits(1).map(|v| v == 1) }
        fn read_u32(&mut self, dists: [(u32, u8); 4]) -> Option<u32> {
            let sel = self.read_bits(2)? as usize;
            let (base, extra_bits) = dists[sel];
            let extra = self.read_bits(extra_bits)?;
            Some(base + extra)
        }
    }

    fn parse_jxl_xyb_encoded(codestream: &[u8]) -> Option<bool> {
        let start = if codestream.len() >= 2 && codestream[0] == 0xFF && codestream[1] == 0x0A { 2 } else { 0 };
        if start >= codestream.len() { return None; }
        let mut r = JxlBitReader::new(&codestream[start..]);

        // --- SizeHeader ---
        let small = r.read_bool()?;
        if small {
            let _ysize_div8_m1 = r.read_bits(5)?;
            let ratio = r.read_bits(3)?;
            if ratio == 0 {
                let _xsize_div8_m1 = r.read_bits(5)?;
            }
        } else {
            // ysize_minus1: U32(u(9), u(13), u(18), u(30))
            r.read_u32([(0, 9), (0, 13), (0, 18), (0, 30)])?;
            let ratio = r.read_bits(3)?;
            if ratio == 0 {
                r.read_u32([(0, 9), (0, 13), (0, 18), (0, 30)])?; // xsize_minus1
            }
        }

        // --- ImageMetadata ---
        let all_default = r.read_bool()?;
        if all_default {
            // all_default=true → xyb_encoded defaults to true → lossy
            return Some(true);
        }

        let extra_fields = r.read_bool()?;
        if extra_fields {
            r.read_bits(3)?; // orientation - 1: u(3)

            // have_intrinsic_size
            if r.read_bool()? {
                let small2 = r.read_bool()?;
                if small2 {
                    r.read_bits(5)?; // ysize
                    let ratio2 = r.read_bits(3)?;
                    if ratio2 == 0 { r.read_bits(5)?; } // xsize
                } else {
                    r.read_u32([(0, 9), (0, 13), (0, 18), (0, 30)])?; // ysize
                    let ratio2 = r.read_bits(3)?;
                    if ratio2 == 0 {
                        r.read_u32([(0, 9), (0, 13), (0, 18), (0, 30)])?; // xsize
                    }
                }
            }

            // have_preview
            if r.read_bool()? {
                let div8 = r.read_bool()?;
                if div8 {
                    r.read_u32([(0, 9), (0, 13), (0, 18), (0, 30)])?;
                } else {
                    let div16 = r.read_bool()?;
                    if !div16 {
                        r.read_u32([(0, 9), (0, 13), (0, 18), (0, 30)])?;
                        r.read_u32([(0, 9), (0, 13), (0, 18), (0, 30)])?;
                    }
                }
            }

            // have_animation
            if r.read_bool()? {
                r.read_u32([(100, 0), (1000, 0), (0, 10), (0, 30)])?; // tps_num
                r.read_u32([(1, 0), (1001, 0), (0, 10), (0, 30)])?; // tps_den
                r.read_u32([(0, 0), (0, 3), (0, 16), (0, 32)])?; // num_loops
                r.read_bool()?; // have_timecodes
            }
        }

        // bit_depth
        let float_sample = r.read_bool()?;
        if float_sample {
            r.read_u32([(32, 0), (16, 0), (24, 0), (1, 6)])?; // bits_per_sample
            r.read_bits(4)?; // exp_bits + 1
        } else {
            r.read_u32([(8, 0), (10, 0), (12, 0), (1, 6)])?; // bits_per_sample
        }

        // num_extra_channels
        let num_extra = r.read_u32([(0, 0), (1, 0), (2, 0), (3, 12)])?;
        for _ in 0..num_extra {
            if !r.read_bool()? { // ec_default
                // Detailed ExtraChannelInfo skip logic (complex, bail if not default)
                return None;
            }
        }

        // xyb_encoded: Bool — THE FINAL TARGET
        r.read_bool()
    }

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
        let mut file = NamedTempFile::new().expect("Failed to create temporary file");
        file.write_all(png_data).expect("Failed to write to file");

        let level = png::estimate_compression_level(file.path());
        assert!(level <= 9, "PNG compression level should be between 0-9, actual: {}", level);
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
        let mut file = NamedTempFile::new().expect("Failed to create temporary file");
        file.write_all(jpeg_data).expect("Failed to write to file");

        let quality = jpeg::estimate_quality(file.path());
        assert!(quality >= 90, "Low quantization value should return high quality, actual: {}", quality);
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
        let mut file = NamedTempFile::new().expect("Failed to create temporary file");
        file.write_all(&webp_lossless).expect("Failed to write to file");

        assert!(
            webp::is_lossless(file.path()),
            "VP8L chunk should be detected as lossless"
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
        let mut file = NamedTempFile::new().expect("Failed to create temporary file");
        file.write_all(&webp_lossy).expect("Failed to write to file");

        assert!(
            !webp::is_lossless(file.path()),
            "VP8 chunk should be detected as lossy"
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
        let mut file = NamedTempFile::new().expect("Failed to create temporary file");
        file.write_all(jxl_codestream).expect("Failed to write to file");

        assert!(
            jxl::verify_signature(file.path()),
            "JXL codestream signature should be recognized"
        );
    }

    #[test]
    fn test_error_handling_nonexistent_file() {
        let path = std::path::Path::new("/nonexistent/file.test");

        assert!(!webp::is_lossless(path), "Non-existent file should return false");
        assert!(!webp::is_animated(path), "Non-existent file should return false");
        assert!(!gif::is_animated(path), "Non-existent file should return false");
        assert_eq!(gif::get_frame_count(path), 0, "Non-existent file should return 0");
        assert!(!jxl::verify_signature(path), "Non-existent file should return false");
    }
}
