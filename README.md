# Modern Format Boost

High-performance media conversion toolkit with intelligent quality matching, SSIM validation, and multi-platform GPU acceleration.

## 🔥 Latest Updates (v7.9.1)

### Dependency Updates & Code Quality Improvements
- **🚀 Major Dependency Updates**: All project dependencies updated to latest versions
  - `indicatif` from v0.17 to v0.18 (progress bars)
  - `console` from v0.15 to v0.16 (terminal colors)
  - `which` from v6.0 to v8.0 (command execution)
  - `libheif-rs` from v1.0 to v2.6 (HEIC/HEIF support)
  - `num_cpus` from v1.16 to v1.17 (CPU detection)
  - And many other dependencies across the workspace
- **🔧 Code Quality Fixes**: Resolved all compiler and clippy warnings
  - Fixed unused import warnings
  - Improved documentation formatting
  - Updated deprecated IO error creation patterns
- **🏗️ Workspace-Level Dependency Management**: Consolidated dependency versions in root `Cargo.toml` for consistent versioning

### Previous (v7.9.0)

### Complete Dash Vulnerability Fix - 100% Coverage
- **✅ CJXL Commands**: All `cjxl` calls now use `cjxl [flags] -- input output` syntax with `--` separator
- **✅ ImageMagick Commands**: All `magick` calls protected with `--` separator
- **✅ FFmpeg Commands**: All `ffmpeg` calls use `safe_path_arg()` to prepend `./` to dash-prefixed paths
- **✅ Comprehensive Testing**: Added `test_dash_fix.sh` script to verify protection against malicious filenames
- **✅ Security Documentation**: Added `SECURITY_FIX_SUMMARY.md` with detailed fix information

**What's Fixed:**
- Filenames starting with `-` or `--` (e.g., `-test.jpg`, `--help.png`) are now handled safely
- Prevents command injection attacks via crafted filenames
- Consistent protection across all external tool invocations (cjxl, ffmpeg, magick, x265)

### Previous (v7.8.1)

### CJXL Optimization & Security Hardening
- **✅ Corrected CJXL Arguments**: Fixed parameter ordering to `cjxl [flags] [input] [output]` for compatibility with latest cjxl versions.
- **✅ Lossless Mode Restored**: Explicitly re-enabled `--lossless_jpeg=1` for guaranteed lossless JPEG transcoding.
- **⚠️ Partial Dash Fix**: Initial `--` separator added (now completed in v7.9.0)
- **✅ Smart Threading**: Apple Silicon optimized (75% core usage) via new smart thread manager.
- **✅ GIF parsing fix**: Proper block parsing (Image Descriptors) eliminates static GIF false positives.

### Previous (v7.7.0)

### Code Quality Improvements - Enhanced Reliability & Maintainability
- **✅ Unified Logging System**: Structured logging to system temp directory with rotation
- **✅ Enhanced Error Handling**: Context-rich errors with transparent reporting
- **✅ Modular Architecture**: video_explorer split into logical submodules
- **✅ Common Utilities**: 15 reusable utility functions extracted
- **✅ Clean Dependencies**: Removed unused dependencies, workspace-level management
- **✅ Zero Warnings**: All clippy warnings fixed, code formatted with rustfmt
- **✅ 735 Tests Passing**: Comprehensive test coverage with property-based testing

**Logging Features:**
- Automatic log rotation (100MB per file, keep 5 files)
- Logs stored in system temp directory (e.g., `/tmp` or `%TEMP%`)
- Structured logging with tracing framework
- External command logging (ffmpeg, x265, etc.)

**Log File Locations:**
```bash
# macOS/Linux
/tmp/imgquality_hevc_*.log
/tmp/vidquality_hevc_*.log

# Windows
%TEMP%\imgquality_hevc_*.log
%TEMP%\vidquality_hevc_*.log
```

**Debugging:**
```bash
# View logs
tail -f /tmp/imgquality_hevc_*.log

# Check for errors
grep ERROR /tmp/vidquality_hevc_*.log
```

### Previous (v7.6.0)

### MS-SSIM Performance Optimization - 10x Faster Quality Verification
- **✅ Intelligent Sampling**: Duration-based frame sampling (1/1, 1/3, 1/10, or skip)
- **✅ Parallel Computation**: Y/U/V channels calculated simultaneously
- **✅ Real-time Progress**: Live progress display with ETA estimation
- **✅ Heartbeat Detection**: Status updates every 30s (Beijing Time)
- **✅ No Freeze Perception**: Users always know the process is alive

**Performance Gains:**
```
Video Duration    Before    After     Speedup
48 seconds        ~180s     ~30s      6x faster
5 minutes         ~600s     ~60s      10x faster
30 minutes        ~1800s    ~120s     15x faster
```

**Sampling Strategy:**
- ≤60s: Full frames (1/1) - Maximum accuracy
- 60-300s: 1/3 sampling - Balanced speed/accuracy
- 300-1800s: 1/10 sampling - Fast with acceptable accuracy
- >1800s: Skip MS-SSIM - Use SSIM fallback

**New Command-Line Options:**
```bash
--ms-ssim-sampling <N>   # Force 1/N sampling rate
--full-ms-ssim           # Force full calculation (no sampling)
--skip-ms-ssim           # Skip MS-SSIM entirely (use SSIM)
```

**Example Usage:**
```bash
# Auto sampling (recommended)
vidquality-hevc input.mp4 --match-quality

# Force full MS-SSIM for critical content
vidquality-hevc input.mp4 --match-quality --full-ms-ssim

# Force 1/5 sampling for custom balance
vidquality-hevc input.mp4 --match-quality --ms-ssim-sampling 5

# Skip MS-SSIM for very long videos
vidquality-hevc input.mp4 --match-quality --skip-ms-ssim
```

### Previous (v7.5.0)

### File Processing Optimization - Small Files First
- **✅ Intelligent Sorting**: Files processed by size (small → large)
- **✅ Quick Feedback**: Small files finish fast, see progress immediately
- **✅ Early Detection**: Problems found sooner with small files
- **✅ No Blocking**: Large files don't hold up the queue
- **✅ Modular Design**: `file_sorter.rs` module for easy maintenance

**Benefits:**
```
Processing order:
  1. tiny.jpg (10KB)    ← Fast feedback
  2. small.png (100KB)  ← Quick wins
  3. medium.gif (1MB)   ← Steady progress
  4. large.mp4 (100MB)  ← No blocking
  5. huge.mov (1GB)     ← Processed last
```

### Previous (v7.4.9)

### Output Directory Timestamp Preservation
- **✅ Root Directory**: Output directory inherits timestamp from source
- **✅ All Subdirectories**: Timestamps preserved recursively
- **Example**: `all/` (2020-01-01) → `all_optimized/` (2020-01-01) ✅

### Previous (v7.4.8)

### Complete Metadata & Structure Preservation - All Scenarios
- **✅ All 4 Tools**: imgquality/vidquality HEVC/AV1 preserve directory metadata
- **✅ All Copy Scenarios**: Conversion success, skip, failure - all preserve structure
- **✅ Folder Timestamps**: Creation, modification, access times preserved
- **✅ Permissions & Xattr**: Unix permissions and extended attributes preserved
- **✅ Directory Structure**: All subdirectories preserved in output
- **✅ File Metadata**: Timestamps, XMP sidecars auto-merged
- **✅ Progress Bars**: Clean single progress bar in parallel mode
- **✅ macOS Compatible**: Works with default bash 3.x
- **✅ Build System**: Fixed smart_build.sh script (set -e compatibility)

**What's Preserved:**
- Media files (converted): Structure + metadata + XMP ✅
- Media files (skipped/failed): Structure + metadata + XMP ✅
- Non-media files (.psd, .txt, etc.): Structure + metadata + XMP ✅
- Directories: Timestamps + permissions + xattr ✅

**Test Results:**
```
Input:  photos/2024/summer/beach.png (2020-01-01)
Output: photos/2024/summer/beach.jxl (2020-01-01) ✅
Folder: photos/2024/summer/ (timestamps preserved) ✅
XMP:    Title & Description merged ✅
```

### Previous (v7.2)
- **✅ Standalone VMAF**: Bypass ffmpeg libvmaf dependency
- **✅ Multi-layer Fallback**: vmaf → libvmaf → SSIM
- **✅ Installation**: `brew install libvmaf`

### Previous (v6.9.17)
- **✅ CPU Encoding**: x265 CLI for reliability
- **✅ GPU Fallback**: Auto CPU fallback on failures
- **✅ GIF Support**: Fixed bgra pixel format

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

### 3. Quality Verification System (v7.2)

**Fallback Chain:**
1. **Standalone vmaf** (preferred) → MS-SSIM 3-channel
2. **ffmpeg libvmaf** → MS-SSIM 3-channel
3. **ffmpeg ssim** → SSIM All (Y+U+V)
4. **ffmpeg ssim** → SSIM Y only

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

| Platform | HEVC | AV1 | H.264 | Fallback |
|----------|------|-----|-------|----------|
| NVIDIA NVENC | ✅ | ✅ | ✅ | → x265 CLI |
| Apple VideoToolbox | ✅ | - | ✅ | → x265 CLI |
| Intel QSV | ✅ | ✅ | ✅ | → x265 CLI |
| AMD AMF | ✅ | ✅ | ✅ | → x265 CLI |

**New in v6.9.17**: Automatic CPU fallback using x265 CLI when GPU encoding fails

### 5. Conversion Logic

**Static Images:** JPEG → JXL (lossless DCT), PNG/TIFF → JXL (mathematical lossless)

**Animated Images (≥3s):** GIF/APNG/WebP → HEVC/AV1 MP4

**Video:** H.264/MPEG → HEVC/AV1, AV1/VP9 → HEVC (`--apple-compat`)

## Installation

```bash
cd modern_format_boost
./smart_build.sh
```

**Dependencies:** 
- FFmpeg (libx265, libsvtav1, libjxl)
- x265 CLI: `brew install x265` (macOS) or `apt install x265` (Linux)
- libvmaf: `brew install libvmaf` (macOS) or `apt install libvmaf` (Linux)
- Rust 1.70+

**Note**: x265 CLI and libvmaf are required for reliable encoding and quality verification

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

**Whitelist + Smart Skip + Fallback Copy** mechanism ensures zero file loss.

### Format Processing Rules

| Format | Lossless | Lossy | Animated |
|--------|----------|-------|----------|
| **JPEG** | - | → JXL (DCT lossless) | - |
| **PNG/TIFF/BMP** | → JXL | - | APNG → HEVC |
| **GIF** | - | - | → HEVC (≥3s) or copy |
| **WebP/AVIF/HEIC** | → JXL | ⏭️ SKIP (avoid loss) | → HEVC (`--apple-compat`) |

### Why Skip Modern Lossy Formats?

Re-encoding lossy → lossy causes **generational quality loss**. The tool protects your files:
- `WebP lossy` → Skip (already compressed)
- `AVIF lossy` → Skip (already compressed)  
- `HEIC lossy` → Skip (already compressed)

Use `--apple-compat` to force convert animated WebP/AVIF to HEVC for Apple device compatibility.

### File Handling Strategy

| Scenario | Action | XMP | Metadata |
|----------|--------|-----|----------|
| Converted successfully | Output new format | Merged | Preserved |
| Skipped (modern lossy) | Copy original | Merged | Preserved |
| Skipped (short <3s) | Copy original | Merged | Preserved |
| Conversion failed | Copy original | Merged | Preserved |
| Unsupported (.psd, .txt) | Copy original | Merge or copy sidecar | Preserved |

### Metadata Preservation (v7.3)

**All files preserve:**
- ✅ Directory structure (all subdirectories)
- ✅ File timestamps (modification & access time)
- ✅ File permissions
- ✅ Extended attributes (xattrs, Finder tags on macOS)
- ✅ Internal metadata (Exif, ICC color profiles)
- ✅ XMP sidecar files (auto-merged)

**XMP Auto-Merge:**
- Detects `photo.jpg.xmp` and `photo.xmp` formats
- Automatically merges into output file
- Preserves all metadata fields

### Verification

`Output files = Total files - XMP sidecars`

---

## 无遗漏设计 (v6.9.16)

**白名单 + 智能跳过 + 回退复制**机制，确保零文件丢失。

### 格式处理规则

| 格式 | 无损 | 有损 | 动图 |
|------|------|------|------|
| **JPEG** | - | → JXL (DCT无损) | - |
| **PNG/TIFF/BMP** | → JXL | - | APNG → HEVC |
| **GIF** | - | - | → HEVC (≥3秒) 或复制 |
| **WebP/AVIF/HEIC** | → JXL | ⏭️ 跳过 (避免损失) | → HEVC (`--apple-compat`) |

### 文件处理策略

| 场景 | 操作 | XMP | 元数据 |
|------|------|-----|--------|
| 转换成功 | 输出新格式 | 已合并 | 已保留 |
| 跳过（现代有损） | 复制原文件 | 已合并 | 已保留 |
| 跳过（短动画<3秒） | 复制原文件 | 已合并 | 已保留 |
| 转换失败 | 复制原文件 | 已合并 | 已保留 |
| 不支持（.psd, .txt） | 复制原文件 | 合并或复制边车 | 已保留 |

### 元数据保留 (v7.3)

**所有文件保留：**
- ✅ 目录结构（所有子目录）
- ✅ 文件时间戳（修改时间和访问时间）
- ✅ 文件权限
- ✅ 扩展属性（xattrs，macOS Finder 标签）
- ✅ 内部元数据（Exif，ICC 颜色配置文件）
- ✅ XMP 边车文件（自动合并）

**XMP 自动合并：**
- 检测 `photo.jpg.xmp` 和 `photo.xmp` 格式
- 自动合并到输出文件
- 保留所有元数据字段

### 为什么跳过现代有损格式？

有损→有损重编码会导致**代际质量损失**。工具保护你的文件：
- `WebP有损` → 跳过（已压缩）
- `AVIF有损` → 跳过（已压缩）
- `HEIC有损` → 跳过（已压缩）

使用 `--apple-compat` 可强制将动态 WebP/AVIF 转换为 HEVC 以兼容 Apple 设备。

### 验证机制

`输出文件数 = 全部文件数 - XMP边车数`

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

| 平台 | HEVC | AV1 | H.264 | 降级方案 |
|------|------|-----|-------|----------|
| NVIDIA NVENC | ✅ | ✅ | ✅ | → x265 CLI |
| Apple VideoToolbox | ✅ | - | ✅ | → x265 CLI |
| Intel QSV | ✅ | ✅ | ✅ | → x265 CLI |
| AMD AMF | ✅ | ✅ | ✅ | → x265 CLI |

**v6.9.17 新增**: GPU 编码失败时自动降级到 x265 CLI CPU 编码

## 🔥 最新更新 (v7.6.0)

### MS-SSIM 性能优化 - 10倍速度提升
- **✅ 智能采样**: 基于时长的帧采样策略（1/1、1/3、1/10 或跳过）
- **✅ 并行计算**: Y/U/V 三通道同时计算
- **✅ 实时进度**: 实时进度显示和 ETA 估算
- **✅ 心跳检测**: 每30秒状态更新（北京时间）
- **✅ 无卡死感知**: 用户始终知道进程在运行

**性能提升：**
```
视频时长      优化前    优化后     加速比
48 秒         ~180秒    ~30秒      6倍
5 分钟        ~600秒    ~60秒      10倍
30 分钟       ~1800秒   ~120秒     15倍
```

**采样策略：**
- ≤60秒: 全帧（1/1）- 最高精度
- 60-300秒: 1/3 采样 - 速度与精度平衡
- 300-1800秒: 1/10 采样 - 快速且精度可接受
- >1800秒: 跳过 MS-SSIM - 使用 SSIM 降级

**新增命令行选项：**
```bash
--ms-ssim-sampling <N>   # 强制 1/N 采样率
--full-ms-ssim           # 强制完整计算（无采样）
--skip-ms-ssim           # 完全跳过 MS-SSIM（使用 SSIM）
```

**使用示例：**
```bash
# 自动采样（推荐）
vidquality-hevc input.mp4 --match-quality

# 对关键内容强制完整 MS-SSIM
vidquality-hevc input.mp4 --match-quality --full-ms-ssim

# 强制 1/5 采样以自定义平衡
vidquality-hevc input.mp4 --match-quality --ms-ssim-sampling 5

# 对超长视频跳过 MS-SSIM
vidquality-hevc input.mp4 --match-quality --skip-ms-ssim
```

### 之前版本 (v7.5.0)

### 文件处理优化 - 小文件优先
- **✅ 智能排序**: 按文件大小处理（小 → 大）
- **✅ 快速反馈**: 小文件快速完成，立即看到进度
- **✅ 早期检测**: 小文件更早发现问题
- **✅ 无阻塞**: 大文件不会阻塞队列
- **✅ 模块化设计**: `file_sorter.rs` 模块便于维护

**优势：**
```
处理顺序：
  1. tiny.jpg (10KB)    ← 快速反馈
  2. small.png (100KB)  ← 快速胜利
  3. medium.gif (1MB)   ← 稳定进展
  4. large.mp4 (100MB)  ← 无阻塞
  5. huge.mov (1GB)     ← 最后处理
```

### 之前版本 (v7.4.9)

### 完整的元数据和结构保留 - 所有场景
- **✅ 全部4个工具**: imgquality/vidquality HEVC/AV1 保留目录元数据
- **✅ 所有复制场景**: 转换成功、跳过、失败 - 全部保留结构
- **✅ 文件夹时间戳**: 创建、修改、访问时间全部保留
- **✅ 权限和扩展属性**: Unix 权限和扩展属性保留
- **✅ 目录结构**: 所有子目录在输出中保留
- **✅ 文件元数据**: 时间戳、XMP 边车自动合并
- **✅ 进度条**: 并行模式下单一清晰进度条
- **✅ macOS 兼容**: 兼容默认 bash 3.x
- **✅ 构建系统**: 修复 smart_build.sh 脚本（set -e 兼容性）

**保留内容：**
- 媒体文件（已转换）：结构 + 元数据 + XMP ✅
- 媒体文件（跳过/失败）：结构 + 元数据 + XMP ✅
- 非媒体文件（.psd、.txt 等）：结构 + 元数据 + XMP ✅
- 目录：时间戳 + 权限 + xattr ✅

### 关键修复
- **✅ CPU 编码可靠性**: 使用 x265 CLI 工具替代 FFmpeg libx265，提高兼容性
- **✅ GPU 降级系统**: GPU 编码在高 CRF 值失败时自动降级到 CPU
- **✅ GIF 格式支持**: 修复动态 GIF 文件的 bgra 像素格式处理
- **✅ CPU 校准**: 使用 x265 CLI 提高 GPU→CPU CRF 映射精度
- **✅ 错误透明化**: 所有失败都提供清晰的错误信息和降级通知

### 修复前后对比
```
❌ 修复前: CPU 校准编码失败，使用静态偏移
❌ 修复前: CRF 19.9 编码失败 - 参数列表分割错误
✅ 修复后: 校准完成: GPU 1020989 → CPU 2902004 (比率 2.842, 偏移 +2.5)
✅ 修复后: GPU 编码失败，降级到 CPU (x265 CLI) → 成功
```

## 安装

```bash
cd modern_format_boost
./smart_build.sh
```

**依赖项：** 
- FFmpeg (libx265, libsvtav1, libjxl)
- x265 CLI 工具: `brew install x265` (macOS) 或 `apt install x265` (Linux)
- Rust 1.70+

**注意**: 现在需要 x265 CLI 工具来确保可靠的 CPU HEVC 编码

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

## 故障排除

### 常见问题

**GPU 编码失败**: 系统自动降级到 CPU (x265 CLI)
```
⚠️  GPU 编码失败，降级到 CPU (x265 CLI)
✅ CPU 编码成功
```

**找不到 x265 CLI**: 安装 x265 命令行工具
```bash
# macOS
brew install x265

# Ubuntu/Debian
sudo apt install x265

# CentOS/RHEL
sudo yum install x265
```

**GIF 文件失败**: 确保 FFmpeg 支持 bgra 像素格式转换
- 系统自动转换 bgra → yuv420p
- 移除 alpha 通道以兼容 HEVC

### 错误信息

所有错误现在都**响亮报告**，提供清晰的上下文：
- `⚠️  GPU boundary verification failed at CRF X.X`
- `🔄 Retrying with CPU encoding (x265 CLI)...`
- `✅ CPU encoding succeeded` / `❌ CPU encoding also failed`

---

**版本**: 6.9.17 | **更新**: 2025-01-18 | [更新日志](CHANGELOG.md)
