<p align="center">
  <img src="https://img.shields.io/badge/version-0.11.2-0969DA?style=for-the-badge&logo=rust&logoColor=white" alt="Version">
  <img src="https://img.shields.io/badge/rust-2021_edition-E57324?style=for-the-badge&logo=rust&logoColor=white" alt="Rust">
  <img src="https://img.shields.io/badge/platform-macOS_%7C_Linux_%7C_Windows-8257E5?style=for-the-badge&logo=apple&logoColor=white" alt="Platform">
  <img src="https://img.shields.io/badge/license-MIT-00B265?style=for-the-badge" alt="License">
</p>

<h1 align="center">Modern Format Boost</h1>

<p align="center">
  <strong>Next-gen media optimization engine — zero quality loss, maximum compression.</strong><br>
</p>

<p align="center">
  <a href="README.md">English</a> ·
  <a href="docs/README_ZH.md">简体中文</a> ·
  <a href="docs/README_ZH_TW.md">繁體中文</a> ·
  <a href="docs/README_JA.md">日本語</a> ·
  <a href="docs/README_KO.md">한국어</a> ·
  <a href="docs/README_ES.md">Español</a> ·
  <a href="docs/README_FR.md">Français</a> ·
  <a href="docs/README_PT.md">Português</a> ·
  <a href="docs/README_RU.md">Русский</a> ·
  <a href="docs/README_AR.md">العربية</a>
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
- ⚡ **Perceived Speed Optimization**: "Deep-First" sorting strategy—prioritizes deeper directory levels first, then sorts by file size and format, to ensure efficient batching and maximum throughput.
- 🎞️ **HDR10+ Dynamic Metadata**: Full retention of SMPTE 2094-40 metadata via extraction sidecars and x265 SEI injection.
- 🌅 **HDR Gainmap Synthesis**: Automatically synthesizes high-fidelity 32-bit linear HDR buffers from Apple/Samsung/ISO HEIC Gainmaps, ensuring maximum dynamic range is preserved when converting to JXL.
- **🔍 Vendor Metadata Awareness**: Intelligent scanning for Samsung/Google specific XMP namespaces in HEIC files to ensure maximum context preservation.

## ⚠️ Disclaimer & Important Notes

1. **Data Safety First**: To avoid any potential data loss, it is highly recommended to output processed files to a separate directory (e.g., using `-o /path/to/output`) rather than using in-place conversion (`--in-place`), especially for irreplaceable media.
2. **Beta Software**: While this program has been extensively tested, debugged, and optimized to prevent quality or data loss (as seen in the changelog), it is not guaranteed to be 100% bug-free. Please report any issues you encounter on GitHub.
3. **Computation Insight**: While optimized for efficiency (especially on Apple Silicon M-series), processing massive batches in `--ultimate` mode can still be time-consuming. It will occupy system resources for an extended period; please plan your task accordingly.
4. **Tool Maturity**: The unified tools (`img`, `vid`) defaults to HEVC, which is more mature and stable than the AV1 strategy. For high-reliability production tasks, HEVC (the default) is recommended.

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
- **Intelligent Checkpoint Reset**: Automatically detects when a user manually deletes the output directory to "start over", triggering a full state reset even in resume mode.

<details>
<summary><b>🛠️ Deep Technical: How It Works — The Pipeline</b></summary>

### Image Pipeline Logic

Every file goes through a multi-stage decision pipeline:

- **Stage 1 — Smart Detection**: Analyzes JPEG DQT tables (UltraHDR gainmap detection), WebP VP8L chunks, and AVIF `av1C` boxes at binary level. Now features **Zero-Debt Architecture** with 100% Clippy compliance and robust `OpenEXR`/`JPEG 2000` header parsing.
- **Stage 2 — Route & Encode**: JXL VarDCT for JPEG (bit-exact); Modular mode for lossless sources (PNG, lossless WebP/AVIF/HEIC/EXR/JP2).
- **Stage 3 — Detour Pathway**: Formats like TIFF/WebP/BMP/HEIC are pre-processed into temporary 16-bit PNGs or **32-bit OpenEXR** to ensure `cjxl` compatibility without quality loss (8/16/32-bit matched pipeline).
- **Stage 4 — HEIC HDR Synthesis**: Intercepts HEIC files with Gainmaps (Apple/Google) and synthesizes 32-bit linear-light HDR buffers via an intermediate **OpenEXR** escort pipeline, delivering true HDR JXL output.
- **Stage 5 — Meme Score v3**: Evaluates animated GIFs (Sharpness 40%, Resolution 18%, Duration 20%) to decide between video conversion or keeping as GIF.

### Video Pipeline: Three-Phase Saturation Search

1. **Phase 1: GPU Coarse Search**: Binary search on hardware encoders (VideoToolbox/NVENC) to find the "quality knee".
2. **Phase 2: CPU Fine-Tune**: Maps GPU CRF to `x265` scale. Uses **Sprint & Backtrack** (double step on success, reset to 0.1 on overshoot).
3. **Phase 3: Ultimate 3D Quality Gate**: Requires simultaneous pass of VMAF-Y ≥ 92.0, CAMBI ≤ 6.0 (banding), and PSNR-UV ≥ 34.0 dB.
   - **Fusion Scoring**: Combines MS-SSIM + SSIM_All (0.6/0.4 weight) for robust structural analysis.
   - **Chroma Guard**: Automatically detects small resolutions that would crash libvmaf MS-SSIM and falls back to Y-only scoring to ensure processing reliability.
   - _Note: In `--ultimate` mode, the search only terminates after **50 consecutive samples** show zero quality gain, ensuring absolute saturation._

### Metadata & HDR Preservation

- **HDR**: Preserves bt2020 primaries, PQ/HLG TRC, and Mastering Display metadata.
- **Dolby Vision**: Extracts RPU via `dovi_tool` and injects into x265 (Profile 7 → 8.1 conversion).
- **macOS xattrs**: Preserves Finder Tags, Date Added, and creation timestamps via `copyfile` and `setattrlist`.

</details>

### 🖥️ Runtime

![Runtime](assets/runtime.png)

<p align="center">Runtime</p>

### The Four Binaries

| Binary         | Purpose            | Target Codec                     |
| -------------- | ------------------ | -------------------------------- |
| **`img`** | Image optimization | → JXL (static) / HEVC / AV1 (animated) |
| **`vid`** | Video optimization | → HEVC / AV1                           |

Plus a **double-click macOS app** (`Modern Format Boost.app`) for drag-and-drop batch processing.

## 📉 Real-World Compression Examples

| Input Format     | Original Size | Output Format  | Output Size | Savings  | Method                            |
| :--------------- | :------------ | :------------- | :---------- | :------- | :-------------------------------- |
| Landscape JPEG   | 4.2 MB        | **JXL**        | 3.3 MB      | **~21%** | Lossless component reconstruction |
| Screenshot PNG   | 2.5 MB        | **JXL**        | 1.1 MB      | **~56%** | Modular d=0.0                     |
| Action Cam H.264 | 1.2 GB        | **HEVC**       | 480 MB      | **~60%** | GPU/CPU CRF Search                |
| Animated WebP    | 15 MB         | **AV1 / HEVC** | 1.8 MB      | **~88%** | Transcoded to video format        |

## 📊 Processing Matrix

### Image Format Decision Matrix

| Input Format  | Lossless? | Animated? | Action                   | Output        | Method                                 |
| :------------ | :-------: | :-------: | :----------------------- | :------------ | :------------------------------------- |
| JPEG          |     —     |    No     | **Lossless reconstruct** | `.jxl`        | `cjxl` VarDCT (bit-exact)              |
| PNG           |    ✅     |    No     | **Lossless convert**     | `.jxl`        | `cjxl` Modular `d=0.0`                 |
| PNG (indexed) |    ❌     |    No     | **Quality-matched**      | `.jxl`        | d=0.001                                |
| WebP          |    ✅     |    No     | **Detour → lossless**    | `.jxl`        | dwebp → JXL d=0.0                      |
| WebP          |    ❌     |    No     | **Skip**                 | (keep)        | Avoid generational loss                |
| WebP          |     —     |    ✅     | **Meme Score**           | `.mov`/`.gif` | HEVC/AV1 or keep GIF                   |
| AVIF          |    ✅     |    No     | **Lossless convert**     | `.jxl`        | d=0.0                                  |
| AVIF          |    ❌     |    No     | **Skip**                 | (keep)        | Avoid generational loss                |
| HEIC/HEIF     |    ✅     |    No     | **Detour → lossless**    | `.jxl`        | `sips`/`magick` → PNG → d=0.0          |
| HEIC/HEIF     |    ❌     |    No     | **HDR Synthesis**        | `.jxl`        | If Gainmap exists -> 32-bit EXR -> JXL |
| HEIC/HEIF     |    ❌     |    No     | **Skip**                 | (keep)        | Standard HEIC: avoid generational loss |
| TIFF          |    ✅     |    No     | **Detour → lossless**    | `.jxl`        | `magick -depth 16` → PNG → d=0.0       |
| TIFF          |    ❌     |    No     | **Quality-matched**      | `.jxl`        | magick → JXL d=0.001                   |
| BMP           |    ✅     |    No     | **Detour → lossless**    | `.jxl`        | `magick` → PNG → d=0.0                 |
| GIF           |     —     |    ✅     | **Meme Score**           | `.mov`/`.gif` | HEVC/AV1 or keep GIF                   |
| GIF           |     —     |    No     | **Frame extract**        | `.jxl`        | ffmpeg → JXL                           |
| JXL           |     —     |    No     | **Skip**                 | (keep)        | Already optimal                        |

### Video Codec Decision Matrix

| Input Codec  |  Compression   | Action                   | Output        | Encoder               |
| :----------- | :------------: | :----------------------- | :------------ | :-------------------- |
| H.264 (AVC)  |     Lossy      | **CRF explore**          | `.mp4` HEVC   | GPU → x265/SVT-AV1    |
| H.264        |    Lossless    | **Lossless encode**      | `.mkv` HEVC   | x265/SVT-AV1 lossless |
| VP9          |     Lossy      | **CRF explore**          | `.mp4` HEVC   | GPU → x265/SVT-AV1    |
| AV1          |     Lossy      | **CRF explore**          | `.mp4` HEVC   | GPU → x265/SVT-AV1    |
| HEVC (H.265) |      Any       | **Skip**                 | (keep)        | Already target codec  |
| ProRes       | Lossy/Lossless | **CRF explore/lossless** | `.mp4`/`.mkv` | x265                  |

### HDR Format Strategy

| HDR Type          | Detection                                | Preservation Strategy                                                                                 |
| :---------------- | :--------------------------------------- | :---------------------------------------------------------------------------------------------------- |
| **HDR10**         | mastering_display + max_cll in side_data | Static metadata fully preserved via FFmpeg args                                                       |
| **HEIC Gainmap**  | HEIC auxiliary image (Apple/Samsung/ISO) | Synthesized to 32-bit linear HDR -> JXL (True HDR)                                                    |
| **UltraHDR JPEG** | JPEG APP1/APP2 + XMP (hdrgm:)            | Metadata preserved; gainmap loss warning emitted                                                      |
| **HLG**           | color_trc = arib-std-b67                 | Color primaries + TRC preserved                                                                       |
| **Dolby Vision**  | DOVI side_data in streams/frames         | RPU extraction via `dovi_tool` → x265 injection; Profile 7 → 8.1 conversion                           |
| **HDR10+**        | ST2094-40 dynamic metadata               | Supported via `hdr10plus_tool` sidecar extraction and x265 injection (Profile A/B metadata retention) |
| **SDR**           | No HDR markers                           | Standard processing (yuv420p)                                                                         |

## ⬇️ Installation

### Pre-compiled Binaries

For users who do not wish to install the Rust toolchain, you can download pre-compiled binaries from the **[Releases](https://github.com/nowaytouse/modern-format-boost/releases)** page.

```bash
# macOS/Linux One-liner (example for macOS ARM64)
curl -LO https://github.com/nowaytouse/modern-format-boost/releases/latest/download/modern-format-boost-aarch64-apple-darwin.tar.gz
tar -xzf modern-format-boost-aarch64-apple-darwin.tar.gz
```

### Prerequisites

| Tool               | Required? | Purpose                     | Install Command                                            |
| ------------------ | :-------: | --------------------------- | ---------------------------------------------------------- | --- |
| **Rust** (1.75+)   |    ✅     | Build & Install             | `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh` |
| **FFmpeg** (5.0+)  |    ✅     | Video processing & Metrics  | `brew install ffmpeg` / `apt install ffmpeg`               |
| **libjxl**         |    ✅     | JXL encoding core           | `brew install jpeg-xl`                                     |
| **ExifTool**       |    ✅     | Metadata preservation       | `brew install exiftool`                                    |
| **ImageMagick**    |    ✅     | Image detour pathway        | `brew install imagemagick`                                 |
| **libwebp**        |    ✅     | WebP native decoding        | `brew install webp`                                        |
| **dovi_tool**      |    ✅     | Dolby Vision RPU extraction | `cargo install dovi_tool`                                  |
| **libheif**        |    ✅     | HEIC/HEIF decode            | `brew install libheif`                                     |
| **hdr10plus_tool** |    ✅     | HDR10+ metadata extraction  | `cargo install hdr10plus_tool`                             |

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

### Build from Source

```bash
git clone https://github.com/nowaytouse/modern-format-boost.git
cd modern-format-boost
cargo build --release
```

## 🚀 Usage

### Quick Start

```bash
# Image path conversion
img run /path/to/media
# Video path conversion
vid run /path/to/media

# To use AV1 strategy:
vid run --codec av1 /path/to/media
```

### Detailed Options

- `--ultimate`: Archival-grade **0.01 precision** search (High quality, high time cost).
- `--apple-compat`: Enable Apple ecosystem compatibility (Live Photos/AAE). (Default: On)
- `--in-place`: Replace original files. **WARNING: IRREVERSIBLE.**
- `-o /dir`: Safe output directory. (Recommended)
- `--verbose`: Show detailed processing logs.
- `--no-recursive`: Do not descend into subdirectories.
- `--force-video`: Force treat animated images as video regardless of Meme Score.

### Advanced Subcommands

- `cache-stats`: View SQLite analysis cache statistics.
- `strategy <path>`: Preview the pipeline strategy for a specific file.
- `restore-timestamps`: Bulk fix creation dates based on filename patterns (metadata recovery).

### 💡 Multi-Instance Note

**Modern Format Boost** natively supports running multiple windows/instances.

- **Concurrent Processing**: Allows running multiple windows to handle different paths independently.
- **Note**: Please scale according to your hardware I/O performance; excessive concurrency may cause file system race conditions.

## 🏗️ Architecture

- `img/`: Image → JXL/HEVC/AV1 tool
- `vid/`: Video → HEVC/AV1 tool
- `shared_utils/`: Core brain (GPU/CPU hybrid engine, HDR mapping, metadata)
- `Modern Format Boost.app/`: macOS drag-and-drop UI

## ❓ FAQ

**1. Is JXL broadly supported?**  
Partially native on macOS Sonoma (14) + iOS 17+. Also supported in Chrome 91+ and Firefox 128+. Note: Animated JXL is currently broken on macOS (static only). JXL remains the best for bit-exact archival.

**2. What happens to HDR10+?**  
Fully supported! We use `hdr10plus_tool` to extract the dynamic metadata and inject it back into `libx265` via `--dhdr10-info`. Ensure the tool is installed for this feature.

**3. Why skip WebP/AVIF/HEIC?**  
They are already modern lossy formats. Re-encoding causes "generational loss," which we strictly avoid.

---

# ⚖️ License

Licensed under the **MIT License**.

### Runtime Dependencies

This project orchestrates several open-source giants. We thank their authors for their contributions:

| Component              | License    | Purpose                    |
| ---------------------- | ---------- | -------------------------- |
| **FFmpeg**             | LGPL/GPL   | Video processing & Metrics |
| **libjxl** (cjxl/djxl) | BSD-3      | JPEG XL encoding           |
| **ExifTool**           | Perl/GPL   | Metadata preservation      |
| **ImageMagick**        | Apache 2.0 | Image detour pathway       |
| **SVT-AV1**            | BSD+Patent | AV1 Encoding               |
| **x265**               | GPL-2.0    | HEVC Encoding              |

All Rust dependencies are managed via `Cargo.toml` and fall under their respective open-source licenses (MIT/Apache/BSD).
