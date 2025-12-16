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
- **BPP Analysis**: Calculates bits-per-pixel from video bitrate (excludes audio)
- **Codec Efficiency**: H.264=1.0, HEVC=0.65, AV1=0.50, VVC=0.35
- **GOP Structure**: Analyzes keyframe interval and B-frame pyramid
- **Content Detection**: Animation/Film/Screen recording optimization
- **HDR Support**: BT.2020 color space detection

### 2. CRF Binary Search Explorer
- **Three-phase search**: Coarse → Fine → Refine
- **SSIM validation**: Default threshold ≥ 0.95
- **Transparency report**: Shows every iteration with metrics
- **Confidence scoring**: Sampling coverage + prediction accuracy

### 3. GPU Hardware Acceleration

| Platform | HEVC | AV1 | H.264 |
|----------|------|-----|-------|
| NVIDIA NVENC | hevc_nvenc | av1_nvenc | h264_nvenc |
| Apple VideoToolbox | hevc_videotoolbox | - | h264_videotoolbox |
| Intel QSV | hevc_qsv | av1_qsv | h264_qsv |
| AMD AMF | hevc_amf | av1_amf | h264_amf |
| VA-API (Linux) | hevc_vaapi | av1_vaapi | h264_vaapi |

### 4. Conversion Logic

**Static Images:**
- JPEG → JXL: Lossless DCT transcode (zero quality loss)
- PNG/TIFF/BMP → JXL: Mathematical lossless
- WebP/AVIF/HEIC (lossy) → Skip (avoid generation loss)

**Animated Images (≥3s duration):**
- GIF/APNG → HEVC/AV1 MP4
- Animated WebP → HEVC MP4 (with `--apple-compat`)
- Short animations (<3s) → Skip

**Video:**
- H.264/MPEG/MJPEG → HEVC/AV1
- HEVC/AV1/VP9 → Skip (already modern)
- AV1/VP9 → HEVC (with `--apple-compat`)

## Installation

```bash
cd modern_format_boost
./build_all.sh
```

**Dependencies:** FFmpeg (libx265, libsvtav1, libjxl), Rust 1.70+


## Commands

### Subcommands

```bash
# Analyze media properties
vidquality-hevc analyze input.mp4
vidquality-hevc analyze input.mp4 --output json

# Auto convert (intelligent mode selection)
vidquality-hevc auto input.mp4 [OPTIONS]

# Simple convert (all → target format)
vidquality-hevc simple input.mp4

# Show recommended strategy
vidquality-hevc strategy input.mp4
```

### Flag Combinations (7 Valid Modes)

| Flags | Mode | Behavior |
|-------|------|----------|
| (none) | Default | Single encode with AI-predicted CRF |
| `--compress` | Compress-Only | Ensure output < input (even 1KB) |
| `--explore` | Size-Only | Binary search for smallest file |
| `--match-quality` | Quality-Match | Single encode + SSIM validation |
| `--compress --match-quality` | Compress+Quality | output < input + SSIM check |
| `--explore --match-quality` | Precise | Binary search + SSIM validation |
| `--explore --match-quality --compress` | Full | Precise quality + must compress |
| `--explore --match-quality --compress --ultimate` | 🔥 Ultimate | Search until SSIM saturates (Domain Wall) |

**Invalid combinations:**
- `--explore --compress` (conflicting goals)
- `--ultimate` alone or with incomplete flag combinations

### All Options

```bash
-o, --output <DIR>     Output directory
-f, --force            Overwrite existing files
-r, --recursive        Recursive directory scan
--delete-original      Delete original after conversion
--in-place             Convert and delete original (replace)
--lossless             Mathematical lossless (very slow)
--apple-compat         Convert AV1/VP9 → HEVC for Apple devices
--ultimate             🔥 v6.2: Ultimate explore mode (SSIM saturation)
                       Must use with --explore --match-quality --compress
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
│   ├── quality_matcher.rs  # BPP→CRF prediction algorithm
│   ├── gpu_accel.rs        # Multi-platform GPU detection
│   ├── flag_validator.rs   # Flag combination validation
│   ├── ssim_mapping.rs     # PSNR→SSIM dynamic mapping
│   ├── lru_cache.rs        # LRU cache with eviction
│   ├── checkpoint.rs       # Checkpoint/resume + atomic delete
│   └── error_handler.rs    # Unified error handling
├── xmp_merger/             # XMP sidecar merging tool
├── scripts/                # Drag-and-drop scripts
└── Modern Format Boost.app # macOS GUI app
```

### 5. Error Handling System
Three-level error classification with loud reporting:
- **Recoverable**: Log warning, use fallback, continue
- **Fatal**: Log error, abort operation
- **Optional**: Log info, continue (non-critical)

### 6. Checkpoint & Resume
- **Progress tracking**: Resume after interruption
- **Atomic delete**: Verify output integrity before deleting original
- **Lock file**: Prevent concurrent processing

### 7. LRU Cache
- **Capacity limit**: Auto-evict oldest entries
- **Persistence**: Save/load to JSON file
- **Memory safety**: Prevent long-running memory leaks

### 8. PSNR→SSIM Mapping
- **Dynamic prediction**: Linear interpolation from collected data
- **Self-correction**: Update mapping with actual measurements
- **Transparency**: Show predicted vs actual in reports


## Quality Validation System

### SSIM Thresholds
- Default: ≥ 0.95 (visually lossless)
- Conservative: ≥ 0.98 (use `--cpu`)
- GPU ceiling: ~0.95 (VideoToolbox limitation)

### Confidence Report
```
┌─────────────────────────────────────────────────────
│ 📊 Confidence Report
├─────────────────────────────────────────────────────
│ 📈 Overall Confidence: 85% 🟡 Good
├─────────────────────────────────────────────────────
│ 📹 Sampling Coverage: 90% (weight 30%)
│ 🎯 Prediction Accuracy: 80% (weight 30%)
│ 💾 Safety Margin: 85% (weight 20%)
│ 📊 SSIM Reliability: 88% (weight 20%)
└─────────────────────────────────────────────────────
```

### Transparency Report
```
┌────┬──────────────┬───────────┬─────────────┬─────────────┐
│ #  │ Phase        │ CRF       │ Size Change │ SSIM        │
├────┼──────────────┼───────────┼─────────────┼─────────────┤
│  1 │ Coarse       │ CRF  23.0 │  -45.2% ✅  │ 0.9612 ✅   │
│  2 │ Fine         │ CRF  20.0 │  -32.1% ✅  │ 0.9734 ✅   │
│  3 │ Refine       │ CRF  18.5 │  -25.8% ✅  │ 0.9821 ✅   │
└────┴──────────────┴───────────┴─────────────┴─────────────┘
```

## Supported Formats

**Video Input:** mp4, mkv, avi, mov, webm, flv, wmv, m4v, mpg, mpeg, ts, mts
**Image Input:** png, jpg, jpeg, webp, gif, tiff, tif, heic, avif
**Video Output:** MP4 (HEVC/AV1), MKV (lossless)
**Image Output:** JXL

## Metadata Preservation

All 4 conversion tools automatically preserve metadata via `shared_utils::copy_metadata`:
- **EXIF/IPTC/XMP**: Via ExifTool (internal metadata)
- **XMP Sidecar (v5.76)**: Auto-detect and merge `photo.jpg.xmp` or `photo.xmp` to output
- **macOS**: ACL, xattr, creation time, Date Added
- **Timestamps**: Access/modification time preserved after conversion

### XMP Sidecar Auto-Merge (v5.76)

During conversion, tools automatically detect XMP sidecar files:
1. `photo.jpg.xmp` (Adobe standard)
2. `photo.xmp` (same stem)
3. Case-insensitive matching (`photo.XMP`, `photo.Xmp`)

### XMP Sidecar Merger (Standalone Tool)

Batch merge XMP sidecar files (from Lightroom/Capture One):

```bash
xmp-merge /path/to/directory
xmp-merge --delete-xmp /path/to/directory  # Delete .xmp after merge
```

## macOS App

Double-click `Modern Format Boost.app` for drag-and-drop conversion with default flags:
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
- **BPP分析**：从视频码率计算每像素比特数（排除音频）
- **编码效率**：H.264=1.0, HEVC=0.65, AV1=0.50, VVC=0.35
- **GOP结构**：分析关键帧间隔和B帧金字塔
- **内容检测**：动画/电影/屏幕录制优化
- **HDR支持**：BT.2020色彩空间检测

### 2. CRF二分搜索探索器
- **三阶段搜索**：粗搜索 → 精搜索 → 微调
- **SSIM验证**：默认阈值 ≥ 0.95
- **透明度报告**：显示每次迭代的详细指标
- **置信度评分**：采样覆盖度 + 预测准确度

### 3. GPU硬件加速

| 平台 | HEVC | AV1 | H.264 |
|------|------|-----|-------|
| NVIDIA NVENC | hevc_nvenc | av1_nvenc | h264_nvenc |
| Apple VideoToolbox | hevc_videotoolbox | - | h264_videotoolbox |
| Intel QSV | hevc_qsv | av1_qsv | h264_qsv |
| AMD AMF | hevc_amf | av1_amf | h264_amf |
| VA-API (Linux) | hevc_vaapi | av1_vaapi | h264_vaapi |

### 4. 转换逻辑

**静态图片：**
- JPEG → JXL：无损DCT转码（零质量损失）
- PNG/TIFF/BMP → JXL：数学无损
- WebP/AVIF/HEIC（有损）→ 跳过（避免代际损失）

**动态图片（≥3秒）：**
- GIF/APNG → HEVC/AV1 MP4
- 动态WebP → HEVC MP4（使用 `--apple-compat`）
- 短动画（<3秒）→ 跳过

**视频：**
- H.264/MPEG/MJPEG → HEVC/AV1
- HEVC/AV1/VP9 → 跳过（已是现代编码）
- AV1/VP9 → HEVC（使用 `--apple-compat`）


## 安装

```bash
cd modern_format_boost
./build_all.sh
```

**依赖：** FFmpeg（libx265, libsvtav1, libjxl），Rust 1.70+

## 命令

### 子命令

```bash
# 分析媒体属性
vidquality-hevc analyze input.mp4
vidquality-hevc analyze input.mp4 --output json

# 自动转换（智能模式选择）
vidquality-hevc auto input.mp4 [选项]

# 简单转换（全部 → 目标格式）
vidquality-hevc simple input.mp4

# 显示推荐策略
vidquality-hevc strategy input.mp4
```

### 参数组合（7种有效模式）

| 参数 | 模式 | 行为 |
|------|------|------|
| (无) | 默认 | 单次编码，使用AI预测CRF |
| `--compress` | 仅压缩 | 确保输出 < 输入（哪怕1KB）|
| `--explore` | 仅体积 | 二分搜索最小文件 |
| `--match-quality` | 质量匹配 | 单次编码 + SSIM验证 |
| `--compress --match-quality` | 压缩+质量 | 输出 < 输入 + SSIM检查 |
| `--explore --match-quality` | 精确 | 二分搜索 + SSIM验证 |
| `--explore --match-quality --compress` | 完整 | 精确质量 + 必须压缩 |
| `--explore --match-quality --compress --ultimate` | 🔥 极限 | 持续搜索直到SSIM饱和（领域墙）|

**无效组合：**
- `--explore --compress`（目标冲突）
- `--ultimate` 单独使用或与不完整组合搭配

### 所有选项

```bash
-o, --output <目录>    输出目录
-f, --force            覆盖已存在文件
-r, --recursive        递归扫描目录
--delete-original      转换后删除原文件
--in-place             原地转换（替换原文件）
--lossless             数学无损（非常慢）
--apple-compat         AV1/VP9 → HEVC（Apple设备兼容）
--ultimate             🔥 v6.2: 极限探索模式（SSIM饱和）
                       必须与 --explore --match-quality --compress 组合使用
```

## 质量验证系统

### SSIM阈值
- 默认：≥ 0.95（视觉无损）
- 保守：≥ 0.98（使用 `--cpu`）
- GPU上限：~0.95（VideoToolbox限制）

## 高级功能

### 5. 错误处理系统
三级错误分类，响亮报告：
- **Recoverable**：记录警告，使用回退，继续执行
- **Fatal**：记录错误，中断操作
- **Optional**：记录信息，继续执行（非关键）

### 6. 断点续传
- **进度追踪**：中断后可恢复
- **原子删除**：验证输出完整性后才删除原文件
- **锁文件**：防止并发处理

### 7. LRU缓存
- **容量限制**：自动驱逐最旧条目
- **持久化**：保存/加载JSON文件
- **内存安全**：防止长时间运行内存泄漏

### 8. PSNR→SSIM映射
- **动态预测**：从收集的数据线性插值
- **自校正**：用实际测量值更新映射
- **透明度**：报告中显示预测值 vs 实际值

## 支持格式

**视频输入：** mp4, mkv, avi, mov, webm, flv, wmv, m4v, mpg, mpeg, ts, mts
**图片输入：** png, jpg, jpeg, webp, gif, tiff, tif, heic, avif
**视频输出：** MP4（HEVC/AV1），MKV（无损）
**图片输出：** JXL

## 元数据保留

所有4个转换工具通过 `shared_utils::copy_metadata` 自动保留元数据：
- **EXIF/IPTC/XMP**：通过ExifTool（内部元数据）
- **XMP边车 (v5.76)**：自动检测并合并 `photo.jpg.xmp` 或 `photo.xmp` 到输出文件
- **macOS**：ACL、xattr、创建时间、Date Added
- **时间戳**：转换后保留访问/修改时间

### XMP边车自动合并 (v5.76)

转换时自动检测XMP边车文件：
1. `photo.jpg.xmp`（Adobe标准）
2. `photo.xmp`（同名）
3. 大小写不敏感（`photo.XMP`、`photo.Xmp`）

### XMP边车合并工具（独立工具）

批量合并XMP边车文件（来自Lightroom/Capture One）：

```bash
xmp-merge /path/to/directory
xmp-merge --delete-xmp /path/to/directory  # 合并后删除.xmp
```

## macOS应用

双击 `Modern Format Boost.app` 即可拖拽转换，默认参数：
`--explore --match-quality --compress --in-place`

---

## Version History / 版本历史

### v6.4.4 (2025-12) - Code Quality Improvements / 代码质量改进
- 🔧 **Strategy helper methods**: `build_result()`, `binary_search_compress()`, `binary_search_quality()`, `log_final_result()` reduce ~40% duplicate code
- 🔧 **Enhanced documentation**: Rustdoc comments with examples for public APIs
- 🔧 **Boundary tests**: Edge cases for metadata margin (0, u64::MAX, threshold boundaries)
- 🔧 **SsimResult helpers**: `is_actual()`, `is_predicted()` methods

### v6.4.4 (2025-12) - 代码质量改进
- 🔧 **Strategy 辅助方法**：`build_result()`, `binary_search_compress()`, `binary_search_quality()`, `log_final_result()` 减少约 40% 重复代码
- 🔧 **增强文档注释**：公开 API 添加 Rustdoc 注释和示例
- 🔧 **边界测试**：元数据余量边界测试（0, u64::MAX, 阈值边界）
- 🔧 **SsimResult 辅助方法**：`is_actual()`, `is_predicted()` 方法

### v6.4.3 (2025-12) - Dynamic Metadata Margin / 动态元数据余量
- 🔥 **Percentage + min/max strategy**: `max(input × 0.5%, 2KB).min(100KB)`
- 🔥 **Small file threshold**: 10MB (was 100KB)
- 🔥 **CompressionVerifyStrategy enum**: Consistent comparison logic
- 🔥 **verify_compression_precise()**: Returns 3-tuple with strategy info

### v6.4.3 (2025-12) - 动态元数据余量
- 🔥 **百分比 + 最小/最大策略**：`max(input × 0.5%, 2KB).min(100KB)`
- 🔥 **小文件阈值**：10MB（原为 100KB）
- 🔥 **CompressionVerifyStrategy 枚举**：统一的比较逻辑
- 🔥 **verify_compression_precise()**：返回 3 元组包含策略信息

### v6.2 (2025-12) - Ultimate Explore Mode / 极限探索模式
- 🔥 **`--ultimate` flag**: Search until SSIM fully saturates (Domain Wall)
- 🔥 **Adaptive wall limit**: `min(ceil(log2(crf_range)) + 6, 20)` based on CRF range
- 🔥 **8 consecutive zero-gains** for SSIM saturation detection (vs 4 in normal mode)
- 🔥 **Smart size diff display**: Auto-select B/KB/MB unit for small files
- 🔥 **Removed `--cpu` flag**: GPU coarse + CPU fine search is now default behavior

### v6.2 (2025-12) - 极限探索模式
- 🔥 **`--ultimate` 参数**：持续搜索直到 SSIM 完全饱和（领域墙）
- 🔥 **自适应撞墙上限**：基于 CRF 范围计算 `min(ceil(log2(crf_range)) + 6, 20)`
- 🔥 **8 次连续零增益** 用于 SSIM 饱和检测（普通模式为 4 次）
- 🔥 **智能大小差异显示**：小文件自动选择 B/KB/MB 单位
- 🔥 **移除 `--cpu` 参数**：GPU 粗搜索 + CPU 精细搜索现为默认行为

---

**Version**: 6.4.4 | **Updated**: 2025-12-16
