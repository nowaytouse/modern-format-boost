# 🚀 Modern Format Boost [![GitHub Stars](https://img.shields.io/github/stars/nowaytouse/modern-format-boost.svg?style=social)](https://github.com/nowaytouse/modern-format-boost/stargazers) [![GitHub Forks](https://img.shields.io/github/forks/nowaytouse/modern-format-boost.svg?style=social)](https://github.com/nowaytouse/modern-format-boost/network/members)

[![Version](https://img.shields.io/badge/version-0.10.26-blue.svg)](https://github.com/nowaytouse/modern-format-boost/releases)
[![License](https://img.shields.io/badge/license-MIT-green.svg)](LICENSE)
[![Platform](https://img.shields.io/badge/platform-macOS%20%7C%20Linux-lightgrey.svg)](https://github.com/nowaytouse/modern-format-boost)
[![Architecture](https://img.shields.io/badge/arch-Rust%20%7C%20Rayon%20%7C%20FFmpeg-orange.svg)](https://github.com/nowaytouse/modern-format-boost)
[![Repo Size](https://img.shields.io/github/repo-size/nowaytouse/modern-format-boost.svg)](https://github.com/nowaytouse/modern-format-boost)
[![Last Commit](https://img.shields.io/github/last-commit/nowaytouse/modern-format-boost.svg)](https://github.com/nowaytouse/modern-format-boost/commits/main)

**The Ultimate Media Optimizer & Repair Tool for the Apple Ecosystem. 专为苹果生态打造的终极媒体优化与修复工具。**

[English](#introduction-en) | [中文文档](#introduction-cn)

---

<a id="introduction-en"></a>

### 📖 Introduction

**Modern Format Boost** is a high-performance, concurrent media optimization engine written in **Rust**. It is designed to modernize legacy archives (JPEG/H.264) into next-generation formats (JXL/HEVC) while strictly adhering to **mathematical lossless** standards for images and **perceptual fidelity** for videos.

Unlike simple wrapper scripts, this tool implements a **cybernetic feedback loop** that analyzes content complexity, optimizes encoding parameters in real-time, and verifies integrity using industrial metrics (SSIM/MS-SSIM).

> [!IMPORTANT]
> **Disclaimer & Current Status:**
> 1. **Tool Maturity**: The HEVC-based tools (`img-hevc`, `vid-hevc`) are significantly more mature and stable than the AV1 variants (`img-av1`, `vid-av1`).
> 2. **Testing Scope**: While the engine has been validated against tens of thousands of files through rigorous debugging and iterative patching, the test dataset is restricted to the author's local environment.
> 3. **Platform Support**: This project is currently optimized and verified for **macOS only**.

### ⚙️ How It Works (Technical Architecture)

#### 1. Image Modernization Engine (`img-hevc`)
The image pipeline operates as a **Format-Aware Routing State Machine**. It inspects the binary signature (magic bytes) and metadata of every file to determine the optimal transformation path.

**Decision Matrix & Conversion Strategy:**

| Input Format | Variants & Detection | Compression | Operation | Target Format | Container | Apple Compat | Technical Principle |
| :--- | :--- | :--- | :--- | :--- | :--- | :--- | :--- |
| **JPEG** | All variants | Lossy (always) | **Reconstruction** | **JXL** (Lossless) | .jxl | Same | **DCT Transcoding**. Parses raw DCT coefficients and maps them directly to JXL's `varDCT`. **Reversible & Bit-exact.** |
| **PNG** | Truecolor/16-bit | Lossless | **Entropic Coding** | **JXL** (Lossless) | .jxl | Same | **Modular Mode**. Utilizes Delta Palettes, Squeeze (Haar transform), and MA-trees. ~40% density improvement. |
| **PNG** | Indexed/Palette | Lossy (quantized) | **Lossy Transcode** | **JXL** (d=1.0) | .jxl | Same | **Multi-factor detection** (palette structure, tRNS, tool signatures, dithering, entropy). Auto-skip if output larger. |
| **WebP** | VP8L chunk | Lossless | **Entropic Coding** | **JXL** (Lossless) | .jxl | Same | **Lossless extraction**. VP8L→JXL modular mode with full quality preservation. |
| **WebP** | VP8 chunk | Lossy | **Hard Skip** | *(Original)* | .webp | Same | **Signal Preservation**. Re-quantizing artifacts mathematically destroys SNR. |
| **WebP** | ANIM chunks | Animated | **Convert** | **HEVC/AV1** (Video) | .mp4 | Same | **Temporal Compress**. ANMF frames→P/B vectors, 90%+ size reduction. |
| **AVIF** | 4:2:0/4:2:2 chroma | Lossy | **Hard Skip** | *(Original)* | .avif | Same | **Signal Preservation**. AVIF already modern; re-encoding unnecessary. |
| **AVIF** | 4:4:4 + high bit depth | Lossless | **Entropic Coding** | **JXL** (Lossless) | .jxl | Same | **Lossless extraction**. AVIF→JXL modular mode for archival. |
| **HEIC/HEIF** | All variants | Any | **Passthrough** | *(Original)* | .heic/.heif | Same | **Zero-Copy**. Native Apple ecosystem format. |
| **Live Photos** | HEIC+MOV pair | Any | **Atomic Skip** | *(Original)* | .heic+.mov | Same | **Asset Integrity**. UUID linkage preservation for "Live" playback. |
| **GIF** | Static/1-frame | Any | **Skip** | *(Original)* | .gif | Same | **Meme-Score Intelligence**. Preserves loop integrity. |
| **GIF** | ≤10 frames/short | Animated | **Skip** | *(Original)* | .gif | Same | **Meme-Score Intelligence**. Low overhead preservation. |
| **GIF** | >10 frames/complex | Animated | **Convert** | **HEVC/AV1** (Video) | .mp4 | Same | **Temporal Compress**. Bitmap frames→P/B vectors, 90%+ reduction. |
| **TIFF** | Compression tag 1/2/5/277 | Lossless | **Entropic Coding** | **JXL** (Lossless) | .jxl | Same | **Lossless extraction**. TIFF→JXL modular mode. |
| **TIFF** | Compression tag 6/7 | Lossy | **Lossy Transcode** | **JXL** (d=1.0) | .jxl | Same | **JPEG-in-TIFF handling**. Extract JPEG→JXL reconstruction. |
| **BMP/ICO/TGA/PSD/DDS** | All variants | Assumed Lossless | **Entropic Coding** | **JXL** (Lossless) | .jxl | Same | **Universal lossless**. Standard JXL modular conversion. |
| **EXR** | Compression NONE/RLE/ZIPS | Lossless | **Entropic Coding** | **JXL** (Lossless) | .jxl | Same | **HDR preservation**. EXR→JXL with full dynamic range. |
| **EXR** | Compression PXR24/B44 | Lossy | **Lossy Transcode** | **JXL** (d=1.0) | .jxl | Same | **HDR lossy handling**. Preserve as much quality as possible. |

#### 2. Video Optimization Engine (`vid-hevc`)
The video pipeline solves the bitrate/quality convex optimization problem using a **Three-Phase Saturation Search**:

*   **Phase I: Hardware Spectrum Scan**
    *   Utilizes ASICs (Apple VideoToolbox, NVENC) to perform a binary search across the CRF spectrum, identifying the "Quality Knee" (diminishing returns point).
*   **Phase II: Psychovisual Fine-Tuning**
    *   Performs a localized, high-precision search around the knee point using software encoders (x265/SVT-AV1) with psychovisual tuning (AQ-mode, psy-rd).
*   **Phase III: Fusion Verification**
    *   Validates output using a weighted fusion of **MS-SSIM** and **SSIM-All**. Candidates below the adaptive threshold (0.95-0.98) are automatically rejected.

**Video Processing Strategy:**

| Input Codec | Category | Bit Depth | Container | Operation | Target Format | Output Container | Apple Compat | Technical Principle |
| :--- | :--- | :--- | :--- | :--- | :--- | :--- | :--- | :--- |
| **H.264/AVC** | Delivery | 8-bit | Any | **Transcode** | **HEVC** | .mp4 | Same | **Smart CRF**. Spatial/temporal complexity analysis → SSIM-targeted CRF 18-20. |
| **H.264/AVC** | Delivery | 10-bit | Any | **Transcode** | **HEVC** | .mp4 | Same | **High-Fidelity**. 10-bit preservation with CRF 18. |
| **ProRes 422/444** | Production | 10-bit | .mov | **Transcode** | **HEVC** | .mp4 | Same | **Production Mode**. 10-bit HEVC with CRF 18, preserves dynamic range. |
| **DNxHD/DNxHR** | Production | 8/10-bit | .mxf | **Transcode** | **HEVC** | .mp4 | Same | **Broadcast Mode**. Avid DNx→HEVC with quality matching. |
| **FFV1** | Archival | Any | .avi/.mkv | **Transcode** | **HEVC** (Lossless) | .mkv | Same | **Lossless Mode**. `-x265-params lossless=1` for bit-exact preservation. |
| **HuffYUV/UTVideo** | Archival | Any | .avi | **Transcode** | **HEVC** (Lossless) | .mkv | Same | **Lossless Mode**. Professional archival codecs→HEVC lossless. |
| **Raw Video** | Archival | Any | .avi | **Transcode** | **HEVC** (Lossless) | .mkv | Same | **Lossless Mode**. Uncompressed→HEVC lossless for storage efficiency. |
| **HEVC/H.265** | Delivery | 8/10-bit | Any | **Skip** | *(Original)* | *(Original)* | Skip | **Efficiency Check**. Already modern; re-encoding yields negative returns. |
| **AV1** | Delivery | 8/10-bit | .mp4/.webm | **Skip** | *(Original)* | *(Original)* | Convert | **Efficiency Check**. Skip normally, convert for Apple compatibility. |
| **VP9** | Delivery | 8/10-bit | .webm | **Skip** | *(Original)* | *(Original)* | Convert | **Google Codec**. Skip normally, convert for Apple compatibility. |
| **VVC/H.266** | Delivery | 8/10-bit | Any | **Skip** | *(Original)* | *(Original)* | Convert | **Next-Gen**. Skip normally, convert for Apple compatibility. |
| **AV2** | Delivery | 8/10-bit | Any | **Skip** | *(Original)* | *(Original)* | Convert | **Future Codec**. Skip normally, convert for Apple compatibility. |
| **MPEG-4/DivX/Xvid** | Delivery | 8-bit | .avi/.mp4 | **Transcode** | **HEVC** | .mp4 | Same | **Legacy Upgrade**. Old MPEG codecs→modern HEVC with quality matching. |
| **MPEG-2/MPEG-1** | Delivery | 8-bit | .mpg/.mpeg/.ts | **Transcode** | **HEVC** | .mp4 | Same | **Broadcast Upgrade**. DVD/TV formats→HEVC with significant compression. |
| **WMV/VC-1** | Delivery | 8-bit | .wmv/.asf | **Transcode** | **HEVC** | .mp4 | Same | **Microsoft Legacy**. WMV→HEVC for modern compatibility. |
| **VP8** | Delivery | 8-bit | .webm/.mkv | **Transcode** | **HEVC** | .mp4 | Same | **Legacy Google**. VP8→HEVC for better compression. |

#### AV1 Variant Processing (`img-av1`, `vid-av1`)

| Input Format | Operation | Target Format | Container | Technical Principle |
| :--- | :--- | :--- | :--- | :--- |
| **JPEG** | **Reconstruction** | **JXL** (Lossless) | .jxl | **DCT Transcoding**. Same as HEVC variant, bit-exact preservation. |
| **Lossless Images** | **Entropic Coding** | **JXL** (Lossless) | .jxl | **Modular Mode**. Enhanced efficiency with CJXL effort 7. |
| **Animated Lossless** | **Convert** | **AV1** (Video) | .mp4 | **SVT-AV1 CRF 0**. LibSVT-AV1 with preset 6 for maximum quality. |
| **Animated Lossy** | **Convert** | **AV1** (Video) | .mp4 | **Quality Match**. SVT-AV1 with matched quality to source. |
| **Static Lossy** | **Convert** | **AVIF** | .avif | **AVIF Encoding**. avifenc with quality matching. |
| **Lossless Videos** | **Transcode** | **AV1** (Lossless) | .mp4 | **SVT-AV1 Lossless**. CRF 0 with high-efficiency preset. |
| **Lossy Videos** | **Transcode** | **AV1** | .mp4 | **SVT-AV1 Standard**. CRF-based quality matching. |

### ✨ Core Features

#### 🍎 Apple Ecosystem Perfected
*   **"Unknown Error" Killer**: Heuristically detects file corruption patterns (e.g., mismatched containers, truncated headers) that crash Apple Photos/iCloud and patches them in-place.
*   **Nuclear Metadata Rebuild**: Parses XMP/EXIF/IPTC data, strips non-standard "toxic" tags (common in files from social media apps), and reconstructs a clean, standard-compliant metadata block.
*   **Directory Timestamp Guard**: Caches `atime`/`mtime` of the entire directory tree before processing and restores them with nanosecond precision post-processing.

#### ⚡ Smart Conversion Strategy
*   **Robust Fallback Pipeline (v0.10.14)**: If the primary JXL encoder (`cjxl`) fails, the engine automatically routes files through a multi-stage ImageMagick fallback pipeline, handling grayscale ICC conflicts and bit-depth issues to ensure maximum conversion success.
*   **Magic Bytes Sniffing**: Bypasses file extensions entirely. Uses a buffered reader to inspect the first 16 bytes (file signature) to identify the true MIME type, correcting `png` files masked as `jpg`.
*   **HDR Pipeline**: Fully color-managed workflow. Detects Transfer Characteristics (PQ/HLG) and Color Primaries (BT.2020/P3). Passes `color_primaries`, `transfer_characteristics`, and `matrix_coefficients` flags to the encoder to prevent "washed out" HDR conversions.

#### 📊 Diagnostic & Logging
*   **Intelligent Log Fusion**: When running via the macOS App, multiple log streams (drag-and-drop script, image engine, video engine) are automatically fused into a single `merged_*.log` file for easier troubleshooting.
*   **Nanosecond Precision**: Directory timestamps (`atime`/`mtime`) are cached before processing and restored with nanosecond precision post-processing.

#### 🌐 Container Decision Matrix

| Content Type | Primary Container | Alternative | Apple Compat | Technical Rationale |
| :--- | :--- | :--- | :--- | :--- |
| **HEVC Lossless** | .mkv | .mp4 | .mp4 | MKV supports lossless mode; MP4 for Apple devices |
| **HEVC Lossy** | .mp4 | .mkv | .mp4 | MP4 has widest device support |
| **AV1 Video** | .mp4 | .webm | .mp4 | MP4 for Apple compatibility; WebM for web |
| **JXL Images** | .jxl | - | .jxl | Universal next-gen format |
| **AVIF Images** | .avif | - | .avif | Modern but limited Apple support |
| **Original Skip** | *(Original)* | - | *(Original)* | Preserve container for compatibility |

#### 🎨 HDR & Color Management Pipeline

| HDR Type | Detection | Preservation | Output Handling | Apple Notes |
| :--- | :--- | :--- | :--- | :--- |
| **HDR10** | master_display + max_cll | Full metadata | HEVC 10-bit | Native Apple support |
| **HLG** | transfer=arib-std-b67 | Color space | HEVC 10-bit | BBC standard, Apple compatible |
| **PQ** | transfer=smpte2084 | EOTF | HEVC 10-bit | Dolby-compatible |
| **Dolby Vision** | is_dolby_vision flag | Static layer only | HDR10 fallback | Dynamic layer stripped |
| **Wide Color Gamut** | primaries=bt2020/p3 | Color primaries | Preserved | P3 native to Apple |

#### 🍎 Apple Ecosystem Optimization

| Feature | Normal Mode | Apple Compat Mode | Implementation |
| :--- | :--- | :--- | :--- |
| **HEVC Content** | Skip | Skip | Already optimal for Apple |
| **AV1/VP9/VVC** | Skip | **Convert to HEVC** | `--apple-compat` flag enables |
| **Container Choice** | Optimal | MP4 preferred | MP4 has best Apple support |
| **Color Space** | Preserve | P3/BT2020 priority | Apple display optimization |
| **Audio Tracks** | Preserve | AAC/AC3 only | Remove unsupported codecs |
| **Metadata** | Clean | Apple-specific | Remove toxic tags, add Apple keys |

#### ⚡ Advanced Processing Modes

| Mode | Purpose | Behavior | Performance Impact |
| :--- | :--- | :--- | :--- |
| **Ultimate Mode** | Maximum quality | 3-Phase saturation search | Very high CPU usage |
| **Size Exploration** | Ensure compression | Try higher CRF if output larger | Moderate overhead |
| **Match Quality** | Perceptual equivalence | SSIM-targeted encoding | Balanced approach |
| **Lossless Mode** | Archival preservation | Bit-exact encoding | Larger files, perfect quality |
| **Tolerance Mode** | Practical compression | Allow 1MB size increase | Better compression ratios |

#### 🔧 Quality Matching Algorithms

| Source Analysis | Target CRF (HEVC) | Target CRF (AV1) | Method |
| :--- | :--- | :--- | :--- |
| **High Quality (>90)** | 18-20 | 0-15 | Preserve high fidelity |
| **Medium Quality (70-90)** | 20-24 | 15-25 | Balance compression/quality |
| **Low Quality (<70)** | 24-28 | 25-35 | Aggressive compression |
| **Animated Content** | 18-22 | 0-20 | Preserve motion quality |
| **Screen Capture** | 20-24 | 15-25 | Optimize for text/clarity |

### 🛠️ Installation

**Prerequisites**: macOS/Linux with `brew` and `rust` (cargo) installed.

#### Method 1: Via Git (Recommended for Contributors)
```bash
# Clone the repository
git clone https://github.com/nowaytouse/modern-format-boost.git
cd modern-format-boost

# Install dependencies & Build
brew install jpeg-xl ffmpeg imagemagick exiftool
./scripts/smart_build.sh
```

#### Method 2: Via Curl (Quick One-Liner)
```bash
# Download, extract, and enter the directory
mkdir -p modern-format-boost && curl -L https://github.com/nowaytouse/modern-format-boost/tarball/main | tar -xz -C modern-format-boost --strip-components 1 && cd modern-format-boost

# Build (ensure brew dependencies are installed)
./scripts/smart_build.sh
```

### 🚀 Usage

#### 1. macOS GUI (Easiest)
This project includes a **macOS App Wrapper** for a seamless experience:
*   **Drag & Drop**: Drag a folder onto `Modern Format Boost.app` to start processing immediately in a new Terminal window.
*   **Double-Click**: Open the App to use a native macOS folder picker with a safety confirmation dialog.

#### 2. Drag & Drop Script (CLI)
Simply drag your folder onto the start script to process everything automatically:
```bash
./scripts/drag_and_drop_processor.sh /path/to/your/photos
```

#### 2. CLI Mode (Advanced)
**Images:**
```bash
# Standard Run (Heuristics + Apple Compat)
./target/release/img-hevc run /path/to/photos --output /path/to/out

# Force "Apple Compatibility" (Ensure everything plays on iPhone)
./target/release/img-hevc run /path/to/photos --apple-compat

# Resume interrupted run (uses .mfb_processed state file)
./target/release/img-hevc run /path/to/photos --resume
```

**Videos:**
```bash
# Standard Run (Auto GPU Detect, Smart Matching)
./target/release/vid-hevc run /path/to/videos --output /path/to/out

# Ultimate Mode (3-Phase Saturation Search)
./target/release/vid-hevc run /path/to/videos --ultimate
```

#### 3. Repair Tool ("Unknown Error")
Fixes corrupted headers and timestamps without re-encoding.

```bash
./scripts/repair_apple_photos.sh "/path/to/bad/files"
```

#### 🛠️ Troubleshooting Edge Cases

| Issue | Cause | Solution | Prevention |
| :--- | :--- | :--- | :--- |
| **JXL conversion fails** | Grayscale ICC conflict | Use ImageMagick fallback | Check ICC profiles first |
| **HEVC output larger** | High complexity source | Enable size exploration | Use ultimate mode |
| **AV1 playback issues** | Device incompatibility | Use Apple compat mode | Convert to HEVC |
| **HDR looks washed out** | Missing metadata | Check color flags | Preserve HDR metadata |
| **GIF conversion fails** | Complex animation | Keep original GIF | Use frame threshold |
| **Audio lost in video** | Unsupported codec | Re-encode audio | Use AAC/AC3 |
| **Live Photos broken** | UUID mismatch | Atomic skip processing | Process pairs together |
| **Metadata corruption** | Social media tags | Nuclear metadata rebuild | Clean metadata first |
| **Permission errors** | System files | Check file permissions | Run with appropriate access |

#### 📊 Performance Benchmarks

| Source Type | Target Format | Size Reduction | Quality Preservation | Processing Speed |
| :--- | :--- | :--- | :--- | :--- |
| **JPEG → JXL** | Lossless | 15-30% | Bit-exact | Fast |
| **PNG → JXL** | Lossless | 40-60% | Bit-exact | Medium |
| **H.264 → HEVC** | CRF 20 | 30-50% | SSIM >0.95 | Medium |
| **ProRes → HEVC** | 10-bit | 50-70% | Visually lossless | Slow |
| **GIF → HEVC** | Video | 80-95% | Motion preserved | Fast |
| **AV1 → HEVC** | Apple compat | Variable | Perceptual | Medium |

---

<a id="introduction-cn"></a>

### 📖 简介

**Modern Format Boost** 是一个用 **Rust** 编写的高性能、高并发媒体优化引擎。它旨在将陈旧的媒体归档（JPEG/H.264）现代化为下一代格式（JXL/HEVC），同时严格遵守图像的**数学无损**标准和视频的**感知保真度**标准。

与简单的脚本包装器不同，本工具实现了一个**控制论反馈回路 (Cybernetic Feedback Loop)**：实时分析内容复杂度，动态调整编码参数，并使用工业级指标（SSIM/MS-SSIM）验证输出完整性。

> [!IMPORTANT]
> **免责声明与现状：**
> 1. **工具成熟度**：基于 HEVC 的工具（`img-hevc`、`vid-hevc`）的完成度与稳定性显著领先于 AV1 变体工具（`img-av1`、`vid-av1`）。
> 2. **测试范围**：尽管程序已通过数万份文件的严苛测试，并经历了反复的修复与 Debug，但目前的测试数据集仅限于作者本机环境。
> 3. **平台支持**：本项目目前仅在 **macOS** 系统上经过完整验证与优化。

### ⚙️ 工作原理 (技术架构)

#### 1. 图像现代化管线 (`img-hevc`)
图像引擎作为一个**格式感知路由状态机 (Format-Aware Routing State Machine)** 运行。它通过检查每个文件的二进制签名（魔术字节）和元数据来确定最佳的转换路径。

**决策矩阵与转换策略：**

| 输入格式 | 变体与检测 | 压缩类型 | 操作 | 目标格式 | 容器 | Apple 兼容 | 技术原理 |
| :--- | :--- | :--- | :--- | :--- | :--- | :--- | :--- |
| **JPEG** | 所有变体 | 有损（总是）| **重构** | **JXL** (无损) | .jxl | 相同 | **DCT 转码**。直接解析原始 DCT 系数并映射到 JXL 的 `varDCT`。**可逆且比特级精确。** |
| **PNG** | 真彩色/16-bit | 无损 | **熵编码** | **JXL** (无损) | .jxl | 相同 | **Modular 模式**。利用增量调色板、Squeeze (Haar 变换) 和 MA 树。密度提升 ~40%。 |
| **PNG** | 索引/调色板 | 有损（量化）| **有损转码** | **JXL** (d=1.0) | .jxl | 相同 | **多因子检测**（调色板结构、tRNS、工具签名、抖动、熵分析）。输出更大时自动跳过。 |
| **WebP** | VP8L 块 | 无损 | **熵编码** | **JXL** (无损) | .jxl | 相同 | **无损提取**。VP8L→JXL modular 模式，完全质量保持。 |
| **WebP** | VP8 块 | 有损 | **硬跳过** | *(原格式)* | .webp | 相同 | **信号保护**。对伪影进行重量化在数学上必然导致信噪比破坏。 |
| **WebP** | ANIM 块 | 动画 | **转换** | **HEVC/AV1** (视频) | .mp4 | 相同 | **时域压缩**。ANMF 帧→P/B 向量，体积减少 90%+。 |
| **AVIF** | 4:2:0/4:2:2 色度 | 有损 | **硬跳过** | *(原格式)* | .avif | 相同 | **信号保护**。AVIF 已是现代格式；重新编码不必要。 |
| **AVIF** | 4:4:4 + 高位深 | 无损 | **熵编码** | **JXL** (无损) | .jxl | 相同 | **无损提取**。AVIF→JXL modular 模式用于归档。 |
| **HEIC/HEIF** | 所有变体 | 任意 | **透传** | *(原格式)* | .heic/.heif | 相同 | **零拷贝**。Apple 生态原生格式。 |
| **Live Photos** | HEIC+MOV 对 | 任意 | **原子跳过** | *(原格式)* | .heic+.mov | 相同 | **资产完整性**。UUID 链路保护用于 "Live" 播放。 |
| **GIF** | 静态/1帧 | 任意 | **跳过** | *(原格式)* | .gif | 相同 | **Meme-Score 智能**。保持循环完整性。 |
| **GIF** | ≤10 帧/短小 | 动画 | **跳过** | *(原格式)* | .gif | 相同 | **Meme-Score 智能**。低开销保护。 |
| **GIF** | >10 帧/复杂 | 动画 | **转换** | **HEVC/AV1** (视频) | .mp4 | 相同 | **时域压缩**。位图帧→P/B 向量，体积减少 90%+。 |
| **TIFF** | 压缩标签 1/2/5/277 | 无损 | **熵编码** | **JXL** (无损) | .jxl | 相同 | **无损提取**。TIFF→JXL modular 模式。 |
| **TIFF** | 压缩标签 6/7 | 有损 | **有损转码** | **JXL** (d=1.0) | .jxl | 相同 | **JPEG-in-TIFF 处理**。提取 JPEG→JXL 重构。 |
| **BMP/ICO/TGA/PSD/DDS** | 所有变体 | 假设无损 | **熵编码** | **JXL** (无损) | .jxl | 相同 | **通用无损**。标准 JXL modular 转换。 |
| **EXR** | 压缩 NONE/RLE/ZIPS | 无损 | **熵编码** | **JXL** (无损) | .jxl | 相同 | **HDR 保护**。EXR→JXL 保持全动态范围。 |
| **EXR** | 压缩 PXR24/B44 | 有损 | **有损转码** | **JXL** (d=1.0) | .jxl | 相同 | **HDR 有损处理**。尽可能保持质量。 |

#### 2. 视频优化管线 (`vid-hevc`)
视频引擎通过**三阶段饱和搜索算法**来求解码率/画质的凸优化问题：

*   **阶段 I：硬件频谱扫描**
    *   利用 ASIC (Apple VideoToolbox, NVENC) 在 CRF 频谱上执行二分搜索，识别“画质拐点”(收益递减点)。
*   **阶段 II：心理视觉精细调优**
    *   在拐点附近使用软件编码器 (x265/SVT-AV1) 进行局部高精度搜索，应用心理视觉调优 (AQ-mode, Psy-rd)。
*   **阶段 III：Fusion 融合验证**
    *   使用 **MS-SSIM** 和 **SSIM-All** 的加权融合评分严格验证输出。评分低于自适应阈值 (0.95-0.98) 的候选者将被自动拒绝。

**视频处理策略：**

| 输入编码 | 类别 | 位深 | 容器 | 操作 | 目标格式 | 输出容器 | Apple 兼容 | 技术原理 |
| :--- | :--- | :--- | :--- | :--- | :--- | :--- | :--- | :--- |
| **H.264/AVC** | 交付 | 8-bit | 任意 | **转码** | **HEVC** | .mp4 | 相同 | **Smart CRF**。空间/时间复杂度分析 → SSIM 目标 CRF 18-20。 |
| **H.264/AVC** | 交付 | 10-bit | 任意 | **转码** | **HEVC** | .mp4 | 相同 | **高保真**。10-bit 保持，CRF 18。 |
| **ProRes 422/444** | 制作 | 10-bit | .mov | **转码** | **HEVC** | .mp4 | 相同 | **制作模式**。10-bit HEVC，CRF 18，保持动态范围。 |
| **DNxHD/DNxHR** | 制作 | 8/10-bit | .mxf | **转码** | **HEVC** | .mp4 | 相同 | **广播模式**。Avid DNx→HEVC 质量匹配。 |
| **FFV1** | 归档 | 任意 | .avi/.mkv | **转码** | **HEVC** (无损) | .mkv | 相同 | **无损模式**。`-x265-params lossless=1` 比特精确保持。 |
| **HuffYUV/UTVideo** | 归档 | 任意 | .avi | **转码** | **HEVC** (无损) | .mkv | 相同 | **无损模式**。专业归档编码→HEVC 无损。 |
| **Raw Video** | 归档 | 任意 | .avi | **转码** | **HEVC** (无损) | .mkv | 相同 | **无损模式**。未压缩→HEVC 无损提高存储效率。 |
| **HEVC/H.265** | 交付 | 8/10-bit | 任意 | **跳过** | *(原格式)* | *(原格式)* | 跳过 | **效率检查**。已是现代格式；重新编码导致负收益。 |
| **AV1** | 交付 | 8/10-bit | .mp4/.webm | **跳过** | *(原格式)* | *(原格式)* | 转换 | **效率检查**。通常跳过，Apple 兼容时转换。 |
| **VP9** | 交付 | 8/10-bit | .webm | **跳过** | *(原格式)* | *(原格式)* | 转换 | **Google 编码**。通常跳过，Apple 兼容时转换。 |
| **VVC/H.266** | 交付 | 8/10-bit | 任意 | **跳过** | *(原格式)* | *(原格式)* | 转换 | **下一代**。通常跳过，Apple 兼容时转换。 |
| **AV2** | 交付 | 8/10-bit | 任意 | **跳过** | *(原格式)* | *(原格式)* | 转换 | **未来编码**。通常跳过，Apple 兼容时转换。 |
| **MPEG-4/DivX/Xvid** | 交付 | 8-bit | .avi/.mp4 | **转码** | **HEVC** | .mp4 | 相同 | **传统升级**。旧 MPEG 编码→现代 HEVC 质量匹配。 |
| **MPEG-2/MPEG-1** | 交付 | 8-bit | .mpg/.mpeg/.ts | **转码** | **HEVC** | .mp4 | 相同 | **广播升级**。DVD/TV 格式→HEVC 显著压缩。 |
| **WMV/VC-1** | 交付 | 8-bit | .wmv/.asf | **转码** | **HEVC** | .mp4 | 相同 | **微软传统**。WMV→HEVC 现代兼容性。 |
| **VP8** | 交付 | 8-bit | .webm/.mkv | **转码** | **HEVC** | .mp4 | 相同 | **传统 Google**。VP8→HEVC 更好压缩。 |

#### AV1 变体处理 (`img-av1`, `vid-av1`)

| 输入格式 | 操作 | 目标格式 | 容器 | 技术原理 |
| :--- | :--- | :--- | :--- | :--- |
| **JPEG** | **重构** | **JXL** (无损) | .jxl | **DCT 转码**。与 HEVC 变体相同，比特精确保持。 |
| **无损图像** | **熵编码** | **JXL** (无损) | .jxl | **Modular 模式**。增强效率，CJXL effort 7。 |
| **动画无损** | **转换** | **AV1** (视频) | .mp4 | **SVT-AV1 CRF 0**。LibSVT-AV1 preset 6 最大质量。 |
| **动画有损** | **转换** | **AV1** (视频) | .mp4 | **质量匹配**。SVT-AV1 匹配源质量。 |
| **静态有损** | **转换** | **AVIF** | .avif | **AVIF 编码**。avifenc 质量匹配。 |
| **无损视频** | **转码** | **AV1** (无损) | .mp4 | **SVT-AV1 无损**。CRF 0 高效率 preset。 |
| **有损视频** | **转码** | **AV1** | .mp4 | **SVT-AV1 标准**。基于 CRF 质量匹配。 |

### ✨ 核心特性

#### 🍎 完美适配苹果生态
*   **“未知错误”终结者**：启发式检测导致 Apple Photos/iCloud 崩溃的文件损坏模式（如容器不匹配、截断的头文件）并进行原地修补。
*   **元数据核弹级重构**：解析 XMP/EXIF/IPTC 数据，剥离非标准的“有毒”标签（常见于社交媒体应用生成的图片），并重构干净、符合标准的元数据块。
*   **文件夹时间守护**：在处理前缓存整个目录树的 `atime`/`mtime`，并在处理后以纳秒级精度还原，确保相册的时间线视图丝毫不差。

#### ⚡ 智能转换策略
*   **鲁棒回退管线 (v0.10.14)**：当主要的 JXL 编码器 (`cjxl`) 失败时，引擎会自动通过多级 ImageMagick 回退管线处理文件，智能解决灰度 ICC 冲突和位深度问题，确保极高的转换成功率。
*   **魔法字节嗅探**: 完全绕过文件扩展名。通过缓冲读取文件头的前 16 个字节（文件签名）来识别真实的 MIME 类型，自动修正伪装成 `jpg` 的 `png` 文件。
*   **HDR 全链路管线**：全色彩管理工作流。自动检测光电传输特性 (PQ/HLG) 和色域 (BT.2020/P3)。在转换时将 `color_primaries`、`transfer_characteristics` 和 `matrix_coefficients` 标志正确传递给编码器，防止 HDR 视频出现“发灰”现象。

#### 📊 诊断与日志
*   **智能日志融合**：通过 macOS App 运行时，多个日志流（脚本、图像引擎、视频引擎）会自动整合成单个 `merged_*.log` 文件，极大地方便了问题排查。
*   **纳秒级时间还原**：在处理前缓存整个目录树的 `atime`/`mtime`，并在处理后以纳秒级精度还原，确保相册的时间线视图丝毫不差。

### 🛠️ 安装

**前置要求**: macOS/Linux 系统，需安装 `brew` 和 `rust` (cargo)。

#### 方法 1：通过 Git 克隆 (推荐)
```bash
# 克隆仓库
git clone https://github.com/nowaytouse/modern-format-boost.git
cd modern-format-boost

# 安装运行时依赖并编译
brew install jpeg-xl ffmpeg imagemagick exiftool
./scripts/smart_build.sh
```

#### 方法 2：通过 Curl 一键下载
```bash
# 下载、解压并进入目录
mkdir -p modern-format-boost && curl -L https://github.com/nowaytouse/modern-format-boost/tarball/main | tar -xz -C modern-format-boost --strip-components 1 && cd modern-format-boost

# 执行智能构建 (请确保已安装上述 brew 依赖)
./scripts/smart_build.sh
```

### 🚀 使用方法

#### 1. macOS 图形化操作 (最简单)
本项目内置了 **macOS App 封装器**，提供原生体验：
*   **拖拽处理**：直接将文件夹拖到 `Modern Format Boost.app` 图标上，即可在自动开启的终端窗口中开始处理。
*   **双击运行**：双击打开 App，将通过 macOS 标准文件夹选择器选取目标，并弹出安全确认对话框。

#### 2. 脚本拖拽 (命令行)
只需将您的文件夹拖到启动脚本上，即可全自动处理：
```bash
./scripts/drag_and_drop_processor.sh /path/to/your/photos
```

#### 2. 命令行模式（高级）
**图片处理:**
```bash
# 标准运行 (启发式策略 + 苹果兼容)
./target/release/img-hevc run /path/to/photos --output /path/to/out

# 强制“苹果兼容模式” (确保所有文件能在 iPhone 上播放)
./target/release/img-hevc run /path/to/photos --apple-compat

# 断点续传 (使用 .mfb_processed 状态文件)
./target/release/img-hevc run /path/to/photos --resume
```

**视频处理:**
```bash
# 标准运行 (自动 GPU 检测，智能匹配)
./target/release/vid-hevc run /path/to/videos --output /path/to/out

# 极致探索模式 (三阶段饱和搜索)
./target/release/vid-hevc run /path/to/videos --ultimate
```

#### 3. 修复工具 ("未知错误")
修复损坏的文件头和时间戳，无需重新编码。

```bash
./scripts/repair_apple_photos.sh "/path/to/bad/files"
```

---

## 📜 License

The entire code in this repository is licensed under the [MIT](LICENSE_MIT) license.

For a detailed overview of all licenses, including assets and runtime dependencies, see [NOTICE.md](NOTICE.md).

Third-party library licenses used in this project are documented in [THIRD_PARTY_LICENSES.md](THIRD_PARTY_LICENSES.md).

---

## 🤝 Thanks

Special thanks to:
*   [FFmpeg](https://ffmpeg.org/) for the powerful video processing engine.
*   [ExifTool](https://exiftool.org/) for the most comprehensive metadata management.
*   All contributors who help make Modern Format Boost faster and safer.
