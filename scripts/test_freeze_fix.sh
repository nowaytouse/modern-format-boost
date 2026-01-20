#!/bin/bash
# 🔴 v7.5.1 Freeze Fix Test - Safe Copy Testing
# 测试卡死修复 - 使用副本安全测试

set -e

echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "🔴 v7.5.1 Freeze Fix Verification"
echo "   Testing with the EXACT file that caused freeze"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""

# 原始文件（卡死的那个）
ORIGINAL_FILE="/Users/nyamiiko/Downloads/all/zz/鬼针草/OC14k60_1.mp4"

# 创建临时测试目录
TEST_DIR="/tmp/v7.5.1_test_$(date +%s)"
mkdir -p "$TEST_DIR"

echo "📁 Test Directory: $TEST_DIR"
echo ""

# 检查原始文件
if [ ! -f "$ORIGINAL_FILE" ]; then
    echo "❌ Error: Original file not found!"
    echo "   Expected: $ORIGINAL_FILE"
    exit 1
fi

echo "✅ Original file found"
ls -lh "$ORIGINAL_FILE"
echo ""

# 创建副本（安全操作，不影响原文件）
echo "📋 Creating safe copy for testing..."
COPY_FILE="$TEST_DIR/test_video.mp4"
cp "$ORIGINAL_FILE" "$COPY_FILE"

if [ ! -f "$COPY_FILE" ]; then
    echo "❌ Error: Failed to create copy"
    exit 1
fi

echo "✅ Copy created: $COPY_FILE"
echo ""

# 获取视频信息
echo "📊 Video Information:"
ffprobe -v error -show_entries format=duration,size -of default=noprint_wrappers=1 "$COPY_FILE" 2>/dev/null | \
    awk '/duration/{printf "   Duration: %.1f seconds (%.1f minutes)\n", $1, $1/60} /size/{printf "   Size: %.1f MB\n", $1/1024/1024}'
echo ""

# 检查二进制文件
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BINARY="$SCRIPT_DIR/../target/release/vidquality_hevc"
if [ ! -f "$BINARY" ]; then
    echo "❌ Error: Binary not found at $BINARY"
    echo "   Please compile first: cargo build --release"
    exit 1
fi

echo "✅ Binary found: $BINARY"
echo ""

echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "🚀 Starting Test"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""
echo "⚠️  This is the EXACT file that caused freeze in v7.5.0"
echo "⚠️  If v7.5.1 works, it should complete in 2-3 minutes"
echo "⚠️  Using 10-minute timeout as safety net"
echo ""

# 记录开始时间
START_TIME=$(date +%s)
START_TIME_BEIJING=$(date +"%Y-%m-%d %H:%M:%S")

echo "🕐 Start Time: $START_TIME_BEIJING (Beijing)"
echo ""

# 创建日志文件
LOG_FILE="$TEST_DIR/test.log"

# 使用 timeout 保护（10分钟）
TIMEOUT=600

echo "Running: $BINARY $COPY_FILE --ultimate"
echo ""

if timeout $TIMEOUT "$BINARY" "$COPY_FILE" --ultimate 2>&1 | tee "$LOG_FILE"; then
    # 成功完成
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
    
    # 分析日志
    echo "📊 Analysis:"
    echo ""
    
    if grep -q "Sampling: 1/" "$LOG_FILE"; then
        SAMPLING=$(grep "Sampling:" "$LOG_FILE" | head -1)
        echo "✅ Smart sampling detected:"
        echo "   $SAMPLING"
    fi
    
    if grep -q "Parallel processing" "$LOG_FILE"; then
        echo "✅ Parallel processing: Y+U+V channels simultaneously"
    fi
    
    if grep -q "Skipping MS-SSIM" "$LOG_FILE"; then
        echo "✅ Long video skip detected (>30min)"
    fi
    
    if grep -q "Beijing" "$LOG_FILE"; then
        echo "✅ Beijing timezone display working"
    fi
    
    # 检查是否有 MS-SSIM 分数
    if grep -q "MS-SSIM" "$LOG_FILE"; then
        echo ""
        echo "📊 MS-SSIM Results:"
        grep -A3 "MS-SSIM Y/U/V:" "$LOG_FILE" | head -4
    fi
    
    echo ""
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    echo "🎉 v7.5.1 Fix Verified Successfully!"
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    echo ""
    echo "The file that caused freeze in v7.5.0 now completes in ${ELAPSED}s"
    echo ""
    
    # 检查输出文件
    OUTPUT_FILE="${COPY_FILE%.*}_hevc.mp4"
    if [ -f "$OUTPUT_FILE" ]; then
        OUTPUT_SIZE=$(ls -lh "$OUTPUT_FILE" | awk '{print $5}')
        echo "✅ Output file created: $OUTPUT_SIZE"
    fi
    
    SUCCESS=true
    
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
        echo "Last 50 lines of log:"
        tail -50 "$LOG_FILE"
        
        SUCCESS=false
    else
        echo ""
        echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
        echo "❌ TEST FAILED - Exit code: $EXIT_CODE"
        echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
        echo ""
        echo "Last 50 lines of log:"
        tail -50 "$LOG_FILE"
        
        SUCCESS=false
    fi
fi

echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "📋 Test Summary"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""
echo "Original: $ORIGINAL_FILE"
echo "Copy:     $COPY_FILE"
echo "Log:      $LOG_FILE"
echo "Test Dir: $TEST_DIR"
echo ""

if [ "$SUCCESS" = true ]; then
    echo "✅ Status: PASSED"
    echo ""
    echo "🧹 Cleanup:"
    echo "   To remove test files: rm -rf $TEST_DIR"
    echo "   (Original file is UNTOUCHED and safe)"
    echo ""
    exit 0
else
    echo "❌ Status: FAILED"
    echo ""
    echo "🔍 Debug:"
    echo "   Log file: $LOG_FILE"
    echo "   Test dir: $TEST_DIR"
    echo ""
    echo "Please check:"
    echo "  1. Are you using v7.5.1? (git log -1)"
    echo "  2. Did compilation succeed? (cargo build --release)"
    echo "  3. Check the full log: cat $LOG_FILE"
    echo ""
    exit 1
fi
