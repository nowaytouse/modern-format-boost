# Modern Format Boost

![Version](https://img.shields.io/badge/version-0.11.3-0969DA?style=for-the-badge&logo=rust&logoColor=white)
![Rust](<https://img.shields.io/badge/rust-2024_edition_(nightly)-E57324?style=for-the-badge&logo=rust&logoColor=white>)
![Platform](https://img.shields.io/badge/platform-macOS_%7C_Linux_%7C_Windows-8257E5?style=for-the-badge&logo=apple&logoColor=white)
![License](https://img.shields.io/badge/license-MIT-00B265?style=for-the-badge)

**下一代媒体优化引擎 —— 零质量损失，最大化压缩。**

[English](../README.md) · [简体中文](README_ZH.md) · [繁體中文](README_ZH_TW.md) · [日本語](README_JA.md) · [한국어](README_KO.md) · [Español](README_ES.md) · [Français](README_FR.md) · [Português](README_PT.md) · [Русский](README_RU.md) · [العربية](README_AR.md)

## 什么是 Modern Format Boost？

**Modern Format Boost** 是一个高性能、基于 Rust 的媒体优化引擎。它按媒体领域划分工作：

- **`img`**：**仅静图** → **JXL**（`--codec hevc`）/ **AVIF**（`--codec av1`）；**已验证动图** → **忽略**（请**另行** `vid run`，img **不转发**）
- **格式识别**：靠文件头/内容（`detect_format_from_bytes` 等），扩展名仅用于纠错/告警，**不单凭 `.gif`/`.webp` 判动图**
- **动图判定**：`detect_animation` + 静图证明（真单帧 GIF/WebP/APNG 等仍可走 img，不是「见扩展名就 ignore」）
- **`vid`**：**视频 + 动图** → **HEVC/AV1**（默认 **HEVC**）
- **`--codec`**：`img` 与 `vid` **都有**（默认 `hevc`），**含义不同**：`img` 的 `hevc`→**JXL**、`av1`→**AVIF 静图**；`vid` 的 `hevc`/`av1`→**视频交付**。**禁止** img 转发/代跑 vid。

典型路线：

- 📸 **`img run`**：静图 → JXL；有损现代静图/**跳过**；**动图忽略**
- 🎬 **`vid run`**：H.264、动图 WebP/GIF 等 → HEVC/AV1；由 `--codec` 与 `--apple-compat` 决定

详细路由表：[`DELIVERY_STRATEGY_ROUTING.md`](DELIVERY_STRATEGY_ROUTING.md)。

可以将其视为一个保守的优化器，宁愿选择诚实的跳过/忽略结果，也不愿造成隐性的质量损坏：

- 🍎 **苹果生态优先**：全苹果兼容模式、实况照片 (Live Photo) 检测、AAE 挂载文件处理。
- 🔒 **元数据守护者**：保留 EXIF、XMP、ICC 配置文件、创建时间戳、macOS xattrs、Finder 标签。
- ⚡ **感知速度优化**：“深度优先”排序策略——优先处理目录层级较深的文件，然后按文件大小和格式排序，以确保高效的批量处理和最大吞吐量。
- 🎞️ **HDR10+ 动态元数据**：通过提取挂载文件和 x265 SEI 注入，实现 SMPTE 2094-40 元数据的完整保留。
- 🌅 **HDR 增益图合成与保留**：自动从 HEIC 和 UltraHDR JPEG 增益图合成高保真 HDR JXL，并将无法内嵌进 JXL 的辅助资产（如 HEIC 深度图、UltraHDR 原始增益图）作为 sidecar 保留。
- **🔍 厂商元数据识别**：智能扫描 HEIC 文件中三星/谷歌特定的 XMP 命名空间，以确保最大程度的上下文保留。

## ⚠️ 免责声明与重要提示

1. **数据安全第一**：为避免任何潜在的数据丢失，强烈建议将处理后的文件输出到单独的目录（例如，使用 `-o /path/to/output`），而不是使用原地转换 (`--in-place`)，特别是对于不可替代的媒体。
2. **测试版软件**：虽然该程序已经过广泛的测试、调试和优化以防止质量或数据丢失（详见更新日志），但不能保证 100% 无错误。请在 GitHub 上报告您遇到的任何问题。
3. **计算洞察**：虽然针对效率进行了优化（尤其是在苹果 M 系列芯片上），但在 `--ultimate` 模式下处理大规模批处理仍可能耗时较长。它将长时间占用系统资源；请相应地规划您的任务。
4. **工具成熟度**：统一工具（`img`、`vid`）默认使用 HEVC，它比 AV1 策略更成熟、更稳定。对于高可靠性的生产任务，建议使用 HEVC（默认）。

## 🔒 隐私与数据完整性

**Modern Format Boost** 构建在“本地优先”架构之上，确保您的创意资产完全在您的控制之下。

- **离线操作**：100% 离线处理。无遥测、无使用跟踪或云端请求。核心二进制文件不含任何网络相关代码。
- **Rust 加固运行时**：使用 Rust 构建，原生消除内存损坏错误（缓冲区溢出等）。
- **安全集成**：所有外部工具（FFmpeg、cjxl）都通过安全的、转义的原语调用——绝不通过原始 shell 执行——从而防止任意命令注入。
- **路径隔离**：先进的规范化处理可防止目录遍历，并保护无关的系统文件。
- **系统路径黑名单**：内置敏感系统目录屏蔽，防止意外修改操作系统文件。
- **动态资源平衡**：根据内存/CPU 负载自动调整处理线程，防止极端任务期间系统崩溃。
- **全方位元数据管家**：严格逐位保留 EXIF、XMP、ICC 和文件系统时间戳 (btime/mtime)。
- **安全处理与会话隔离**：
  - **零工作区污染**：集中式跟踪 (`~/.mfb_progress/`) 保持您的媒体文件夹 100% 清洁。您的照片/视频中不会留下隐藏的元数据文件。
  - **无冲突临时文件**：每个中间分析文件（YUV 流、分析分段）都使用随机 UUID 进行唯一标识。这可以防止多实例碰撞，并确保清理时的“手术级精度”。
  - **启动即清理**：无论任务成功完成还是在中断后恢复，系统都会自动清除所有瞬态数据。这种“自清理”架构确保您的磁盘不会留下被遗弃的处理残余。
  - **智能检查点重置**：自动检测用户何时手动删除输出目录以“重新开始”，即使在恢复模式下也会触发完整的状态重置。

## 🛠️ 技术深入：工作流程 —— 流水线

### 图像流水线逻辑

每个文件都会经过多阶段决策流水线：

- **阶段 1 — 智能检测**：在二进制级别分析 JPEG DQT 表（UltraHDR 增益图检测）、WebP VP8L 块和 AVIF `av1C` 框。现在具备 **零技术债架构**，100% 符合 Clippy 标准，并具有稳健的 `OpenEXR`/`JPEG 2000` 标题解析。
- **阶段 2 — 路由与编码**：JPEG 使用 JXL VarDCT（位精确）；无损源（PNG、无损 WebP/AVIF/HEIC/EXR/JP2）使用 Modular 模式。
- **阶段 3 — 迂回路径**：TIFF/WebP/BMP/HEIC 等格式被预处理为临时 16 位 PNG 或 **32 位 OpenEXR**，以确保在不损失质量的情况下兼容 `cjxl`（8/16/32 位匹配流水线）。
- **阶段 4 — HDR 增益图合成**：拦截带有增益图（苹果/谷歌）的 HEIC 资产和 UltraHDR JPEG，合成真实 HDR JXL 输出，并将无法内嵌的原始增益图/深度图作为 sidecar 保留。
- **阶段 5 — img 仅静图**：`img run` 对**已验证动图** **ignore**（`IMG_ANIMATED_HANDOFF`）；**真单帧** GIF/WebP 等可走 JXL；其余动图与所有视频请用 **`vid run`**。
- **阶段 6 — 循环意图 v3**：共享的循环意图逻辑决定动画媒体是保持类似 GIF 的状态还是进入视频流水线。苹果兼容的现代动画交付策略在此集中管理。

### 视频流水线：三阶段饱和搜索

1. **阶段 1：GPU 粗略搜索**：在硬件编码器（VideoToolbox/NVENC）上进行二分搜索，以找到“质量拐点”。
2. **阶段 2：CPU 精细调节**：将 GPU CRF 映射到 `x265` 等级。使用 **冲刺与回溯 (Sprint & Backtrack)**（成功时双倍步长，超出时重置为 0.1）。
3. **阶段 3：终极 3D 质量闸门**：要求同时通过 VMAF-Y ≥ 86.0（基准线，动态关联）、CAMBI ≤ 6.0（色带）和 PSNR-UV ≥ 30.0 dB（色度基准线）。
   - **融合评分**：结合 MS-SSIM + SSIM_All（权重 0.6/0.4）进行稳健的结构分析。
   - **色度防护**：自动检测可能导致 libvmaf MS-SSIM 崩溃的小分辨率，并回退到仅 Y 评分，以确保处理可靠性。
   - _注：在 `--ultimate` 模式下，只有在 **连续 50 个样本** 显示零质量增益后，搜索才会终止，确保绝对饱和。_

### 元数据与 HDR 保留

- **HDR**：保留 bt2020 原色、PQ/HLG TRC 和母带显示元数据。
- **杜比视界 (Dolby Vision)**：通过 `dovi_tool` 提取 RPU 并注入 x265（Profile 7 → 8.1 转换）。
- **macOS xattrs**：通过 `copyfile` 和 `setattrlist` 保留 Finder 标签、添加日期和创建时间戳。

### 🖥️ 运行状态

![Runtime](../assets/runtime.png)

运行状态

### 两个二进制程序

| 二进制    | 输入        | 主要输出           | `--codec hevc\|av1`                     |
| --------- | ----------- | ------------------ | --------------------------------------- |
| **`img`** | 仅静图      | JXL / AVIF / 跳过  | `hevc`→JXL，`av1`→AVIF；动图 **ignore** |
| **`vid`** | 视频 + 动图 | MP4/MOV/GIF / 跳过 | **有**（默认 `hevc`）                   |

此外还有一个 **macOS 双击应用** (`Modern Format Boost.app`)，用于拖放式批量处理。

## 交付策略（HEVC / AV1）

Rust SSOT：[`delivery_codec_strategy.rs`](../crates/foundation/src/delivery_codec_strategy.rs)。
工程师详表：[`DELIVERY_STRATEGY_ROUTING.md`](DELIVERY_STRATEGY_ROUTING.md)。

**`img run` 与 `vid run` 都接受 `--codec hevc|av1`**（默认 **hevc**），但**不是同一套语义**：

| 二进制    | `hevc`           | `av1`                          |
| --------- | ---------------- | ------------------------------ |
| **`img`** | 静图批量 **JXL** | 静图 **AVIF** 策略（有损分支） |
| **`vid`** | 视频 **HEVC**    | 视频 **AV1**                   |

**`img` 只编码静图**，**不会**把文件交给 `vid` 处理。动图/无法确认仅静图 → **ignore**（审计类 `img_animated_handoff` 仅表示跳过，**不是转发**）。**真单帧** GIF/WebP/APNG 等经多方验证后可走 JXL/AVIF。**`vid`** 单独处理视频与动图（含 GIF、动图 WebP、**APNG**、动图 AVIF/HEIC 等）。

### 两层策略

| 层                                  | `img` | `vid` |
| ----------------------------------- | ----- | ----- |
| **静图格式**（JXL / API 可走 AVIF） | ✅    | —     |
| **视频交付编码**（HEVC/AV1）        | ❌    | ✅    |

### `img run`（无 `--codec`）

| 输入                                    | 行为                                                               |
| --------------------------------------- | ------------------------------------------------------------------ |
| 静图 / **真单帧**可动画容器（多方验证） | → JXL                                                              |
| 动图 / 封面流歧义                       | **忽略**（需动图时请**另行**运行 **`vid run`**，img 不会自动转发） |

### `vid run` + `--codec hevc`（默认）

| 阶段         | 行为                                                                              |
| ------------ | --------------------------------------------------------------------------------- |
| **循环意图** | 可能保留 GIF（尤其 `--apple-compat` 短动画）；`--force` 可强制走视频。            |
| **跳过规则** | 普通模式常跳过已是 HEVC；Apple 模式可把 VP9/AV1 转为 HEVC 交付。                  |
| **无损源**   | HEVC 无损 MKV。                                                                   |
| **有损视频** | MP4/MOV + `explore_hevc_with_gpu` + x265；ultimate 双预设；HDR 探索带 x265 参数。 |

### `vid run` + `--codec av1`

| 阶段         | 行为                                                |
| ------------ | --------------------------------------------------- |
| **有损视频** | 仅 MP4 `av01` + AV1 探索；不支持 `--apple-compat`。 |
| **无损归档** | 当前仍走 **HEVC 无损 MKV**（AV1 归档未实现）。      |

### 共享标志（视频路径）

| 标志             | HEVC                 | AV1           |
| ---------------- | -------------------- | ------------- |
| `--explore`      | GPU HEVC + x265      | GPU AV1 + SVT |
| `--ultimate`     | 快搜 → 慢成片 x265   | SVT 单预设    |
| `--apple-compat` | MOV `hvc1`、GIF 策略 | **CLI 拒绝**  |

完整逐步表见 [`DELIVERY_STRATEGY_ROUTING.md`](DELIVERY_STRATEGY_ROUTING.md)。

## 📉 真实世界压缩示例

| 输入格式       | 原始大小 | 输出格式       | 输出大小 | 节省     | 方法             |
| :------------- | :------- | :------------- | :------- | :------- | :--------------- |
| 风景 JPEG      | 4.2 MB   | **JXL**        | 3.3 MB   | **~21%** | 无损组件重建     |
| 截图 PNG       | 2.5 MB   | **JXL**        | 1.1 MB   | **~56%** | Modular d=0.0    |
| 运动相机 H.264 | 1.2 GB   | **HEVC**       | 480 MB   | **~60%** | GPU/CPU CRF 搜索 |
| 动画 WebP      | 15 MB    | **AV1 / HEVC** | 1.8 MB   | **~88%** | 转码为视频格式   |

## 📊 处理矩阵

### 图像格式决策矩阵

| 输入格式                                   | 静态？ | `img run` 中的动作 | 输出       | 备注                              |
| :----------------------------------------- | :----: | :----------------- | :--------- | :-------------------------------- |
| JPEG                                       |   ✅   | **无损重建**       | `.jxl`     | 位精确 `cjxl --lossless_jpeg=1`   |
| PNG / TIFF / BMP / 其他无损静态图          |   ✅   | **无损转换**       | `.jxl`     | 可能先走迂回路径                  |
| WebP / AVIF / HEIC / HEIF (无损静态)       |   ✅   | **转换**           | `.jxl`     | 允许转换无损现代静态图            |
| 带有增益图的 HEIC / HEIF                   |   ✅   | **HDR 合成**       | `.jxl`     | 增益图路径合成线性 HDR            |
| 静态验证后的遗留有损静态图                 |   ✅   | **近无损转换**     | `.jxl`     | 当前 `img run` 批量路径专注于 JXL |
| 有损 WebP / AVIF / HEIC / HEIF 静态图      |   ✅   | **跳过**           | 保留原文件 | 避免代际损失                      |
| JXL 静态图                                 |   ✅   | **跳过**           | 保留原文件 | 已经是最佳格式                    |
| 动画 GIF / WebP / APNG / HEIC / HEIF / JXL |   ❌   | **img 忽略**       | —          | 请用 **`vid run`**                |

### `img` 入口

| 入口                  | 静图输出                               | 动图        | AVIF                      |
| --------------------- | -------------------------------------- | ----------- | ------------------------- |
| **`img run`**         | JXL（`hevc`）或 AVIF 有损分支（`av1`） | **忽略**    | 仅 `av1` 有损分支         |
| **`smart_convert()`** | JXL 或 AVIF（`determine_strategy`）    | 域外 ignore | 部分有损非 JPEG 仍走 AVIF |

### 动画媒体决策矩阵（仅 `vid`）

| 输入格式                                    | 归属                  | 动作              | 输出            | 备注                         |
| :------------------------------------------ | :-------------------- | :---------------- | :-------------- | :--------------------------- |
| GIF                                         | `vid`                 | **循环意图**      | `.gif` 或视频   | `--apple-compat` 策略        |
| 动画 WebP / AVIF / APNG / HEIC / HEIF / JXL | `vid`                 | **HEVC/AV1 交付** | `.mp4` / `.mov` | `animated_image` + `--codec` |
| 带有 `--apple-compat` 的短无声现代动画      | `vid` + `loop_intent` | **强制 GIF**      | `.gif`          | 时长 `<= 6s`                 |
| 带有 `--apple-compat` 的长现代动画          | `vid` + `loop_intent` | **不强制 GIF**    | 视频目标        | 时长 `>= 15s` 保持视频风格   |
| 带有 `--apple-compat` 的不确定现代动画      | `vid` + `loop_intent` | **强制 GIF**      | `.gif`          | 兼容性回退                   |

### 视频编码决策矩阵

| 输入编码                  | 普通模式     | `--apple-compat` 模式 | 备注                            |
| :------------------------ | :----------- | :-------------------- | :------------------------------ |
| H.264 (AVC)               | **转换**     | **转换**              | 两种模式下都不会被预跳过        |
| VP9                       | **跳过**     | **转换为 HEVC**       | 苹果不兼容的源                  |
| AV1                       | **跳过**     | **转换为 HEVC**       | 苹果不兼容的源                  |
| VVC / AV2                 | **跳过**     | **转换为 HEVC**       | 苹果不兼容的源                  |
| HEVC (H.265)              | **跳过**     | **跳过**              | 已经是苹果原生目标              |
| ProRes / DNxHD / 遗留编码 | **按需转换** | **按需转换**          | 最终的保留/跳过仍取决于优化结果 |

路由后仍需通过质量和大小门槛。在 `--ultimate` 和其他质量匹配流程中，符合转换条件的路由如果生成的文件未通过质量/大小要求且没有允许的最佳实践回退，最终仍可能以跳过告终。

### HDR 格式策略

| HDR 类型          | 检测                                       | 保留策略                                                                         |
| :---------------- | :----------------------------------------- | :------------------------------------------------------------------------------- |
| **HDR10**         | side_data 中的 mastering_display + max_cll | 通过 FFmpeg 参数完整保留静态元数据                                               |
| **HEIC 增益图**   | HEIC 辅助图像 (苹果/三星/ISO)              | 合成为 32 位线性 HDR -> JXL (真实 HDR)                                           |
| **UltraHDR JPEG** | JPEG APP1/APP2 + XMP (hdrgm:)              | 合成为真实 HDR JXL；原始增益图保留为 `.gainmap.jpg`；桥接 `hdrgm` 元数据用于溯源 |
| **HLG**           | color_trc = arib-std-b67                   | 保留原色 + TRC                                                                   |
| **杜比视界**      | 流/帧中的 DOVI side_data                   | 通过 `dovi_tool` 提取 RPU → x265 注入；Profile 7 → 8.1 转换                      |
| **HDR10+**        | ST2094-40 动态元数据                       | 通过 `hdr10plus_tool` 挂载提取和 x265 注入支持（保留 Profile A/B 元数据）        |
| **SDR**           | 无 HDR 标记                                | 标准处理 (yuv420p)                                                               |

## ⬇️ 安装

### 预编译二进制文件

对于不想安装 Rust 工具链的用户，可以从 **[Releases](https://github.com/nowaytouse/modern-format-boost/releases)** 页面下载预编译的二进制文件。

```bash
# macOS/Linux 一键安装（以 macOS ARM64 为例）
curl -LO https://github.com/nowaytouse/modern-format-boost/releases/latest/download/modern-format-boost-aarch64-apple-darwin.tar.gz
tar -xzf modern-format-boost-aarch64-apple-darwin.tar.gz

```

### 前提条件

| 工具                 | 必需？ | 用途                 | 安装命令                                                                                    |
| :------------------- | :----: | :------------------- | :------------------------------------------------------------------------------------------ |
| **Rust** (nightly)   |   ✅   | 构建与安装           | `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \| sh && rustup default nightly` |
| **FFmpeg** (5.0+)    |   ✅   | 视频处理与指标计算   | `brew install ffmpeg` / `apt install ffmpeg`                                                |
| **libjxl**           |   ✅   | JXL 编码核心         | `brew install jpeg-xl`                                                                      |
| **ExifTool**         |   ✅   | 元数据保留           | `brew install exiftool`                                                                     |
| **ImageMagick**      |   ✅   | 图像迂回路径         | `brew install imagemagick`                                                                  |
| **libwebp**          |   ✅   | WebP 原生解码        | `brew install webp`                                                                         |
| **libheif**          |   ✅   | HEIC/HEIF 解码       | `brew install libheif`                                                                      |
| **PostgreSQL** (12+) |   ✅   | 缓存与质量特征数据库 | `brew install postgresql pgvector` / `apt install postgresql`                               |
| **dovi_tool**        |  可选  | 杜比视界 RPU 提取    | `cargo install dovi_tool`                                                                   |
| **hdr10plus_tool**   |  可选  | HDR10+ 元数据提取    | `cargo install hdr10plus_tool`                                                              |

#### macOS (Homebrew)

```bash
brew install ffmpeg jpeg-xl exiftool imagemagick webp libheif postgresql pgvector
# 可选（用于杜比视界 / HDR10+ 源）：
# cargo install dovi_tool hdr10plus_tool
```

> [!TIP]
> 对于想要所有高级功能（AI 滤镜、FDK-AAC 等）的高级用户，请参阅我们的 [高级 FFmpeg 设置指南](FFMPEG_SETUP.md)，了解如何在不破坏系统依赖的情况下安装功能齐全的版本。

#### Linux (Ubuntu/Debian)

```bash
sudo apt update && sudo apt install ffmpeg libimage-exiftool-perl imagemagick \
  webp libheif-dev postgresql postgresql-contrib postgresql-server-dev-all
# Linux 下必须编译并安装 pgvector 扩展：
git clone --branch v0.5.1 https://github.com/pgvector/pgvector.git
cd pgvector
make && sudo make install
# JPEG XL (libjxl) 在旧发行版上可能需要 PPA 或源码构建
```

#### Windows

推荐：使用 **winget** 进行一键安装：

```powershell
winget install ffmpeg.ffmpeg ImageMagick.ImageMagick OliverBetz.ExifTool \
  libheif.libheif Google.WebP PostgreSQL.PostgreSQL
# 注意：将 pgvector 二进制文件拷贝至您的 PostgreSQL 目录下。参见 https://github.com/pgvector/pgvector
# 可选（用于杜比视界 / HDR10+ 源）：
# cargo install dovi_tool hdr10plus_tool
```

### 🗄️ 数据库设置

Modern Format Boost 使用 PostgreSQL (并启用 `pgvector` 扩展) 作为强制的本地缓存与质量特征推理引擎。`img` 和 `vid` 二进制程序在启动时均会尝试连接数据库，如果数据库服务无法连接，程序将直接报错并退出。

#### 1. 启动 PostgreSQL 服务

- **macOS**: `brew services start postgresql`
- **Linux**: `sudo systemctl start postgresql`

#### 2. 创建数据库

默认的数据库名称为 `modern_format_boost`。在运行工具前，请创建该数据库：

```bash
createdb modern_format_boost
```

或通过 SQL：

```sql
CREATE DATABASE modern_format_boost;
```

_注意：在程序首次成功连接数据库时，会自动初始化所需的表结构、视图，并自动启用 `vector` 扩展。您不需要手动执行 SQL 迁移文件进行初始化。_

## ⚙️ 环境变量与参数调优

您可以通过设置以下环境变量来自定义引擎的运行时行为：

| 环境变量             | 默认值                                       | 说明                                                                                           |
| :------------------- | :------------------------------------------- | :--------------------------------------------------------------------------------------------- |
| `MFB_PG_CONNSTR`     | `postgresql://localhost/modern_format_boost` | PostgreSQL 数据库连接字符串。                                                                  |
| `MFB_HOME_ROOT`      | `~/.modern_format_boost`                     | 配置根路径以及持久化训练数据存放目录。                                                         |
| `MFB_LOG_DIR`        | `~/.modern_format_boost/logs`                | 运行日志与会话日志目录（严禁设置在 Git 工作区下）。                                            |
| `MFB_PERF_TIER`      | （自动检测）                                 | 线程与算力调度档位：`relaxed` (宽松/高并发), `balanced` (默认均衡), 或 `tight` (紧凑/低并发)。 |
| `MFB_LOW_MEMORY`     | `0` (false)                                  | 设为 `1` 时，强制性能调度器进入 `tight`（紧凑）模式运行。                                      |
| `MFB_MULTI_INSTANCE` | `0` (false)                                  | 设为 `1` 时，通知性能调度器当前有多实例竞争，主动下调并发资源上限。                            |

### ⚡ 动态资源调度器 (SSOT)

MFB 使用内存与负载感知调度器，根据系统运行压力实时调整并行批处理数与子线程配额。它有三个档位：

- **`relaxed`**：最大并发模式，系统内存压力极低且没有多实例竞争时启用。
- **`balanced`**：默认均衡模式，兼顾吞吐量与系统平稳度。
- **`tight`**：紧凑限额模式，严格控制并发以防止内存溢出（OOM）或系统卡死。当系统可用内存低于 **2304 MB** 或可用 RAM 比例低于 **24%** 时，将由 `PREEMPTIVE_TIGHT` 保护机制自动触发。

### 从源码构建

```bash
git clone https://github.com/nowaytouse/modern_format_boost.git
cd modern_format_boost
cargo build --release
```

## 🚀 使用方法

### 快速开始

```bash
# 静图 → JXL（目录内动图会被忽略）
img run /path/to/media

# 视频 + 动图（HEVC 默认）
vid run /path/to/media

# AV1 交付
vid run --codec av1 /path/to/media

vid strategy --codec hevc /path/to/video.mp4
```

### ⚡ 极速模式与智能断点续传

针对拖放式 UI 工作流，**Fast Img Flow**（极速模式，由 `crates/dev/src/bin/drag_and_drop_processor.rs` 驱动）带来了高可靠性的断点续传能力：

- **`WorkingCopyMarker` 状态管理**：安全追踪关闭和中断时的处理进度。
- **陈旧源文件检测**：如果原文件发生变更（数量或 hash 不匹配），系统会自动探测到源数据集过时，放弃脏重试并触发全新重建。
- **Fail-Closed 错误保护**：深度上下文捕获和 Blake3 校验保证在 `img run` 中断时不会出现任何文件损坏，安全回退。

### 详细选项

- `--ultimate`：存档级 **0.01 精度** 搜索（高质量，高时间成本）。
- `--apple-compat`：启用苹果生态系统兼容性（实况照片/AAE）。CLI 默认为开启；`--no-apple-compat` 可禁用。
- `--in-place`：替换原始文件。**警告：不可逆。**
- `-o /dir`：安全输出目录。（推荐）
- `--verbose`：显示详细的处理日志。
- `--no-recursive`：不进入子目录。
- `--force-video`：强制将动画图像视为视频，不考虑循环意图 (Loop Intent)。

### 高级子命令

- `img cache-stats`：查看 SQLite 分析缓存统计信息。
- `vid strategy <file>`：预览特定文件的流水线策略。
- `img restore-timestamps`：根据文件名模式批量修复创建日期（元数据恢复）。

### 💡 多实例说明

**Modern Format Boost** 原生支持运行多个窗口/实例。

- **并发处理**：允许运行多个窗口以独立处理不同路径。
- **注意**：请根据您的硬件 I/O 性能进行扩展；过高的并发可能会导致文件系统竞态条件。

## 🏗️ 架构

### CI/CD 与测试门控体系

Modern Format Boost 使用严苛的质量门控系统保证核心架构的零技术债：

- **Rust 优先开发工具链**：工程入口是 `crates/dev/src/bin` 下的 Rust 二进制；Python 原件仅作为兼容参考保留，直到确认可安全删除。
- **本地 CI 校验**：开发前务必使用 `just fix-gate` 或 `cargo run --locked -p dev --bin check_all -- --allow-non-nightly` 进行检查，这是代码格式、静态检查、以及自动化测试的“单一事实来源”(SSOT)。
- **测试强化与稳定性**：禁用 "Fail Fast" 以便在多平台上收集全量诊断信息；同时增加了对图像（如 JPEG 恢复断言）错误状态的深度上下文捕获。

### 核心架构

- `crates/img/`：静态图像优化器（当前 CLI 路径中为 `JXL` / 跳过 / 忽略）
- `crates/vid/`：视频和动画媒体优化器（`HEVC` / `AV1` / `GIF`）
- `crates/foundation/`：核心大脑（GPU/CPU 混合引擎、HDR 映射、元数据）
- `Modern Format Boost.app/`：macOS 拖放式 UI

## 📐 分层契约与训练

交付、推理、终端 UI 与训练栈的行为以**可审计的契约文档**为准（CI 密封测试约束）。扩展 `img` / `vid`、探索管线或 PostgreSQL 训练时请优先查阅：

| 层                    | 文档                                                                                                                                                  |
| --------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------- |
| **系统 SSOT 与审计**  | [`../.agents/harding/SSOT.md`](../.agents/harding/SSOT.md) (核心基准单一事实来源，替代了以往碎片化的审计文档)                                         |
| 媒体转换交付 (M1–M66) | [`MEDIA_CONVERSION_LAYER_CONTRACT.md`](MEDIA_CONVERSION_LAYER_CONTRACT.md) · [`MEDIA_CONVERSION_DELIVERY_SEAL.md`](MEDIA_CONVERSION_DELIVERY_SEAL.md) |
| 算法 / 推理门控       | [`ALGORITHM_LAYER_CONTRACT.md`](ALGORITHM_LAYER_CONTRACT.md)                                                                                          |
| 终端 UI               | [`UI_LAYER_CONTRACT.md`](UI_LAYER_CONTRACT.md)                                                                                                        |
| 日志 / 会话           | [`LOGGING_LAYER_CONTRACT.md`](LOGGING_LAYER_CONTRACT.md)                                                                                              |
| 数据库 / 多场景       | [`DATABASE_LAYER_CONTRACT.md`](DATABASE_LAYER_CONTRACT.md) · [`MULTI_SCENARIO_IMPLEMENTATION_GUIDE.md`](MULTI_SCENARIO_IMPLEMENTATION_GUIDE.md)       |

**静图质量训练**（high/low 分层、入库审计）：

- **唯一推荐入口**：`python3 crates/dev/scripts/run_training.py`（禁止 shell 包装，所有入口保护逻辑已统一至 SSOT.md）。
- **规则**：提交版 [`training_rules.json`](../crates/dev/src/config/training_rules.json)；本机目录与 ingest 上限写在 gitignore 的 `training_rules.local.json`。
- **Tier 引擎**：Rust [`training_tier_audit.rs`](../crates/foundation/src/training_tier_audit.rs) 与 JSON 阈值同步（熵死区、几何护栏）。
- **后台**：`python3 crates/dev/scripts/run_training.py --background` → 日志在统一日志目录（见 `docs/LOGGING_LAYOUT.md`）。

完整硬化说明见 [`CHANGELOG.md`](CHANGELOG.md) **0.11.3**。

文档总索引：[`DOCUMENTATION_INDEX.md`](DOCUMENTATION_INDEX.md)。

## ❓ 常见问题

**1. JXL 是否得到广泛支持？**
macOS 14+ / iOS 17+、Chrome 91+ 和 Firefox 128+ 已提供原生支持。然而，目前仍存在一些已知的生态系统问题：

- **动画**：现代动画格式 (JXL/AV1/HEIF) 在原生 macOS/iOS 照片应用或 Finder 中经常无法作为动画预览（仅显示静态），尤其是在通过 iCloud 同步时。它们在现代浏览器或专用工具中可以正常播放。
- **缩略图**：使用**灰度 ICC 配置文件**的 JXL 文件在 Finder/iCloud 中可能显示为**黑色缩略图**，尽管打开时渲染完美。
  对于位精确存档和高保真 HDR 存储，JXL 仍然是卓越的格式。

**2. HDR10+ 如何处理？**
完全支持。我们使用 `hdr10plus_tool` 提取 SMPTE 2094-40 动态元数据，并通过 `libx265` 的 `--dhdr10-info` 参数将其注回 HEVC 流。请确保已安装该工具以启用此功能。

**3. 为什么跳过 WebP/AVIF/HEIC？**
静态有损 WebP/AVIF/HEIC/HEIF 通常会被跳过，因为它们本身已经是现代有损格式，重新编码可能会面临代际损失，而收益有限。当前代码中的重要例外包括：

- 无损现代静态图仍可转换为 JXL
- HEIC/HEIF 增益图资产可以合成为 HDR JXL
- UltraHDR JPEG 会合成为 HDR JXL，并把内嵌增益图保留为原始 `.gainmap.jpg` sidecar，便于审计与回滚恢复
- 动画现代格式不由 `img` 处理；它们通过 `vid` 和 `loop_intent` 进行路由

---

## ⚖️ 许可证

根据 **MIT 许可证** 授权。

## 运行时依赖

本项目编排了多个开源巨作。我们感谢其作者的贡献：

| 组件                   | 许可证     | 用途               |
| ---------------------- | ---------- | ------------------ |
| **FFmpeg**             | LGPL/GPL   | 视频处理与指标计算 |
| **libjxl** (cjxl/djxl) | BSD-3      | JPEG XL 编码       |
| **ExifTool**           | Perl/GPL   | 元数据保留         |
| **ImageMagick**        | Apache 2.0 | 图像迂回路径       |
| **SVT-AV1**            | BSD+Patent | AV1 编码           |
| **x265**               | GPL-2.0    | HEVC 编码          |

所有 Rust 依赖项均通过 `Cargo.toml` 管理，并遵循其各自的开源许可证（MIT/Apache/BSD）。
