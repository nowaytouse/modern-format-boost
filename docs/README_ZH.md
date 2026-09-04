# Modern Format Boost

![Version](https://img.shields.io/badge/version-0.11.3-0969DA?style=for-the-badge&logo=rust&logoColor=white)
![Rust](<https://img.shields.io/badge/rust-2024_edition_(nightly)-E57324?style=for-the-badge&logo=rust&logoColor=white>)
![Platform](https://img.shields.io/badge/platform-macOS_%7C_Linux_%7C_Windows-8257E5?style=for-the-badge&logo=apple&logoColor=white)
![License](https://img.shields.io/badge/license-MIT-00B265?style=for-the-badge)

**以证据驱动媒体焕新：保留源语义、核验最终交付、无法证明时失败关闭。**

[English](../README.md) · [简体中文](README_ZH.md)

## 什么是 Modern Format Boost？

**Modern Format Boost** 是一个基于 Rust 的媒体格式焕新工具，按媒体领域明确分工：

- **`img run`**：只处理静态图片，普通路径只交付 **JXL**。默认使用本地精确检测；只有显式开启可选质量启发功能时才查询 PostgreSQL。
- **`img fast-img --strategy jxl`**：真实 JPEG → 可逐字节重建的 JXL；确认有损的现代静态格式可进入独立的 Apple Photos 托管路径。
- **`img fast-img --strategy avif`**：非 AVIF 静图进入 Meme Mode 搜索；已有 AVIF 绝不重复编码。已清洁的 AVIF 逐字节托管，含描述元数据的 AVIF 只做容器级清理，并要求主图像数据哈希保持完全一致。
- **`vid run`**：拥有视频与动图的 HEVC/AV1 交付。`img` 不会静默转发或代跑 `vid`。
- **格式识别**：文件头、容器结构与动画证据优先于扩展名；扩展名不决定真实媒体类型。

典型路线：

- 📸 **`img run`**：JPEG、普通无损栅格图，以及已被正面证明为无损的现代静态格式（WebP/AVIF/HEIC/JP2）→ JXL；有损或压缩语义不明的现代容器保留原生字节；**动图忽略**
- ⚡ **`img fast-img --strategy jxl`**：真实 JPEG → 永久可逆 JXL；已证实有损的现代静图进入独立 JXL Tier 2
- 🧩 **`img fast-img --strategy avif`**：非 AVIF 静图 → 有界 AVIF 质量/大小搜索；已有 AVIF → 原样托管或仅清理容器元数据，绝不重新编码。内嵌 Exif/XMP/ICC 会被清理，增益图等结构性图像项目不会被压平；有效 XMP 侧车只在最终交付证明后删除
- 🎬 **`vid run`**：H.264、动图 WebP/GIF 等 → HEVC/AV1；由 `--codec` 与 `--apple-compat` 决定

路由实现来源：[`delivery_codec_strategy.rs`](../crates/foundation/src/convert/delivery_codec_strategy.rs)。

### 项目保证什么，又不保证什么？

- 只有通过对应路径的解码、质量、元数据与完整性闸门，候选文件才会交付。受大小约束的路径比较的是编码媒体载荷；找不到符合策略的候选时，保留源文件并明确跳过或失败。
- JPEG→JXL 会直接用真实 JXL 尝试 `djxl --reconstruct_jpeg`，不会把帮助文本当作能力真相；只有解码器明确报告“不支持该参数”时，才退回由 `.jpg` 扩展名选择的官方兼容接口。两条路径都必须得到正向 JPEG 重建诊断、非空输出、无像素转 JPEG 回退以及逐字节哈希证明，不会因版本号或退出码猜测成功。JBRD、原始 Exif/XMP/JUMBF 与编码数据会被冻结。JPEG→JXL 时，外部 XMP 会作为 `xml ` overlay **追加进 JXL 容器内**并再次核验精确重建；JXL→JPEG 时，`restore-jpeg` 不改写逐字节恢复出的 JPEG，而把最新有效 overlay 作为单独哈希核验的同名 `.xmp` 侧车交付，因为把新增 XMP 嵌进 JPEG 必然会改变原 JPEG 字节。
- Overlay 采用同目录唯一临时文件、源身份/哈希复核、原子替换与文件/父目录刷盘；版本化审计链记录 JBRD、overlay、最终容器与重建哈希但不记录媒体内容。完整规则见 [`JXL_XMP_ARCHIVE_CONTRACT.md`](hardening/JXL_XMP_ARCHIVE_CONTRACT.md)。
- 现代容器的元数据处理采用“先正面合并、再证明”的路径。已证实无损的 AVIF、HEIC/HEIF、WebP 与 JP2 进入 JXL 转换；XMP 会随 JXL 容器写入，并核验像素、尺寸、辅助/HDR/来源证明标记及其余结构元数据。FastImg JXL Tier 2 对原生现代源文件使用暂存副本和格式原生写入器，只有同一组证明通过才提交；AVIF Meme Mode 则明确清理内嵌 Exif/XMP/ICC，不把侧车合并进输出，只在最终证明后精确删除侧车。已有 AVIF 不重新编码：清洁文件逐字节保留，需清理时也必须证明主图像 SHA-256 及 `avifdec` 可见的编码/HDR/增益图特征不变。若选定路径无法证明增益图、辅助项目、签名、未知属性/chunk 或编码载荷完整，才明确保留原始媒体与侧车供复核，绝不静默丢弃。JPEG、PNG、JPEG XL、AVIF/HEIF、WebP、TIFF/BigTIFF 与 GIF 内已有的 C2PA/JUMBF 真实性清单一律视为不可改写的存档数据：合并元数据时保留已签名媒体和侧车，不以主图像哈希未变冒充完整签名仍有效。
- 项目不是“魔法缩容器”。完整文件可能因容器与元数据而更大，高质量候选也可能没有任何空间收益。
- 探索以时间换取更精确的候选：目标是在有效质量/大小策略内寻找最高质量点，而不是无条件得到最小文件。输入越多、越复杂、差异越大，耗时越高；`--ultimate` 会主动扩大搜索成本。

### IMG 生产矩阵与测试边界

IMG 回归套件直接覆盖公开的检测、转换与交付边界；仅编译成功不视为生产证据。目前矩阵包括：

- JPEG、PNG、GIF、WebP、TIFF、BMP、JXL、AVIF、HEIC、HEIF、JP2 的内容识别，含伪装扩展名、截断头和垃圾输入；
- baseline、progressive、灰度 JPEG，以及显式失败关闭的 CMYK 路径；
- EXIF Orientation 1–8、逐字节 JBRD 重建、源文件保留和单输出计数；
- 有/无 XMP 侧车及带 ICC 配置文件的 JPEG，包含 XMP 提取和源侧车不可变核验；
- foundation 图像分析测试覆盖 UltraHDR/MPF JPEG 检测、增益图元数据边界，以及 HDR
  合成失败关闭与保留原件路径；
- 真实 PNG/TIFF/WebP/GIF/AVIF/JXL/HEIC 夹具、静图/动图分类、权威解码器、尺寸和非空像素检查；
- 合成 PNG/BMP/TIFF/TGA/ICO/CUR/NetPBM → 容器化 JXL 的真实编码，RGBA16 逐像素一致、源文件不变及 XMP overlay 提取核验；以及
- foundation 与 IMG 套件中已有的损坏/截断输入、元数据 overlay、输出计数和空目录清理契约。

`cargo test --locked -p img --all-targets -- --list` 是当前检出 revision 的可复现
测试清单。生产矩阵锁定“截断 JPEG + XMP 保留源文件”、动画 WebP 分块分类、
  AVIF/HEIC sequence brand 边界、JXL Tier 2 与 AVIF Meme Mode 路由、已有 AVIF
  禁止重复编码与容器清理证明，以及清理空目录时保留
无关隐藏文件等回归。可选编解码器分支会明确报告可用性；清单不表示每台机器都
执行了所有外部编解码器或真实 Photos 事务。

运行完整 IMG 套件：

```bash
cargo test --locked -p img --all-targets -- --nocapture --test-threads=1
```

日常维护可以使用 `check_all` 的本地单包检查，避免把整仓库的昂贵门禁
混入 IMG 判断：

```bash
cargo run --locked -p dev --bin check_all -- \
  --allow-non-nightly --package img --required-only --no-expensive
```

需要单独检查视频时将 `img` 换成 `vid`。单包作用域使用该包的默认特性，
不会启动工作区 CI 配置；`--ci` 仍然只接受工作区作用域。GitHub 也把它们
显示为独立的 `IMG package quality` 与 `VID package quality` 作业，因此
视频失败不会混淆 IMG 的结果。

依赖外部工具的测试会明确报告编码器/解码器不可用，绝不会把未执行的分支宣称为已验证。这是充分的本地生产候选证据，但不等同于所有第三方编解码器或真实 Photos/iCloud 事务在所有机器上都绝无缺陷。

### 实际静态图像范围

| 范围 | 格式 / 行为 |
| :--- | :--- |
| 内容签名识别 | JPEG/JFIF、PNG/APNG、WebP、GIF、TIFF/BigTIFF、BMP、HEIC/HEIF/HIF、AVIF、JXL、JP2/J2K、ICO/CUR、QOI、EXR、FLIF、PSD、PNM 和 DDS；能识别不等于承诺转换 |
| 已核验转换核心 | 已确认静态的 JPEG，以及已证明无损的 PNG/TIFF/BMP/TGA/ICO/CUR/NetPBM/单帧 GIF/WebP/AVIF/HEIC/HEIF/JP2；使用格式专用的像素、元数据和结构证明 |
| 仅原件归档 | SVG/SVGZ 与相机 RAW（`CR2`、`CR3`、`NEF`、`ARW`、`DNG`、`RAF`、`RW2` 及项目已枚举的扩展列表）可被发现，但只逐字节保留；改名后的 DNG 仍按 TIFF `DNGVersion` 标签识别；栅格化会丢失矢量语义或传感器/CFA/厂商私有数据 |
| 不进入普通栅格转换 | 视频/动图、PSD/PSB/KRA/CLIP/Procreate/笔刷、AI/EPS/PDF、2D/3D/模型/工程源文件、DDS、HDR/EXR、QOI/FLIF 及未知/私有格式会被保留、跳过或忽略，不靠扩展名猜测 |

IMG 不会静默压平动图、多帧或视频容器：已证明只有一帧的 GIF/WebP/AVIF/HEIC/HEIF 仍是 IMG 静图；动态实例交给 `vid`。MP4/MOV/MKV/WebM 即使只有一帧也仍是视频容器，因为帧数不会消除时间轴、轨道、变换与色彩语义。直接像素→JXL 明确关闭渐进 DC 和合成噪声；普通任务使用 effort 7，ultimate/归档任务使用 effort 10，effort 11 仅用于独立且快速的 JPEG 比特流可逆转码。已生成的 JXL 无法原地无损切换渐进特性；改变它必须重新编码并重跑完整归档证明。

### 谁适合使用？

实机覆盖最充分的是 macOS、Apple 生态媒体库与显式 Photos 交付。Linux/Windows 仍可使用相邻输出和本地转换，但 Photos、TCC 与 iCloud 托管核验仅限 macOS，非 Apple 平台的生产实测覆盖目前较少。任何平台处理不可替代的档案前都应保留独立备份。

### 为什么开发这个项目？

多数一次性转换器会把同一套质量、努力值和速度策略应用到所有文件。Modern Format Boost 将每个媒体视为独立的搜索与交付决策：投入更多时间寻找满足当前大小/质量策略的候选，然后在清理源文件前核验元数据与完整性。额外耗时是有意设计；项目不会用“更快”或“更小”代替可审计的可信结果。

数据库、训练和质量门禁等较大的工程面用于让决策可检查，并捕获静默回退。可选系统仍然保持可选：普通 `img run`、FastImg 与 JPEG 恢复默认使用精确的本地分析；只有明确选择对应命令或选项时才使用 PostgreSQL 与学习型质量启发式。项目欢迎更多平台的生产证据，尤其是目前实机覆盖较少的 Linux 与 Windows 路径。

可以将其视为一个保守的优化器，宁愿选择诚实的跳过/忽略结果，也不愿造成隐性的质量损坏：

- 🍎 **苹果生态优先**：全苹果兼容模式、实况照片 (Live Photo) 检测、AAE 挂载文件处理。
- 🔒 **元数据守护者**：保留 EXIF、XMP、ICC 配置文件、创建时间戳、macOS xattrs、Finder 标签。
- ⚡ **感知速度优化**：“深度优先”排序策略——优先处理目录层级较深的文件，然后按文件大小和格式排序，以确保高效的批量处理和最大吞吐量。
- 🎞️ **HDR10+ 动态元数据**：通过提取挂载文件和 x265 SEI 注入，实现 SMPTE 2094-40 元数据的完整保留。
- 🌅 **HDR 增益图保留**：HEIC 增益图只有在能够合成高保真 HDR JXL、并把已解码增益图及已识别深度图作为核验侧车交付时才会转换；遇到未知辅助图像关系会显式保留原容器。UltraHDR JPEG 默认走精确 JPEG 归档路径，完整保留可逐字节重建的 MPF/增益图容器。
- **🔍 厂商元数据识别**：智能扫描 HEIC 文件中三星/谷歌特定的 XMP 命名空间，以确保最大程度的上下文保留。

## ⚠️ 免责声明与重要提示

1. **数据安全第一**：为避免任何潜在的数据丢失，强烈建议将处理后的文件输出到单独的目录（例如，使用 `-o /path/to/output`），而不是使用原地转换 (`--in-place`)，特别是对于不可替代的媒体。
2. **生产候选状态**：IMG 具有失败关闭的交付契约、单包可复现测试清单和明确的源文件保留证明。正式发布仍取决于目标机器的编解码器版本；Photos 流程还需要真实 macOS/TCC/iCloud 验收。不可替代的档案请保留独立备份，并在 GitHub 报告可复现问题。
3. **计算洞察**：虽然针对效率进行了优化（尤其是在苹果 M 系列芯片上），但在 `--ultimate` 模式下处理大规模批处理仍可能耗时较长。它将长时间占用系统资源；请相应地规划您的任务。
4. **命令语义不同**：普通 `img run` 当前只交付 JXL；FastImg AVIF 用 `--strategy avif`；只有 `vid run` 的 `hevc|av1` 表示视频编码器。不要跨命令推断参数语义。

## 🔒 隐私与数据完整性

**Modern Format Boost** 构建在“本地优先”架构之上，确保您的创意资产完全在您的控制之下。

- **本地媒体处理**：转换与核验不上传媒体、没有遥测；安装依赖、下载工具和 CI 本身仍可能访问网络。
- **Rust 加固运行时**：Rust 消除了大量内存不安全代码类别；路径边界、解析上限与交付闸门继续处理逻辑和外部工具风险。
- **安全集成**：所有外部工具（FFmpeg、cjxl）都通过安全的、转义的原语调用——绝不通过原始 shell 执行——从而防止任意命令注入。
- **路径隔离**：先进的规范化处理可防止目录遍历，并保护无关的系统文件。
- **系统路径黑名单**：内置敏感系统目录屏蔽，防止意外修改操作系统文件。
- **动态资源平衡**：根据内存/CPU 负载自动调整处理线程，防止极端任务期间系统崩溃。
- **元数据托管**：按目标格式能力保留或显式规范化 EXIF、XMP、ICC、时间戳与 macOS 扩展属性；项目不宣称所有容器的元数据都能逐位复制。
- **安全处理与会话隔离**：
  - **受控状态目录**：集中式进度状态位于 `~/.mfb_progress/`；相邻工作副本和用户明确选择的输出仍会出现在媒体目录旁。
  - **无冲突临时文件**：每个中间分析文件（YUV 流、分析分段）都使用随机 UUID 进行唯一标识。这可以防止多实例碰撞，并确保清理时的“手术级精度”。
  - **有界清理**：可丢弃的中间文件会清理；用于显式恢复与身份核验的持久状态会保留，不会把“发现状态”当作静默恢复或删除源文件的许可。
  - **智能检查点重置**：自动检测用户何时手动删除输出目录以“重新开始”，即使在恢复模式下也会触发完整的状态重置。

## 🛠️ 技术深入：工作流程 —— 流水线

### 图像流水线逻辑

每个文件都会经过多阶段决策流水线：

- **阶段 1 — 精确检测**：通过文件头、容器结构与权威工具证据识别 JPEG、WebP、AVIF、HEIC、JP2 等格式；无法证明有损/无损或静态/动画语义时失败关闭。
- **阶段 2 — 路由与编码**：JPEG 只接受能逐字节重建的 JXL 转码；PNG/TIFF/BMP/TGA/ICO/CUR/NetPBM/单帧 GIF 以及被正面证明为无损的 WebP/AVIF/HEIC/HEIF/JP2 进入像素精确 JXL；SVG/SVGZ 和相机 RAW 逐字节保留；已有 JXL 与有损/语义不明的现代容器保留原编码。
- **阶段 3 — 异常媒体路径**：普通 IMG 可对非 JPEG 的解码器敌对格式使用受控预处理；JPEG 不以像素相等或重新编码冒充可逆转码，FastImg JXL 也不使用破坏性兜底。
- **阶段 4 — HDR 增益图处理**：带增益图的 HEIC/HEIF 进入专用 HDR JXL 合成链，并把增益图、深度图等不能内嵌的辅助资产作为逐项核验的 sidecar；AVIF 在解码前由 `avifdec --info` 实时核查增益图，当前无法无损表达时保留原件而不扁平化。UltraHDR JPEG 通过 JBRD 归档，包含 MPF 增益图与私有元数据的原 JPEG 必须逐字节重建。
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

| 二进制    | 输入        | 主要输出                      | `--codec hevc\|av1`                      |
| --------- | ----------- | ----------------------------- | ---------------------------------------- |
| **`img`** | 仅静图      | JXL / 跳过；FastImg Meme→AVIF | `img run` 仅 `hevc`→JXL；动图 **ignore** |
| **`vid`** | 视频 + 动图 | MP4/MOV/GIF / 跳过            | **有**（默认 `hevc`）                    |

此外还有一个 **macOS 双击应用** (`Modern Format Boost.app`)，用于拖放式批量处理。

## 交付策略（HEVC / AV1）

Rust SSOT：[`delivery_codec_strategy.rs`](../crates/foundation/src/convert/delivery_codec_strategy.rs)。

普通 `img run` 与 `vid run` 的 `--codec` **不是同一套语义**：

| 二进制    | `hevc`           | `av1`                              |
| --------- | ---------------- | ---------------------------------- |
| **`img`** | 静图批量 **JXL** | **拒绝**；请使用 FastImg Meme Mode |
| **`vid`** | 视频 **HEVC**    | 视频 **AV1**                       |

**`img` 只编码静图**，**不会**把文件交给 `vid` 处理。动图/无法确认仅静图 → **ignore**（审计类 `img_animated_handoff` 仅表示跳过，**不是转发**）。普通 IMG 中经多方验证的真单帧 GIF/WebP/APNG 只走 JXL；AVIF 输出仅属于 FastImg Meme Mode。**`vid`** 单独处理视频与动图（含 GIF、动图 WebP、**APNG**、动图 AVIF/HEIC 等）。

### 两层策略

| 层                                                 | `img` | `vid` |
| -------------------------------------------------- | ----- | ----- |
| **静图格式**（普通 IMG 为 JXL；Meme Mode 为 AVIF） | ✅    | —     |
| **视频交付编码**（HEVC/AV1）                       | ❌    | ✅    |

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

完整逐步表见本节及下方处理矩阵；运行时路由以 Rust SSOT 为准。

## 📉 真实世界压缩示例

| 输入格式       | 原始大小 | 输出格式       | 输出大小 | 节省     | 方法             |
| :------------- | :------- | :------------- | :------- | :------- | :--------------- |
| 风景 JPEG      | 4.2 MB   | **JXL**        | 3.3 MB   | **~21%** | 无损组件重建     |
| 截图 PNG       | 2.5 MB   | **JXL**        | 1.1 MB   | **~56%** | Modular d=0.0    |
| 运动相机 H.264 | 1.2 GB   | **HEVC**       | 480 MB   | **~60%** | GPU/CPU CRF 搜索 |
| 动画 WebP      | 15 MB    | **AV1 / HEVC** | 1.8 MB   | **~88%** | 转码为视频格式   |

## 📊 处理矩阵

### 图像格式决策矩阵

| 输入格式                                    | 静态？ | `img run` 中的动作 | 输出       | 备注                              |
| :------------------------------------------ | :----: | :----------------- | :--------- | :-------------------------------- |
| JPEG                                        |   ✅   | **无损重建**       | `.jxl`     | 位精确 `cjxl --lossless_jpeg=1`   |
| UltraHDR JPEG                               |   ✅   | **精确归档**       | `.jxl`     | 完整 MPF/增益图 JPEG 可逐字节重建 |
| PNG / TIFF / BMP / 其他无损静态图           |   ✅   | **无损转换**       | `.jxl`     | 可能先走迂回路径                  |
| 已证明无损的 WebP / AVIF / HEIC / HEIF / JP2 |   ✅   | **无损转换**       | `.jxl`     | 像素、元数据与已知功能资产逐项核验 |
| 有损/语义不明的现代容器或已有 JXL             |   ✅   | **逐字节保留**     | 保留原文件 | 避免代际损失或破坏未知归档结构      |
| 带有增益图的 HEIC / HEIF                      |   ✅   | **专用 HDR 路径**  | `.jxl` + sidecar | 合成 HDR 并核验辅助资产          |
| 静态验证后的遗留有损静态图                  |   ✅   | **近无损转换**     | `.jxl`     | 当前 `img run` 批量路径专注于 JXL |
| 动画 GIF / WebP / APNG / HEIC / HEIF / JXL  |   ❌   | **img 忽略**       | —          | 请用 **`vid run`**                |

### `img` 入口

| 入口                               | 静图输出                                           | 动图        | AVIF                             |
| ---------------------------------- | -------------------------------------------------- | ----------- | -------------------------------- |
| **`img run`**                      | JPEG 与已证明无损静图进入 JXL；现代有损/未知源原样保留 | **忽略**    | 不作为输出 codec；无损 AVIF 可进入 JXL |
| **`img fast-img --strategy jxl`**  | 真实 JPEG 可逆 JXL；已确认现代有损源进入验证交付层 | 保留/忽略   | 可作为原样交付的现代有损源       |
| **`img fast-img --strategy avif`** | 非 AVIF 静图走有界搜索；已有 AVIF 只原样托管或做容器元数据清理，绝不重复编码 | 拒绝/保留   | 主图像/HDR/增益图特征必须保持一致；有效 XMP 侧车仅在最终证明后删除 |
| **`smart_convert()`**              | JXL 或按 `determine_strategy` 原样保留             | 域外 ignore | 不可用，请使用 FastImg Meme Mode |

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

| HDR 类型          | 检测                                       | 保留策略                                                                              |
| :---------------- | :----------------------------------------- | :------------------------------------------------------------------------------------ |
| **HDR10**         | side_data 中的 mastering_display + max_cll | 通过 FFmpeg 参数完整保留静态元数据                                                    |
| **HEIC 增益图**   | HEIC 辅助图像 (苹果/三星/ISO)              | 合成为 32 位线性 HDR -> JXL (真实 HDR)                                                |
| **UltraHDR JPEG** | JPEG APP1/APP2 + XMP (hdrgm:)              | 默认精确归档为 JXL，完整 MPF/增益图 JPEG 可逐字节重建；显式像素合成仅允许非破坏性使用 |
| **HLG**           | color_trc = arib-std-b67                   | 保留原色 + TRC                                                                        |
| **杜比视界**      | 流/帧中的 DOVI side_data                   | 通过 `dovi_tool` 提取 RPU → x265 注入；Profile 7 → 8.1 转换                           |
| **HDR10+**        | ST2094-40 动态元数据                       | 通过 `hdr10plus_tool` 挂载提取和 x265 注入支持（保留 Profile A/B 元数据）             |
| **SDR**           | 无 HDR 标记                                | 标准处理 (yuv420p)                                                                    |

## ⬇️ 安装

### 预编译二进制文件

对于不想安装 Rust 工具链的用户，可以从 **[Releases](https://github.com/nowaytouse/modern-format-boost/releases)** 页面下载预编译的二进制文件。

```bash
# macOS/Linux 一键安装（以 macOS ARM64 为例）
curl -LO https://github.com/nowaytouse/modern-format-boost/releases/latest/download/modern-format-boost-aarch64-apple-darwin.tar.gz
tar -xzf modern-format-boost-aarch64-apple-darwin.tar.gz

```

### 前提条件

| 工具                 | 必需？ | 用途                            | 安装命令                                                                                    |
| :------------------- | :----: | :------------------------------ | :------------------------------------------------------------------------------------------ |
| **Rust** (nightly)   |   ✅   | 构建与安装                      | `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \| sh && rustup default nightly` |
| **FFmpeg** (5.0+)    |   ✅   | 视频处理与指标计算              | `brew install ffmpeg` / `apt install ffmpeg`                                                |
| **libjxl**           |   ✅   | JXL 编码核心                    | `brew install jpeg-xl`                                                                      |
| **ExifTool**         |   ✅   | 元数据保留                      | `brew install exiftool`                                                                     |
| **ImageMagick**      |   ✅   | 图像迂回路径                    | `brew install imagemagick`                                                                  |
| **libwebp**          |   ✅   | WebP 原生解码                   | `brew install webp`                                                                         |
| **libheif**          |   ✅   | HEIC/HEIF 解码                  | `brew install libheif`                                                                      |
| **PostgreSQL** (12+) |  按需  | Vid、训练、缓存及可选质量启发式 | `brew install postgresql pgvector` / `apt install postgresql`                               |
| **dovi_tool**        |  可选  | 杜比视界 RPU 提取               | `cargo install dovi_tool`                                                                   |
| **hdr10plus_tool**   |  可选  | HDR10+ 元数据提取               | `cargo install hdr10plus_tool`                                                              |

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

Modern Format Boost 使用 PostgreSQL（并启用 `pgvector` 扩展）支撑 Vid、训练、缓存管理命令及用户显式开启的质量启发式。普通 `img run`、FastImg 和 JPEG 恢复默认采用精确本地检测，不连接 PostgreSQL；只有选择数据库功能时，连接失败才会保持 fail-closed 并阻止启动。

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

# FastImg JXL 与 AVIF Meme Mode
img fast-img --strategy jxl /path/to/media
img fast-img --strategy avif /path/to/media

# 将 AVIF 或 JXL 结果交付到 Apple Photos（macOS，包含实时 UUID/哈希核验）
img fast-img --strategy avif --shortest-path /path/to/media
img fast-img --strategy jxl --shortest-path /path/to/media

# 自动精确恢复 JPEG，并隔离历史版本产生的不可逆 JXL
img restore-jpeg /path/to/archive

# macOS 下选择照片图库会自动核验实时资产
img restore-jpeg /path/to/Library.photoslibrary

# 列出实时用户文件夹/相册，并使用原生 UUID 精确限定同一审计流程
img photos-albums /path/to/Library.photoslibrary
img restore-jpeg /path/to/Library.photoslibrary --photos-album-id ALBUM_UUID
img restore-jpeg /path/to/Library.photoslibrary --photos-folder-id FOLDER_UUID

# 实时复核受影响 JXL，仅从备份收集对应原件与 XMP
collect_optimized /path/to/audited /path/to/recovered --backup /path/to/backup --yes

# 只读比较两个 Photos 图库；普通文件查重请使用专用外部工具
collect_optimized /path/to/current /path/to/report --backup /path/to/backup --compare

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
- **显式恢复**：匹配的中断任务需要用户选择 `--retry`（或交互确认）；需要全新任务时使用 `--no-resume` 创建隔离工作副本。
- **实时交付核对**：Photos 导入中断后，先与图库中的实时 UUID/内容哈希核对，再恢复原有进度；这与单次编码/导入失败后的有界重试是两种不同语义。

### 详细选项

- `--ultimate`：存档级 **0.01 精度** 搜索，并使用生产 JXL effort 10（高质量，高时间成本）。
- `--archive`：直接像素编码仍以 effort 10 为生产上限；JPEG 比特流转码属于不同负载，默认使用专用 effort 11，若编码器不支持或该次尝试失败则回退 effort 10，最终仍必须通过逐字节重建证明。
- `--apple-compat`：启用苹果生态系统编码兼容性，CLI 默认为开启；`--no-apple-compat` 可关闭这些编码选择，但 AAE 编辑侧车仍作为归档数据保留。
- `--in-place`：替换原始文件。**警告：不可逆。**
- `-o /dir`：安全输出目录。（推荐）
- `--verbose`：显示详细的处理日志。
- `--no-recursive`：不进入子目录。
- `--force-video`：强制将动画图像视为视频，不考虑循环意图 (Loop Intent)。
- `img restore-jpeg INPUT` 不再要求用户选择行为模式。普通文件或文件夹会逐字节恢复所有精确可逆 JPEG 与已核验 XMP；需要从备份恢复或无法判定的 JXL 保持原样，并按原目录生成 `Reconstruction Blocked` / `Needs Review` 标记。选择照片图库（或图库包内的一个具体资产路径）时会自动实时核验 UUID：精确可逆资产不作标记，受影响的现有资产以引用方式加入保留原文件夹/相册层级的 `MFB JXL Audit` 相册。MFB 不改写媒体字节，也不直接编辑 Photos 数据库文件；仅由 Photos 记录相册成员关系。外部 BLAKE3/UUID 检查点用于可恢复、幂等重跑。原生 AppKit GUI 会在选中图库后显示实时文件夹/相册选择器；CLI 通过 `img photos-albums` 提供相同 UUID。相册范围精确匹配，文件夹范围按 Photos 原生层级展开其全部后代相册，不按显示名称猜测，也不混用不兼容的数据库文件夹标识。
- `collect_optimized AUDITED DEST --backup BACKUP` 负责后续原件收集，并会再次读取当前 JXL 内容，不盲信旧标记。选择单个 JXL 时可指定同名备份原件或备份文件夹；普通备份文件夹只接受相同相对目录、相同文件名主干且内容识别为静态图片的唯一原件；备份图库只接受相同 Photos UUID。流程仅复制/导出真正受影响的原件与 XMP，不修改备份或 Photos 数据库；缺失、歧义、格式不符都会显式失败。成功或部分成功状态写入带逐文件 BLAKE3 的 `.mfb_recovery_collection.json`，可安全重跑。原生 GUI 中对应“收集恢复原件”。
- `collect_optimized CURRENT REPORT --backup BACKUP --compare` 仅支持两个照片图库包的只读比对。它委托已安装的 `osxphotos` 比对原生资产，只生成原子写入且不含绝对路径的 `mfb_backup_comparison.json`，不会修改图库或媒体。普通文件/文件夹的查重不属于 MFB，请使用专用外部查重工具；原生 GUI 对这类输入会明确拒绝。

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

Modern Format Boost 使用分层质量门控降低回归与静默损坏风险；这些门控不等于“零技术债”或绝对无缺陷：

- **Rust 优先开发工具链**：工程入口是 `crates/dev/src/bin` 下的 Rust 二进制；Python 原件仅作为兼容参考保留，直到确认可安全删除。
- **CI 校验**：GitHub CI 运行仓库级检查；本地开发应优先执行与改动范围相符的构建和运行回归，再按发布流程运行完整门禁。
- **失败语义**：解码、重建、元数据或交付证明失败时显式报告；不会用空成功或默认值伪装完成。

### 核心架构

- `crates/img/`：静态图像优化器（当前 CLI 路径中为 `JXL` / 跳过 / 忽略）
- `crates/vid/`：视频和动画媒体优化器（`HEVC` / `AV1` / `GIF`）
- `crates/foundation/`：核心大脑（GPU/CPU 混合引擎、HDR 映射、元数据）
- `Modern Format Boost.app/`：macOS 拖放式 UI

## 📐 分层契约与训练

交付、推理、终端 UI 与训练栈的行为以**可审计的契约文档**为准（CI 密封测试约束）。扩展 `img` / `vid`、探索管线或 PostgreSQL 训练时请优先查阅：

| 层                                               | 文档                                                                                                                                                                      |
| ------------------------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| 媒体转换契约（M1–M251 注册表；M1–M206 交付密封） | [`MEDIA_CONVERSION_LAYER_CONTRACT.md`](hardening/MEDIA_CONVERSION_LAYER_CONTRACT.md) · [`MEDIA_CONVERSION_DELIVERY_SEAL.md`](hardening/MEDIA_CONVERSION_DELIVERY_SEAL.md) |
| 算法 / 推理门控                                  | [`ALGORITHM_LAYER_CONTRACT.md`](hardening/ALGORITHM_LAYER_CONTRACT.md)                                                                                                    |
| 原生 macOS UI                                    | [`UI_LAYER_CONTRACT.md`](hardening/UI_LAYER_CONTRACT.md)                                                                                                                  |
| JXL/XMP 归档与 JPEG 恢复                         | [`JXL_XMP_ARCHIVE_CONTRACT.md`](hardening/JXL_XMP_ARCHIVE_CONTRACT.md)                                                                                                    |
| 日志 / 会话                                      | [`LOGGING_LAYER_CONTRACT.md`](hardening/LOGGING_LAYER_CONTRACT.md) · [`LOGGING_LAYOUT.md`](hardening/LOGGING_LAYOUT.md)                                                   |
| 数据库                                           | [`DATABASE_LAYER_CONTRACT.md`](hardening/DATABASE_LAYER_CONTRACT.md)                                                                                                      |

**静图质量训练**（high/low 分层、入库审计）：

- **唯一推荐入口**：源码树使用 `cargo run --locked -p dev --bin run_training --`，已编译/打包环境使用 `target/release/run_training`。
- **规则**：提交版 [`training_rules.json`](../crates/dev/src/config/training_rules.json)；本机目录与 ingest 上限写在 gitignore 的 `training_rules.local.json`。
- **Tier 引擎**：Rust [`training_tier_audit.rs`](../crates/foundation/src/train/training_tier_audit.rs) 与 JSON 阈值同步（熵死区、几何护栏）。
- **后台**：`cargo run --locked -p dev --bin run_training -- --background` → 日志在统一日志目录（见 [`LOGGING_LAYOUT.md`](hardening/LOGGING_LAYOUT.md)）。
- **迁移策略**：仅保留 ML 生态、测试/夹具、模糊测试和兼容桥所需的 Python；详见 [`PYTHON_RUST_MIGRATION.md`](PYTHON_RUST_MIGRATION.md)。

完整硬化说明见 [`CHANGELOG.md`](CHANGELOG.md) **0.11.3**。

## ❓ 常见问题

**1. JXL 是否得到广泛支持？**
macOS 14+ / iOS 17+、Chrome 91+ 和 Firefox 128+ 已提供原生支持。然而，目前仍存在一些已知的生态系统问题：

- **动画**：现代动画格式 (JXL/AV1/HEIF) 在原生 macOS/iOS 照片应用或 Finder 中经常无法作为动画预览（仅显示静态），尤其是在通过 iCloud 同步时。它们在现代浏览器或专用工具中可以正常播放。
- **缩略图**：使用**灰度 ICC 配置文件**的 JXL 文件在 Finder/iCloud 中可能显示为**黑色缩略图**，尽管打开时渲染完美。
  对于位精确存档和高保真 HDR 存储，JXL 仍然是卓越的格式。

**2. HDR10+ 如何处理？**
完全支持。我们使用 `hdr10plus_tool` 提取 SMPTE 2094-40 动态元数据，并通过 `libx265` 的 `--dhdr10-info` 参数将其注回 HEVC 流。请确保已安装该工具以启用此功能。

**3. 为什么跳过 WebP/AVIF/HEIC？**
现代有损源会保留原编码以避免代际损失；被正面证明为无损的 WebP/AVIF/HEIC/HEIF/JP2 则进入 JXL 无损链。AVIF 使用权威 `avifdec` 解码并在解码前核查增益图，输出必须通过 RGBA16 像素、方向、颜色和源元数据证明；已有 JXL 仍是归档目标。其他边界包括：

- 带增益图的 HEIC/HEIF 使用专用 HDR JXL 与已核验辅助 sidecar；AVIF 增益图当前不能安全映射时明确保留原件
- UltraHDR JPEG 默认通过 JBRD 精确归档，内嵌增益图与私有元数据仍属于原始 JPEG 字节流，可由 `restore-jpeg` 逐字节恢复；不可逆的像素级 HDR 合成不会被默认或删除源文件路径采用
- FastImg JXL Tier 2 可原样托管已证实有损的现代静图；AVIF Meme Mode 只重编码非 AVIF 输入，已有 AVIF 会原样托管或仅清理容器元数据
- 动画现代格式不由 `img` 处理；它们通过 `vid` 和 `loop_intent` 进行路由

**4. `img run` 与 FastImg 有什么不同？**
`img run` 是覆盖面更广的静图优化器：默认使用本地精确检测，也可显式启用数据库质量启发式；JPEG 与已证明无损的普通/现代静图进入 JXL，有损或语义不明的现代源保持原编码。FastImg 是边界更窄的持久交付工作流：JXL 主层只接受可逐字节重建的真实 JPEG，JXL Tier 2 只托管已被正向证明为有损的现代静图；AVIF 策略对非 AVIF 静图执行有界编码搜索，对已有 AVIF 则只做原样托管或容器元数据清理，并证明主图像与 HDR/增益图特征不变。两者复用底层安全闸门，但候选选择、搜索预算、状态恢复和 Photos 策略并不相同。

**5. FastImg Tier 2 能否精确判断现代静图的有损/无损？**
JXL 策略 Tier 2 只有存在格式级正向证据时才会判为有损并准入。WebP、JP2、AVIF、HEIC/HEIF 与 JXL 各自使用结构化解析和权威工具证据；`Unknown`、无损、动画、损坏容器和带 JPEG 重建数据的 JXL 都会失败关闭并保留原件。AVIF Meme Mode 只对非 AVIF 输入执行有界编码搜索；已有 AVIF 若已清洁则逐字节托管，否则只在暂存副本清理容器元数据，并要求主图像 SHA-256 与 `avifdec` 的编码/HDR/增益图特征完全不变。匹配的有效 XMP 侧车不合并进 Meme Mode 输出，只在最终交付证明通过后精确删除；任何失败都保留源文件和侧车。JXL Tier 2 使用隔离副本的元数据保留路径，并继续核验 Photos UUID/内容哈希。

**6. `restore-jpeg` 会改变原始 JPEG 吗？**
不会。它按 `djxl` 实际公开能力选择显式 `--reconstruct_jpeg` 或官方 `.jpg` 默认重建，并且只接受带正向重建诊断、非空输出、无像素回退且逐字节哈希一致的 JXL；不支持这两种受控接口的 CLI 会在能力预检阶段被拒绝。重建 JPEG 后也不会再用元数据工具改写该文件。JXL 内追加的 XMP overlay 会作为同名 `.xmp` 侧车单独校验、哈希和提交；删除源 JXL 前，JPEG 与 XMP（如有）的最终哈希都会再次核对。若要求单一“已嵌入新 XMP”的 JPEG，它必然不是原 JPEG 的逐字节副本，因此项目把“精确 JPEG + 已核验 XMP”作为一对交付物。

**7. `restore-jpeg` 还需要选择模式吗？**
不需要，输入位置会自动决定安全行为。普通文件或文件夹恢复逐字节一致的 JPEG，且只有 Manifest V3 删除闸门完整通过才可能清理精确可逆源；像素级 JXL 或被官方工具拒绝重建的项会生成备份恢复标记，不可读或身份无法证明的项会生成复核标记，标记树保留相对目录结构。选择照片图库时改为实时 UUID 审计：精确可逆资产不作标记，受影响资产仅以引用方式加入保留原层级的 `MFB JXL Audit` 相册。默认审计整库；GUI 或 `img photos-albums` 可按原生 UUID 选择一个相册或文件夹，文件夹会覆盖其后代相册。

**8. 程序中断或断电后如何继续？**
FastImg 不会把单次编码重试和任务恢复混为一谈。下次启动时由用户明确选择继续（`--retry`）或隔离启动（`--no-resume`）；程序会重新核对已保存的路径与哈希，并在继续 Photos 导入前核查实时状态。

**9. JXL 是否在所有设备上都受支持？**
不是。操作系统、应用、缩略图和动画支持取决于版本；苹果兼容策略只约束本项目的编码/容器选择，并不保证第三方客户端都能解码。替换不可替代原件前应先验证目标设备。

**10. HDR10+ 如何处理？**
视频路径在存在工具和流证据时，使用 `hdr10plus_tool` 提取 SMPTE 2094-40 元数据，再通过 `libx265` 的 `--dhdr10-info` 注入 HEVC；缺少证据时不会静默声称已保留。

**11. 验证清理后源文件夹会怎样？**
只有在受控根目录内、确认目录真正为空时才会删除空目录；单文件输入不会因此删除隐式父目录。Photos Library 包、危险根路径、越界或符号链接候选会被拒绝。剩余媒体、侧车、用户隐藏文件或并发创建的文件都会阻止清理；Finder 生成的 `.DS_Store` 只有在它是唯一剩余项时才会按规则移除。

**12. `restore-jpeg` 会改变原始 JPEG 或图库数据库吗？**
不会。文件/文件夹输入只提交通过严格重建证明的 JPEG，并按相对目录生成恢复/复核标记；Photos 输入使用实时 UUID 审计，以相册引用表达需要关注的资产，不直接改写媒体字节或 SQLite 数据库。GUI 与 `img photos-albums` 使用原生 UUID 选择相册/文件夹，文件夹会包含后代相册并保留层级。

**13. 被标记为不可重建的 JXL 是否一定能在没有备份时修复？**
不一定。JXL 可能保留 JPEG 系数，但逐字节恢复还要求原始 Exif/XMP/JUMBF 容器字节也未被改写。可解码的像素只能生成派生 JPEG，不能证明原始文件可恢复；因此程序保留 JXL 与侧车并要求使用精确原件或元数据备份完成恢复。

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
