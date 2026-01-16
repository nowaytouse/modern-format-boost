# Modern Format Boost

High-performance media conversion toolkit with intelligent quality matching, SSIM validation, and multi-platform GPU acceleration.

## Core Tools

| Tool | Function | Output Format |
|------|----------|---------------|
| `vidquality-hevc` | Video → HEVC/H.265 | MP4 (Apple compatible) |
| `vidquality-av1` | Video → AV1 | MP4 (max compression) |
| `imgquality-hevc` | Image/Animation → JXL/HEVC | JXL + MP4 |
| `imgquality-av1` | Image/Animation → JXL/AV1 | JXL + MP4 |

## Key Features

### 1. Smart Quality Matching System
- **BPP Analysis**: Calculates bits-per-pixel from video bitrate
- **Codec Efficiency**: H.264=1.0, HEVC=0.65, AV1=0.50, VVC=0.35
- **Content Detection**: Animation/Film/Screen recording optimization
- **HDR Support**: BT.2020 color space detection

### 2. CRF Binary Search Explorer
- **Three-phase search**: Coarse → Fine → Refine (±0.1 precision)
- **SSIM validation**: Default threshold ≥ 0.95
- **Transparency report**: Every iteration with metrics
- **Confidence scoring**: Sampling coverage + prediction accuracy

### 3. Quality Verification System (v6.9.9)

| Mode | Metric | Threshold | Description |
|------|--------|-----------|-------------|
| Short video (≤5min) | Fusion Score | ≥0.91 | `0.6×MS-SSIM + 0.4×SSIM_All` |
| Long video (>5min) | SSIM All | ≥0.92 | Y+U+V weighted average |

**MS-SSIM (Multi-Scale SSIM):**
- 5-level resolution analysis, closer to human perception
- 3-channel (Y+U+V) average, includes chroma quality
- Enabled with `--ms-ssim` flag

**Fusion Formula:** `Final = 0.6 × MS-SSIM(3-ch) + 0.4 × SSIM_All`

### 4. GPU Hardware Acceleration

| Platform | HEVC | AV1 | H.264 |
|----------|------|-----|-------|
| NVIDIA NVENC | ✅ | ✅ | ✅ |
| Apple VideoToolbox | ✅ | - | ✅ |
| Intel QSV | ✅ | ✅ | ✅ |
| AMD AMF | ✅ | ✅ | ✅ |

### 5. Conversion Logic

**Static Images:** JPEG → JXL (lossless DCT), PNG/TIFF → JXL (mathematical lossless)

**Animated Images (≥3s):** GIF/APNG/WebP → HEVC/AV1 MP4

**Video:** H.264/MPEG → HEVC/AV1, AV1/VP9 → HEVC (`--apple-compat`)

## Installation

```bash
cd modern_format_boost
./smart_build.sh
```

**Dependencies:** FFmpeg (libx265, libsvtav1, libjxl), Rust 1.70+

## Commands

### Flag Combinations (7 Valid Modes)

| Flags | Mode | Behavior |
|-------|------|----------|
| (none) | Default | Single encode with AI-predicted CRF |
| `--compress` | Compress-Only | Ensure output < input |
| `--explore` | Size-Only | Binary search for smallest file |
| `--match-quality` | Quality-Match | Single encode + SSIM validation |
| `--explore --match-quality` | Precise | Binary search + SSIM validation |
| `--explore --match-quality --compress` | Full | Precise quality + must compress |
| `--explore --match-quality --compress --ultimate` | 🔥 Ultimate | Search until SSIM saturates |

### All Options

```bash
-o, --output <DIR>     Output directory
-f, --force            Overwrite existing files
-r, --recursive        Recursive directory scan
--delete-original      Delete original after conversion
--in-place             Convert and delete original (replace)
--apple-compat         Convert AV1/VP9 → HEVC for Apple devices
--ultimate             🔥 Ultimate explore mode (SSIM saturation)
```

## Architecture

```
modern_format_boost/
├── vidquality_hevc/        # Video → HEVC converter
├── vidquality_av1/         # Video → AV1 converter  
├── imgquality_hevc/        # Image → JXL/HEVC converter
├── imgquality_av1/         # Image → JXL/AV1 converter
├── shared_utils/           # Core modules
│   ├── video_explorer.rs   # CRF binary search + SSIM
│   ├── quality_matcher.rs  # BPP→CRF prediction
│   ├── gpu_accel.rs        # Multi-platform GPU detection
│   ├── ffprobe.rs          # Media analysis + audio detection
│   └── types/              # Type-safe wrappers (v7.1)
├── xmp_merger/             # XMP sidecar merging tool
└── Modern Format Boost.app # macOS GUI app
```

## No-Loss Design (v6.9.16)

The toolkit uses a **Whitelist + Fallback Copy** mechanism to ensure zero file loss:

### Processing Strategy

| File Type | Action | XMP Handling |
|-----------|--------|--------------|
| **Supported Images** (jpg, png, gif, webp, heic, avif, etc.) | Convert → JXL/HEVC | Merge into output |
| **Supported Videos** (mp4, mov, mkv, avi, webm, etc.) | Convert → HEVC/AV1 | Merge into output |
| **Skipped Files** (short animation <3s, modern lossy) | Copy original | Merge XMP |
| **Failed Conversions** | Copy original | Merge XMP |
| **Unsupported Files** (.psd, .txt, .pdf, etc.) | Copy original | Merge XMP (ExifTool) or copy sidecar |
| **XMP Sidecars** (.xmp) | Merged into media | Not output separately |

### Whitelist (Supported Formats)

**Images:** `png, jpg, jpeg, jpe, jfif, webp, gif, tiff, tif, heic, heif, avif, bmp`

**Videos:** `mp4, mov, mkv, avi, webm, m4v, wmv, flv, mpg, mpeg, ts, mts`

### Verification

After processing, the system verifies: `Output files = Total files - XMP sidecars`

If mismatch detected, a loud warning is displayed.

---

## 无遗漏设计 (v6.9.16)

工具集采用**白名单 + 回退复制**机制，确保零文件丢失：

### 处理策略

| 文件类型 | 操作 | XMP处理 |
|----------|------|---------|
| **支持的图像** (jpg, png, gif, webp, heic, avif等) | 转换 → JXL/HEVC | 合并到输出 |
| **支持的视频** (mp4, mov, mkv, avi, webm等) | 转换 → HEVC/AV1 | 合并到输出 |
| **跳过的文件** (短动画<3秒, 现代有损格式) | 复制原始 | 合并XMP |
| **转换失败** | 复制原始 | 合并XMP |
| **不支持的文件** (.psd, .txt, .pdf等) | 复制原始 | 合并XMP (ExifTool) 或复制边车 |
| **XMP边车** (.xmp) | 合并到媒体文件 | 不单独输出 |

### 白名单（支持的格式）

**图像：** `png, jpg, jpeg, jpe, jfif, webp, gif, tiff, tif, heic, heif, avif, bmp`

**视频：** `mp4, mov, mkv, avi, webm, m4v, wmv, flv, mpg, mpeg, ts, mts`

### 验证机制

处理完成后，系统验证：`输出文件数 = 全部文件数 - XMP边车数`

如检测到不匹配，会响亮警告。

---

## Supported Formats

**Video Input:** mp4, mkv, avi, mov, webm, flv, wmv, m4v, mpg, mpeg, ts, mts
**Image Input:** png, jpg, jpeg, webp, gif, tiff, tif, heic, avif
**Video Output:** MP4 (HEVC/AV1), MKV (lossless)
**Image Output:** JXL

## macOS App

Double-click `Modern Format Boost.app` for drag-and-drop conversion:
`--explore --match-quality --compress --in-place`

---

# 中文文档

高性能媒体转换工具集，支持智能质量匹配、SSIM验证和多平台GPU加速。

## 核心工具

| 工具 | 功能 | 输出格式 |
|------|------|----------|
| `vidquality-hevc` | 视频 → HEVC/H.265 | MP4（Apple兼容）|
| `vidquality-av1` | 视频 → AV1 | MP4（最大压缩）|
| `imgquality-hevc` | 图片/动图 → JXL/HEVC | JXL + MP4 |
| `imgquality-av1` | 图片/动图 → JXL/AV1 | JXL + MP4 |

## 核心功能

### 1. 智能质量匹配系统
- **BPP分析**：从视频码率计算每像素比特数
- **编码效率**：H.264=1.0, HEVC=0.65, AV1=0.50
- **内容检测**：动画/电影/屏幕录制优化

### 2. CRF二分搜索探索器
- **三阶段搜索**：粗搜索 → 精搜索 → 微调（±0.1精度）
- **SSIM验证**：默认阈值 ≥ 0.95
- **透明度报告**：显示每次迭代的详细指标

### 3. 质量验证系统 (v6.9.9)

| 模式 | 指标 | 阈值 | 说明 |
|------|------|------|------|
| 短视频 (≤5分钟) | 融合评分 | ≥0.91 | `0.6×MS-SSIM + 0.4×SSIM_All` |
| 长视频 (>5分钟) | SSIM All | ≥0.92 | Y+U+V 加权平均 |

**MS-SSIM（多尺度SSIM）：**
- 5级分辨率分析，更接近人眼感知
- 3通道 (Y+U+V) 平均，包含色度质量
- 使用 `--ms-ssim` 参数启用

**融合公式：** `最终分数 = 0.6 × MS-SSIM(3通道) + 0.4 × SSIM_All`

### 4. GPU硬件加速
支持 NVIDIA NVENC、Apple VideoToolbox、Intel QSV、AMD AMF

## 安装

```bash
cd modern_format_boost
./smart_build.sh
```

**依赖：** FFmpeg（libx265, libsvtav1, libjxl），Rust 1.70+

## 命令

### 参数组合（7种有效模式）

| 参数 | 模式 | 行为 |
|------|------|------|
| (无) | 默认 | 单次编码，使用AI预测CRF |
| `--compress` | 仅压缩 | 确保输出 < 输入 |
| `--explore` | 仅体积 | 二分搜索最小文件 |
| `--match-quality` | 质量匹配 | 单次编码 + SSIM验证 |
| `--explore --match-quality` | 精确 | 二分搜索 + SSIM验证 |
| `--explore --match-quality --compress` | 完整 | 精确质量 + 必须压缩 |
| `--explore --match-quality --compress --ultimate` | 🔥 极限 | 持续搜索直到SSIM饱和 |

## macOS应用

双击 `Modern Format Boost.app` 即可拖拽转换，默认参数：
`--explore --match-quality --compress --in-place`

---

**Version**: 6.9.16 | **Updated**: 2025-12-25 | [CHANGELOG](CHANGELOG.md)
