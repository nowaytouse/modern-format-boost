# Modern Format Boost

🗃️ Collection-Grade Media Archive Tool - Premium Quality for Long-term Storage

[English](#tools-overview) | [中文](#工具概览)

---

## 🎯 Positioning: Collection/Archive Optimization Tool

**Target Users**: Digital collectors, archivists, media libraries, long-term storage

**Core Philosophy**: Preserve Everything, Upgrade Wisely

| Priority | Description |
|----------|-------------|
| 🥇 Preservation | Complete metadata, ICC profiles, timestamps |
| 🥈 Quality | Lossless or visually lossless only |
| 🥉 Compatibility | Apple ecosystem support (HEVC option) |

### Tool Ecosystem Comparison

| Tool | Target | Strategy | Quality | Speed |
|------|--------|----------|---------|-------|
| **static2jxl** | Photographers | Lossless JPEG transcode | ⭐⭐⭐⭐⭐ | ⭐⭐⭐ |
| **static2avif** | Meme/Stickers | Lossy compression | ⭐⭐⭐ | ⭐⭐⭐⭐ |
| **modern_format_boost** | Collections | Smart upgrade | ⭐⭐⭐⭐⭐ | ⭐⭐ |

---

High-quality media format upgrade toolkit with complete metadata preservation. Converts legacy formats to modern efficient formats (JXL, HEVC/H.265, AV1) while preserving all metadata.

---

## Tools Overview

| Tool | Input | Output | Encoder | Use Case |
|------|-------|--------|---------|----------|
| **imgquality** | Images/Animations | JXL / AV1 MP4 | cjxl, SVT-AV1 | Best compression ratio |
| **imgquality-hevc** | Images/Animations | JXL / HEVC MP4 | cjxl, x265 | Apple ecosystem compatibility |
| **vidquality** | Videos | AV1 MP4 | SVT-AV1 | Best compression ratio |
| **vidquality-hevc** | Videos | HEVC MP4 | x265 | Apple ecosystem compatibility |

## Key Features

### Smart Format Detection & Conversion Logic

**Static Images:**
| Input | Lossless? | Output | Notes |
|-------|-----------|--------|-------|
| JPEG | N/A | JXL (lossless transcode) | Preserves DCT coefficients, reversible |
| PNG/BMP/TIFF | Yes | JXL (d=0) | Mathematical lossless |
| WebP/AVIF/HEIC | Yes | JXL (d=0) | Modern lossless → JXL |
| WebP/AVIF/HEIC | No | SKIP | Avoid generation loss |

**Animations (≥3 seconds only):**
| Input | Output | Notes |
|-------|--------|-------|
| GIF/APNG/WebP (lossless) | HEVC/AV1 MP4 | Significant size reduction |
| GIF/APNG/WebP (lossy) | SKIP | Unless `--lossless` or `--match-quality` |

**Videos:**
| Input Codec | Output | Notes |
|-------------|--------|-------|
| H.264 | HEVC/AV1 | Upgrade to modern codec |
| H.265/AV1/VP9 | SKIP | Already modern |
| Lossless | Lossless HEVC/AV1 | Preserve quality |

### Quality Modes

- **Default** - Lossless transcode for JPEG, mathematical lossless for PNG/BMP
- **`--match-quality`** - Auto-calculate optimal CRF based on input quality analysis
  - **Video tools**: Enabled by default (use `--match-quality=false` to disable)
  - **Image tools**: Disabled by default (use `--match-quality` to enable)
- **`--lossless`** - Mathematical lossless HEVC/AV1 (very slow, large files)

### Complete Metadata Preservation

- **EXIF/IPTC/XMP** - All image metadata via exiftool
- **ICC Profiles** - Color profiles preserved
- **Timestamps** - mtime/atime/ctime preserved
- **macOS xattr** - Extended attributes (WhereFroms, quarantine, etc.)
- **macOS birthtime** - Creation time preserved

### Safety Features

- **Smart rollback** - Skips if output is larger than input
- **Dangerous directory detection** - Prevents accidental conversion in system directories
- **Duration threshold** - Animations <3 seconds are skipped
- **Format validation** - Skips already-modern lossy formats to avoid generation loss

### Performance

- **Parallel processing** - Multi-threaded with configurable concurrency
- **Progress visualization** - Real-time progress bar with ETA
- **CPU-aware** - Auto-limits threads to prevent system overload

## Usage

### Build

```bash
# Build all tools
cargo build --release

# Binaries will be in target/release/
```

### Image Conversion

```bash
# Auto-convert directory (JPEG→JXL, PNG→JXL, long animations→HEVC)
./target/release/imgquality-hevc auto /path/to/images

# With original file deletion after successful conversion
./target/release/imgquality-hevc auto /path/to/images --delete-original

# In-place mode (same as --delete-original)
./target/release/imgquality-hevc auto /path/to/images --in-place

# Match quality mode (auto-calculate CRF for animations)
./target/release/imgquality-hevc auto /path/to/images --match-quality --delete-original

# Mathematical lossless mode (very slow!)
./target/release/imgquality-hevc auto /path/to/images --lossless
```

### Video Conversion

```bash
# Auto-convert videos (H.264→HEVC, quality matching enabled by default)
./target/release/vidquality-hevc auto /path/to/videos

# With original deletion
./target/release/vidquality-hevc auto /path/to/videos --delete-original

# Disable quality matching (use fixed CRF)
./target/release/vidquality-hevc auto /path/to/videos --match-quality=false
```

### Analysis & Verification

```bash
# Analyze image quality
./target/release/imgquality-hevc analyze image.jpg --recommend

# Analyze with JSON output (for scripting)
./target/release/imgquality-hevc analyze image.jpg --output json

# Verify conversion quality (PSNR/SSIM comparison)
./target/release/imgquality-hevc verify original.png converted.jxl
```

## Commands

### `auto` - Smart Auto-Conversion

| Option | Description |
|--------|-------------|
| `--output`, `-o` | Output directory (default: same as input) |
| `--force`, `-f` | Force conversion even if already processed |
| `--recursive`, `-r` | Process subdirectories |
| `--delete-original` | Delete original after successful conversion |
| `--in-place` | Same as --delete-original |
| `--lossless` | Mathematical lossless mode (very slow) |
| `--match-quality` | Auto-calculate CRF based on input quality |

### `analyze` - Quality Analysis

| Option | Description |
|--------|-------------|
| `--recursive`, `-r` | Analyze directory recursively |
| `--output`, `-o` | Output format: `human` or `json` |
| `--recommend`, `-r` | Include upgrade recommendation |

### `verify` - Conversion Verification

Compares original and converted files using PSNR and SSIM metrics.

## Dependencies

```bash
# macOS
brew install jpeg-xl ffmpeg exiftool

# Linux (Debian/Ubuntu)
apt install libjxl-tools ffmpeg libimage-exiftool-perl
```

## Project Structure

```
modern_format_boost/
├── imgquality_API/      # Image tool with AV1 encoder
├── imgquality_hevc/     # Image tool with HEVC encoder (Apple compatible)
├── vidquality_API/      # Video tool with AV1 encoder
├── vidquality_hevc/     # Video tool with HEVC encoder (Apple compatible)
└── shared_utils/        # Common utilities (progress bar, safety checks)
```

## Why HEVC vs AV1?

| Aspect | HEVC (x265) | AV1 (SVT-AV1) |
|--------|-------------|---------------|
| Compression | Good | Better (~20% smaller) |
| Speed | Fast | Slower |
| Apple Support | Native | Requires software decode |
| Browser Support | Safari only | Chrome, Firefox, Edge |

**Recommendation:** Use `*-hevc` tools for Apple ecosystem, `*_API` tools for maximum compression.

---

## 工具概览

| 工具 | 输入 | 输出 | 编码器 | 适用场景 |
|------|------|------|--------|----------|
| **imgquality** | 图像/动图 | JXL / AV1 MP4 | cjxl, SVT-AV1 | 最佳压缩率 |
| **imgquality-hevc** | 图像/动图 | JXL / HEVC MP4 | cjxl, x265 | Apple 生态兼容 |
| **vidquality** | 视频 | AV1 MP4 | SVT-AV1 | 最佳压缩率 |
| **vidquality-hevc** | 视频 | HEVC MP4 | x265 | Apple 生态兼容 |

## 核心特性

### 智能格式检测与转换逻辑

**静态图像：**
| 输入 | 无损？ | 输出 | 说明 |
|------|--------|------|------|
| JPEG | N/A | JXL（无损转码） | 保留 DCT 系数，可逆 |
| PNG/BMP/TIFF | 是 | JXL (d=0) | 数学无损 |
| WebP/AVIF/HEIC | 是 | JXL (d=0) | 现代无损 → JXL |
| WebP/AVIF/HEIC | 否 | 跳过 | 避免代际损失 |

**动图（仅 ≥3 秒）：**
| 输入 | 输出 | 说明 |
|------|------|------|
| GIF/APNG/WebP（无损） | HEVC/AV1 MP4 | 显著减小体积 |
| GIF/APNG/WebP（有损） | 跳过 | 除非使用 `--lossless` 或 `--match-quality` |

**视频：**
| 输入编码 | 输出 | 说明 |
|----------|------|------|
| H.264 | HEVC/AV1 | 升级到现代编码 |
| H.265/AV1/VP9 | 跳过 | 已是现代格式 |
| 无损 | 无损 HEVC/AV1 | 保持质量 |

### 质量模式

- **默认** - JPEG 无损转码，PNG/BMP 数学无损
- **`--match-quality`** - 根据输入质量分析自动计算最佳 CRF
  - **视频工具**：默认开启（使用 `--match-quality=false` 关闭）
  - **图像工具**：默认关闭（使用 `--match-quality` 开启）
- **`--lossless`** - 数学无损 HEVC/AV1（非常慢，文件大）

### 完整元数据保留

- **EXIF/IPTC/XMP** - 通过 exiftool 保留所有图像元数据
- **ICC 配置文件** - 保留颜色配置
- **时间戳** - 保留 mtime/atime/ctime
- **macOS xattr** - 扩展属性（WhereFroms、quarantine 等）
- **macOS birthtime** - 保留创建时间

### 安全特性

- **智能回退** - 输出大于输入时跳过
- **危险目录检测** - 防止在系统目录中意外转换
- **时长阈值** - <3 秒的动图被跳过
- **格式验证** - 跳过已是现代有损格式以避免代际损失

### 性能

- **并行处理** - 多线程，可配置并发数
- **进度可视化** - 实时进度条和预计剩余时间
- **CPU 感知** - 自动限制线程数防止系统过载

## 使用方法

### 编译

```bash
# 编译所有工具
cargo build --release

# 二进制文件在 target/release/
```

### 图像转换

```bash
# 自动转换目录（JPEG→JXL, PNG→JXL, 长动图→HEVC）
./target/release/imgquality-hevc auto /path/to/images

# 成功转换后删除原文件
./target/release/imgquality-hevc auto /path/to/images --delete-original

# 原地模式（等同于 --delete-original）
./target/release/imgquality-hevc auto /path/to/images --in-place

# 质量匹配模式（自动计算动图的 CRF）
./target/release/imgquality-hevc auto /path/to/images --match-quality --delete-original

# 数学无损模式（非常慢！）
./target/release/imgquality-hevc auto /path/to/images --lossless
```

### 视频转换

```bash
# 自动转换视频（H.264→HEVC，默认开启质量匹配）
./target/release/vidquality-hevc auto /path/to/videos

# 删除原文件
./target/release/vidquality-hevc auto /path/to/videos --delete-original

# 关闭质量匹配（使用固定 CRF）
./target/release/vidquality-hevc auto /path/to/videos --match-quality=false
```

### 分析与验证

```bash
# 分析图像质量
./target/release/imgquality-hevc analyze image.jpg --recommend

# JSON 输出（用于脚本）
./target/release/imgquality-hevc analyze image.jpg --output json

# 验证转换质量（PSNR/SSIM 对比）
./target/release/imgquality-hevc verify original.png converted.jxl
```

## 命令说明

### `auto` - 智能自动转换

| 选项 | 说明 |
|------|------|
| `--output`, `-o` | 输出目录（默认：与输入相同） |
| `--force`, `-f` | 强制转换即使已处理过 |
| `--recursive`, `-r` | 处理子目录 |
| `--delete-original` | 成功转换后删除原文件 |
| `--in-place` | 等同于 --delete-original |
| `--lossless` | 数学无损模式（非常慢） |
| `--match-quality` | 根据输入质量自动计算 CRF |

### `analyze` - 质量分析

| 选项 | 说明 |
|------|------|
| `--recursive`, `-r` | 递归分析目录 |
| `--output`, `-o` | 输出格式：`human` 或 `json` |
| `--recommend`, `-r` | 包含升级建议 |

### `verify` - 转换验证

使用 PSNR 和 SSIM 指标对比原始文件和转换后的文件。

## 依赖

```bash
# macOS
brew install jpeg-xl ffmpeg exiftool

# Linux (Debian/Ubuntu)
apt install libjxl-tools ffmpeg libimage-exiftool-perl
```

## 项目结构

```
modern_format_boost/
├── imgquality_API/      # 图像工具（AV1 编码器）
├── imgquality_hevc/     # 图像工具（HEVC 编码器，Apple 兼容）
├── vidquality_API/      # 视频工具（AV1 编码器）
├── vidquality_hevc/     # 视频工具（HEVC 编码器，Apple 兼容）
└── shared_utils/        # 公共工具（进度条、安全检查）
```

## 为什么选择 HEVC vs AV1？

| 方面 | HEVC (x265) | AV1 (SVT-AV1) |
|------|-------------|---------------|
| 压缩率 | 好 | 更好（约小 20%） |
| 速度 | 快 | 较慢 |
| Apple 支持 | 原生 | 需要软件解码 |
| 浏览器支持 | 仅 Safari | Chrome、Firefox、Edge |

**建议：** Apple 生态使用 `*-hevc` 工具，追求最大压缩率使用 `*_API` 工具。

---

MIT License
