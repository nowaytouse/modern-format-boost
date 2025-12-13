# Modern Format Boost - Usage Guide

> Based on actual code logic audit (v5.1).

The project consists of three main tool categories:
1.  **Image Tools**: (`imgquality`) Intelligent image format upgrade (JXL/AV1/HEVC).
2.  **Video Tools**: (`vidquality`) Visual quality matching video conversion (AV1/HEVC).
3.  **Metadata Tool**: (`xmp-merge`) Reliable XMP sidecar merging.

---

## 1. Image Tools (`imgquality`)

**Binary Names:** `imgquality-av1` (AV1/JXL) / `imgquality-hevc` (HEVC/JXL)

### Core Functions
*   **Static Images**: Intelligently converts legacy formats (JPEG, PNG) to **JPEG XL (JXL)**.
    *   **JPEG**: Performs **mathematically lossless** transcoding (metadata & DCT coefficients preserved).
    *   **Modern Formats (WebP/AVIF/HEIC)**:
        *   **Lossy**: Skipped by default to prevent generation loss.
        *   **Lossless**: Converted to JXL for better compression.
*   **Animated Images**: Converts GIFs and animated WebPs to efficiently compressed Video (AV1 or HEVC).

### Commands
#### `analyze` - Inspect Image Quality
Analyzes image properties (metrics, compression type, quality score).
```bash
imgquality analyze <INPUT_FILE_OR_DIR> [-r] [--output json|human]
```
*   `-r`: Recursive scan.
*   `--recommend`: Include upgrade recommendation in output.

#### `auto` - Intelligent Conversion
The main mode for batch processing.
```bash
imgquality auto <INPUT_PATH> [FLAGS]
```

**Common Flags:**
*   `-r`, `--recursive`: Recursively process directories.
*   `--in-place`: Replaces the original file (converts -> verifies -> deletes original).
*   `--cpu`: Forces CPU encoding (`libaom`/`libx265`) instead of GPU acceleration. High quality but slower.
*   `--lossless`: Forces mathematical lossless conversion (very slow).
    *   *Note*: Static JPEG/PNG are *always* converted losslessly to JXL regardless of this flag. This flag primarily affects animations (GIF -> Lossless Video).

**Animation/Video Specific Flags:**
*   `--match-quality`: (Default: `false` for img) Auto-calculates CRF to match input visual quality via SSIM.
*   `--explore`: Binary searches for the lowest bitrate that maintains quality.
*   `--compress`: Enforces that the output file **must** be smaller than the input (otherwise skips).
*   `--apple-compat` (HEVC Only):
    *   Converts animated WebP (VP8/VP9) to HEVC.
    *   Logic: High quality/Long (>3s) -> HEVC MP4; Low quality/Short (<3s) -> GIF (Bayer 256).

#### `verify` - Verify Conversion Quality
Compares original and converted files, calculating SSIM/PSNR and size reduction.
```bash
imgquality verify <ORIGINAL> <CONVERTED>
```

---

## 2. Video Tools (`vidquality`)

**Binary Names:** `vidquality-av1` / `vidquality-hevc`

### Core Functions
Matches the visual quality of the input video to produce a modern optimized copy.
*   **Lossy Sources**: Converted to AV1/HEVC MP4 with an auto-calculated CRF based on input bitrate and resolution.
*   **Lossless Sources**: Converted to Lossless AV1/HEVC.

### Commands
#### `auto` - Intelligent Video Conversion
```bash
vidquality auto <INPUT_PATH> [FLAGS]
```

**Key Flags:**
*   `--match-quality`: (Default: `true`) Enables the smart quality matching algorithm.
*   `--explore`: Uses binary search to find the optimal CRF for minimum size at target quality.
*   `--compress`: Skips conversion if the result is larger than the original.
*   `--cpu`: Forces CPU encoding.
    *   `av1`: Uses `libaom` (Maximize SSIM).
    *   `hevc`: Uses `libx265` (Target SSIM ≥ 0.98).
*   `--apple-compat`:
    *   `av1`: **Skips** AV1 conversion (as it's not natively supported on older Apple hardware).
    *   `hevc`: **Converts** modern non-Apple formats (AV1/VP9) *to* HEVC.

#### `simple` - Force Conversion
Simple mathematical lossless conversion for all inputs (Very slow, huge files).
```bash
vidquality simple <INPUT>
```

#### `analyze` & `strategy`
*   `analyze`: Output video detection results (Codec, Bitrate, SSIM-suitability).
*   `strategy`: Shows what the `auto` mode *would* do without running it.

---

## 3. Metadata Tool (`xmp-merge`)

**Binary Name:** `xmp-merge`

### Core Functions
Scans a directory for `.xmp` sidecar files and merges them into their corresponding media files (JPG, MP4, etc.).

### Usage
```bash
xmp-merge <DIRECTORY> [FLAGS]
```

**Flags:**
*   `-d`, `--delete-xmp`: Deletes the .xmp file after a successful merge.
*   `--keep-backup`: Keeps the original media file (e.g., `image.jpg_original`).
*   `--fresh`: Ignores the progress checkpoint and restarts scanning from scratch.
*   `-v`: Verbose output showing the matching strategy used (Direct Match, DocumentID, etc.).

---
---

# Modern Format Boost - 项目使用说明

> 基于实际代码逻辑审计 (v5.1).

本项目主要包含三大工具类别：
1.  **图像工具**: (`imgquality`) 智能图像格式升级 (JXL/AV1/HEVC)。
2.  **视频工具**: (`vidquality`) 视觉质量匹配视频转换 (AV1/HEVC)。
3.  **元数据工具**: (`xmp-merge`) 可靠的 XMP Sidecar 合并。

---

## 1. 图像工具 (`imgquality`)

**程序名称:** `imgquality-av1` (AV1/JXL) / `imgquality-hevc` (HEVC/JXL)

### 核心功能
*   **静态图片**: 智能将旧格式 (JPEG, PNG) 转换为 **JPEG XL (JXL)**。
    *   **JPEG**: 执行 **数学无损** 转码 (保留元数据和 DCT 系数)。
    *   **现代格式 (WebP/AVIF/HEIC)**:
        *   **有损**: 默认跳过（防止二次压缩导致的画质代际损失）。
        *   **无损**: 转换为 JXL 以获得更好的压缩率。
*   **动态图片**: 将 GIF 和动态 WebP 转换为高压缩率的视频 (AV1 或 HEVC)。

### 常用指令
#### `analyze` - 分析图像质量
分析图像属性（指标、压缩类型、质量评分）。
```bash
imgquality analyze <输入文件或目录> [-r] [--output json|human]
```
*   `-r`: 递归扫描。
*   `--recommend`: 在输出中包含升级建议。

#### `auto` - 智能转换
批量处理的主要模式。
```bash
imgquality auto <输入路径> [参数]
```

**通用参数:**
*   `-r`, `--recursive`: 递归处理子目录。
*   `--in-place`: 原地替换模式 (转换成功 -> 验证 -> 删除源文件)。
*   `--cpu`: 强制使用 CPU 编码 (`libaom`/`libx265`) 而非 GPU 加速。画质更高但速度较慢。
*   `--lossless`: 强制数学无损模式 (非常慢)。
    *   *注意*: 静态 JPEG/PNG 无论是否加此参数都会无损转为 JXL。此参数主要影响动图 (GIF -> 无损视频)。

**动图/视频专用参数:**
*   `--match-quality`: (默认为 `false` 对于图片) 通过 SSIM 自动计算 CRF 以匹配输入画质。
*   `--explore`: 启用二分搜索，尝试寻找能保持画质的最小体积。
*   `--compress`: 强制要求输出文件**必须**小于源文件 (否则跳过)。
*   `--apple-compat` (仅限 HEVC 版):
    *   将动态 WebP (VP8/VP9) 转为 HEVC。
    *   逻辑: 高画质/长动画 (>3s) -> HEVC MP4; 低画质/短动画 (<3s) -> GIF (Bayer 256色)。

#### `verify` - 验证转换质量
对比源文件和转换后的文件，计算 SSIM/PSNR 及体积缩减率。
```bash
imgquality verify <源文件> <转换后文件>
```

---

## 2. 视频工具 (`vidquality`)

**程序名称:** `vidquality-av1` / `vidquality-hevc`

### 核心功能
匹配输入视频的视觉质量，生成优化后的现代格式副本。
*   **有损源**: 转换为 AV1/HEVC MP4，CRF 值根据输入码率和分辨率自动计算。
*   **无损源**: 转换为无损 AV1/HEVC。

### 常用指令
#### `auto` - 智能视频转换
```bash
vidquality auto <输入路径> [参数]
```

**关键参数:**
*   `--match-quality`: (默认为 `true`) 启用智能画质匹配算法。
*   `--explore`: 使用二分搜索寻找目标画质下的最小体积。
*   `--compress`: 如果结果体积大于源文件，则跳过转换。
*   `--cpu`: 强制 CPU 编码。
    *   `av1`: 使用 `libaom` (追求最高 SSIM)。
    *   `hevc`: 使用 `libx265` (目标 SSIM ≥ 0.98)。
*   `--apple-compat`:
    *   `av1`: **跳过** AV1 转换 (因为旧款 Apple 设备原生不支持)。
    *   `hevc`: 将现代非 Apple 格式 (AV1/VP9) **转换** 为 HEVC。

#### `simple` - 强制转换
对所有输入进行简单的数学无损转换 (非常慢，文件巨大)。
```bash
vidquality simple <输入>
```

#### `analyze` & `strategy`
*   `analyze`: 输出视频检测结果 (编码、码率、SSIM 适用性)。
*   `strategy`: 显示 `auto` 模式将要执行的策略，但不实际执行。

---

## 3. 元数据工具 (`xmp-merge`)

**程序名称:** `xmp-merge`

### 核心功能
扫描目录下的 `.xmp` 伴随文件 (Sidecar)，并将元数据合并入对应的媒体文件 (JPG, MP4 等)。

### 使用方法
```bash
xmp-merge <目录> [参数]
```

**参数:**
*   `-d`, `--delete-xmp`: 合并成功后删除 .xmp 文件。
*   `--keep-backup`: 保留源媒体文件的备份 (例如 `image.jpg_original`)。
*   `--fresh`: 忽略之前的进度断点，重新开始扫描。
*   `-v`: 详细模式，显示使用的匹配策略 (直接匹配、文件名匹配、DocumentID 匹配等)。
