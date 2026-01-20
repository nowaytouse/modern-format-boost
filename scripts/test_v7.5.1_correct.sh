#!/bin/bash
# 🔴 v7.5.1 正确参数测试 - 使用与原始卡死相同的参数

set -e

echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "🔴 v7.5.1 修复验证 - 使用原始卡死时的确切参数"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""

# 原始文件（卡死的那个）
ORIGINAL_FILE="/Users/nyamiiko/Downloads/all/zz/鬼针草/OC14k60_1.mp4"

# 创建临时测试目录
TEST_DIR="/tmp/v7.5.1_correct_test_$(date +%s)"
mkdir -p "$TEST_DIR"

echo "📁 测试目录: $TEST_DIR"
echo ""

# 检查原始文件
if [ ! -f "$ORIGINAL_FILE" ]; then
    echo "❌ 错误: 原始文件不存在!"
    echo "   路径: $ORIGINAL_FILE"
    exit 1
fi

echo "✅ 原始文件找到"
ls -lh "$ORIGINAL_FILE"
echo ""

# 创建副本（安全操作）
echo "📋 创建安全副本..."
COPY_FILE="$TEST_DIR/test_video.mp4"
cp "$ORIGINAL_FILE" "$COPY_FILE"

if [ ! -f "$COPY_FILE" ]; then
    echo "❌ 错误: 创建副本失败"
    exit 1
fi

echo "✅ 副本创建: $COPY_FILE"
echo ""

# 获取视频信息
echo "📊 视频信息:"
ffprobe -v error -show_entries format=duration,size -of default=noprint_wrappers=1 "$COPY_FILE" 2>/dev/null | \
    awk '/duration/{printf "   时长: %.1f 秒 (%.1f 分钟)\n", $1, $1/60} /size/{printf "   大小: %.1f MB\n", $1/1024/1024}'
echo ""

# 二进制文件
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
BINARY="$PROJECT_ROOT/target/release/vidquality-hevc"

if [ ! -f "$BINARY" ]; then
    echo "❌ 错误: 二进制文件不存在: $BINARY"
    echo "   请先编译: cd modern_format_boost && cargo build --release"
    exit 1
fi

echo "✅ 二进制文件: $BINARY"
echo ""

echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "🚀 开始测试"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""
echo "⚠️  使用与原始卡死相同的参数:"
echo "   auto --explore --match-quality --compress --apple-compat --ultimate"
echo ""
echo "⚠️  这是v7.5.0中导致卡死的确切文件"
echo "⚠️  如果v7.5.1修复有效，应在2-3分钟内完成"
echo "⚠️  使用10分钟超时作为安全保护"
echo ""

# 记录开始时间
START_TIME=$(date +%s)
START_TIME_BEIJING=$(TZ='Asia/Shanghai' date +"%Y-%m-%d %H:%M:%S")

echo "🕐 开始时间: $START_TIME_BEIJING (北京时间)"
echo ""

# 创建日志文件
LOG_FILE="$TEST_DIR/test.log"

# 使用timeout保护（10分钟）
TIMEOUT=600

# 使用与原始卡死相同的参数
# 原始命令: vidquality-hevc auto --explore --match-quality --compress --apple-compat --recursive --ultimate --in-place /path
# 测试命令: 使用单个文件，不需要--recursive和--in-place
echo "执行: $BINARY auto --explore --match-quality --compress --apple-compat --ultimate $COPY_FILE"
echo ""

if timeout $TIMEOUT "$BINARY" auto --explore --match-quality --compress --apple-compat --ultimate "$COPY_FILE" 2>&1 | tee "$LOG_FILE"; then
    # 成功完成
    END_TIME=$(date +%s)
    END_TIME_BEIJING=$(TZ='Asia/Shanghai' date +"%Y-%m-%d %H:%M:%S")
    ELAPSED=$((END_TIME - START_TIME))
    
    echo ""
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    echo "✅ 测试通过 - 没有卡死!"
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    echo ""
    echo "🕐 开始:  $START_TIME_BEIJING"
    echo "🕐 结束:  $END_TIME_BEIJING"
    echo "⏱️  总计: ${ELAPSED}秒 ($(($ELAPSED / 60))分 $(($ELAPSED % 60))秒)"
    echo ""
    
    # 分析日志
    echo "📊 分析结果:"
    echo ""
    
    if grep -q "Sampling: 1/" "$LOG_FILE"; then
        SAMPLING=$(grep "Sampling:" "$LOG_FILE" | head -1)
        echo "✅ 智能采样已启用:"
        echo "   $SAMPLING"
    fi
    
    if grep -q "Parallel processing" "$LOG_FILE"; then
        echo "✅ 并行处理: Y+U+V 通道同时计算"
    fi
    
    if grep -q "Beijing" "$LOG_FILE"; then
        echo "✅ 北京时区显示正常"
    fi
    
    if grep -q "MS-SSIM" "$LOG_FILE"; then
        echo "✅ MS-SSIM计算完成"
        echo ""
        echo "📊 MS-SSIM 详情:"
        grep -A5 "MS-SSIM" "$LOG_FILE" | head -10
    fi
    
    echo ""
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    echo "🎉 v7.5.1 修复验证成功!"
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    echo ""
    echo "v7.5.0中卡死的文件现在在${ELAPSED}秒内完成"
    echo ""
    
    # 检查输出文件
    OUTPUT_FILE="${COPY_FILE%.*}_hevc.mp4"
    if [ -f "$OUTPUT_FILE" ]; then
        OUTPUT_SIZE=$(ls -lh "$OUTPUT_FILE" | awk '{print $5}')
        echo "✅ 输出文件已创建: $OUTPUT_SIZE"
    fi
    
    SUCCESS=true
    
else
    EXIT_CODE=$?
    
    if [ $EXIT_CODE -eq 124 ]; then
        echo ""
        echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
        echo "❌ 测试失败 - 超时 ${TIMEOUT}秒"
        echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
        echo ""
        echo "⚠️  进程在${TIMEOUT}秒后被终止"
        echo "⚠️  这表明卡死BUG未修复"
        echo ""
        echo "日志最后50行:"
        tail -50 "$LOG_FILE"
        
        SUCCESS=false
    else
        echo ""
        echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
        echo "❌ 测试失败 - 退出码: $EXIT_CODE"
        echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
        echo ""
        echo "日志最后50行:"
        tail -50 "$LOG_FILE"
        
        SUCCESS=false
    fi
fi

echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "📋 测试摘要"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""
echo "原始文件: $ORIGINAL_FILE"
echo "副本文件: $COPY_FILE"
echo "日志文件: $LOG_FILE"
echo "测试目录: $TEST_DIR"
echo ""

if [ "$SUCCESS" = true ]; then
    echo "✅ 状态: 通过"
    echo ""
    echo "🧹 清理:"
    echo "   删除测试文件: rm -rf $TEST_DIR"
    echo "   (原始文件未被触碰，安全)"
    echo ""
    exit 0
else
    echo "❌ 状态: 失败"
    echo ""
    echo "🔍 调试:"
    echo "   日志文件: $LOG_FILE"
    echo "   测试目录: $TEST_DIR"
    echo ""
    echo "请检查:"
    echo "  1. 是否使用v7.5.1? (git log -1)"
    echo "  2. 编译是否成功? (cargo build --release)"
    echo "  3. 查看完整日志: cat $LOG_FILE"
    echo ""
    exit 1
fi
