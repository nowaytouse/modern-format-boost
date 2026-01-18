#!/bin/bash
# 安装带 libvmaf 的 ffmpeg
set -e

echo "🔥 Installing FFmpeg with libvmaf Support"
echo "=========================================="
echo ""

# 方案 1: 尝试 Homebrew tap
echo "📦 Method 1: Trying homebrew-ffmpeg tap..."
if ! brew tap | grep -q "homebrew-ffmpeg"; then
    brew tap homebrew-ffmpeg/ffmpeg
fi

echo ""
echo "⚠️  Uninstalling current ffmpeg..."
brew uninstall --ignore-dependencies ffmpeg 2>/dev/null || true

echo ""
echo "📦 Installing ffmpeg with libvmaf..."
brew install homebrew-ffmpeg/ffmpeg/ffmpeg --with-libvmaf || {
    echo ""
    echo "⚠️  Method 1 failed, trying Method 2..."
    echo ""
    
    # 方案 2: 从源码编译
    echo "📦 Method 2: Building from source..."
    
    # 安装依赖
    brew install libvmaf x265 svt-av1 dav1d x264 opus lame vpx
    
    # 下载 ffmpeg 源码
    cd /tmp
    rm -rf ffmpeg-8.0.1
    curl -O https://ffmpeg.org/releases/ffmpeg-8.0.1.tar.xz
    tar xf ffmpeg-8.0.1.tar.xz
    cd ffmpeg-8.0.1
    
    # 配置编译选项
    ./configure \
        --prefix=/usr/local/ffmpeg-libvmaf \
        --enable-gpl \
        --enable-version3 \
        --enable-libvmaf \
        --enable-libx265 \
        --enable-libx264 \
        --enable-libsvtav1 \
        --enable-libdav1d \
        --enable-libvpx \
        --enable-libopus \
        --enable-libmp3lame \
        --enable-videotoolbox \
        --enable-audiotoolbox
    
    # 编译（使用多核）
    make -j$(sysctl -n hw.ncpu)
    
    # 安装
    sudo make install
    
    # 创建符号链接
    sudo ln -sf /usr/local/ffmpeg-libvmaf/bin/ffmpeg /usr/local/bin/ffmpeg
    sudo ln -sf /usr/local/ffmpeg-libvmaf/bin/ffprobe /usr/local/bin/ffprobe
    
    echo "✅ Built from source"
}

echo ""
echo "🔍 Verification:"
echo "================"
ffmpeg -version | head -1
echo ""

if ffmpeg -hide_banner -filters 2>&1 | grep -q "libvmaf.*VV->V"; then
    echo "✅ libvmaf filter is now available!"
else
    echo "❌ libvmaf filter still not available"
    echo "💡 You may need to restart your terminal"
fi

echo ""
echo "💡 Next: Rebuild project and test"
