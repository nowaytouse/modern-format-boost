<p align="center">
  <img src="https://img.shields.io/badge/version-0.10.81-0969DA?style=for-the-badge&logo=rust&logoColor=white" alt="Version">
  <img src="https://img.shields.io/badge/rust-2021_edition-E57324?style=for-the-badge&logo=rust&logoColor=white" alt="Rust">
  <img src="https://img.shields.io/badge/platform-macOS_%7C_Linux_%7C_Windows-8257E5?style=for-the-badge&logo=apple&logoColor=white" alt="Platform">
  <img src="https://img.shields.io/badge/license-MIT-00B265?style=for-the-badge" alt="License">
</p>

<h1 align="center">Modern Format Boost</h1>

<p align="center">
  <strong>Next-gen media optimization engine — zero quality loss, maximum compression.</strong><br>
  <em>下一代媒体优化引擎 — 画质零损失，体积最大压缩。</em>
</p>

<p align="center">
  <a href="#-english">📖 English</a> · <a href="#-简体中文">📖 简体中文</a> · <a href="#-installation--安装说明">⬇️ Install/安装</a> · <a href="#-usage--使用方法">🚀 Usage/使用</a> · <a href="#-license">⚖️ License</a>
</p>

---
# 📖 English

## What is Modern Format Boost?

**Modern Format Boost** is a high-performance, Rust-based media optimization engine. It converts legacy image and video formats (JPEG, PNG, H.264, VP9…) into cutting-edge codecs (**JPEG XL** for images, **HEVC/AV1** for videos) — achieving dramatic file size reductions while preserving or even bit-exactly matching the original quality.

Think of it as a "smart compressor" that **never degrades your media**:

- 📸 **Images**: JPEG → JXL lossless reconstruction (bit-exact, ~20% smaller); PNG/WebP/TIFF/HEIC → JXL
- 🎬 **Videos**: H.264/VP9/AV1 → HEVC with GPU-accelerated quality search
- 🍎 **Apple ecosystem first**: Full Apple compatibility mode, Live Photo detection, AAE sidecar handling
- 🔒 **Metadata guardian**: Preserves EXIF, XMP, ICC profiles, creation timestamps, macOS xattrs, Finder tags

## ⚠️ Disclaimer & Important Notes

1. **Data Safety First**: To avoid any potential data loss, it is highly recommended to output processed files to a separate directory (e.g., using `-o /path/to/output`) rather than using in-place conversion (`--in-place`), especially for irreplaceable media.
2. **Beta Software**: While this program has been extensively tested, debugged, and optimized to prevent quality or data loss (as seen in the changelog), it is not guaranteed to be 100% bug-free. Please report any issues you encounter on GitHub.
3. **Computation Insight**: While optimized for efficiency (especially on Apple Silicon M-series), processing massive batches in `--ultimate` mode can still be time-consuming. It will occupy system resources for an extended period; please plan your task accordingly.
4. **Tool Maturity**: The HEVC-based tools (`img-hevc`, `vid-hevc`) are currently more mature and stable than the AV1-based tools (`img-av1`, `vid-av1`). For high-reliability production tasks, HEVC tools are recommended.

## 🔒 Privacy & Data Integrity

**Modern Format Boost** is built on a "Local-First" architecture, ensuring your creative assets remain entirely within your control.

- **Air-Gapped Operation**: 100% offline processing. No telemetry, usage tracking, or cloud pings. The core binaries contain zero network-related code.
- **Rust-Hardened Runtime**: Built with Rust to natively eliminate memory corruption bugs (buffer overflows, etc.).
- **Secure Integration**: All external tools (FFmpeg, cjxl) are invoked via safe, escaped primitives—never through raw shell execution—preventing arbitrary command injection.
- **Path Isolation**: Advanced normalization prevents directory traversal and protects unrelated system files.
- **System Path Blocklist**: Built-in shields for sensitive system directories to prevent accidental OS file modifications.
- **Dynamic Resource Balancing**: Automatically adjusts processing threads based on memory/CPU load to prevent system crashes during extreme tasks.
- **Comprehensive Metadata Custodian**: Strict bit-for-bit preservation of EXIF, XMP, ICC, and file system timestamps (btime/mtime).
- **Secure Processing & Session Isolation**: 
  - **Zero Workspace Pollution**: Centralized tracking (`~/.mfb_progress/`) keeps your media folders 100% clean. No hidden metadata files remain among your photos/videos.
  - **Conflict-Free Temp Files**: Every intermediate analysis file (YUV streams, analysis segments) is uniquely identified with a randomized UUID. This prevents multi-instance collisions and ensures "Surgical Precision" during cleanup.
  - **Scrub-on-Start Cleanup**: Whether a task completes successfully or is resumed after an interruption, the system automatically purges all transient data. This "Self-Cleaning" architecture ensures your disk remains free of abandoned processing leftovers.


<details>
<summary><b>🛠️ Deep Technical: How It Works — The Pipeline</b></summary>

### Image Pipeline Logic
Every file goes through a multi-stage decision pipeline:
- **Stage 1 — Smart Detection**: Analyzes JPEG DQT tables, WebP VP8L chunks, and AVIF `av1C` boxes at binary level.
- **Stage 2 — Route & Encode**: JXL VarDCT for JPEG (bit-exact); Modular mode for lossless sources.
- **Stage 3 — Detour Pathway**: Formats like TIFF/WebP/BMP/HEIC are pre-processed into temporary 16-bit PNGs to ensure `cjxl` compatibility without quality loss.
- **Stage 4 — Meme Score v3**: Evaluates animated GIFs (Sharpness 40%, Resolution 18%, Duration 20%) to decide between video conversion or keeping as GIF.

### Video Pipeline: Three-Phase Saturation Search
1. **Phase 1: GPU Coarse Search**: Binary search on hardware encoders (VideoToolbox/NVENC) to find the "quality knee".
2. **Phase 2: CPU Fine-Tune**: Maps GPU CRF to `x265` scale. Uses **Sprint & Backtrack** (double step on success, reset to 0.1 on overshoot).
3. **Phase 3: Ultimate 3D Quality Gate**: Requires simultaneous pass of VMAF-Y ≥ 93.0, CAMBI ≤ 5.0 (banding), and PSNR-UV ≥ 35.0 dB. 
   - *Note: In `--ultimate` mode, the search only terminates after **50 consecutive samples** show zero quality gain, ensuring absolute saturation.*

### Metadata & HDR Preservation
- **HDR**: Preserves bt2020 primaries, PQ/HLG TRC, and Mastering Display metadata.
- **Dolby Vision**: Extracts RPU via `dovi_tool` and injects into x265 (Profile 7 → 8.1 conversion).
- **macOS xattrs**: Preserves Finder Tags, Date Added, and creation timestamps via `copyfile` and `setattrlist`.
</details>


### The Four Binaries

| Binary | Purpose | Target Codec |
|--------|---------|-------------|
| **`img-hevc`** | Image optimization | → JXL (static) / HEVC (animated) |
| **`img-av1`** | Image optimization | → JXL (static) / AV1 (animated) |
| **`vid-hevc`** | Video optimization | → HEVC / H.265 |
| **`vid-av1`** | Video optimization | → AV1 / SVT-AV1 |

Plus a **double-click macOS app** (`Modern Format Boost.app`) for drag-and-drop batch processing.

## 📉 Real-World Compression Examples

| Input Format | Original Size | Output Format | Output Size | Savings | Method |
|:---|:---|:---|:---|:---|:---|
| Landscape JPEG | 4.2 MB | **JXL** | 3.3 MB | **~21%** | Lossless component reconstruction |
| Screenshot PNG | 2.5 MB | **JXL** | 1.1 MB | **~56%** | Modular d=0.0 |
| Action Cam H.264 | 1.2 GB | **HEVC** | 480 MB | **~60%** | GPU/CPU CRF Search |
| Animated WebP | 15 MB | **AV1 / HEVC** | 1.8 MB | **~88%** | Transcoded to video format |

## 📊 Processing Matrix

### Image Format Decision Matrix

| Input Format | Lossless? | Animated? | Action | Output | Method |
|:-------------|:---------:|:---------:|:-------|:-------|:-------|
| JPEG | — | No | **Lossless reconstruct** | `.jxl` | `cjxl` VarDCT (bit-exact) |
| PNG | ✅ | No | **Lossless convert** | `.jxl` | `cjxl` Modular `d=0.0` |
| PNG (indexed) | ❌ | No | **Quality-matched** | `.jxl` | d=0.1 |
| WebP | ✅ | No | **Detour → lossless** | `.jxl` | dwebp → JXL d=0.0 |
| WebP | ❌ | No | **Skip** | (keep) | Avoid generational loss |
| WebP | — | ✅ | **Meme Score** | `.mov`/`.gif` | HEVC/AV1 or keep GIF |
| AVIF | ✅ | No | **Lossless convert** | `.jxl` | d=0.0 |
| AVIF | ❌ | No | **Skip** | (keep) | Avoid generational loss |
| HEIC/HEIF | ✅ | No | **Detour → lossless** | `.jxl` | `sips`/`magick` → PNG → d=0.0 |
| HEIC/HEIF | ❌ | No | **Skip** | (keep) | Avoid generational loss |
| TIFF | ✅ | No | **Detour → lossless** | `.jxl` | `magick -depth 16` → PNG → d=0.0 |
| TIFF | ❌ | No | **Quality-matched** | `.jxl` | magick → JXL d=0.1 |
| BMP | ✅ | No | **Detour → lossless** | `.jxl` | `magick` → PNG → d=0.0 |
| GIF | — | ✅ | **Meme Score** | `.mov`/`.gif` | HEVC/AV1 or keep GIF |
| GIF | — | No | **Frame extract** | `.jxl` | ffmpeg → JXL |
| JXL | — | No | **Skip** | (keep) | Already optimal |

### Video Codec Decision Matrix

| Input Codec | Compression | Action | Output | Encoder |
|:-----------|:----------:|:-------|:-------|:--------|
| H.264 (AVC) | Lossy | **CRF explore** | `.mp4` HEVC | GPU → x265/SVT-AV1 |
| H.264 | Lossless | **Lossless encode** | `.mkv` HEVC | x265/SVT-AV1 lossless |
| VP9 | Lossy | **CRF explore** | `.mp4` HEVC | GPU → x265/SVT-AV1 |
| AV1 | Lossy | **CRF explore** | `.mp4` HEVC | GPU → x265/SVT-AV1 |
| HEVC (H.265) | Any | **Skip** | (keep) | Already target codec |
| ProRes | Lossy/Lossless | **CRF explore/lossless** | `.mp4`/`.mkv` | x265 |

### HDR Format Strategy

| HDR Type | Detection | Preservation Strategy |
|:---------|:----------|:---------------------|
| **HDR10** | mastering_display + max_cll in side_data | Static metadata fully preserved via FFmpeg args |
| **HLG** | color_trc = arib-std-b67 | Color primaries + TRC preserved |
| **Dolby Vision** | DOVI side_data in streams/frames | RPU extraction via `dovi_tool` → x265 injection; Profile 7 → 8.1 auto-convert |
| **HDR10+** | ST2094 dynamic metadata | ⚠️ Dynamic metadata stripped; static HDR10 layer preserved |
| **SDR** | No HDR markers | Standard processing (yuv420p) |

## ⬇️ Installation / 安装说明

### Pre-compiled Binaries / 预编译版下载
For users who do not wish to install the Rust toolchain, you can download pre-compiled binaries from the **[Releases](https://github.com/nowaytouse/modern-format-boost/releases)** page.

```bash
# macOS/Linux One-liner (example for macOS ARM64)
curl -LO https://github.com/nowaytouse/modern-format-boost/releases/latest/download/modern-format-boost-aarch64-apple-darwin.tar.gz
tar -xzf modern-format-boost-aarch64-apple-darwin.tar.gz
```

### Prerequisites / 前置环境

| Tool / 工具 | Required? / 必须? | Purpose / 用途 | Install / 安装命令 |
|------|:--------:|---------|---------|
| **Rust** (1.75+) | ✅ | 编译安装 | `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh` |
| **FFmpeg** (5.0+) | ✅ | 视频处理与质量检测 | `brew install ffmpeg` / `apt install ffmpeg` |
| **libjxl** | ✅ | JXL 编码核心 | `brew install jpeg-xl` |
| **ExifTool** | ✅ | 元数据保留 | `brew install exiftool` |
| **ImageMagick** | ✅ | 图片格式中转 | `brew install imagemagick` |
| **libwebp** | ✅ | WebP 原生解码 | `brew install webp` |
| **dovi_tool** | ✅ | 杜比视界 RPU 提取 | `cargo install dovi_tool` |
| **libheif** | ✅ | HEIC/HEIF 解码 | `brew install libheif` |

#### macOS (Homebrew)
```bash
brew install ffmpeg jpeg-xl exiftool imagemagick webp libheif
cargo install dovi_tool
```

#### Linux (Ubuntu/Debian)
```bash
sudo apt update && sudo apt install ffmpeg libimage-exiftool-perl imagemagick webp libheif-dev
# JPEG XL (libjxl) may need PPA or source build on older distros
```

#### Windows
Recommended: Use **winget** for one-liner installation:
```powershell
winget install ffmpeg.ffmpeg ImageMagick.ImageMagick OliverBetz.ExifTool libheif.libheif Google.WebP
# Note: dovi_tool must be installed via cargo or manual binary download
```

### Build from Source / 从源码构建
```bash
git clone https://github.com/nowaytouse/modern-format-boost.git
cd modern-format-boost
cargo build --release
```

## 🚀 Usage / 使用方法

### Quick Start / 快速开始
```bash
# Image path conversion
img-hevc run /path/to/media
# Video path conversion
vid-hevc run /path/to/media
```

### Detailed Options / 详细参数
- `--ultimate`: Archival-grade **0.01 precision** search (High quality, high time cost).
- `--apple-compat`: Enable Apple ecosystem compatibility (Live Photos/AAE). (Default: On)
- `--in-place`: Replace original files. **WARNING: IRREVERSIBLE.**
- `-o /dir`: Safe output directory. (Recommended)
- `--verbose`: Show detailed processing logs.
- `--no-recursive`: Do not descend into subdirectories.
- `--force-video`: Force treat animated images as video regardless of Meme Score.

### Advanced Subcommands / 进阶子命令
- `cache-stats`: View SQLite analysis cache statistics.
- `strategy <path>`: Preview the pipeline strategy for a specific file.
- `restore-timestamps`: Bulk fix creation dates based on filename patterns (metadata recovery).

### 💡 Multi-Instance Note / 多开须知
**Modern Format Boost** natively supports running multiple windows/instances.
- **Concurrent Processing**: Allows running multiple windows to handle different paths independently.
- **Note**: Please scale according to your hardware I/O performance; excessive concurrency may cause file system race conditions.

## 🏗️ Architecture

- `img_hevc/`: Image → JXL/HEVC tool
- `vid_hevc/`: Video → HEVC tool
- `shared_utils/`: Core brain (GPU/CPU hybrid engine, HDR mapping, metadata)
- `Modern Format Boost.app/`: macOS drag-and-drop UI

## ❓ FAQ

**1. Is JXL broadly supported?**  
Partially native on macOS Sonoma (14) + iOS 17+. Also supported in Chrome 91+ and Firefox 128+. Note: Animated JXL is currently broken on macOS (static only). JXL remains the best for bit-exact archival.

**2. What happens to HDR10+?**  
⚠️ Dynamic metadata is stripped due to `libx265` limits; static HDR10 layer is fully preserved.

**3. Why skip WebP/AVIF/HEIC?**  
They are already modern lossy formats. Re-encoding causes "generational loss," which we strictly avoid.

---
# ⚖️ License
Licensed under the **MIT License**.

### Runtime Dependencies / 运行时依赖授权
This project orchestrates several open-source giants. We thank their authors for their contributions:

| Component | License | Purpose |
|-----------|---------|---------|
| **FFmpeg** | LGPL/GPL | Video processing & Metrics |
| **libjxl** (cjxl/djxl) | BSD-3 | JPEG XL encoding |
| **ExifTool** | Perl/GPL | Metadata preservation |
| **ImageMagick** | Apache 2.0 | Image detour pathway |
| **SVT-AV1** | BSD+Patent | AV1 Encoding |
| **x265** | GPL-2.0 | HEVC Encoding |

All Rust dependencies are managed via `Cargo.toml` and fall under their respective open-source licenses (MIT/Apache/BSD).


---
# 📖 简体中文

## Modern Format Boost 是什么？

**Modern Format Boost** 是一个基于 Rust 开发的高性能媒体优化引擎。它将传统图片和视频格式（JPEG、PNG、H.264、VP9…）转换为最新一代编码（图片 → **JPEG XL**，视频 → **HEVC/AV1**），在**保持甚至完全匹配原始画质**的前提下，大幅减小文件体积。

你可以把它理解为一个"智能压缩器"，**永远不会降低你的媒体画质**：

- 📸 **图片**：JPEG → JXL 无损重建（位一致，体积减少约 20%）；PNG/WebP/TIFF/HEIC → JXL
- 🎬 **视频**：H.264/VP9/AV1 → HEVC，配合 GPU 加速质量搜索
- 🍎 **苹果生态优先**：完整的 Apple 兼容模式、Live Photo 检测、AAE 编辑指令保留
- 🔒 **元数据守护**：保留 EXIF、XMP、ICC 色彩配置文件、创建时间、macOS 扩展属性、Finder 标签

## ⚠️ 免责声明与重要提示

1. **数据安全第一**：为避免操作失误造成不可挽回的数据损失，强烈建议始终使用“输出到独立目标目录”功能（比如 `-o /path/to/output`），尽量避免覆盖式原地转换（In-place 按需开启）。
2. **测试版软件**：如更新日志所示，本程序经过了大量的极端情况测试与调试，修复了大量可能导致输出质量受损或数据丢失的 BUG。但这依然不能作为“百分百无 BUG”的绝对担保。如在使用中遇到任何异常，请及时在 GitHub 提交 Issue 寻求修复。
3. **计算任务建议**：程序已针对 Apple Silicon（如 M4 芯片）等现代架构进行了性能优化。尽管如此，在开启 `--ultimate`（极致模式）处理海量高分辨率媒体时，仍会占用一定的系统资源并持续较长时间。建议根据任务规模合理安排处理进度。
4. **工具成熟度建议**：目前基于 HEVC 的工具（`img-hevc`, `vid-hevc`）在功能细节和运行稳定性方面领先于基于 AV1 的工具（`img-av1`, `vid-av1`）。对于追求极致稳定性的生产环境任务，建议优先选用 HEVC 工具。

## 🔒 隐私安全

**Modern Format Boost** 采用“本地优先”设计，从底层代码层面确保您的数字资产完全受控。

- **100% 离线脱机**：所有媒体处理逻辑均严格在本地执行，不含任何遥测统计或云端请求代码，彻底杜绝数据泄露。
- **Rust 内存防御**：利用 Rust 语言特性，从编译器级别杜绝了缓冲区溢出和双重释放等常见的内存攻击风险。
- **指令调用屏障**：所有外部调用（如 FFmpeg、cjxl）均采用类型安全的管道传参，绝不执行原始 Shell 字符串，从根本上封锁了命令注入漏洞。
- **路径隔离技术**：内置路径规范化逻辑，有效防止目录穿越风险，坚壁清野，确保处理过程仅限于目标范围。
- **系统路径屏蔽名单**：内置敏感系统目录防护，严防误操作修改 OS 核心文件。
- **动态资源负载均衡**：自动监测内存与 CPU 压力并实时调整线程分配，极致任务下亦能确保系统运行不崩溃。
- **全方位元数据守护**：不放过任何一个元数据单元，严格位对位还原 EXIF、XMP、ICC 配置文件及文件系统时间戳 (btime/mtime)。
- **安全处理与会话隔离**：
  - **零工作区污染**：集中式进度追踪（`~/.mfb_progress/`）实现了对媒体目录的“零足迹”访问。主目录下不再产生任何隐藏的进度文件。
  - **防冲突随机命名**：转换中产生的中间文件（如 YUV 原始流、分析切片）均采用 **UUID 随机命名**。这确保了在多窗口并发任务下互不干扰，并为“外科手术式”的定向清理提供了唯一识别号。
  - **自我净化机制**：无论是任务成功收尾，还是在任务中断后重启，系统都会先通过识别码“打扫战场”，彻底清除所有残留的临时数据。这确保了处理过程永远在净空状态下开始，硬盘空间不再被无主碎片占用。


<details>
<summary><b>🛠️ 技术深探：工作原理与核心算法</b></summary>

### 图片处理管线细节
- **第一阶段 — 智能检测**：在二进制层面分析 JPEG DQT 量化表、WebP VP8L 数据块及 AVIF `av1C` box。
- **第二阶段 — 路由决策**：JPEG 使用 JXL VarDCT 模式（位一致重建）；无损源使用 Modular 模式。
- **第三阶段 — "绕路"兼容性**：TIFF/WebP/BMP/HEIC 会先转为临时 **16-bit PNG**，确保护航 `cjxl` 的同时不发生 8-bit 精度降级。
- **第四阶段 — Meme Score v3**：多维度评估动图（清晰度 40%、分辨率 18%、时长 20%），聪明地决定是转为视频还是保留 GIF。

### 视频处理：三阶段饱和搜索
1. **第一阶段：GPU 粗搜索**：利用硬件编码器进行快速二分搜索，定位“画质拐点”。
2. **第二阶段：CPU 精调**：将结果映射至 `x265` 刻度。使用 **Sprint & Backtrack（冲刺与回退）算法**：连续成功时步长翻倍，过冲时立即降至 0.1 步长。
3. **第三阶段：极致 3D 质量门控**：必须同时通过 VMAF-Y ≥ 93.0（感知画质）、CAMBI ≤ 5.0（色带检测）及 PSNR-UV ≥ 35.0 dB。
   - *注：在 `--ultimate` 极致模式下，搜索算法要求连续 **50 次采样** 均达到零画质增益方可停机，确保绝对画质饱和。*

### 元数据与 HDR 保留
- **HDR 守护**：强制透传 bt2020 原色、PQ/HLG 传输特性及母带显示数据。
- **杜比视界**：通过 `dovi_tool` 提取 RPU 并注入编码器，支持 Profile 7 自动转 8.1 增强兼容性。
- **macOS 特性**：利用 `copyfile` 和 `setattrlist` 完美保留 Finder 标签、添加日期及原始创建时间。
</details>


### 四个二进制工具

| 工具 | 用途 | 目标编码 |
|------|------|---------|
| **`img-hevc`** | 图片优化 | → JXL (静态) / HEVC (动图) |
| **`img-av1`** | 图片优化 | → JXL (静态) / AV1 (动图) |
| **`vid-hevc`** | 视频优化 | → HEVC / H.265 |
| **`vid-av1`** | 视频优化 | → AV1 / SVT-AV1 |

还有一个 **macOS 双击应用** (`Modern Format Boost.app`)，支持拖放批量处理。

## 📉 实际压缩效果示例

| 原图格式 | 原始大小 | 输出格式 | 输出大小 | 空间节省 | 压缩手段 |
|:---|:---|:---|:---|:---|:---|
| 风景 JPEG | 4.2 MB | **JXL** | 3.3 MB | **~21%** | 无损二进制成分重建 (Bit-exact) |
| 截图 PNG | 2.5 MB | **JXL** | 1.1 MB | **~56%** | Modular 模式 d=0.0 无损 |
| H.264 视频 | 1.2 GB | **HEVC** | 480 MB | **~60%** | 三阶段 GPU+CPU 深度视觉搜索 |
| 动图 WebP | 15 MB | **AV1 / HEVC** | 1.8 MB | **~88%** | 重制为高压缩比视频格式 |

## 📊 处理矩阵

### 图片格式决策矩阵

| 格式 | 无损? | 动图? | 处理 | 输出 | 算法与方式 |
|:-------------|:---------:|:---------:|:-------|:-------|:-------|
| JPEG | — | 否 | **无损成分重建** | `.jxl` | VarDCT (位一致) |
| PNG | ✅ | 否 | **无损转换** | `.jxl` | Modular d=0.0 |
| PNG (索引色) | ❌ | 否 | **画质匹配** | `.jxl` | d=0.1 |
| WebP | ✅ | 否 | **无损(绕路)** | `.jxl` | dwebp → JXL d=0.0 |
| WebP | ❌ | 否 | **跳过** | (保留) | 避免代际损伤 |
| WebP | — | 是 | **Meme Score 判定** | `.mov`/`.gif` | 转视频或保留 GIF |
| AVIF | ✅ | 否 | **无损转换** | `.jxl` | d=0.0 |
| AVIF | ❌ | 否 | **跳过** | (保留) | 避免代际损伤 |
| HEIC/HEIF | ✅ | 否 | **无损(绕路)** | `.jxl` | `sips`/`magick` → PNG → d=0.0 |
| HEIC/HEIF | ❌ | 否 | **跳过** | (保留) | 避免代际损伤 |
| TIFF | ✅ | 否 | **无损(绕路)** | `.jxl` | `magick -depth 16` → PNG → d=0.0 |
| TIFF | ❌ | 否 | **画质匹配** | `.jxl` | magick → JXL d=0.1 |
| BMP | ✅ | 否 | **无损(绕路)** | `.jxl` | `magick` → PNG → d=0.0 |
| GIF | — | 是 | **Meme Score 判定** | `.mov`/`.gif` | 转视频或保留 GIF |
| GIF | — | 否 | **单帧提取** | `.jxl` | ffmpeg → JXL |
| JXL | — | 否 | **跳过** | (保留) | 最优格式 |

### 视频编码决策矩阵

| 输入编码 | 压缩方式 | 处理动作 | 输出格式 | 编码内核 |
|:-----------|:----------:|:-------|:-------|:--------|
| H.264 (AVC) | 有损 | **CRF 视觉搜索** | `.mp4` HEVC/AV1 | GPU → CPU x265/SVT-AV1 |
| H.264 | 无损 | **无损再编码** | `.mkv` HEVC/AV1 | x265/SVT-AV1 无损模式 |
| VP9 | 有损 | **CRF 视觉搜索** | `.mp4` HEVC/AV1 | GPU → CPU x265/SVT-AV1 |
| AV1 | 有损 | **CRF 视觉搜索** | `.mp4` HEVC | GPU → CPU x265/SVT-AV1 |
| HEVC (H.265) | 任何 | **跳过** | (保留) | 已经是目标编码格式 |
| ProRes | 有损/无损 | **CRF 搜索/无损** | `.mp4`/`.mkv` | x265 |

### HDR 格式处理策略

| HDR 类型 | 检测依据 | 核心保留策略 |
|:---------|:----------|:---------------------|
| **HDR10** | 元数据 side_data 捕获 | 通过 FFmpeg 参数完整保留静态元数据并注入 |
| **HLG** | 传输特性 TRC 识别 | 原色 (Primaries) 及 TRC 完整保留 |
| **Dolby Vision** | DOVI RPU 数据流 | 经 `dovi_tool` 提取 RPU 并注入编码器；Profile 7 自动转 8.1 |
| **HDR10+** | ST2094 动态元数据 | ⚠️ 动态层丢失；保留底层的 HDR10 静态核心数据 |
| **SDR** | 无 HDR 标记 | 自动进入标准处理流 (yuv420p) |

## ⬇️ 安装说明

### 前置要求

| 工具 | 必须? | 用途 | 安装命令 |
|------|:--------:|---------|---------|
| **Rust** (1.75+) | ✅ | 编译安装 | `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh` |
| **FFmpeg** (5.0+) | ✅ | 视频处理与质量检测 | `brew install ffmpeg` |
| **libjxl** | ✅ | JXL 编码核心 | `brew install jpeg-xl` |
| **ExifTool** | ✅ | 元数据保留 | `brew install exiftool` |
| **ImageMagick** | ✅ | 图片格式中转 | `brew install imagemagick` |
| **libwebp** | ✅ | WebP 原生解码 | `brew install webp` |
| **dovi_tool** | ✅ | 杜比视界 RPU 提取 | `cargo install dovi_tool` |
| **libheif** | ✅ | HEIC/HEIF 解码 | `brew install libheif` |

### macOS (Homebrew)
```bash
brew install ffmpeg jpeg-xl exiftool imagemagick webp libheif
cargo install dovi_tool
```


#### Linux (Ubuntu/Debian)
```bash
sudo apt update && sudo apt install ffmpeg libimage-exiftool-perl imagemagick webp libheif-dev
# JPEG XL (libjxl) 可能需要通过 PPA 或源码构建
```

#### Windows
推荐使用 **winget** 一键安装所有依赖：
```powershell
winget install ffmpeg.ffmpeg ImageMagick.ImageMagick OliverBetz.ExifTool
```

### 从源码构建
```bash
git clone https://github.com/nowaytouse/modern-format-boost.git
cd modern-format-boost
cargo build --release
```

## 🚀 使用方法

### 快速开始
```bash
# 图片优化
img-hevc run /图片/路径
# 视频优化
vid-hevc run /视频/路径
```

### 详细参数
- `--ultimate`: 档案级 **0.01 精度**搜索（高质量，高耗时）。
- `--apple-compat`: 开启苹果生态兼容 (Live Photos/AAE)。（默认：开启）
- `--in-place`: 原地替换原始文件。**警告：不可逆。**
- `-o /dir`: 指定安全输出目录。（建议使用）
- `--verbose`: 显示详细处理日志。
- `--no-recursive`: 不递归进入子目录。
- `--force-video`: 强制将动图视为视频处理（忽略 Meme Score）。

### 进阶子命令
- `cache-stats`: 查看 SQLite 分析缓存统计。
- `strategy <path>`: 预览特定文件的处理管线策略。
- `restore-timestamps`: 根据文件名模式批量修复创建日期（元数据恢复）。

### 💡 多开须知
**Modern Format Boost** 原生支持多开运行。
- **多开并发说明**：允许开启多个窗口独立处理不同路径。
- **注意**：请根据硬件 I/O 性能量力而行，过度并发可能引发文件系统竞态冲突。

## 🏗️ 系统架构

- `img_hevc/`: 图片核心工具
- `vid_hevc/`: 视频核心工具
- `shared_utils/`: 处理大脑 (GPU 混合引擎, HDR 映射, 元数据)
- `Modern Format Boost.app/`: macOS 拖拽图形界面

## ❓ FAQ

**1. JXL 格式目前的兼容性如何？**  
虽然 macOS 14 (Sonoma) 及 iOS 17+ 已提供初步原生支持，但仍存在诸多已知缺陷（如动图暂无法播放）。此外 Chrome 91+ 及 Firefox 128+ 也已原生支持。建议将其作为位一致的无损档案格式进行战略存储。

**2. 为什么 HDR10+ 动态视频会失效？**  
⚠️ 受限于底层 `libx265` 能力，目前会丢弃动态层元数据，但保留 HDR10 静态基础层。

**3. 为什么程序会自动跳过我的 WebP / AVIF / HEIC 图像？**  
因为这三种格式已经是现代有损编码。二次编码会导致画质代际损伤。

---
<p align="center">
  <sub>Built with ❤️ in Rust · <a href="https://github.com/nowaytouse/modern-format-boost">GitHub</a></sub>
</p>
