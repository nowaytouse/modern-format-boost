# Modern Format Boost

<img src="assets/mfb-logo.png" alt="Modern Format Boost" width="128" height="128">

**以私人归档策略整理媒体：减少占用，保留重要信息，并核验处理结果。**

[English](../README.md) · [简体中文](README_ZH.md) · [更新日志](CHANGELOG.md)

Modern Format Boost（MFB）根据个人长期归档需求，处理图片、视频和动图。
它不是通用格式转换器，也不是媒体管理器。只有质量、元数据和交付检查满足要求时，
缩小文件才有意义；无法证明安全时，保留受影响的原件并报告问题，而不是勉强算作成功。

macOS 界面使用原生 AppKit；Rust 命令行工具也支持 Linux 和 Windows。
Apple 照片图库集成仅限 macOS。当前开发版本为 **0.12.0**，开发分支为 `nightly`。

## 先选合适的处理方式

| 处理方式 | 主要行为 | 适合什么情况 |
| --- | --- | --- |
| **普通图片** / `img run` | 按完整静图策略处理，通常输出 JXL；不适合再次编码的现代有损格式保留原格式。 | 一般图片归档。 |
| **Fast IMG · JXL** / `img fast-img --strategy jxl` | 将真实 JPEG 可逆转码为 JXL；符合条件的现代有损静图另走经过核验的照片图库交付路径。 | 集中归档 JPEG，或交付到 Apple 照片图库。 |
| **Fast IMG · AVIF** / `img fast-img --strategy avif` | 使用独立的 AVIF 体积与质量策略；已有 AVIF 不再重新编码。 | 允许清理元数据的表情包等图片集合。 |
| **视频** / `vid run` | 处理视频和动图，默认 HEVC，也可选择 AV1。 | 有动态语义的媒体，而不是静图转换。 |

MFB 根据实际内容识别格式，不只看扩展名。图片和视频各有职责：`img` 不会把不适合的
文件悄悄交给 `vid`。Fast IMG 表示处理范围更集中，不保证每个文件都比普通模式更快。

### 理解默认归档策略

- JPEG 转 JXL 必须能够**逐字节恢复原始 JPEG**。默认比较 E1 至 E11 全部 11 个
  编码档位，复用完整文件最小、且验证通过的候选；E 越高，不一定越小。
- 真正的无损图片按私人 IMG 策略保持无损。其他符合条件的路径可以搜索同时满足质量和
  体积要求的结果；设置参数不等于获准改变该类媒体的归档规则。
- 普通 JXL 归档保留策略要求的元数据和关联关系。Fast AVIF 使用**另一套元数据清理
  策略**，不能把两者的保留承诺混为一谈。

[私人 IMG 策略](dev/config/IMG_PRIVATE_POLICY.md)解释具体处理路线和搜索算法。
应用内的 **?** 按钮也会打开处理指南，用例子说明这些选择，不必先读源码。

## 安全开始

**保留独立备份，先用少量文件的副本试跑。** 某些流程会在验证通过后清理原件。
尤其是 Fast IMG：验证后清理来源属于流程本身，不需要通过 `--delete-source` 才启用。
仅仅编码完成，不足以允许删除原件。

### macOS 应用

1. 打开 `Modern Format Boost.app`，选择或拖入文件、文件夹。
2. 选择对应的图片或视频处理方式，在**设置**中检查该模式自己的质量、搜索、工具和
   照片图库选项。
3. 第一次运行前先用**预览**确认范围。只有确实要交付到当前选中图库时，才勾选
   **导入到照片**。
4. 完成后先看摘要；通过**处理历史**查看当次的数量、路径、验证、清理和诊断记录，
   无需手动翻找日志文件夹。

暂停和停止会等待后端到达安全边界，不会把正在编码的进程直接冻结。
任务中断也不代表已经完成。

界面支持简体中文、英语和日语；外观可以跟随系统，也可选择浅色或深色。
Linux 和 Windows 当前使用命令行。

### 获取或构建

到 [Releases](https://github.com/nowaytouse/modern-format-boost/releases) 查看实际产物
及对应版本。目前发布流程提供的是 **macOS Apple Silicon 命令行归档包**，不是通用
安装器，也不代表每个平台都有打包好的 GUI。Nightly 是开发快照。

MFB 会调用外部媒体工具。常见依赖包括 FFmpeg/ffprobe、JPEG XL 工具
（`cjxl`、`djxl`、`jxlinfo`）、ExifTool、ImageMagick、WebP 工具和 libheif。
源码构建需要仓库指定的 Rust nightly，以及 **libheif 1.23.5 或更新版本**；特定格式
或功能还可能要求其他工具。

例如，macOS 可用 Homebrew 安装常见依赖：

```bash
brew install ffmpeg jpeg-xl exiftool imagemagick webp libheif pkgconf
```

从源码构建命令行工具：

```bash
git clone https://github.com/nowaytouse/modern-format-boost.git
cd modern-format-boost
cargo build --locked --release -p img -p vid
```

macOS 原生应用的构建入口：

```bash
cargo run --locked -p dev --bin smart_build -- --all
```

它会构建应用所需的工具，在 `target/release/bundle/macos/` 生成应用，
并替换仓库根目录的 `Modern Format Boost.app`。
**该应用正在处理文件时，不要执行此构建命令。** 原生构建还需要 Apple 命令行开发工具。

Linux、Windows 的依赖配置可参考[媒体依赖安装器](../crates/dev/src/bin/install_media_dependencies.rs)
和[构建工作流](../.github/workflows)。设置中的**检查处理工具**可查看实际使用的路径
和版本输出，但版本命令成功不等于编解码能力或交付验证通过。

### 命令行示例

以下假设 `img`、`vid` 已加入 `PATH`；从源码构建后，也可将命令替换为
`target/release/img` 或 `target/release/vid`。

```bash
# 普通静图：指定独立输出，不加删除原件的参数。
img run /path/to/copied-images --output /path/to/image-output

# 只查看 Fast IMG 的处理计划，不转换、不删除媒体。
img fast-img --strategy jxl --dry-run /path/to/copied-images

# 视频和动图；需要 AV1 策略时再加 --codec av1。
vid run /path/to/copied-videos

# 查看实际生效配置、校验配置或检查某个工具。
img config show --effective
img config validate
img config tool cjxl
vid config show --effective
```

通过 `img --help`、`img run --help`、`img fast-img --help` 或 `vid run --help`
查看对应模式的当前参数，不要直接把某种模式的参数套到另一种模式。
普通 `vid run` 要求能连接到 PostgreSQL；数据库分析、训练还使用 pgvector。
`vid config`、`vid fast-gif`、普通 IMG 的本地识别和 Fast IMG **不需要 PostgreSQL**。

## 设置、失败与恢复

GUI 和 CLI 使用同一套配置规则。普通 IMG、Fast IMG 和 VID 分别保留适用于自身的
选项。[运行配置说明](dev/config/RUNTIME_CONFIG.md)列出配置位置、覆盖顺序、实际生效值
和完整参数，README 不重复维护另一份参数表。

默认**记录并继续**：记录可恢复的单文件失败，保留失败原件，让互不影响的文件继续。
**遇错即停**会更早停止后续工作，不会把已验证成果作废；共享安全前提失效时，仍会阻止
受影响的交付。失败不会为了让结果好看而被改成“跳过”。

**部分完成**表示一部分文件已独立验证成功，另一些失败，不是全成功。
缺少或相互矛盾的证据仍然标为无法确认。编码数量、照片图库验证数量和清理数量代表
不同事实；总数相等不等于文件内容已经对应。

Fast IMG 会先查找匹配的保存状态，再决定是否需要建立新输出目录；复用要求身份和
检查点有效，重新开始是另一种选择。不要仅因编码结束，就手动删除 `_optimized`
目录、检查点或残留文件，应先核对交付和清理记录。

**照片图库验证证明的是本地交付，不代表 iCloud 已同步。** 未知或发生变化的输出会被
保留，不会直接清理。运行状态和暂存文件放在来源目录外，但不能保证系统 ctime 或被
外部程序改动过的目录日期完全不变。

## 文档导航

| 想了解什么 | 对应文档 |
| --- | --- |
| 版本改动和开发记录 | [更新日志](CHANGELOG.md) |
| 版本、阶段历史与完整原始提交归档 | [Git 历史说明](dev/HISTORY.md) |
| 设置、配置文件、工具与失败处理 | [运行配置](dev/config/RUNTIME_CONFIG.md) |
| 图片处理路线、JPEG 档位搜索和质量搜索 | [私人 IMG 策略](dev/config/IMG_PRIVATE_POLICY.md) |
| 计数、部分完成和验证范围 | [计数约定](hardening/COUNTING_CONTRACT.md) |
| 持久日志、历史和诊断文件位置 | [日志布局](hardening/LOGGING_LAYOUT.md) |
| 精确恢复 JPEG 与 XMP 关联 | [JXL/XMP 归档约定](hardening/JXL_XMP_ARCHIVE_CONTRACT.md) |
| 高级 FFmpeg 配置 | [FFmpeg 指南](FFMPEG_SETUP.md) |
| 架构与扩展边界 | [策略准备](hardening/POLICY_READINESS.md) · [媒体转换约定](hardening/MEDIA_CONVERSION_LAYER_CONTRACT.md) |

## 开发与许可

主要模块是 `crates/img`、`crates/vid`、共享的 `crates/foundation`、Rust 开发工具
`crates/dev`，以及原生 GUI `crates/gui/src-macos`。改动应保留相关行为约定，并补充
有针对性的回归检查。[CI](../.github/workflows/ci-quality.yml)覆盖的是某个代码版本，
不是任意真实媒体批次或实时照片图库行为的完成证明。

训练、分析开发使用 Rust 入口 `cargo run --locked -p dev --bin run_training --`；
日常图片归档不需要运行训练。共享规则位于
[`training_rules.json`](../crates/dev/src/config/training_rules.json)；本机路径和导入上限放在
不提交到 Git 的同目录 `training_rules.local.json`。在上述命令后加 `--help` 查看当前选项，
加 `--background` 可转入后台；日志遵循[统一布局](hardening/LOGGING_LAYOUT.md)。
正式命令和保留的 Python 用途见 [Python/Rust 迁移说明](PYTHON_RUST_MIGRATION.md#canonical-commands)。
贡献时不要提交私人媒体、图库、本机路径、数据库或凭据。

MFB 采用 [MIT 许可证](../LICENSE)。外部编码器和依赖仍遵守各自的许可证，
详见[依赖声明](LICENSES.json)。本项目仍是持续开发中的私人工具，不能替代备份，
也不承诺所有格式和环境都能处理。
