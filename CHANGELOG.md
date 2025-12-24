# Changelog

All notable changes to Modern Format Boost will be documented in this file.

## [6.9.10] - 2025-12-25

### 🔧 XMP Sidecar Merge Fix

- **Fixed false-positive XMP merge failures for JXL files**
  - ExifTool outputs `[minor] Will wrap JXL codestream in ISO BMFF container` as informational message
  - Previously this was incorrectly treated as an error, causing `⚠️ Failed to merge XMP sidecar` warnings
  - XMP data was actually written successfully - now correctly recognized as success
  - PNG→JXL conversions with XMP sidecars now report `✅ XMP sidecar merged successfully`

## [6.5.2] - 2025-12-20

### 🔧 Adjacent Directory Mode Fix

- **Copy original when skipped**: Fixed issue where skipped files were missing from output directory
  - Short animations (< 3s) now copied to output directory instead of being silently skipped
  - Videos that cannot be compressed (VP8, already optimized) now copied to output directory
  - Modern formats (WebP, AVIF, HEIC) skipped but copied to preserve directory completeness
  
- **Quality Protection with Copy**: When video stream compression fails:
  - Original file protected (not replaced with larger file)
  - Original copied to output directory in adjacent mode
  - Clear logging with `📋 Copied original to output dir` message

### 🎯 VP8 Source Compression Fix

- **Added VP8 codec detection**: VP8 sources now properly identified with efficiency factor 0.85
  - Previously VP8 was treated as `Unknown` (efficiency 1.0), causing CRF underestimation
  - VP8 → HEVC conversion now starts with more appropriate (higher) CRF values
  - Improved chance of achieving compression for VP8 sources

### 📊 GPU Coarse Search Range Expansion

- **Expanded GPU max CRF**: 40 → 48
  - GPU phase now explores a wider CRF range
  - Better compression boundary detection for already-efficient codecs (VP8, VP9)
  - Reduces "GPU didn't find compression boundary" failures

### 🎬 Comprehensive Codec Support

- **Added 15+ legacy and lossless codecs** to prevent "Unknown codec" efficiency mismatches:
  - **Legacy Video**: MPEG-4 (XviD/DivX), MPEG-2 (DVD), MPEG-1 (VCD), WMV/VC-1, Theora, RealVideo, Flash Video
  - **Lossless Video**: RawVideo, Lagarith, MagicYUV
  - **Image Formats**: BMP, TIFF
  
- **Efficiency factors calibrated for all codecs**:
  | Codec | Efficiency Factor | Notes |
  |-------|------------------|-------|
  | MPEG-4 | 1.3 | ~30% less efficient than H.264 |
  | MPEG-2 | 1.8 | ~80% less efficient (DVD era) |
  | MPEG-1 | 2.5 | Very old (VCD era) |
  | WMV/VC-1 | 1.1 | Similar to H.264 |
  | Theora | 1.2 | Similar to MPEG-4 ASP |
  | RealVideo | 2.0 | Ancient, very inefficient |
  | Flash Video | 1.5 | FLV1/VP6 legacy |

---

## [6.9.1] - 2025-12-19

### 🎵 Smart Audio Transcoding Strategy

- **Quality-aware audio handling**: Intelligent codec selection based on source quality
  - High-quality/Lossless (>256kbps, FLAC, PCM) → ALAC (Apple Lossless)
  - Medium-quality (128-256kbps) → AAC 256kbps
  - Low-quality (<128kbps) → AAC 192kbps
  - Compatible codecs → Direct copy (`-c:a copy`)

- **FFprobe audio detection**: New fields for quality analysis
  - `audio_bit_rate`: Audio bitrate in bps
  - `audio_sample_rate`: Sample rate in Hz
  - `audio_channels`: Channel count

- **VP9/WebM compatibility fix**: Opus/Vorbis audio now properly transcoded for MOV/MP4 containers

### 📝 Documentation & Cleanup

- Merged CHANGELOG files (removed CHANGELOG_v5.5.md)
- Updated README to v6.9.1 with all recent features
- Removed sensitive data (user paths) from Cargo.toml and .gitignore

---

## [6.9.0] - 2025-12-18

### 🔥 Iteration Optimization

- **Adaptive Zero-gains Threshold**: CRF range < 20 scales threshold (factor 0.5-1.0), minimum 3
- **VP9 Duration Detection**: 3-method detection with loud reporting
- **Property-Based Tests**: 3 new proptest properties for correctness validation

---

## [6.8.0] - 2025-12-17

### 🎯 Evaluation Consistency

- Unified SSIM threshold comparison across all modules
- Type-safe wrappers for CRF, SSIM, FileSize, Iteration
- Float comparison utilities with domain-specific precision

---

## [6.7.0] - 2025-12-16

### 📦 Container Overhead Fix

- Pure media stream size comparison (excludes container overhead)
- Accurate compression ratio calculation
- Stream size extraction via ffprobe

---

## [6.6.0] - 2025-12-15

### 🗄️ Unified Cache Refactor

- LRU cache with configurable capacity
- JSON persistence for cache data
- Memory-safe long-running operations

---

## [6.5.0] - 2025-12-14

### 🔄 Explore Strategy Pattern

- Modular search strategies (Binary, Golden Section, Linear)
- CrfCache for efficient result storage
- Strategy selection based on search space

---

## [6.4.0] - 2025-12-13

### 📊 Dynamic Metadata Margin

- Adaptive metadata margin calculation
- Small file precision handling
- Pure video size comparison

---

## [6.2.0] - 2025-12-12

### 🔥 Ultimate Explore Mode

- SSIM saturation detection (Domain Wall)
- Adaptive wall-hit limits based on CRF range
- Long video optimization strategies

---

## [0.4.0] - 2025-12-11 (v4.9)

### Performance Optimization

- Smart final encoding (avoid redundant re-encoding)
- Unified caching mechanism
- Real-time progress output

---

## [0.3.0] - 2025-12-10

### Apple Compatibility Mode

- `--apple-compat` flag for AV1/VP9 → HEVC conversion
- Animated WebP → HEVC MP4 support

---

## [0.2.0] - 2025-12-09

### Code Quality

- Zero Clippy warnings
- PNG/JPEG quality detection
- XMP metadata merge

---

## [0.1.0] - Initial Release

- Core video/image conversion tools
- SSIM validation system
- GPU hardware acceleration
