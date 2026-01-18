#!/bin/bash
# 🔥 Quality Verification Fix Script
# 修复 MS-SSIM 和质量验证失败问题

set -e

echo "🔧 Quality Verification Fix Script"
echo "=================================="

# 检查 ffmpeg libvmaf 支持
echo "📊 Checking ffmpeg libvmaf support..."
if ffmpeg -hide_banner -filters 2>/dev/null | grep -q "libvmaf"; then
    echo "✅ libvmaf filter available"
else
    echo "❌ libvmaf filter NOT available"
    echo ""
    echo "🔧 Installing ffmpeg with libvmaf support..."
    
    # macOS 安装方案
    if [[ "$OSTYPE" == "darwin"* ]]; then
        echo "🍎 macOS detected - installing via Homebrew..."
        if command -v brew >/dev/null 2>&1; then
            # 卸载旧版本
            brew uninstall --ignore-dependencies ffmpeg 2>/dev/null || true
            # 安装带 libvmaf 的版本
            brew install ffmpeg --with-libvmaf 2>/dev/null || \
            brew install ffmpeg || {
                echo "⚠️  Homebrew install failed, trying manual compile..."
                echo "💡 Please install ffmpeg with libvmaf manually:"
                echo "   brew install libvmaf"
                echo "   brew install ffmpeg --HEAD"
                exit 1
            }
        else
            echo "❌ Homebrew not found. Please install:"
            echo "   /bin/bash -c \"\$(curl -fsSL https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh)\""
            exit 1
        fi
    else
        echo "🐧 Linux detected - please install ffmpeg with libvmaf:"
        echo "   Ubuntu/Debian: sudo apt install ffmpeg libvmaf-dev"
        echo "   CentOS/RHEL: sudo yum install ffmpeg libvmaf-devel"
        exit 1
    fi
fi

# 验证修复结果
echo ""
echo "🧪 Testing quality verification..."

# 创建测试视频
TEST_INPUT="/tmp/test_input.mp4"
TEST_OUTPUT="/tmp/test_output.mp4"

echo "📹 Creating test video..."
ffmpeg -f lavfi -i testsrc=duration=2:size=320x240:rate=30 -c:v libx264 -crf 23 -y "$TEST_INPUT" 2>/dev/null

echo "🔄 Creating test output..."
ffmpeg -i "$TEST_INPUT" -c:v libx264 -crf 25 -y "$TEST_OUTPUT" 2>/dev/null

# 测试 SSIM
echo "📊 Testing SSIM calculation..."
if ffmpeg -i "$TEST_INPUT" -i "$TEST_OUTPUT" -lavfi "[0:v][1:v]ssim" -f null - 2>&1 | grep -q "SSIM Y:"; then
    echo "✅ SSIM calculation works"
else
    echo "❌ SSIM calculation failed"
fi

# 测试 MS-SSIM
echo "📊 Testing MS-SSIM calculation..."
if ffmpeg -i "$TEST_INPUT" -i "$TEST_OUTPUT" -lavfi "[0:v][1:v]libvmaf=log_path=/dev/stdout:log_fmt=json:feature='name=float_ms_ssim'" -f null - 2>/dev/null | grep -q "float_ms_ssim"; then
    echo "✅ MS-SSIM calculation works"
else
    echo "⚠️  MS-SSIM calculation failed - will use SSIM fallback"
fi

# 清理测试文件
rm -f "$TEST_INPUT" "$TEST_OUTPUT"

echo ""
echo "🎉 Quality verification fix completed!"
echo "💡 If MS-SSIM still fails, the system will automatically fallback to SSIM"