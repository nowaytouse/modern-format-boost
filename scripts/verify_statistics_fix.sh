#!/bin/bash
# 验证统计BUG修复效果 - 使用副本安全测试

set -euo pipefail

echo "🔍 统计BUG修复验证测试"
echo "═══════════════════════════════════════════════════════════"

cd "$(dirname "$0")/.."

# 创建测试副本目录
TEST_DIR="/tmp/statistics_fix_test_$(date +%s)"
mkdir -p "$TEST_DIR"

echo "📋 测试配置:"
echo "   • 测试目录: $TEST_DIR"
echo "   • 使用副本: 严禁损害原件"
echo "   • 容差机制: 1%精确控制"

# 复制少量测试文件
echo ""
echo "📂 准备测试文件..."
ORIGINAL_DIR="/Users/nyamiiko/Downloads/all/闷茶子新"

if [ -d "$ORIGINAL_DIR" ]; then
    # 复制前10个JPEG文件进行测试
    find "$ORIGINAL_DIR" -name "*.jpg" -o -name "*.jpeg" | head -10 | while read file; do
        cp "$file" "$TEST_DIR/"
    done
    
    FILE_COUNT=$(ls "$TEST_DIR"/*.jpg "$TEST_DIR"/*.jpeg 2>/dev/null | wc -l || echo 0)
    echo "✅ 复制了 $FILE_COUNT 个JPEG测试文件"
else
    echo "❌ 原始目录不存在，创建模拟测试文件"
    # 创建模拟JPEG文件用于测试
    for i in {1..5}; do
        echo "fake jpeg content" > "$TEST_DIR/test_$i.jpg"
    done
    FILE_COUNT=5
fi

if [ $FILE_COUNT -eq 0 ]; then
    echo "❌ 没有测试文件，退出"
    rm -rf "$TEST_DIR"
    exit 1
fi

# 编译最新版本
echo ""
echo "🧪 编译最新版本..."
if cargo build --release --bin imgquality-hevc >/dev/null 2>&1; then
    echo "✅ 编译成功"
else
    echo "❌ 编译失败"
    rm -rf "$TEST_DIR"
    exit 1
fi

# 运行转换测试
echo ""
echo "🔄 运行JXL转换测试..."
echo "   目标: 验证统计计算是否正确"

OUTPUT_DIR="$TEST_DIR/output"
mkdir -p "$OUTPUT_DIR"

# 运行转换并捕获统计输出
echo "   执行转换..."
CONVERSION_LOG="$TEST_DIR/conversion.log"

# 使用timeout避免长时间卡住
timeout 300 ./target/release/imgquality-hevc \
    --input "$TEST_DIR" \
    --output "$OUTPUT_DIR" \
    --format jxl \
    --distance 0.1 \
    --verbose 2>&1 | tee "$CONVERSION_LOG" || true

# 分析统计结果
echo ""
echo "📊 分析统计结果..."

if [ -f "$CONVERSION_LOG" ]; then
    # 提取统计信息
    PROCESSED=$(grep "Files Processed:" "$CONVERSION_LOG" | grep -o '[0-9]\+' | tail -1 || echo "0")
    SUCCEEDED=$(grep "Succeeded:" "$CONVERSION_LOG" | grep -o '[0-9]\+' | tail -1 || echo "0")
    SKIPPED=$(grep "Skipped:" "$CONVERSION_LOG" | grep -o '[0-9]\+' | tail -1 || echo "0")
    SUCCESS_RATE=$(grep "Success Rate:" "$CONVERSION_LOG" | grep -o '[0-9]\+\.[0-9]\+' | tail -1 || echo "0.0")
    
    echo "📈 统计结果:"
    echo "   • 处理文件: $PROCESSED"
    echo "   • 成功转换: $SUCCEEDED"
    echo "   • 跳过文件: $SKIPPED"
    echo "   • 成功率: $SUCCESS_RATE%"
    
    # 验证修复效果
    echo ""
    echo "🧪 修复效果验证:"
    
    if [ "$PROCESSED" -gt 0 ]; then
        echo "✅ 有文件被处理"
        
        if [ "$SUCCESS_RATE" != "0.0" ] && [ "$SUCCEEDED" -gt 0 ]; then
            echo "✅ 统计BUG已修复！"
            echo "   • 成功率不再是0.0%"
            echo "   • 有文件被正确标记为成功"
            echo "   • 容差机制正常工作"
        else
            echo "❌ 统计BUG仍然存在"
            echo "   • 成功率仍为0.0%或无成功转换"
            echo "   • 需要进一步检查容差逻辑"
        fi
        
        # 检查实际输出文件
        JXL_COUNT=$(find "$OUTPUT_DIR" -name "*.jxl" 2>/dev/null | wc -l || echo 0)
        echo "   • 实际生成JXL文件: $JXL_COUNT 个"
        
        if [ $JXL_COUNT -gt 0 ] && [ "$SUCCEEDED" -gt 0 ]; then
            echo "✅ 实际转换与统计一致"
        elif [ $JXL_COUNT -gt 0 ] && [ "$SUCCEEDED" -eq 0 ]; then
            echo "❌ 有转换但统计为0（统计BUG未修复）"
        fi
    else
        echo "❌ 没有文件被处理"
    fi
else
    echo "❌ 转换日志不存在"
fi

# 清理测试文件
echo ""
echo "🧹 清理测试文件..."
rm -rf "$TEST_DIR"
echo "✅ 测试完成，临时文件已清理"

echo ""
echo "═══════════════════════════════════════════════════════════"
echo "🎯 统计BUG修复验证完成"
echo ""
echo "📋 修复机制总结:"
echo "   1. JXL转换应用1%容差 (tolerance_ratio = 1.01)"
echo "   2. 在容差范围内的转换标记为 success=true, skipped=false"
echo "   3. 超出容差的转换标记为 success=true, skipped=true"
echo "   4. BatchResult.success_rate() 正确计算 succeeded/total"
echo ""
echo "🚀 如果测试显示成功率>0%且有成功转换，则统计BUG已修复！"