# FFmpeg Advanced Setup Guide / FFmpeg 进阶安装指南

This guide explains how to install a full-featured FFmpeg (with plugins like
FDK-AAC, Chromaprint, and AI filters) on macOS using the
[homebrew-ffmpeg](https://github.com/homebrew-ffmpeg/homebrew-ffmpeg) tap, while
maintaining compatibility with system dependencies.

本指南介绍如何在 macOS 上通过 `homebrew-ffmpeg` tap 安装全功能版 FFmpeg（包含 FDK-AAC、Chromaprint 及
AI 滤镜），并确保系统依赖兼容。

---

## 📖 English

### The Tap-Owned FFmpeg Strategy

Use the enhanced `homebrew-ffmpeg` formula as the one Homebrew-managed
`ffmpeg` installation. Current tap guidance does not support keeping the core
formula linked alongside it, so do not use `brew unlink`/`brew link --overwrite`
as a switching mechanism. Resolve any pre-existing core installation before
installing the tap formula.

#### 1. Install the Full-Featured Tap Version

Install the enhanced version directly from the tap:

```bash
brew tap homebrew-ffmpeg/ffmpeg
brew install flite
brew install homebrew-ffmpeg/ffmpeg/ffmpeg \
    --with-chromaprint \
    --with-dvd \
    --with-fdk-aac \
    --with-game-music-emu \
    --with-ggml \
    --with-jack \
    --with-jpeg-xl \
    --with-libaribcaption \
    --with-libmodplug \
    --with-libopenmpt \
    --with-libplacebo \
    --with-librist \
    --with-librsvg \
    --with-libsoxr \
    --with-libssh \
    --with-tensorflow \
    --with-tesseract \
    --with-libvidstab \
    --with-openal-soft \
    --with-openapv \
    --with-opencore-amr \
    --with-openh264 \
    --with-openjpeg \
    --with-openvino \
    --with-rav1e \
    --with-rtmpdump \
    --with-rubberband \
    --with-two-lame \
    --with-webp \
    --with-whisper-cpp \
    --with-xvid \
    --with-zeromq \
    --with-zimg \
    --with-srt \
    --with-libvmaf \
    --with-libxml2 \
    --with-libzvbi \
    --with-aribb24 \
    --with-libbluray \
    --with-libbs2b \
    --with-libcaca \
    --with-libflite \
    --with-libgsm \
    --with-openssl@3 \
    --with-speex

```

_Note: `--with-dvd` is the tap's single DVD switch and enables both
`libdvdnav` and `libdvdread`. `--with-decklink` is intentionally excluded
because it needs the Blackmagic DeckLink SDK. The tap does not declare Flite
as a dependency, so the command installs `flite` first before enabling
`--with-libflite`. This adds text-to-speech support, not a media encoder used by
MFB. `--with-alt-name` only changes command names
and does not add codec capability. The current formula exposes `ggml` as an
optional dependency, while `--with-whisper-cpp` is the switch that adds
FFmpeg's `--enable-whisper`._

This is the widest Homebrew-only feature set. DeckLink remains the only
capability that requires a separately supplied SDK.

#### 2. Verify the Installed Capability Set

`ffprobe` inspects media; encoder and filter availability belongs to `ffmpeg`.
Verify the active binary rather than inferring capability from its package name:

```bash
ffmpeg -hide_banner -buildconf
ffmpeg -hide_banner -encoders
ffmpeg -hide_banner -decoders
ffmpeg -hide_banner -filters
ffprobe -version
brew linkage --test ffmpeg
```

If `brew linkage --test ffmpeg` reports missing versioned libraries after an
optional dependency upgrade (for example OpenVINO), rerun the same command from
step 1 with `brew reinstall` in place of `brew install`, then repeat every
verification command. Do not fabricate compatibility symlinks, downgrade the
dependency, or link the core formula over this tap-owned installation.

---

## 🇨🇳 简体中文

### Tap 独占 FFmpeg 策略

将增强版 `homebrew-ffmpeg` 作为 Homebrew 管理的唯一 `ffmpeg` 安装。
当前 tap 的说明不支持与 core 版同时保持链接，因此不要把
`brew unlink` / `brew link --overwrite` 当作切换机制；若系统已有 core
版，应先单独处理该冲突，再安装 tap 版。

#### 1. 安装“终极全功能版”

直接从 `homebrew-ffmpeg` 安装增强版：

```bash
brew tap homebrew-ffmpeg/ffmpeg
brew install flite
brew install homebrew-ffmpeg/ffmpeg/ffmpeg \
    --with-chromaprint \
    --with-dvd \
    --with-fdk-aac \
    --with-game-music-emu \
    --with-ggml \
    --with-jack \
    --with-jpeg-xl \
    --with-libaribcaption \
    --with-libmodplug \
    --with-libopenmpt \
    --with-libplacebo \
    --with-librist \
    --with-librsvg \
    --with-libsoxr \
    --with-libssh \
    --with-tensorflow \
    --with-tesseract \
    --with-libvidstab \
    --with-openal-soft \
    --with-openapv \
    --with-opencore-amr \
    --with-openh264 \
    --with-openjpeg \
    --with-openvino \
    --with-rav1e \
    --with-rtmpdump \
    --with-rubberband \
    --with-two-lame \
    --with-webp \
    --with-whisper-cpp \
    --with-xvid \
    --with-zeromq \
    --with-zimg \
    --with-srt \
    --with-libvmaf \
    --with-libxml2 \
    --with-libzvbi \
    --with-aribb24 \
    --with-libbluray \
    --with-libbs2b \
    --with-libcaca \
    --with-libflite \
    --with-libgsm \
    --with-openssl@3 \
    --with-speex

```

_注意：`--with-dvd` 是该 tap 唯一的 DVD 开关，会同时启用 `libdvdnav` 与
`libdvdread`。`--with-decklink` 被有意排除，因为它需要 Blackmagic DeckLink
SDK。该 tap 没有为 Flite 声明依赖，因此命令会先安装 `flite`，再启用
`--with-libflite`。它增加文本转语音支持，不增加 MFB 使用的媒体编码器。
`--with-alt-name` 只改变命令名，不增加编解码能力。当前 formula 将
`ggml` 暴露为可选依赖；真正为 FFmpeg 添加 `--enable-whisper` 的开关是
`--with-whisper-cpp`。_

这就是 Homebrew 可提供的最宽功能集。DeckLink 仍是唯一必须另外提供 SDK 的能力。

#### 2. 验证实际能力集

`ffprobe` 负责探测媒体；编解码器和滤镜能力属于 `ffmpeg`。不要只根据包名推断，直接检查当前实际运行的二进制：

```bash
ffmpeg -hide_banner -buildconf
ffmpeg -hide_banner -encoders
ffmpeg -hide_banner -decoders
ffmpeg -hide_banner -filters
ffprobe -version
brew linkage --test ffmpeg
```

如果升级可选依赖后（例如 OpenVINO）`brew linkage --test ffmpeg` 报告带版本号的动态库
缺失，应把步骤 1 中同一条命令的 `brew install` 改为 `brew reinstall` 后完整重建，再
重新执行全部验证命令。不要伪造兼容软链接、降级依赖，也不要把 core formula 强制
链接到这套由 tap 管理的安装之上。
