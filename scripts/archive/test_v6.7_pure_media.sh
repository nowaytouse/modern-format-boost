#!/bin/bash
# 🔥 v6.7: 纯媒体对比功能测试脚本

set -e

echo "═══════════════════════════════════════════════════════════"
echo "🔬 v6.7 Pure Media Comparison Test"
echo "═══════════════════════════════════════════════════════════"

# 测试文件
TEST_FILE="../test_videos/test_short.mp4"
OUTPUT_FILE="/tmp/test_v6.7_output.mp4"

if [ ! -f "$TEST_FILE" ]; then
    echo "❌ Test file not found: $TEST_FILE"
    exit 1
fi

# 获取输入文件信息
echo ""
echo "📁 Input file: $TEST_FILE"
INPUT_SIZE=$(stat -f%z "$TEST_FILE" 2>/dev/null || stat -c%s "$TEST_FILE")
echo "   Total size: $INPUT_SIZE bytes"

# 使用 ffprobe 获取视频流比特率和时长
VIDEO_BITRATE=$(ffprobe -v quiet -select_streams v:0 -show_entries stream=bit_rate -of default=noprint_wrappers=1:nokey=1 "$TEST_FILE")
DURATION=$(ffprobe -v quiet -show_entries format=duration -of default=noprint_wrappers=1:nokey=1 "$TEST_FILE")

echo "   Video bitrate: $VIDEO_BITRATE bps"
echo "   Duration: $DURATION seconds"

# 计算视频流大小
if [ -n "$VIDEO_BITRATE" ] && [ -n "$DURATION" ]; then
    VIDEO_STREAM_SIZE=$(echo "$VIDEO_BITRATE * $DURATION / 8" | bc)
    echo "   Video stream size: $VIDEO_STREAM_SIZE bytes (calculated)"
    CONTAINER_OVERHEAD=$((INPUT_SIZE - VIDEO_STREAM_SIZE))
    echo "   Container overhead: $CONTAINER_OVERHEAD bytes"
fi

echo ""
echo "═══════════════════════════════════════════════════════════"
echo "🎬 Running vidquality-hevc with pure media comparison..."
echo "═══════════════════════════════════════════════════════════"

# 运行 vidquality-hevc（使用 MOV 容器来测试容器开销）
cd ../vidquality_hevc
./target/release/vidquality-hevc auto "$TEST_FILE" --explore --match-quality true --compress --apple-compat --output "$OUTPUT_FILE" 2>&1 | head -120

# 检查输出
if [ -f "$OUTPUT_FILE" ]; then
    OUTPUT_SIZE=$(stat -f%z "$OUTPUT_FILE" 2>/dev/null || stat -c%s "$OUTPUT_FILE")
    echo ""
    echo "═══════════════════════════════════════════════════════════"
    echo "📊 Results:"
    echo "   Input total: $INPUT_SIZE bytes"
    echo "   Output total: $OUTPUT_SIZE bytes"
    
    # 获取输出视频流大小
    OUTPUT_VIDEO_BITRATE=$(ffprobe -v quiet -select_streams v:0 -show_entries stream=bit_rate -of default=noprint_wrappers=1:nokey=1 "$OUTPUT_FILE")
    OUTPUT_DURATION=$(ffprobe -v quiet -show_entries format=duration -of default=noprint_wrappers=1:nokey=1 "$OUTPUT_FILE")
    
    if [ -n "$OUTPUT_VIDEO_BITRATE" ] && [ -n "$OUTPUT_DURATION" ]; then
        OUTPUT_VIDEO_STREAM_SIZE=$(echo "$OUTPUT_VIDEO_BITRATE * $OUTPUT_DURATION / 8" | bc)
        echo "   Output video stream: $OUTPUT_VIDEO_STREAM_SIZE bytes"
        OUTPUT_CONTAINER_OVERHEAD=$((OUTPUT_SIZE - OUTPUT_VIDEO_STREAM_SIZE))
        echo "   Output container overhead: $OUTPUT_CONTAINER_OVERHEAD bytes"
    fi
    
    rm -f "$OUTPUT_FILE"
    echo "✅ Test completed!"
else
    echo "❌ Output file not created"
fi
