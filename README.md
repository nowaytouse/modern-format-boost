# 🚀 Modern Format Boost

[![Version](https://img.shields.io/badge/version-0.10.50-blue.svg)](https://github.com/nowaytouse/modern-format-boost/releases)
[![Platform](https://img.shields.io/badge/platform-macOS%20%7C%20Linux-lightgrey.svg)](https://github.com/nowaytouse/modern-format-boost)
[![License](https://img.shields.io/badge/license-MIT-green.svg)](LICENSE)

**The Ultimate Media Modernization Engine for the Apple Ecosystem.**
**专为极致画质与苹果生态打造的下一代媒体优化引擎。**

---

## 📖 Introduction / 简介

**Modern Format Boost** 是一款基于 **Rust** 开发的高性能并发媒体优化工具。它不仅是简单的格式转换器，更是一个集成 **网络反馈回路 (Cybernetic Feedback Loop)** 的转码决策系统。其核心使命是将陈旧的 JPEG/H.264 档案转化为极致精简且高度兼容的 **JXL/HEVC** 格式，同时在图像上实现**数学无损**，在视频上实现**视觉巅峰**。

---

## 🛠️ Core Technologies / 核心技术架构

### 1. 图像现代化引擎 (`img-hevc` / `img-av1`)
采用 **比特流深度解析 (Bitstream Parsing)** 策略，拒绝任何形式的“启发式盲猜”。

*   **JPEG 无损重建**：解析原始 DCT 系数，直接映射至 JXL 的 `varDCT` 模式。实现**位一致 (Bit-exact)** 的无损转码，体积减少约 20%。
*   **高动态/高位深支持**：自动识别 10-bit/12-bit 源文件及 HDR 元数据，强制开启高精度编码路径。
*   **动画压缩革命**：将 WebP/GIF 动图序列拆解为视频帧，利用运动矢量（Motion Vectors）实现 90% 以上的体积削减。
*   **精准质量感知 (v2)**：
    *   **精确探测**：读取 JPEG DQT 表和 WebP VP8 分区索引，获取真实量化分值。
    *   **响亮降级**：若元数据丢失，触发 **Heuristic v2**。结合 **BPP (Bits-per-pixel)**、**格式效率因子 (Efficiency Factor)** 及 **图像熵 (Entropy)** 进行多维度估算，并伴有带文件名的 ANSI 颜色警告。

### 2. 视频搜索逻辑 (`vid-hevc` / `vid-av1`)
采用 **三阶段饱和搜索算法 (Three-Phase Saturation Search)** 寻找物理画质墙。

*   **Phase I: 硬件频谱扫描**
    *   利用 Apple VideoToolbox 或 NVENC 进行全频谱二分查找，极速锁定“画质拐点”（Quality Knee）。
*   **Phase II: 心理视觉精调 (Psychovisual Fine-Tuning)**
    *   在拐点附近使用软件编码器（x265 Slower / SVT-AV1）进行高精度步进搜索（0.1 step）。
*   **Phase III: 3D 质量门控 (Ultimate Quality Gate)**
    *   **VMAF-Y** (亮度感知): 确保 Netflix 级别的感知体验。
    *   **PSNR-UV** (色度保真): 杜绝高对比度素材常见的色度塌陷。
    *   **CAMBI** (色彩断层检测): 专门针对平滑渐变区域，防止产生色带 (Banding)。

---

## 💎 Ultimate Mode / 极致模式

针对高价值收藏级素材，极致模式开启了严苛的审核标准：

*   **30次饱和深度检测**：要求连续 30 次 CRF 步进（0.1 step）均无画质增益（Δ < 0.00005），才会判定为“触碰物理极限”。
*   **双向质量意识 (Quality Awareness)**：
    *   **向上止损 (Fast-Fail)**：若向上搜索时 VMAF 连续 3 次跌破及格线，立即中止任务。
    *   **向下上限拦截 (Ceiling Check)**：若达到物理饱和时 VMAF 仍未达标，直接判定为不可转换。
*   **全量缓存复用**：搜索阶段产生的每一帧 VMAF 数据均被缓存， Phase III 验证秒级完成，杜绝重复计算。

---

## 📊 Heuristic & Meme Scoring / 启发式判定

*   **Meme-Score v3**：智能判定 GIF 是否应保留原格式。
    *   **FPS 去权重化**：完美支持 Live2D 等高帧率表情包。
    *   **维度权重**：`清晰度 (40%)` > `分辨率 (18%)` > `时长 (20%)` > `长宽比 (10%)` > `文件名 (8%)` > `循环频率 (4%)`。
*   **1MB 容差机制**：在 `--allow-size-tolerance` 开启时，允许文件体积有微小（<1MB）的增长以换取突破性的画质提升。

---

## 🚀 Usage / 使用指南

### 快捷处理 (macOS)
双击 `Modern Format Boost.app` 或将文件夹拖入图标即可。默认已开启 `1MB 容差` 以平衡画质与体积。

### 命令行模式
```bash
# 智能视频探索
./vid-hevc run --recursive --ultimate /path/to/media

# 图像无损现代化
./img-hevc run --in-place /path/to/images
```

---

## 📜 Changelog / 最近更新

### v0.10.46
*   **Mega-Release Cumulative Update**: Comprehensive consolidation of all project leaps since v0.10.9.
*   **High-Fidelity Algorithm Leap**: Extreme Mode with 0.01-granularity fine-tuning, Sprint & Backtrack search, and 3D Quality Gate (VMAF/PSNR/CAMBI).
*   **Image Intelligence v2**: JPEG lossless reconstruction, Heuristic v2 estimation, and precision-first routing for palette formats.
*   **Modern UI & 24-bit Colors**: Full TrueColor terminal support with minimized video milestone tracking and title-bar progress.
*   **System Resiliency**: Unique temp files, Ctrl+C job protection, and 1MB unified size tolerance.

### v0.10.43
*   **Minimalist Video Logs**: Implementation of minimalist, abbreviated milestones (`V:`, `P:`, `X:`) tailored for video processing to reduce terminal noise.
*   **Intelligent Mode Detection**: Automated switching between Image and Video milestone displays based on the active tool.
*   **Refined UI Aesthetics**: Condensed log format with improved visual separators and removed redundant icons for a cleaner stage-focused experience.
*   **Critical Fixes**: Resolved format string issues and logic redundancies in the core logging system.

---

## 🤝 Acknowledgments
*   [FFmpeg](https://ffmpeg.org/) - The engine of media processing.
*   [VMAF](https://github.com/Netflix/vmaf) - Perceptual quality metrics by Netflix.
*   [Rust](https://www.rust-lang.org/) - For unparalleled performance and safety.
