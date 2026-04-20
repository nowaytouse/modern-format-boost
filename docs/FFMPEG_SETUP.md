# FFmpeg Advanced Setup Guide / FFmpeg 进阶安装指南

This guide explains how to install a full-featured FFmpeg (with plugins like FDK-AAC, Chromaprint, and AI filters) on macOS using the [homebrew-ffmpeg](https://github.com/homebrew-ffmpeg/homebrew-ffmpeg) tap, while maintaining compatibility with system dependencies.

本指南介绍如何在 macOS 上通过 `homebrew-ffmpeg` tap 安装全功能版 FFmpeg（包含 FDK-AAC、Chromaprint 及 AI 滤镜），并确保系统依赖兼容。

---

## 📖 English

### The "Link Overwrite" Strategy
Homebrew's standard `chromaprint` package strictly depends on the formula named `ffmpeg`. To satisfy this dependency while using the enhanced version, we install both and use Homebrew's linking system to toggle the active version.

#### 1. Install Official FFmpeg
This satisfies dependencies for other packages like `chromaprint`.
```bash
brew install ffmpeg
```

#### 2. Install Full-Featured Tap Version
Install the enhanced version from the tap. It will be built in its own Cellar.
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
    --with-libdvdnav \
    --with-libdvdread \
    --with-libgsm \
    --with-openssl@3 \
    --with-speex
```
*Note: `--with-decklink` and `--with-libflite` are excluded as they require manual SDKs or have current platform issues.*

#### 3. Overwrite Systems Links
Toggle the active `ffmpeg` command to the tap version.
```bash
# Unlink the core version
brew unlink ffmpeg
# Link the full-featured version
brew link --overwrite homebrew-ffmpeg/ffmpeg/ffmpeg
```

#### 4. Accessing Official Version (Optional)
The core version is still in the Cellar. You can create a dedicated alias for it:
```bash
# Create a symbolic link to the core version with a unique name
ln -sf $(brew --prefix)/opt/ffmpeg/bin/ffmpeg $(brew --prefix)/bin/ffmpeg-official
```

---

## 🇨🇳 简体中文

### “链接覆盖”策略
Homebrew 标准的 `chromaprint` 软件包严格依赖于名为 `ffmpeg` 的公式。为了满足此依赖，我们先安装官方版，再安装增强版并利用 Homebrew 的链接机制进行切换。

#### 1. 安装官方 FFmpeg
这步是为了安装 `chromaprint` 等依赖于 `ffmpeg` 名称的包。
```bash
brew install ffmpeg
```

#### 2. 安装“终极全功能版”
从 `homebrew-ffmpeg` 安装增强版。它会安装在独立的目录中，不会覆盖原版。
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
    --with-libdvdnav \
    --with-libdvdread \
    --with-libgsm \
    --with-openssl@3 \
    --with-speex
```
*注意：`--with-decklink` 和 `--with-libflite` 已排除，前者需要手动下载 SDK，后者存在兼容性问题。*

#### 3. 覆盖系统链接
将 `ffmpeg` 命令指向全功能版。
```bash
# 断开官方版链接
brew unlink ffmpeg
# 强制链接全功能版
brew link --overwrite homebrew-ffmpeg/ffmpeg/ffmpeg
```

#### 4. 调用官方原版（可选）
官方版本依然在系统中，你可以通过建立别名来随时调用它：
```bash
# 将官方核心版链接为 'ffmpeg-official'
ln -sf $(brew --prefix)/opt/ffmpeg/bin/ffmpeg $(brew --prefix)/bin/ffmpeg-official
```
