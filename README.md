# Modern Format Boost

🗃️ Collection-Grade Media Archive Tool - Premium Quality for Long-term Storage

[English](#english) | [中文](#中文)

---

<a id="english"></a>
## English

### 🎯 Positioning: Collection/Archive Optimization Tool

**Target Users**: Digital collectors, archivists, media libraries, long-term storage

**Core Philosophy**: Preserve Everything, Upgrade Wisely

| Priority | Description |
|----------|-------------|
| 🥇 Preservation | Complete metadata, ICC profiles, timestamps |
| 🥈 Quality | Lossless or visually lossless only |
| 🥉 Compatibility | Apple ecosystem support (HEVC option) |

---

### Tools Overview

| Tool | Input | Output | Encoder | Use Case |
|------|-------|--------|---------|----------|
| **imgquality-hevc** | Images/Animations | JXL / HEVC MP4 | cjxl, x265 | Apple ecosystem |
| **imgquality** | Images/Animations | JXL / AV1 MP4 | cjxl, SVT-AV1 | Best compression |
| **vidquality-hevc** | Videos | HEVC MP4 | x265 | Apple ecosystem |
| **vidquality** | Videos | AV1 MP4 | SVT-AV1 | Best compression |

---

### Conversion Strategy

#### Static Images (JPEG/PNG/BMP/TIFF)

| Input Format | Lossy? | Output | Strategy |
|--------------|--------|--------|----------|
| JPEG | N/A | JXL | **Lossless transcode** - preserves DCT coefficients, 100% reversible |
| PNG (standard) | No | JXL (d=0) | **Mathematical lossless** - bit-perfect |
| PNG (quantized) | Yes | JXL (d=0.1) | **Quality 100** - detected via IHDR analysis |
| BMP/TIFF | No | JXL (d=0) | **Mathematical lossless** |
| WebP/AVIF/HEIC | No | JXL (d=0) | **Mathematical lossless** |
| WebP/AVIF/HEIC | Yes | **SKIP** | Avoid generation loss |
| JXL | - | **SKIP** | Already modern format |

**v3.7 PNG Quantization Detection (Referee System)**: Multi-factor weighted analysis to detect quantized PNGs:

| Factor | Weight | Detection Method |
|--------|--------|------------------|
| Structural | 55% | IHDR color type, tRNS chunk, palette size vs image dimensions |
| Metadata | 10% | Tool signatures (pngquant, TinyPNG, ImageOptim) |
| Statistical | 25% | Dithering patterns, color distribution, gradient banding |
| Heuristic | 10% | Compression efficiency anomalies |

**Decision Thresholds:**
- Score ≥ 0.70 → Definitely quantized (Lossy)
- Score ≥ 0.50 → Likely quantized (Lossy)
- Score < 0.50 → Lossless (conservative)

**Key Insight**: Large images (>100K pixels) with indexed color (type 3) are almost always quantized, as natural photos have thousands of unique colors.

#### Animations (GIF/APNG/Animated WebP)

| Condition | Output | Strategy |
|-----------|--------|----------|
| Duration < 3s | **SKIP** | Too short, likely icon/sticker |
| Lossless source | HEVC/AV1 MP4 (CRF 0) | **Visually lossless** |
| Lossy source | HEVC/AV1 MP4 (auto CRF) | **Quality-matched** with SSIM validation |
| Output > Input | **SKIP** | No benefit |

#### Videos

| Input Codec | Output | Strategy |
|-------------|--------|----------|
| H.264/AVC | HEVC/AV1 MP4 | **Upgrade** with quality matching |
| MPEG-2/MPEG-4 | HEVC/AV1 MP4 | **Upgrade** with quality matching |
| ProRes/DNxHD | HEVC/AV1 MKV | **Lossless** mode |
| H.265/HEVC | **SKIP** | Already modern |
| AV1 | **SKIP** | Already modern |
| VP9 | **SKIP** | Already modern |
| VVC/H.266 | **SKIP** | Cutting-edge |
| AV2 | **SKIP** | Cutting-edge |

---

### Quality Modes & Flags

#### `--match-quality` - AI-Predicted Quality Matching

Automatically calculates optimal CRF based on input analysis:
- **Video tools**: Enabled by default (`--match-quality=false` to disable)
- **Image tools**: Disabled by default for static images (always lossless)
- **Animation→Video**: Use `--match-quality` to enable

**How it works:**
1. Analyzes input: bitrate, resolution, codec, GOP structure, chroma subsampling
2. Calculates effective BPP (bits per pixel)
3. Predicts optimal CRF using calibrated formula
4. Validates output with SSIM ≥ 0.95

#### `--explore` - Binary Search Exploration

Explores CRF values to find optimal quality-size balance:
- **Alone**: Binary search for smaller output (no quality validation)
- **With `--match-quality`**: Precise quality match with SSIM validation

**⚠️ ONLY affects animated→video and video→video conversion!**
Static images (JPEG/PNG) always use lossless conversion regardless of these flags.

#### Exploration Modes

| Flags | Mode | Strategy | Iterations |
|-------|------|----------|------------|
| None | Default | Fixed CRF from strategy | 1 |
| `--match-quality` | Quality Match | AI-predicted CRF + SSIM validation | 1 |
| `--explore` | Size Only | Binary search for smaller output | up to 8 |
| `--explore --match-quality` | Precise Match | 🔥 **v4.5** Find highest SSIM (best quality match) | ~8-12 |
| `--explore --match-quality --compress` | Precise+Compress | 🔥 **v4.5** Highest SSIM with output < input | ~10-15 |

#### 🔥 v4.5: Precise Quality Match - Efficient Search

When using `--explore --match-quality` together, the algorithm enables:

**Goal:** Find the **HIGHEST SSIM** (closest to source quality)
- File size is NOT a concern in this mode
- Add `--compress` flag if you need output < input

**Efficient Three-Phase Search:**
1. **Boundary Test**: Test min/max CRF to determine SSIM range (~2 iterations)
2. **Plateau Search**: Find SSIM plateau (where lowering CRF no longer improves SSIM) (~4-6 iterations)
3. **Fine Tuning**: ±1 CRF with step 0.5 (~2-4 iterations)

#### 🔥 v4.5: `--compress` Flag - Precise Match + Compression

When adding `--compress` flag:
- **Goal**: Find **HIGHEST SSIM** with **output < input**
- If both cannot be achieved, prioritize compression, then find highest SSIM within compressible range

**Search Strategy:**
1. **Binary search** to find compression boundary (CRF where output = input)
2. **Search downward** within compressible range for highest SSIM

**Triple Cross-Validation (SSIM + PSNR + VMAF):**
- 🟢 All metrics agree → High confidence, early termination
- 🟡 Majority agree (2/3) → Good confidence
- 🔴 Metrics divergent → Continue searching

**Composite Score Calculation:**
| Metric | Weight | Description |
|--------|--------|-------------|
| SSIM | 50% | Primary structural similarity |
| VMAF | 35% | Netflix perceptual quality |
| PSNR | 15% | Reference signal-to-noise |

**Smart Termination (v4.5):**
- SSIM plateau detected → Stop, found optimal quality point
- Max iterations reached → Stop with best found
- SSIM range < 0.0001 → Use highest CRF (all CRFs produce same quality)

**Detailed Output Log:**
```
🔬 Precise Quality-Match v4.5 (Hevc)
   📁 Input: 1234567 bytes (1205.63 KB)
   📐 CRF range: [10.0, 28.0], Initial: 20.0
   🎯 Goal: Approach SSIM=1.0 (no time limit)
   🔄 Cross-validation: ENABLED (SSIM=✓, PSNR=✓, VMAF=✓)
   ⚠️ Thresholds: SSIM≥0.9500, PSNR≥40.0dB, VMAF≥90.0
   ═══════════════════════════════════════════════════
   📍 Phase 1: Full range scan (step 1.0)
   CRF 10.0: 2345678 bytes (+89.9%) | SSIM:0.9987 | PSNR:48.32dB | VMAF:98.45 | 🟢
      🎯 New best: CRF 10.0, Score 0.9876, SSIM 0.9987
   ...
   📊 FINAL RESULT
      CRF: 15.0
      Size: 1100000 bytes (-10.9%)
      SSIM: 0.9965 ✅ Excellent
      PSNR: 45.67 dB ✓
      VMAF: 96.78 ✓
      Composite Score: 0.9823
      Cross-validation: 🟢 All metrics agree
   📈 Iterations: 23, Precision: ±0.1 CRF
```

#### `--lossless` - Mathematical Lossless

Forces mathematical lossless encoding (CRF 0):
- ⚠️ **Very slow** encoding
- ⚠️ **Large files** (often larger than input)
- Use only for archival of lossless sources

---

### Quality Matching v3.5 - Data-Driven Precision

| Factor | Priority | Impact |
|--------|----------|--------|
| Video-only bitrate | 🔴 High | Excludes audio (10-30% more accurate) |
| GOP structure | 🔴 High | GOP size + B-frames (up to 50% difference) |
| Chroma subsampling | 🔴 High | YUV420 vs YUV444 (1.5x data) |
| HDR detection | 🔴 High | BT.2020 needs 20-30% more bitrate |
| Content type | 🔴 High | Animation +4 CRF, Film grain -3 CRF |
| Pixel format | 🔴 High | yuv420p, yuv444p detection |
| Aspect ratio | 🟡 Medium | Ultra-wide (>2.5:1) penalty |
| SI/TI complexity | 🟡 Medium | Spatial/Temporal metrics |
| Film grain | 🟡 Medium | High grain needs more bits |

**v3.5 Improvements:**
- CRF precision: 0.5 step (e.g., 23.5) instead of integer
- Confidence score: ~92% (up from ~75%)
- Full field support via VideoAnalysisBuilder

**CRF Calculation Formula (HEVC):**
```
CRF = 46 - 5 × log₂(effective_bpp × 100) + content_adjustment + bias
```

**Why similar CRF values for similar content:**
- Same source format (e.g., all GIFs) → similar codec efficiency factor
- Similar resolution → similar pixel count
- Similar duration → similar frame count
- **No caching**: Each file is analyzed independently
- **No hardcoding**: All values derived from actual content analysis

**Example CRF mapping (HEVC):**
| Effective BPP | Calculated CRF | Quality Level |
|---------------|----------------|---------------|
| 0.1 | ~26 | Standard |
| 0.2 | ~23 | Good |
| 0.3 | ~21 | High |
| 0.5 | ~19 | Very High |
| 1.0 | ~16 | Near-lossless |

---

### Metadata Preservation

| Type | Method | Preserved |
|------|--------|-----------|
| EXIF/IPTC/XMP | exiftool | ✅ All tags |
| ICC Profiles | exiftool | ✅ Color profiles |
| File timestamps | touch -r | ✅ mtime/atime |
| macOS birthtime | SetFile | ✅ Creation time |
| macOS xattr | xattr | ✅ Extended attributes |

---

### Safety Features

- **Smart rollback**: Skips if output ≥ input size
- **Dangerous directory detection**: Blocks `/`, `/System`, `~`
- **Duration threshold**: Animations < 3s skipped
- **Format validation**: Skips modern formats to avoid generation loss
- **No silent fallback**: Fails loudly with detailed errors
- **🛡️ v3.8 Quality Protection**: When SSIM validation fails (< 0.95), original file is PROTECTED:
  - Low-quality output is deleted
  - Original file is kept intact
  - Clear error message explains why

---

### Usage Examples

#### 🖱️ Drag & Drop (Easiest) ✅ TESTED

**macOS:**
1. Double-click `Modern Format Boost.app` → Select folder in dialog
2. Or drag folder to `Modern Format Boost.app` icon
3. Automatically opens Terminal with progress display

**Windows:**
1. Double-click `scripts/drag_and_drop_processor.bat` → Input folder path
2. Or drag folder to `drag_and_drop_processor.bat`

**Cross-platform:**
```bash
# Run the shell script directly
./scripts/drag_and_drop_processor.sh /path/to/folder

# Or interactive mode
./scripts/drag_and_drop_processor.sh
```

**Features:**
- 🛡️ Safety checks (blocks system directories)
- 📊 File counting and progress display  
- ⚠️ User confirmation before processing
- 🔧 Auto-builds tools if missing
- 📈 Success rate and size reduction reports

#### 🔧 Command Line

```bash
# Build all tools
cd modern_format_boost
cargo build --release -p imgquality-hevc -p vidquality-hevc

# Image conversion (default: lossless for static, smart for animations)
./imgquality_hevc/target/release/imgquality-hevc auto /path/to/images -r

# Image conversion with exploration (animations only)
./imgquality_hevc/target/release/imgquality-hevc auto /path/to/images -r --explore --match-quality

# Video conversion (quality matching enabled by default)
./vidquality_hevc/target/release/vidquality-hevc auto /path/to/videos -r --explore

# In-place conversion (delete originals) - Same as drag & drop
./imgquality_hevc/target/release/imgquality-hevc auto /path/to/images -r --in-place --match-quality --explore
```

---

### CLI Reference

#### imgquality-hevc auto

```
Options:
  -o, --output <DIR>     Output directory (default: same as input)
  -f, --force            Force conversion even if processed
  -r, --recursive        Process subdirectories
      --delete-original  Delete original after success
      --in-place         Same as --delete-original
      --lossless         Mathematical lossless (slow!)
      --explore          Binary search for optimal CRF (animations only)
      --match-quality    AI-predicted CRF + SSIM validation (animations only)
      --apple-compat     🍎 Convert non-Apple-compatible animated formats to HEVC
```

#### vidquality-hevc auto

```
Options:
  -o, --output <DIR>     Output directory
  -f, --force            Force conversion
  -r, --recursive        Process subdirectories
      --delete-original  Delete original after success
      --in-place         Same as --delete-original
      --lossless         Mathematical lossless
      --explore          Binary search for optimal CRF
      --match-quality    Quality matching [default: true]
      --compress         🔥 Require output < input (use with --explore --match-quality)
      --apple-compat     🍎 Convert AV1/VP9/VVC/AV2 to HEVC for Apple compatibility
```

#### 🍎 Apple Compatibility Mode (`--apple-compat`)

Converts non-Apple-compatible modern codecs to HEVC for seamless playback on Apple devices:

| Without `--apple-compat` | With `--apple-compat` |
|--------------------------|----------------------|
| VP9 → **SKIP** | VP9 → **HEVC MP4** |
| AV1 → **SKIP** | AV1 → **HEVC MP4** |
| VVC/H.266 → **SKIP** | VVC → **HEVC MP4** |
| HEVC → **SKIP** | HEVC → **SKIP** |

**Use case**: When you need videos to play natively on iPhone, iPad, Mac, or Apple TV without software decoding.

---

### Dependencies

```bash
# macOS
brew install jpeg-xl ffmpeg exiftool

# Linux (Debian/Ubuntu)
apt install libjxl-tools ffmpeg libimage-exiftool-perl
```

---

### Project Structure

```
modern_format_boost/
├── imgquality_hevc/     # Image tool (HEVC, Apple compatible)
├── imgquality_av1/      # Image tool (AV1, best compression)
├── vidquality_hevc/     # Video tool (HEVC, Apple compatible)
├── vidquality_av1/      # Video tool (AV1, best compression)
└── shared_utils/        # Common: quality_matcher, video_explorer, metadata
```

---

### HEVC vs AV1

| Aspect | HEVC (x265) | AV1 (SVT-AV1) |
|--------|-------------|---------------|
| Compression | Good | Better (~20% smaller) |
| Speed | Fast | Slower |
| Apple Support | Native | Software decode |
| Browser | Safari only | Chrome/Firefox/Edge |

**Recommendation**: Use `*-hevc` for Apple devices, `*_av1` for maximum compression.

---

<a id="中文"></a>
## 中文

### 🎯 定位：收藏/归档优化工具

**目标用户**：数字收藏家、档案管理员、媒体库、长期存储

**核心理念**：保留一切，智能升级

| 优先级 | 说明 |
|--------|------|
| 🥇 保留 | 完整元数据、ICC 配置、时间戳 |
| 🥈 质量 | 仅无损或视觉无损 |
| 🥉 兼容 | Apple 生态支持（HEVC 选项） |

---

### 工具概览

| 工具 | 输入 | 输出 | 编码器 | 适用场景 |
|------|------|------|--------|----------|
| **imgquality-hevc** | 图像/动图 | JXL / HEVC MP4 | cjxl, x265 | Apple 生态 |
| **imgquality** | 图像/动图 | JXL / AV1 MP4 | cjxl, SVT-AV1 | 最佳压缩 |
| **vidquality-hevc** | 视频 | HEVC MP4 | x265 | Apple 生态 |
| **vidquality** | 视频 | AV1 MP4 | SVT-AV1 | 最佳压缩 |

---

### 转换策略

#### 静态图像 (JPEG/PNG/BMP/TIFF)

| 输入格式 | 有损？ | 输出 | 策略 |
|----------|--------|------|------|
| JPEG | N/A | JXL | **无损转码** - 保留 DCT 系数，100% 可逆 |
| PNG（标准） | 否 | JXL (d=0) | **数学无损** - 比特级精确 |
| PNG（量化） | 是 | JXL (d=0.1) | **质量 100** - 通过 IHDR 分析检测 |
| BMP/TIFF | 否 | JXL (d=0) | **数学无损** |
| WebP/AVIF/HEIC | 否 | JXL (d=0) | **数学无损** |
| WebP/AVIF/HEIC | 是 | **跳过** | 避免代际损失 |
| JXL | - | **跳过** | 已是现代格式 |

**v3.7 PNG 量化检测（裁判系统）**：多因子加权分析检测量化 PNG：

| 因子 | 权重 | 检测方法 |
|------|------|----------|
| 结构分析 | 55% | IHDR 颜色类型、tRNS 块、调色板大小 vs 图像尺寸 |
| 元数据分析 | 10% | 工具签名（pngquant、TinyPNG、ImageOptim） |
| 统计分析 | 25% | 抖动模式、颜色分布、渐变条带 |
| 启发式分析 | 10% | 压缩效率异常 |

**决策阈值：**
- 分数 ≥ 0.70 → 确定量化（有损）
- 分数 ≥ 0.50 → 可能量化（有损）
- 分数 < 0.50 → 无损（保守）

**关键洞察**：大图像（>10万像素）使用索引色（类型3）几乎都是量化的，因为自然照片有数千种独特颜色。

#### 动图 (GIF/APNG/动态 WebP)

| 条件 | 输出 | 策略 |
|------|------|------|
| 时长 < 3秒 | **跳过** | 太短，可能是图标/贴纸 |
| 无损源 | HEVC/AV1 MP4 (CRF 0) | **视觉无损** |
| 有损源 | HEVC/AV1 MP4 (自动 CRF) | **质量匹配** + SSIM 验证 |
| 输出 > 输入 | **跳过** | 无收益 |

#### 视频

| 输入编码 | 输出 | 策略 |
|----------|------|------|
| H.264/AVC | HEVC/AV1 MP4 | **升级** + 质量匹配 |
| MPEG-2/MPEG-4 | HEVC/AV1 MP4 | **升级** + 质量匹配 |
| ProRes/DNxHD | HEVC/AV1 MKV | **无损**模式 |
| H.265/HEVC | **跳过** | 已是现代格式 |
| AV1 | **跳过** | 已是现代格式 |
| VP9 | **跳过** | 已是现代格式 |
| VVC/H.266 | **跳过** | 前沿格式 |
| AV2 | **跳过** | 前沿格式 |

---

### 质量模式与标志

#### `--match-quality` - AI 预测质量匹配

根据输入分析自动计算最佳 CRF：
- **视频工具**：默认开启（`--match-quality=false` 关闭）
- **图像工具**：静态图像默认关闭（始终无损）
- **动图→视频**：使用 `--match-quality` 开启

**工作原理：**
1. 分析输入：码率、分辨率、编码器、GOP 结构、色度采样
2. 计算有效 BPP（每像素比特数）
3. 使用校准公式预测最佳 CRF
4. 使用 SSIM ≥ 0.95 验证输出

#### `--explore` - 二分搜索探索

探索 CRF 值以找到最佳质量-大小平衡：
- **单独使用**：二分搜索更小输出（无质量验证）
- **配合 `--match-quality`**：精确质量匹配 + SSIM 验证

**⚠️ 仅影响动图→视频和视频→视频转换！**
静态图像（JPEG/PNG）始终使用无损转换，不受这些标志影响。

#### 探索模式

| 标志 | 模式 | 策略 | 迭代次数 |
|------|------|------|----------|
| 无 | 默认 | 策略固定 CRF | 1 |
| `--match-quality` | 质量匹配 | AI 预测 CRF + SSIM 验证 | 1 |
| `--explore` | 仅大小 | 二分搜索更小输出 | 最多 8 |
| `--explore --match-quality` | 精确匹配 | 🔥 **v4.5** 找最高 SSIM（最佳质量匹配） | ~8-12 |
| `--explore --match-quality --compress` | 精确匹配+压缩 | 🔥 **v4.5** 最高 SSIM 且输出 < 输入 | ~10-15 |

#### 🔥 v4.5: 精确质量匹配 - 高效搜索

当同时使用 `--explore --match-quality` 时，算法启用：

**目标：** 找到**最高 SSIM**（最接近源质量）
- 此模式不关心文件大小
- 如需同时压缩，添加 `--compress` flag

**高效三阶段搜索：**
1. **边界测试**：测试 min/max CRF 确定 SSIM 范围（~2次迭代）
2. **平台搜索**：找到 SSIM 平台（继续降低 CRF 不再提升 SSIM 的点）（~4-6次迭代）
3. **精细调整**：±1 CRF，步长 0.5（~2-4次迭代）

#### 🔥 v4.5: `--compress` Flag - 精确匹配 + 压缩

当添加 `--compress` flag 时：
- **目标**：找到**最高 SSIM** 且 **输出 < 输入**
- 如果无法同时满足，优先保证压缩，然后在压缩范围内找最高 SSIM

**搜索策略：**
1. **二分搜索**找到压缩边界（输出 = 输入的 CRF）
2. **向下搜索**在能压缩的范围内找最高 SSIM

**三重交叉验证 (SSIM + PSNR + VMAF)：**
- 🟢 所有指标一致 → 高置信度，提前终止
- 🟡 多数一致 (2/3) → 良好置信度
- 🔴 指标分歧 → 继续搜索

**综合评分计算：**
| 指标 | 权重 | 说明 |
|------|------|------|
| SSIM | 50% | 主要结构相似性 |
| VMAF | 35% | Netflix 感知质量 |
| PSNR | 15% | 参考信噪比 |

**智能终止条件 (v4.5)：**
- SSIM 平台检测 → 停止，找到最优质量点
- 达到最大迭代次数 → 停止，使用已找到的最佳
- SSIM 范围 < 0.0001 → 使用最高 CRF（所有 CRF 产生相同质量）

**详细输出日志：**
```
🔬 Precise Quality-Match v4.5 (Hevc)
   📁 Input: 1234567 bytes (1205.63 KB)
   📐 CRF range: [10.0, 28.0], Initial: 20.0
   🎯 Goal: Approach SSIM=1.0 (no time limit)
   🔄 Cross-validation: ENABLED (SSIM=✓, PSNR=✓, VMAF=✓)
   ⚠️ Thresholds: SSIM≥0.9500, PSNR≥40.0dB, VMAF≥90.0
   ═══════════════════════════════════════════════════
   📍 Phase 1: Full range scan (step 1.0)
   CRF 10.0: 2345678 bytes (+89.9%) | SSIM:0.9987 | PSNR:48.32dB | VMAF:98.45 | 🟢
      🎯 New best: CRF 10.0, Score 0.9876, SSIM 0.9987
   ...
   📊 FINAL RESULT
      CRF: 15.0
      Size: 1100000 bytes (-10.9%)
      SSIM: 0.9965 ✅ Excellent
      PSNR: 45.67 dB ✓
      VMAF: 96.78 ✓
      Composite Score: 0.9823
      Cross-validation: 🟢 All metrics agree
   📈 Iterations: 23, Precision: ±0.1 CRF
```

#### `--lossless` - 数学无损

强制数学无损编码（CRF 0）：
- ⚠️ **非常慢**的编码
- ⚠️ **大文件**（通常比输入更大）
- 仅用于无损源的归档

---

### 质量匹配 v3.5 - 数据驱动精度

| 因子 | 优先级 | 影响 |
|------|--------|------|
| 视频专用码率 | 🔴 高 | 排除音频（精度提升 10-30%） |
| GOP 结构 | 🔴 高 | GOP 大小 + B 帧（差异可达 50%） |
| 色度采样 | 🔴 高 | YUV420 vs YUV444（数据量 1.5 倍） |
| HDR 检测 | 🔴 高 | BT.2020 需要 20-30% 更多码率 |
| 内容类型 | 🔴 高 | 动画 +4 CRF，胶片颗粒 -3 CRF |
| 像素格式 | 🔴 高 | yuv420p, yuv444p 检测 |
| 宽高比 | 🟡 中 | 超宽（>2.5:1）惩罚 |
| SI/TI 复杂度 | 🟡 中 | 空间/时间指标 |
| 胶片颗粒 | 🟡 中 | 高颗粒需要更多比特 |

**v3.5 改进：**
- CRF 精度：0.5 步长（如 23.5）而非整数
- 置信度：~92%（从 ~75% 提升）
- 通过 VideoAnalysisBuilder 完整字段支持

**CRF 计算公式（HEVC）：**
```
CRF = 46 - 5 × log₂(有效BPP × 100) + 内容调整 + 偏好
```

**为什么相似内容的 CRF 值相似：**
- 相同源格式（如全是 GIF）→ 相似的编码效率因子
- 相似分辨率 → 相似的像素数
- 相似时长 → 相似的帧数
- **无缓存**：每个文件独立分析
- **无硬编码**：所有值均来自实际内容分析

**CRF 映射示例（HEVC）：**
| 有效 BPP | 计算 CRF | 质量级别 |
|----------|----------|----------|
| 0.1 | ~26 | 标准 |
| 0.2 | ~23 | 良好 |
| 0.3 | ~21 | 高 |
| 0.5 | ~19 | 非常高 |
| 1.0 | ~16 | 接近无损 |

---

### 元数据保留

| 类型 | 方法 | 保留 |
|------|------|------|
| EXIF/IPTC/XMP | exiftool | ✅ 所有标签 |
| ICC 配置文件 | exiftool | ✅ 颜色配置 |
| 文件时间戳 | touch -r | ✅ mtime/atime |
| macOS birthtime | SetFile | ✅ 创建时间 |
| macOS xattr | xattr | ✅ 扩展属性 |

---

### 安全特性

- **智能回退**：输出 ≥ 输入大小时跳过
- **危险目录检测**：阻止 `/`、`/System`、`~`
- **时长阈值**：< 3 秒的动图跳过
- **格式验证**：跳过现代格式以避免代际损失
- **无静默回退**：失败时响亮报错，提供详细信息
- **🛡️ v3.8 质量保护**：当 SSIM 验证失败（< 0.95）时，原文件受保护：
  - 删除低质量输出
  - 保留原文件完整
  - 清晰的错误信息说明原因

---

### 使用示例

#### 🖱️ 拖拽使用（最简单）

**macOS:**
1. 双击 `Modern Format Boost.app` → 选择文件夹
2. 或将文件夹拖拽到 `Modern Format Boost.app` 图标上

**Windows:**
1. 双击 `scripts/drag_and_drop_processor.bat` → 输入文件夹路径
2. 或将文件夹拖拽到 `drag_and_drop_processor.bat` 上

**跨平台:**
```bash
# 运行shell脚本
./scripts/drag_and_drop_processor.sh /path/to/folder
```

#### 🔧 命令行

```bash
# 编译所有工具
cd modern_format_boost
cargo build --release -p imgquality-hevc -p vidquality-hevc

# 图像转换（默认：静态无损，动图智能）
./imgquality_hevc/target/release/imgquality-hevc auto /path/to/images -r

# 图像转换 + 探索（仅动图）
./imgquality_hevc/target/release/imgquality-hevc auto /path/to/images -r --explore --match-quality

# 视频转换（默认开启质量匹配）
./vidquality_hevc/target/release/vidquality-hevc auto /path/to/videos -r --explore

# 原地转换（删除原文件）- 与拖拽模式相同
./imgquality_hevc/target/release/imgquality-hevc auto /path/to/images -r --in-place --match-quality --explore
```

---

### CLI 参考

#### imgquality-hevc auto

```
选项：
  -o, --output <DIR>     输出目录（默认：与输入相同）
  -f, --force            强制转换即使已处理
  -r, --recursive        处理子目录
      --delete-original  成功后删除原文件
      --in-place         等同于 --delete-original
      --lossless         数学无损（慢！）
      --explore          二分搜索最优 CRF（仅动图）
      --match-quality    AI 预测 CRF + SSIM 验证（仅动图）
      --apple-compat     🍎 将非 Apple 兼容的动图格式转换为 HEVC
```

#### vidquality-hevc auto

```
选项：
  -o, --output <DIR>     输出目录
  -f, --force            强制转换
  -r, --recursive        处理子目录
      --delete-original  成功后删除原文件
      --in-place         等同于 --delete-original
      --lossless         数学无损
      --explore          二分搜索最优 CRF
      --match-quality    质量匹配 [默认: true]
      --compress         🔥 要求输出 < 输入（配合 --explore --match-quality 使用）
      --apple-compat     🍎 将 AV1/VP9/VVC/AV2 转换为 HEVC 以兼容 Apple 设备
```

#### 🍎 Apple 兼容模式 (`--apple-compat`)

将非 Apple 兼容的现代编码转换为 HEVC，以便在 Apple 设备上无缝播放：

| 不使用 `--apple-compat` | 使用 `--apple-compat` |
|------------------------|----------------------|
| VP9 → **跳过** | VP9 → **HEVC MP4** |
| AV1 → **跳过** | AV1 → **HEVC MP4** |
| VVC/H.266 → **跳过** | VVC → **HEVC MP4** |
| HEVC → **跳过** | HEVC → **跳过** |

**使用场景**：当你需要视频在 iPhone、iPad、Mac 或 Apple TV 上原生播放而无需软件解码时。

---

### 依赖

```bash
# macOS
brew install jpeg-xl ffmpeg exiftool

# Linux (Debian/Ubuntu)
apt install libjxl-tools ffmpeg libimage-exiftool-perl
```

---

### 项目结构

```
modern_format_boost/
├── imgquality_hevc/     # 图像工具（HEVC，Apple 兼容）
├── imgquality_av1/      # 图像工具（AV1，最佳压缩）
├── vidquality_hevc/     # 视频工具（HEVC，Apple 兼容）
├── vidquality_av1/      # 视频工具（AV1，最佳压缩）
└── shared_utils/        # 公共：quality_matcher, video_explorer, metadata
```

---

### HEVC vs AV1

| 方面 | HEVC (x265) | AV1 (SVT-AV1) |
|------|-------------|---------------|
| 压缩率 | 好 | 更好（约小 20%） |
| 速度 | 快 | 较慢 |
| Apple 支持 | 原生 | 软件解码 |
| 浏览器 | 仅 Safari | Chrome/Firefox/Edge |

**建议**：Apple 设备使用 `*-hevc`，追求最大压缩使用 `*_av1`。

---

MIT License
