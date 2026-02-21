# 🚀 Modern Format Boost

![Version](https://img.shields.io/badge/version-8.2.0-blue.svg)
![License](https://img.shields.io/badge/license-MIT-green.svg)
![Platform](https://img.shields.io/badge/platform-macOS%20%7C%20Linux-lightgrey.svg)

**The Ultimate Media Optimizer & Repair Tool for the Apple Ecosystem.**
**专为苹果生态打造的终极媒体优化与修复工具。**

---

## 📖 简介 / Introduction

**Modern Format Boost** is a professional-grade media optimization suite designed to modernize your photo and video library. It losslessly converts legacy formats (JPEG, PNG, GIF, AVC) to modern, high-efficiency standards (JXL, AVIF, HEVC), saving 30-80% storage space without losing a single pixel of quality.

**Modern Format Boost** 是一套专业级的媒体优化套件，旨在将您的照片和视频库现代化。它能将过时的格式（JPEG, PNG, GIF, AVC）无损转换为现代高效标准（JXL, AVIF, HEVC），在不损失任何画质的前提下节省 30-80% 的存储空间。

Unlike simple converters, it features a robust **"Self-Healing" engine** specifically engineered to fix files that Apple Photos refuses to import ("Unknown Error"). It handles corrupted headers, mismatched extensions, and toxic metadata automatically.

与简单的转换器不同，它内置了强大的**“自愈”引擎**，专门用于修复 Apple 照片无法导入（报“未知错误”）的文件。它能自动处理损坏的文件头、扩展名不匹配以及有毒的元数据。

---

## ✨ 核心特性 / Key Features

### 🍎 Apple Ecosystem Perfected / 完美适配苹果生态
*   **"Unknown Error" Killer**: Automatically detects and fixes files that crash Apple Photos (e.g., WebP files renamed as .jpeg).
    *   **“未知错误”终结者**：自动检测并修复导致苹果相册崩溃的文件（例如被重命名为 .jpeg 的 WebP 文件）。
*   **Nuclear Metadata Rebuild**: Strips "toxic" non-standard EXIF tags left by third-party editors (Meitu, etc.) while preserving all valid data (GPS, Date, Captions).
    *   **元数据核弹级重构**：剔除第三方编辑器（如美图秀秀）留下的非标准“有毒”标签，同时完美保留所有有效数据（GPS、日期、说明）。
*   **Directory Timestamp Guard**: Preserves creation/modification dates for **folders** as well as files, keeping your timeline intact.
    *   **文件夹时间守护**：不仅保留文件的时间，还完美还原**文件夹**的创建/修改日期，确保相册时间线不乱。

### ⚡ Smart Conversion / 智能转换
*   **Lossless JXL**: Converts JPEG/PNG/GIF to JPEG XL (JXL) with mathematically lossless recompression.
    *   **无损 JXL**：将 JPEG/PNG/GIF 转换为 JPEG XL (JXL)，实现数学上的无损压缩。
*   **Smart Fallback**: If `cjxl` fails (due to corruption), the tool automatically switches to `magick` or `ffmpeg` pipelines to sanitize the file and try again.
    *   **智能回退**：如果 `cjxl` 转换失败（因文件损坏），工具会自动切换到 `magick` 或 `ffmpeg` 管道清洗文件并重试。
*   **Magic Bytes Detection**: Ignores file extensions. It reads the binary header to determine the *real* format (e.g., detecting a PNG disguised as a JPG).
    *   **魔法字节检测**：不信任文件扩展名。它读取二进制文件头来确定*真实*格式（例如检测伪装成 JPG 的 PNG）。

---

## 🛠️ 安装 / Installation

### Prerequisites / 前置要求
You need `brew` installed on macOS.
您需要在 macOS 上安装 `brew`。

```bash
# 1. Install dependencies
brew install jpeg-xl ffmpeg imagemagick exiftool

# 2. Clone the repository
git clone https://github.com/nyamiiko/modern_format_boost.git
cd modern_format_boost

# 3. Build the project
./scripts/smart_build.sh
```

---

## 🚀 使用方法 / Usage

### Drag & Drop (Recommended) / 拖拽使用（推荐）
Simply drag your folder onto the start script:
只需将您的文件夹拖到启动脚本上：

```bash
./scripts/drag_and_drop_processor.sh /path/to/your/photos
```

### CLI Mode / 命令行模式
For advanced users:
高级用户模式：

```bash
# Convert a folder to JXL (Images)
./target/release/imgquality_av1 --input "/path/to/photos" --quality 100 --effort 7

# Convert a folder to HEVC (Videos)
./target/release/vidquality_hevc --input "/path/to/videos" --crf 18
```

---

## 🚑 故障排除 / Troubleshooting

### "Unknown Error" in Apple Photos / 苹果相册“未知错误”
If you have files that refuse to import, use the dedicated repair tool:
如果您有无法导入的文件，请使用专用修复工具：

```bash
./scripts/repair_apple_photos.sh "/path/to/bad/files"
```
**This script will / 该脚本将：**
1.  Scan for extension mismatches (Real WebP vs Fake JPEG). / 扫描扩展名不匹配。
2.  Fix corrupted JPEG headers. / 修复损坏的 JPEG 文件头。
3.  Rebuild metadata from scratch. / 重构元数据。
4.  Restore original timestamps. / 恢复原始时间戳。

---

## 🔧 Development / 开发

```bash
cargo build          # Debug 构建
cargo build --release
cargo test           # 运行测试
cargo clippy         # 代码质量与潜在问题检查
```

Release 构建已启用 LTO 与单 codegen-unit，以最大化运行效率。

---

## 📜 License

MIT License. See `LICENSE` for details.
