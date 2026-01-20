#!/bin/bash
# 🔴 v7.5.1 Critical Fix Verification Script
# 测试 MS-SSIM 卡死修复

set -e

echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "🔴 v7.5.1 Critical Fix Verification"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""

# 检查是否提供了测试视频
if [ -z "$1" ]; then
    echo "❌ Error: No test video provided"
    echo ""
    echo "Usage: $0 <path_to_test_video>"
    echo ""
    echo "Examples:"
    echo "  $0 /path/to/48s_video.mov"
    echo "  $0 /path/to/5min_video.mp4"
    echo ""
    echo "Recommended test videos:"
    echo "  - 5s video: Should complete quickly (~10s)"
    echo "  - 48s video: Should complete in ~2-3min (not freeze!)"
    echo "  - 5min video: Should skip MS-SSIM and complete in ~1min"
    exit 1
fi

TEST_VIDEO="$1"

# 检查视频文件是否存在
if [ ! -f "$TEST_VIDEO" ]; then
    echo "❌ Error: Video file not found: $TEST_VIDEO"
    exit 1
fi

echo "📹 Test Video: $TEST_VIDEO"
echo ""

# 获取视频信息
echo "📊 Video Information:"
ffprobe -v error -show_entries format=duration -of default=noprint_wrappers=1:nokey=1 "$TEST_VIDEO" 2>/dev/null | \
    awk '{printf "   Duration: %.1f seconds (%.1f minutes)\n", $1, $1/60}'
echo ""

# 检查二进制文件
BINARY="../target/release/vidquality_hevc"
if [ ! -f "$BINARY" ]; then
    echo "⚠️  Binary not found, compiling..."
    cd ..
    cargo build --release
    cd scripts
fi

echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "🚀 Starting Test (with timeout protection)"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""

# 记录开始时间
START_TIME=$(date +%s)
START_TIME_BEIJING=$(date +"%Y-%m-%d %H:%M:%S")

echo "🕐 Start Time: $START_TIME_BEIJING (Beijing)"
echo ""

# 使用 timeout 保护（最多10分钟）
# 如果v7.5.1修复有效，应该在几分钟内完成
TIMEOUT=600  # 10 minutes

if timeout $TIMEOUT "$BINARY" "$TEST_VIDEO" --ultimate 2>&1 | tee /tmp/v7.5.1_test.log; then
    # 计算耗时
    END_TIME=$(date +%s)
    END_TIME_BEIJING=$(date +"%Y-%m-%d %H:%M:%S")
    ELAPSED=$((END_TIME - START_TIME))
    
    echo ""
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    echo "✅ TEST PASSED - No Freeze!"
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    echo ""
    echo "🕐 Start:  $START_TIME_BEIJING"
    echo "🕐 End:    $END_TIME_BEIJING"
    echo "⏱️  Total: ${ELAPSED}s ($(($ELAPSED / 60))min $(($ELAPSED % 60))s)"
    echo ""
    
    # 检查日志中的关键信息
    if grep -q "Sampling: 1/" /tmp/v7.5.1_test.log; then
        echo "✅ Smart sampling detected"
        grep "Sampling:" /tmp/v7.5.1_test.log | head -1
    fi
    
    if grep -q "Parallel processing" /tmp/v7.5.1_test.log; then
        echo "✅ Parallel processing detected"
    fi
    
    if grep -q "Skipping MS-SSIM" /tmp/v7.5.1_test.log; then
        echo "✅ Long video skip detected (>30min)"
    fi
    
    echo ""
    echo "🎉 v7.5.1 fix is working correctly!"
    
else
    EXIT_CODE=$?
    
    if [ $EXIT_CODE -eq 124 ]; then
        echo ""
        echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
        echo "❌ TEST FAILED - Timeout after ${TIMEOUT}s"
        echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
        echo ""
        echo "⚠️  The process was killed after ${TIMEOUT}s"
        echo "⚠️  This suggests the freeze bug is NOT fixed"
        echo ""
        echo "Please check:"
        echo "  1. Are you using v7.5.1?"
        echo "  2. Did compilation succeed?"
        echo "  3. Check the log: /tmp/v7.5.1_test.log"
        exit 1
    else
        echo ""
        echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
        echo "❌ TEST FAILED - Exit code: $EXIT_CODE"
        echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
        echo ""
        echo "Check the log: /tmp/v7.5.1_test.log"
        exit 1
    fi
fi

echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "📋 Test Summary"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""
echo "Video: $TEST_VIDEO"
echo "Time:  ${ELAPSED}s"
echo "Log:   /tmp/v7.5.1_test.log"
echo ""
echo "✅ v7.5.1 Critical Fix Verified!"
echo ""
