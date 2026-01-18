#!/bin/bash
# 🔥 Rebuild FFmpeg with Full Features (libvmaf, libx265, libsvtav1, etc.)
set -e

echo "🔨 FFmpeg Full Feature Rebuild Script"
echo "======================================"
echo ""

# 检查当前 ffmpeg
echo "📊 Current FFmpeg Status:"
ffmpeg -version | head -1
echo ""
echo "Current features:"
ffmpeg -filters 2>&1 | grep -E "(libvmaf|ssim)" || echo "  ⚠️  No libvmaf/ssim found"
echo ""

# 询问用户确认
read -p "⚠️  This will reinstall ffmpeg. Continue? (y/N): " -n 1 -r
echo
if [[ ! $REPLY =~ ^[Yy]$ ]]; then
    echo "❌ Aborted"
    exit 1
fi

echo ""
echo "🍎 macOS Homebrew Installation"
echo "================================"

# 1. 卸载现有 ffmpeg
echo "📦 Step 1: Removing existing ffmpeg..."
brew uninstall --ignore-dependencies ffmpeg 2>/dev/null || true

# 2. 安装依赖库
echo ""
echo "📦 Step 2: Installing dependencies..."
brew install libvmaf x265 svt-av1 aom dav1d jpeg-xl || true

# 3. 重新安装 ffmpeg（从源码编译，启用所有特性）
echo ""
echo "📦 Step 3: Installing ffmpeg with all features..."
brew install ffmpeg --HEAD || brew install ffmpeg

echo ""
echo "✅ Installation complete!"
echo ""

# 4. 验证安装
echo "🔍 Verification:"
echo "================"
echo ""

echo "1. FFmpeg version:"
ffmpeg -version | head -1

echo ""
echo "2. Checking libvmaf filter:"
if ffmpeg -hide_banner -filters 2>&1 | grep -q "libvmaf"; then
    echo "   ✅ libvmaf filter available"
else
    echo "   ❌ libvmaf filter NOT available"
fi

echo ""
echo "3. Checking encoders:"
for encoder in libx265 libsvtav1 libaom-av1; do
    if ffmpeg -hide_banner -encoders 2>&1 | grep -q "$encoder"; then
        echo "   ✅ $encoder available"
    else
        echo "   ⚠️  $encoder not available"
    fi
done

echo ""
echo "4. Checking libvmaf library:"
if [ -f "/opt/homebrew/lib/libvmaf.dylib" ] || [ -f "/usr/local/lib/libvmaf.dylib" ]; then
    echo "   ✅ libvmaf library installed"
else
    echo "   ⚠️  libvmaf library not found"
fi

echo ""
echo "💡 Next Steps:"
echo "=============="
echo "1. Rebuild your project: cd modern_format_boost && cargo build --release"
echo "2. Test quality verification: ./scripts/e2e_quality_test.sh"
echo ""
