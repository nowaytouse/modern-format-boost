#!/bin/bash
# 安装带 libx265 支持的 FFmpeg
# 解决 "Unrecognized option 'x265-params'" 错误

set -e

echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "🔧 Installing FFmpeg with libx265 support"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

# 检查当前 FFmpeg 配置
echo ""
echo "📊 Current FFmpeg configuration:"
ffmpeg -version 2>&1 | grep configuration | grep -o "enable-[^ ]*" | sort

echo ""
echo "❌ Missing: --enable-libx265"
echo ""

# 方案1: 使用 Homebrew tap 安装完整版 FFmpeg
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "📦 Solution: Install FFmpeg from homebrew-ffmpeg tap"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""
echo "This tap provides FFmpeg with more codecs including libx265"
echo ""

read -p "Continue with installation? (y/n) " -n 1 -r
echo
if [[ ! $REPLY =~ ^[Yy]$ ]]; then
    echo "Installation cancelled"
    exit 1
fi

echo ""
echo "🔄 Step 1: Uninstall current FFmpeg..."
brew uninstall --ignore-dependencies ffmpeg

echo ""
echo "🔄 Step 2: Add homebrew-ffmpeg tap..."
brew tap homebrew-ffmpeg/ffmpeg

echo ""
echo "🔄 Step 3: Install FFmpeg with libx265..."
brew install homebrew-ffmpeg/ffmpeg/ffmpeg --with-x265

echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "✅ Installation complete!"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

echo ""
echo "📊 New FFmpeg configuration:"
ffmpeg -version 2>&1 | grep configuration | grep -o "enable-[^ ]*" | sort

echo ""
echo "🔍 Checking for libx265..."
if ffmpeg -encoders 2>&1 | grep -q libx265; then
    echo "✅ libx265 encoder is available!"
    ffmpeg -h encoder=libx265 2>&1 | head -5
else
    echo "❌ libx265 encoder NOT found!"
    exit 1
fi

echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "🎉 FFmpeg with libx265 is ready!"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
