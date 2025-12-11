# Modern Format Boost

**[English]**

A suite of high-performance tools designed for analyzing and upgrading image and video files to modern, efficient formats. This workspace focuses on providing cutting-edge compression while maintaining verifiable quality and preserving metadata.

**[中文]**

一套专为分析和升级图像、视频文件到现代高效格式而设计的高性能工具集。本项目专注于提供最前沿的压缩技术，同时保持可验证的质量和元数据完整性。

---

## 🚀 Core Features / 核心功能

**[English]**

*   **Advanced Quality Analysis**: Performs deep analysis of both images (JPEG, PNG, HEIC) and videos to determine optimal encoding parameters. It uses content-based detection (magic bytes) instead of trusting file extensions.
*   **Intelligent Format Conversion**:
    *   **Images**: Upgrades traditional formats to AVIF or HEIC, with specialized handling for animated images using HEVC encoding.
    *   **Videos**: Converts videos to AV1 for superior compression or FFV1 for lossless archival. HEVC encoding is also supported for a balance of quality and compatibility.
*   **Precise Quality Matching**: Aims to match the perceptual quality of the source file, using metrics like SSIM and automatically calculating the correct CRF/distance values for target encoders (AV1, HEVC, JXL).
*   **Video Explorer Engine**: A powerful utility to find the optimal balance between file size and quality. It supports multiple exploration modes, including a binary search algorithm to precisely match a target quality level.
*   **Metadata Preservation**: Safely carries over all essential metadata during conversion, including EXIF, IPTC, XMP, file timestamps, and extended attributes (xattr).
*   **Batch Processing**: Built for efficiency, with parallel processing capabilities to handle large collections of files quickly, complete with progress tracking and summary reports.
*   **Safety First**: Includes safeguards against operating in potentially dangerous system directories.

**[中文]**

*   **高级质量分析**: 对图像 (JPEG, PNG, HEIC) 和视频进行深度分析，以确定最佳编码参数。它使用基于内容（魔数）的检测，而不是信任文件扩展名。
*   **智能格式转换**:
    *   **图像**: 将传统格式升级为 AVIF 或 HEIC，并使用 HEVC 编码专门处理动态图像。
    *   **视频**: 将视频转换为 AV1 以获得卓越的压缩率，或转换为 FFV1 用于无损归档。同时支持 HEVC 编码以平衡质量与兼容性。
*   **精确质量匹配**: 旨在匹配源文件的感知质量，使用 SSIM 等指标，并为目标编码器 (AV1, HEVC, JXL) 自动计算正确的 CRF/距离值。
*   **视频探索引擎**: 一个强大的实用程序，用于寻找文件大小和质量之间的最佳平衡。它支持多种探索模式，包括使用二分搜索算法来精确匹配目标质量水平。
*   **元数据保留**: 在转换过程中安全地保留所有基本元数据，包括 EXIF、IPTC、XMP、文件时间戳和扩展属性 (xattr)。
*   **批量处理**: 为效率而生，具备并行处理能力，可快速处理大量文件集合，并提供进度跟踪和摘要报告。
*   **安全第一**: 包含安全措施，防止在潜在危险的系统目录中操作。

---

## 📦 Workspace Crates / 项目模块

**[English]**

This repository is a Cargo workspace containing the following key crates:

*   `imgquality_API`: A command-line tool for high-performance image quality analysis and format conversion (e.g., to AVIF).
*   `imgquality_hevc`: A specialized version of the image tool, optimized for handling animated images by leveraging HEVC encoding.
*   `vidquality_API`: A command-line tool for video analysis and conversion, focusing on AV1 (for compression) and FFV1 (for archival).
*   `vidquality_hevc`: A specialized version of the video tool that utilizes HEVC for efficient video encoding.
*   `shared_utils`: The core shared library that provides all the underlying logic for quality analysis, format conversion, file processing, and metadata handling used by the other crates.

**[中文]**

该仓库是一个 Cargo 工作区，包含以下关键模块：

*   `imgquality_API`: 用于高性能图像质量分析和格式转换（例如，转换为 AVIF）的命令行工具。
*   `imgquality_hevc`: 图像工具的专用版本，通过利用 HEVC 编码优化了对动态图像的处理。
*   `vidquality_API`: 用于视频分析和转换的命令行工具，专注于 AV1（用于压缩）和 FFV1（用于归档）。
*   `vidquality_hevc`: 利用 HEVC 进行高效视频编码的视频工具专用版本。
*   `shared_utils`: 核心共享库，为其他模块提供了所有底层的质量分析、格式转换、文件处理和元数据处理逻辑。

---

## 🛠️ General Usage / 基本用法

**[English]**

Each crate builds into a command-line tool. Use them to analyze or convert files from your terminal.

**Example (Image Analysis & Upgrade):**
`bash
# Analyze a single image
./target/release/imgquality analyze --path /path/to/image.jpg

# Batch upgrade a folder of images to a modern format
./target/release/imgquality batch-upgrade --input-dir /path/to/images --output-dir /path/to/output
`

**Example (Video Exploration):**
`bash
# Use the video explorer to find a smaller file size for a video
./target/release/vidquality explore --path /path/to/video.mov
`

*Note: The exact tool name (`imgquality`, `vidquality-hevc`, etc.) and commands may vary. Use the `--help` flag for detailed instructions.*

**[中文]**

每个模块都会构建成一个命令行工具。您可以在终端中使用它们来分析或转换文件。

**示例 (图像分析与升级):**
`bash
# 分析单个图像
./target/release/imgquality analyze --path /path/to/image.jpg

# 批量将文件夹中的图像升级为现代格式
./target/release/imgquality batch-upgrade --input-dir /path/to/images --output-dir /path/to/output
`

**示例 (视频探索):**
`bash
# 使用视频探索器为视频找到更小的文件大小
./target/release/vidquality explore --path /path/to/video.mov
`

*注意: 确切的工具名称 (`imgquality`, `vidquality-hevc` 等) 和命令可能会有所不同。请使用 `--help` 标志获取详细说明。*

---

## 📄 License / 许可证

**[English]**

This project is licensed under the MIT License.

**[中文]**

本项目采用 MIT 许可证。
