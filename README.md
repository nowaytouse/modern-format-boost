# 🚀 Modern Format Boost [![GitHub Stars](https://img.shields.io/github/stars/nowaytouse/modern-format-boost.svg?style=social)](https://github.com/nowaytouse/modern-format-boost/stargazers) [![GitHub Forks](https://img.shields.io/github/forks/nowaytouse/modern-format-boost.svg?style=social)](https://github.com/nowaytouse/modern-format-boost/network/members)

[![Version](https://img.shields.io/badge/version-0.9.0-blue.svg)](https://github.com/nowaytouse/modern-format-boost/releases)
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

| Input Category | Condition / State | Operation | Target Format | Technical Principle |
| :--- | :--- | :--- | :--- | :--- |
| **JPEG** | Legacy Lossy | **Reconstruction** | **JXL** (Lossless) | **DCT Transcoding**. Parses raw DCT coefficients and maps them directly to JXL's `varDCT`. **Reversible & Bit-exact.** |
| **PNG / TIFF** | Lossless (truecolor/16-bit) | **Entropic Coding** | **JXL** (Lossless) | **Modular Mode**. Utilizes Delta Palettes, Squeeze (Haar transform), and MA-trees. ~40% density improvement. |
| **PNG** | Lossy (palette-quantized, e.g. TinyPNG/pngquant) | **Lossy Transcode** | **JXL** (d=1.0) | **Multi-factor quantization detection** (palette structure, tRNS, tool signatures, dithering, entropy). Attempts lossy JXL; skipped automatically if output is larger. |
| **WebP / AVIF** | Lossy | **Hard Skip** | *(Original)* | **Signal Preservation**. Re-quantizing artifacts (cascade compression) mathematically destroys SNR. Preserved as-is. |
| **HEIC / HEIF** | Any | **Passthrough** | *(Original)* | **Zero-Copy**. Native format for Apple ecosystem. No processing required. |
| **Live Photos** | HEIC + MOV Pair | **Atomic Skip** | *(Original)* | **Asset Integrity**. Identified via heuristic graph analysis. Locked to preserve the UUID linkage for "Live" playback. |
| **GIF** | Meme-like (Simple/Short) | **Restoration** | **GIF** | **Meme-Score (Intelligence)**. Multi-factor analysis (sharpness, resolution, duration, FPS, BPP) to identify memes. Re-encodes with optimized palettes. |
| **GIF** | Video-like (Complex/Long) | **Temporal Compress** | **HEVC** (Video) | **Inter-frame Prediction**. Transforms redundant bitmap frames into P/B vectors, reducing size by 90%+. |

#### 2. Video Optimization Engine (`vid-hevc`)
The video pipeline solves the bitrate/quality convex optimization problem using a **Three-Phase Saturation Search**:

*   **Phase I: Hardware Spectrum Scan**
    *   Utilizes ASICs (Apple VideoToolbox, NVENC) to perform a binary search across the CRF spectrum, identifying the "Quality Knee" (diminishing returns point).
*   **Phase II: Psychovisual Fine-Tuning**
    *   Performs a localized, high-precision search around the knee point using software encoders (x265/SVT-AV1) with psychovisual tuning (AQ-mode, psy-rd).
*   **Phase III: Fusion Verification**
    *   Validates output using a weighted fusion of **MS-SSIM** and **SSIM-All**. Candidates below the adaptive threshold (0.95-0.98) are automatically rejected.

**Video Processing Strategy:**

| Input Codec | Condition | Operation | Target Format | Technical Principle |
| :--- | :--- | :--- | :--- | :--- |
| **H.264 / AVC** | Legacy Lossy | **Transcode** | **HEVC** (H.265) | **Smart CRF**. Analyzes spatial/temporal complexity to target a specific SSIM. |
| **ProRes / DNx** | Visually Lossless | **Transcode** | **HEVC** (H.265) | **High-Fidelity Mode**. Uses 10-bit color depth to preserve dynamic range. |
| **Raw / FFV1** | Lossless | **Transcode** | **HEVC** (Lossless) | **Lossless Mode**. Enabled via `-x265-params lossless=1` (MKV container). |
| **HEVC / AV1** | Modern | **Skip** | *(Original)* | **Efficiency Check**. Already highly compressed; re-encoding yields negative returns. |
| **AV1 / VP9** | Modern (Non-Apple) | **Compat Convert** | **HEVC** (H.265) | **Apple Compat Mode**. Only triggered with `--apple-compat` flag for playback support. |

### ✨ Core Features

#### 🍎 Apple Ecosystem Perfected
*   **"Unknown Error" Killer**: Heuristically detects file corruption patterns (e.g., mismatched containers, truncated headers) that crash Apple Photos/iCloud and patches them in-place.
*   **Nuclear Metadata Rebuild**: Parses XMP/EXIF/IPTC data, strips non-standard "toxic" tags (common in files from social media apps), and reconstructs a clean, standard-compliant metadata block.
*   **Directory Timestamp Guard**: Caches `atime`/`mtime` of the entire directory tree before processing and restores them with nanosecond precision post-processing.

#### ⚡ Smart Conversion Strategy
*   **Magic Bytes Sniffing**: Bypasses file extensions entirely. Uses a buffered reader to inspect the first 16 bytes (file signature) to identify the true MIME type, correcting `png` files masked as `jpg`.
*   **HDR Pipeline**: Fully color-managed workflow. Detects Transfer Characteristics (PQ/HLG) and Color Primaries (BT.2020/P3). Passes `color_primaries`, `transfer_characteristics`, and `matrix_coefficients` flags to the encoder to prevent "washed out" HDR conversions.

### 🛠️ Installation

**Prerequisites**: macOS/Linux with `brew` installed.

```bash
# 1. Install Runtime Dependencies
brew install jpeg-xl ffmpeg imagemagick exiftool

# 2. Clone Repository
git clone https://github.com/nowaytouse/modern-format-boost.git
cd modern_format_boost

# 3. Compile (Release Profile with LTO)
./scripts/smart_build.sh
```

### 🚀 Usage

#### 1. Drag & Drop (Recommended)
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

| 输入类别 | 状态 / 条件 | 操作 | 目标格式 | 技术原理 |
| :--- | :--- | :--- | :--- | :--- |
| **JPEG** | 陈旧有损 | **重构 (Reconstruction)** | **JXL** (无损) | **DCT 转码**。直接解析原始 DCT 系数并映射到 JXL 的 `varDCT` 结构。**可逆且比特级精确。** |
| **PNG / TIFF** | 无损（真彩色/16-bit）| **熵编码 (Entropic Coding)** | **JXL** (无损) | **Modular 模式**。利用增量调色板 (Delta Palette)、Squeeze (Haar 变换) 和 MA 树。密度提升 ~40%。 |
| **PNG** | 有损（调色板量化，如 TinyPNG/pngquant）| **有损转码 (Lossy Transcode)** | **JXL** (d=1.0) | **多因子量化检测**（调色板结构、tRNS、工具签名、抖动、熵分析）。尝试有损 JXL；若输出更大则自动跳过保护。 |
| **WebP / AVIF** | 有损 | **硬跳过 (Hard Skip)** | *(原格式)* | **信号保护**。对伪影进行重量化（级联压缩）在数学上必然导致信噪比 (SNR) 破坏。按原样保留。 |
| **HEIC / HEIF** | 任意 | **透传 (Passthrough)** | *(原格式)* | **零拷贝**。Apple 生态原生格式，无需处理。 |
| **Live Photo** | HEIC + MOV 配对 | **原子跳过 (Atomic Skip)** | *(原格式)* | **资产完整性**。通过启发式图谱分析识别。锁定文件对以保护 "Live" 播放所需的 UUID 链路。 |
| **GIF** | 表情包类 (简单/短促) | **修复 (Restoration)** | **GIF** | **Meme-Score 智能判定**。综合分析清晰度、分辨率、时长、帧率和体积密度以识别表情包。使用优化调色板重编码。 |
| **GIF** | 视频类 (复杂/长篇) | **时域压缩 (Temporal Compress)** | **HEVC** (视频) | **帧间预测**。将冗余的位图帧转换为 P/B 向量，体积通常减少 90% 以上。 |

#### 2. 视频优化管线 (`vid-hevc`)
视频引擎通过**三阶段饱和搜索算法**来求解码率/画质的凸优化问题：

*   **阶段 I：硬件频谱扫描**
    *   利用 ASIC (Apple VideoToolbox, NVENC) 在 CRF 频谱上执行二分搜索，识别“画质拐点”(收益递减点)。
*   **阶段 II：心理视觉精细调优**
    *   在拐点附近使用软件编码器 (x265/SVT-AV1) 进行局部高精度搜索，应用心理视觉调优 (AQ-mode, Psy-rd)。
*   **阶段 III：Fusion 融合验证**
    *   使用 **MS-SSIM** 和 **SSIM-All** 的加权融合评分严格验证输出。评分低于自适应阈值 (0.95-0.98) 的候选者将被自动拒绝。

**视频处理策略：**

| 输入编码 | 条件 | 操作 | 目标格式 | 技术原理 |
| :--- | :--- | :--- | :--- | :--- |
| **H.264 / AVC** | 陈旧有损 | **转码 (Transcode)** | **HEVC** (H.265) | **Smart CRF**。分析空间/时间复杂度以定位特定的 SSIM 目标。 |
| **ProRes / DNx** | 视觉无损 | **转码 (Transcode)** | **HEVC** (H.265) | **高保真模式**。使用 10-bit 色深以保留动态范围。 |
| **Raw / FFV1** | 无损 | **转码 (Transcode)** | **HEVC** (无损) | **无损模式**。通过 `-x265-params lossless=1` 启用 (MKV 容器)。 |
| **HEVC / AV1** | 现代 | **跳过 (Skip)** | *(原格式)* | **效率检查**。已处于高压缩率状态；重编码会导致负收益。 |
| **AV1 / VP9** | 现代 (非 Apple) | **兼容转换 (Compat Convert)** | **HEVC** (H.265) | **Apple 兼容模式**。仅在启用 `--apple-compat` 标志时触发，以支持播放。 |

### ✨ 核心特性

#### 🍎 完美适配苹果生态
*   **“未知错误”终结者**：启发式检测导致 Apple Photos/iCloud 崩溃的文件损坏模式（如容器不匹配、截断的头文件）并进行原地修补。
*   **元数据核弹级重构**：解析 XMP/EXIF/IPTC 数据，剥离非标准的“有毒”标签（常见于社交媒体应用生成的图片），并重构干净、符合标准的元数据块。
*   **文件夹时间守护**：在处理前缓存整个目录树的 `atime`/`mtime`，并在处理后以纳秒级精度还原，确保相册的时间线视图丝毫不差。

#### ⚡ 智能转换策略
*   **魔法字节嗅探**: 完全绕过文件扩展名。通过缓冲读取文件头的前 16 个字节（文件签名）来识别真实的 MIME 类型，自动修正伪装成 `jpg` 的 `png` 文件。
*   **HDR 全链路管线**：全色彩管理工作流。自动检测光电传输特性 (PQ/HLG) 和色域 (BT.2020/P3)。在转换时将 `color_primaries`、`transfer_characteristics` 和 `matrix_coefficients` 标志正确传递给编码器，防止 HDR 视频出现“发灰”现象。

### 🛠️ 安装

**前置要求**: macOS/Linux 系统，并已安装 `brew`。

```bash
# 1. 安装运行时依赖
brew install jpeg-xl ffmpeg imagemagick exiftool

# 2. 克隆仓库
git clone https://github.com/nowaytouse/modern-format-boost.git
cd modern_format_boost

# 3. 编译 (Release Profile with LTO)
./scripts/smart_build.sh
```

### 🚀 使用方法

#### 1. 拖拽使用（推荐）
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

MIT License. See `LICENSE` for details.
