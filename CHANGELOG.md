# Changelog

All notable changes to Modern Format Boost will be documented in this file.

## [7.2] - 2025-01-18

### 🔥 Quality Verification Fix - Standalone VMAF Integration

#### Problem
MS-SSIM calculation failed when ffmpeg lacks libvmaf support:
```
⚠️⚠️⚠️  ALL QUALITY CALCULATIONS FAILED!  ⚠️⚠️⚠️
- libvmaf not available in ffmpeg
```

#### Solution
Integrated standalone `vmaf` CLI tool (Netflix official) to bypass ffmpeg dependency.

#### Changes
- **New Module**: `vmaf_standalone.rs` - Independent VMAF tool wrapper
- **Modified**: `video_explorer.rs` - Priority: standalone vmaf → ffmpeg libvmaf → SSIM fallback
- **Updated**: `lib.rs` - Export vmaf_standalone module

#### Fallback Chain
1. **Standalone vmaf** (preferred) → MS-SSIM
2. **ffmpeg libvmaf** → MS-SSIM  
3. **ffmpeg ssim** → SSIM All (Y+U+V)
4. **ffmpeg ssim** → SSIM Y only

#### Benefits
- ✅ No ffmpeg recompilation required
- ✅ More reliable MS-SSIM calculation
- ✅ Graceful multi-layer fallback
- ✅ Loud error reporting (no silent failures)

#### Installation
```bash
# macOS
brew install libvmaf

# Verify
vmaf --version
```

#### Testing
```bash
./scripts/e2e_quality_test.sh
./scripts/verify_fix.sh
```

---

## [6.9.17] - 2025-01-18

### 🔥 Critical Fixes - CPU Encoding & GPU Fallback

#### CPU Encoding Reliability
- **Fixed**: Replaced FFmpeg libx265 with x265 CLI tool for better compatibility
- **Problem**: FFmpeg 8.0.1's libx265 fails on GIF files with bgra pixel format
- **Solution**: Three-step encoding process:
  1. FFmpeg decode input → Y4M (raw YUV)
  2. x265 CLI encode Y4M → HEVC bitstream  
  3. FFmpeg mux HEVC + audio → MP4 container
- **Benefits**: Higher reliability, better format support, 0.1 CRF precision

#### GPU Fallback System
- **New**: Automatic CPU fallback when GPU encoding fails
- **Triggers**: GPU boundary verification failures, high CRF encoding failures
- **Logging**: Clear error messages and fallback notifications
- **Example**: `⚠️  GPU encoding failed, falling back to CPU (x265 CLI)`

#### Input Format Compatibility  
- **Fixed**: GIF files with bgra pixel format now supported
- **Auto-conversion**: bgra → yuv420p, removes alpha channel
- **Dimension fix**: Adjusts odd dimensions to even numbers

#### CPU Calibration Improvements
- **Fixed**: CPU calibration now uses x265 CLI instead of libx265
- **Result**: Accurate GPU→CPU CRF mapping with confidence reporting
- **Fallback**: Static offset used when calibration fails (with warning)

#### Error Transparency
- **Principle**: All errors are "loudly reported" (响亮报错)
- **No silent failures**: Every fallback has clear user notification
- **Context**: Detailed error messages with troubleshooting hints

### 🔧 Files Modified
- `shared_utils/src/video_explorer.rs`: GPU fallback logic, x265 CLI integration
- `shared_utils/src/x265_encoder.rs`: Three-step encoding implementation
- Added test scripts: `test_gpu_boundary_fallback.sh`, `test_x265_cli_fix.sh`

### 🧪 Testing
- **Verified**: GIF files with problematic formats now convert successfully
- **Verified**: GPU failures automatically fallback to CPU
- **Verified**: CPU calibration accuracy improved
- **Verified**: All error paths provide clear feedback
- **Verified**: Eliminated "Error splitting the argument list" errors
- **Verified**: x265_encoder.rs compiles without tracing dependency

### Test Results
```bash
✅ CPU calibration: GPU 1020989 → CPU 2902004 (ratio 2.842, offset +2.5)
✅ CPU encoding: Using x265 CLI completed successfully
✅ No parameter errors: "Error splitting the argument list" eliminated
✅ Modified files: video_explorer.rs (fallback) + x265_encoder.rs (tracing removed)
```

---

## [6.9.17] - 2025-01-18 (中文版)

### 🔥 关键修复 - CPU 编码与 GPU 降级

#### CPU 编码可靠性
- **修复**: 使用 x265 CLI 工具替代 FFmpeg libx265，提高兼容性
- **问题**: FFmpeg 8.0.1 的 libx265 在处理 bgra 像素格式的 GIF 文件时失败
- **解决方案**: 三步编码流程：
  1. FFmpeg 解码输入 → Y4M (原始 YUV)
  2. x265 CLI 编码 Y4M → HEVC 比特流
  3. FFmpeg 封装 HEVC + 音频 → MP4 容器
- **优势**: 更高可靠性，更好格式支持，0.1 CRF 精度

#### GPU 降级系统
- **新增**: GPU 编码失败时自动降级到 CPU
- **触发条件**: GPU 边界验证失败，高 CRF 编码失败
- **日志记录**: 清晰的错误信息和降级通知
- **示例**: `⚠️  GPU 编码失败，降级到 CPU (x265 CLI)`

#### 输入格式兼容性
- **修复**: 现在支持带 bgra 像素格式的 GIF 文件
- **自动转换**: bgra → yuv420p，移除 alpha 通道
- **尺寸修复**: 将奇数尺寸调整为偶数

#### CPU 校准改进
- **修复**: CPU 校准现在使用 x265 CLI 而不是 libx265
- **结果**: 准确的 GPU→CPU CRF 映射，带置信度报告
- **降级**: 校准失败时使用静态偏移（带警告）

#### 错误透明化
- **原则**: 所有错误都"响亮报告"（响亮报错）
- **无静默失败**: 每个降级都有清晰的用户通知
- **上下文**: 详细的错误信息和故障排除提示

### 🔧 修改文件
- `shared_utils/src/video_explorer.rs`: GPU 降级逻辑，x265 CLI 集成
- `shared_utils/src/x265_encoder.rs`: 三步编码实现
- 新增测试脚本: `test_gpu_boundary_fallback.sh`, `test_x265_cli_fix.sh`

### 🧪 测试验证
- **已验证**: 有问题格式的 GIF 文件现在可以成功转换
- **已验证**: GPU 失败自动降级到 CPU
- **已验证**: CPU 校准精度提高
- **已验证**: 所有错误路径都提供清晰反馈
- **已验证**: 消除了 "Error splitting the argument list" 错误
- **已验证**: x265_encoder.rs 编译时不再依赖 tracing

### 测试结果
```bash
✅ CPU 校准成功: GPU 1020989 → CPU 2902004 (比率 2.842, 偏移 +2.5)
✅ CPU 编码成功: 使用 x265 CLI 完成编码
✅ 无参数错误: 完全消除 "Error splitting the argument list"
✅ 修改文件: video_explorer.rs (降级机制) + x265_encoder.rs (移除 tracing)
```

---

## [6.9.16] - 2025-12-25

### 🔧 XMP Merge Priority

- **Always try merge first**: ExifTool supports XMP merge for PSD and many other formats
- **Fallback to copy**: Only copy XMP sidecar if merge fails
- **Clear logging**: Shows merge success/failure/fallback status

## [6.9.15] - 2025-12-25

### 🔧 No-Loss Design - XMP Handling for Unsupported Files

- **XMP for unsupported files**: When copying .psd/.txt etc, also copy their XMP sidecars
- **Dual strategy**: Media files → merge XMP; Non-media files → copy XMP sidecar
- **New function**: `copy_xmp_sidecar_if_exists()` handles XMP for non-media files

## [6.9.14] - 2025-12-25

### 🔧 No-Loss Design - Failed Files Fallback

- **Failed files now copied**: When conversion fails, original file is copied to output
- **XMP merged for failed files**: XMP sidecars merged even for failed conversions
- **Build fix**: Added `build.rs` for dynamic Homebrew library path detection (dav1d/libheif)
- **Loud error reporting**: All failures reported with clear messages

## [6.9.13] - 2025-12-25

### 🔧 No-Loss Design - Core Implementation

- **Moved to core program**: Copy unsupported files + verification now in Rust code
- **New module**: `shared_utils/file_copier.rs` - handles file copying and verification
- **Functions**: `copy_unsupported_files()`, `count_all_files()`, `verify_output_completeness()`
- **Shell script simplified**: Only UI/wrapper, logic moved to main programs
- **Verification**: Automatic output completeness check after directory processing

## [6.9.12] - 2025-12-25

### 🔧 Format Support Enhancement + Validation Mechanism

- **Added image formats**: `.jpe`, `.jfif` (JPEG variants)
- **Added video formats**: `.wmv`, `.flv`
- **Output integrity verification**: Compares input/output file counts after processing
  - Reports missing files with clear warnings
  - Detects unsupported formats (`.psd`, RAW files) and reports them
- **Updated**: `imgquality_hevc`, `imgquality_av1`, `shared_utils/batch.rs`, `drag_and_drop_processor.sh`

## [6.9.11] - 2025-12-25

### 🔧 XMP Sidecar Merge for Skipped Files

- **Fixed: Skipped files now have XMP sidecars merged**
  - Previously, files skipped (short animations, modern formats, quality failures) were copied without XMP metadata
  - Now `merge_xmp_for_copied_file()` is called after copying to merge XMP sidecars
  - Affects: short animations (<3s), modern lossy formats (WebP/AVIF/HEIC), quality validation failures
  - Added new helper function `shared_utils::merge_xmp_for_copied_file()` for reuse

## [6.9.10] - 2025-12-25

### 🔧 XMP Sidecar Merge Fix

- **Fixed false-positive XMP merge failures for JXL files**
  - ExifTool outputs `[minor] Will wrap JXL codestream in ISO BMFF container` as informational message
  - Previously this was incorrectly treated as an error
  - PNG→JXL conversions with XMP sidecars now report `✅ XMP sidecar merged successfully`

### 🔧 Quality Validation Error Message Fix

- **Fixed misleading error messages when video stream compression fails**
  - Previously showed `SSIM X < Y` even when SSIM was actually higher than threshold
  - Root cause: `quality_passed=false` due to video stream not compressing, not SSIM failure
  - Now correctly shows `VIDEO STREAM COMPRESSION FAILED` with size details
  - Accurate distinction between: compression failure / SSIM calculation failure / SSIM below threshold

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
