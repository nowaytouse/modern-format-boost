#!/bin/bash
# 🔥 v7.5.3 心跳监控测试

set -e

echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "🔥 v7.5.3 心跳监控和进度显示测试"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""

# 原始问题文件
ORIGINAL_FILE="/Users/nyamiiko/Downloads/all/zz/鬼针草/OC14k60_1.mp4"

# 创建临时测试目录
TEST_DIR="/tmp/v7.5.3_test_$(date +%s)"
mkdir -p "$TEST_DIR"

echo "📁 测试目录: $TEST_DIR"
echo ""

# 检查原始文件
if [ ! -f "$ORIGINAL_FILE" ]; then
    echo "❌ 错误: 原始文件不存在!"
    exit 1
fi

echo "✅ 原始文件: $ORIGINAL_FILE"
ls -lh "$ORIGINAL_FILE"
echo ""

# 创建副本
echo "📋 创建安全副本..."
COPY_FILE="$TEST_DIR/test_video.mp4"
cp "$ORIGINAL_FILE" "$COPY_FILE"

echo "✅ 副本: $COPY_FILE"
echo ""

# 二进制文件
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
BINARY="$PROJECT_ROOT/target/release/vidquality-hevc"

if [ ! -f "$BINARY" ]; then
    echo "❌ 错误: 二进制文件不存在"
    exit 1
fi

echo "✅ 二进制: $BINARY"
ls -lh "$BINARY" | awk '{print "   时间戳:", $6, $7, $8}'
echo ""

echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "🚀 开始测试"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""
echo "预期行为:"
echo "  1. 显示 '🔄 GPU Encoding started (heartbeat active)'"
echo "  2. 每30秒显示 '💓 Heartbeat: XXs ago (Beijing: ...)'"
echo "  3. 显示进度 '⏳ Progress: XX% ... ETA: XXs Speed: X.XXx'"
echo "  4. 完成时显示 '✅ Encoding completed, heartbeat stopped'"
echo ""
echo "如果卡死:"
echo "  - 5分钟后显示 '⚠️ FREEZE DETECTED'"
echo "  - 自动终止进程"
echo ""

# 记录开始时间
START_TIME=$(date +%s)
START_TIME_BEIJING=$(TZ='Asia/Shanghai' date +"%Y-%m-%d %H:%M:%S")

echo "🕐 开始时间: $START_TIME_BEIJING (北京时间)"
echo ""

# 执行测试（使用10分钟超时作为安全保护）
LOG_FILE="$TEST_DIR/test.log"

if timeout 600 "$BINARY" auto --explore --match-quality --compress --apple-compat --ultimate "$COPY_FILE" 2>&1 | tee "$LOG_FILE"; then
    END_TIME=$(date +%s)
    END_TIME_BEIJING=$(TZ='Asia/Shanghai' date +"%Y-%m-%d %H:%M:%S")
    ELAPSED=$((END_TIME - START_TIME))
    
    echo ""
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    echo "✅ 测试通过!"
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    echo ""
    echo "🕐 开始: $START_TIME_BEIJING"
    echo "🕐 结束: $END_TIME_BEIJING"
    echo "⏱️  总计: ${ELAPSED}秒 ($(($ELAPSED / 60))分 $(($ELAPSED % 60))秒)"
    echo ""
    
    echo "📊 验证结果:"
    echo ""
    
    if grep -q "🔄 GPU Encoding started" "$LOG_FILE"; then
        echo "✅ 启动消息正常"
    else
        echo "❌ 缺少启动消息"
    fi
    
    if grep -q "💓 Heartbeat:" "$LOG_FILE"; then
        echo "✅ 心跳监控正常"
        HEARTBEAT_COUNT=$(grep -c "💓 Heartbeat:" "$LOG_FILE")
        echo "   心跳次数: $HEARTBEAT_COUNT"
    else
        echo "❌ 缺少心跳消息"
    fi
    
    if grep -q "⏳ Progress:" "$LOG_FILE"; then
        echo "✅ 进度显示正常"
    else
        echo "❌ 缺少进度显示"
    fi
    
    if grep -q "Beijing" "$LOG_FILE"; then
        echo "✅ 北京时间显示正常"
    else
        echo "❌ 缺少北京时间"
    fi
    
    if grep -q "✅ Encoding completed" "$LOG_FILE"; then
        echo "✅ 完成消息正常"
    else
        echo "❌ 缺少完成消息"
    fi
    
    echo ""
    echo "🎉 v7.5.3修复验证成功!"
    
else
    EXIT_CODE=$?
    
    if [ $EXIT_CODE -eq 124 ]; then
        echo ""
        echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
        echo "❌ 测试失败 - 超时600秒"
        echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
        echo ""
        echo "日志最后50行:"
        tail -50 "$LOG_FILE"
    else
        echo ""
        echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
        echo "❌ 测试失败 - 退出码: $EXIT_CODE"
        echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    fi
fi

echo ""
echo "📋 测试文件:"
echo "   日志: $LOG_FILE"
echo "   目录: $TEST_DIR"
echo ""
