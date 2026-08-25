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
    --with-libgsm \
    --with-openssl@3 \
    --with-speex

```

_Note: `--with-dvd` is the tap's single DVD switch and enables both
`libdvdnav` and `libdvdread`. `--with-decklink` is intentionally excluded
because it needs the Blackmagic DeckLink SDK. The tap does not declare Flite
as a dependency; Homebrew now provides it, so install `flite` first and append
`--with-libflite` only when text-to-speech support is required. This does not
add a media encoder used by MFB. `--with-alt-name` only changes command names
and does not add codec capability. The current formula exposes `ggml` as an
optional dependency, while `--with-whisper-cpp` is the switch that adds
FFmpeg's `--enable-whisper`._

For the widest Homebrew-only feature set, run `brew install flite` before the
command above and append `--with-libflite`. DeckLink remains the only
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
```

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
    --with-libgsm \
    --with-openssl@3 \
    --with-speex

```

_注意：`--with-dvd` 是该 tap 唯一的 DVD 开关，会同时启用 `libdvdnav` 与
`libdvdread`。`--with-decklink` 被有意排除，因为它需要 Blackmagic DeckLink
SDK。该 tap 没有为 Flite 声明依赖；Homebrew 目前已经提供 `flite`，如确实需要文本
转语音，可先安装它，再为上面的命令追加 `--with-libflite`。这不会增加 MFB 使用的
媒体编码器。`--with-alt-name` 只改变命令名，不增加编解码能力。当前 formula 将
`ggml` 暴露为可选依赖；真正为 FFmpeg 添加 `--enable-whisper` 的开关是
`--with-whisper-cpp`。_

如果追求 Homebrew 可提供的最宽功能集，可先执行 `brew install flite`，再给上面的
命令追加 `--with-libflite`。DeckLink 仍是唯一必须另外提供 SDK 的能力。

#### 2. 验证实际能力集

`ffprobe` 负责探测媒体；编解码器和滤镜能力属于 `ffmpeg`。不要只根据包名推断，直接检查当前实际运行的二进制：

```bash
ffmpeg -hide_banner -buildconf
ffmpeg -hide_banner -encoders
ffmpeg -hide_banner -decoders
ffmpeg -hide_banner -filters
ffprobe -version
```
