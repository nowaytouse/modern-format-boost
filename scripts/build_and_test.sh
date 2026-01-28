#!/bin/bash
set -e
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"

cd "$PROJECT_ROOT/shared_utils"

echo "🔨 Building shared_utils..."
cargo build --release 2>&1 | tail -10

echo ""
echo "✅ Build complete"
echo ""
echo "🧪 Creating test environment..."

# 创建测试目录
TEST_DIR="/tmp/quality_test_$$"
mkdir -p "$TEST_DIR"

# 找一个测试视频并复制
SOURCE_VIDEO=$(find ~/Downloads -iname "*.mp4" -o -iname "*.mov" 2>/dev/null | head -1)

if [ -z "$SOURCE_VIDEO" ]; then
    echo "⚠️  No test video found, creating synthetic test..."
    ffmpeg -f lavfi -i testsrc=duration=5:size=640x480:rate=30 \
        -c:v libx264 -crf 18 -y "$TEST_DIR/test_input.mp4" 2>/dev/null
    TEST_VIDEO="$TEST_DIR/test_input.mp4"
else
    echo "📹 Copying test video (safe copy)..."
    cp "$SOURCE_VIDEO" "$TEST_DIR/test_input.mp4"
    TEST_VIDEO="$TEST_DIR/test_input.mp4"
fi

echo "✅ Test video ready: $(ls -lh "$TEST_VIDEO" | awk '{print $5}')"
echo ""
echo "💡 Test command:"
echo "   cd $PROJECT_ROOT/vidquality_hevc"
echo "   cargo run --release -- \"$TEST_VIDEO\" --explore --match-quality"
echo ""
echo "🔍 Watch for: '📊 Using standalone vmaf tool...'"
echo ""
echo "🧹 Cleanup: rm -rf $TEST_DIR"
