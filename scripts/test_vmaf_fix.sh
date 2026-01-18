#!/bin/bash
# 测试 vmaf 修复是否有效
set -e

echo "🧪 Testing VMAF Fix"
echo "==================="

# 创建测试目录
TEST_DIR="/tmp/vmaf_fix_test_$$"
mkdir -p "$TEST_DIR"

echo ""
echo "📹 Creating test videos..."
ffmpeg -f lavfi -i testsrc=duration=3:size=320x240:rate=30 \
    -c:v libx264 -crf 18 -y "$TEST_DIR/ref.mp4" 2>/dev/null

ffmpeg -i "$TEST_DIR/ref.mp4" -c:v libx264 -crf 25 \
    -y "$TEST_DIR/dist.mp4" 2>/dev/null

echo "✅ Test videos created"

# 转换为 Y4M
echo ""
echo "🔄 Converting to Y4M..."
ffmpeg -i "$TEST_DIR/ref.mp4" -pix_fmt yuv420p \
    -f yuv4mpegpipe -y "$TEST_DIR/ref.y4m" 2>/dev/null

ffmpeg -i "$TEST_DIR/dist.mp4" -pix_fmt yuv420p \
    -f yuv4mpegpipe -y "$TEST_DIR/dist.y4m" 2>/dev/null

echo "✅ Y4M ready"

# 测试 vmaf 命令
echo ""
echo "📊 Testing vmaf command..."
vmaf --reference "$TEST_DIR/ref.y4m" \
     --distorted "$TEST_DIR/dist.y4m" \
     --feature name=float_ms_ssim \
     --output "$TEST_DIR/result.json" \
     --json 2>&1 | grep -E "(VMAF|ms_ssim|error|WARNING)" || true

# 检查结果
echo ""
if [ -f "$TEST_DIR/result.json" ]; then
    echo "✅ VMAF output generated"
    
    # 解析 MS-SSIM
    if command -v python3 &>/dev/null; then
        MS_SSIM=$(python3 << EOF
import json
try:
    with open('$TEST_DIR/result.json') as f:
        data = json.load(f)
        score = data['pooled_metrics']['float_ms_ssim']['mean']
        print(f'{score:.4f}')
except Exception as e:
    print(f'Error: {e}')
EOF
)
        if [[ "$MS_SSIM" =~ ^[0-9]+\.[0-9]+$ ]]; then
            echo "✅ MS-SSIM score: $MS_SSIM"
            echo ""
            echo "🎉 Fix verified! VMAF is working correctly."
        else
            echo "⚠️  Could not parse MS-SSIM: $MS_SSIM"
        fi
    else
        echo "⚠️  Python3 not available for parsing"
    fi
else
    echo "❌ VMAF output not found"
    echo "⚠️  Fix may not be working"
fi

# 清理
rm -rf "$TEST_DIR"

echo ""
echo "🧹 Cleanup complete"
