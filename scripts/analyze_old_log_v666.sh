#!/bin/bash

# 🔍 较旧版本日志分析脚本 (666文件)
# 分析72352行的历史日志，识别BUG模式和问题

set -e

echo "🔍 较旧版本日志分析 (v666)"
echo "========================================"

LOG_FILE="../666"

if [[ ! -f "$LOG_FILE" ]]; then
    echo "❌ 日志文件不存在: $LOG_FILE"
    exit 1
fi

echo "📊 日志基本信息:"
echo "   文件: $LOG_FILE"
echo "   总行数: $(wc -l < "$LOG_FILE")"
echo "   文件大小: $(du -h "$LOG_FILE" | cut -f1)"

echo ""
echo "🔍 错误模式分析:"
echo "----------------------------------------"

# 1. 搜索错误关键词
echo "1. 一般错误:"
grep -i "error\|failed\|panic\|crash" "$LOG_FILE" | head -10 || echo "   ✅ 未发现一般错误"

echo ""
echo "2. 质量计算失败:"
grep -i "ssim.*fail\|psnr.*fail\|quality.*fail" "$LOG_FILE" | head -5 || echo "   ✅ 未发现质量计算失败"

echo ""
echo "3. 内存/资源问题:"
grep -i "memory\|allocation\|limit.*exceed" "$LOG_FILE" | head -5 || echo "   ✅ 未发现内存问题"

echo ""
echo "4. 格式兼容性问题:"
grep -i "format.*incompatib\|pixel.*format\|unsupported" "$LOG_FILE" | head -5 || echo "   ✅ 未发现格式兼容性问题"

echo ""
echo "5. 心跳/超时问题:"
grep -i "heartbeat\|timeout\|duplicate" "$LOG_FILE" | head -5 || echo "   ✅ 未发现心跳问题"

echo ""
echo "📈 处理统计分析:"
echo "----------------------------------------"

# 6. 转换进度分析
echo "6. 转换进度信息:"
PROGRESS_LINES=$(grep -c "Converting.*%" "$LOG_FILE" || echo "0")
echo "   进度更新次数: $PROGRESS_LINES"

if [[ $PROGRESS_LINES -gt 0 ]]; then
    echo "   最后进度:"
    grep "Converting.*%" "$LOG_FILE" | tail -3
fi

echo ""
echo "7. 文件处理统计:"
TOTAL_FILES=$(grep -o "Total Files: [0-9]*" "$LOG_FILE" | head -1 | grep -o "[0-9]*" || echo "未知")
IMAGE_FILES=$(grep -o "Images:.*[0-9]*" "$LOG_FILE" | head -1 | grep -o "[0-9]*" || echo "未知")
VIDEO_FILES=$(grep -o "Videos:.*[0-9]*" "$LOG_FILE" | head -1 | grep -o "[0-9]*" || echo "未知")

echo "   总文件数: $TOTAL_FILES"
echo "   图片文件: $IMAGE_FILES"  
echo "   视频文件: $VIDEO_FILES"

echo ""
echo "8. XMP元数据处理:"
XMP_COUNT=$(grep -c "Found XMP sidecar" "$LOG_FILE" || echo "0")
echo "   XMP文件发现: $XMP_COUNT 次"

echo ""
echo "🚨 潜在问题识别:"
echo "----------------------------------------"

# 9. 检查是否有处理中断
echo "9. 处理完成状态:"
LAST_LINES=$(tail -20 "$LOG_FILE")
if echo "$LAST_LINES" | grep -q "Converting.*%"; then
    echo "   ⚠️  处理可能未完成 - 最后仍在转换中"
    echo "   最后进度:"
    echo "$LAST_LINES" | grep "Converting.*%" | tail -1
elif echo "$LAST_LINES" | grep -q "completed\|finished\|done"; then
    echo "   ✅ 处理正常完成"
else
    echo "   ❓ 处理状态不明确"
fi

echo ""
echo "10. 性能指标:"
# 查找ETA和处理时间信息
ETA_LINES=$(grep -o "ETA: [0-9:]*" "$LOG_FILE" | tail -5)
if [[ -n "$ETA_LINES" ]]; then
    echo "   最近ETA估算:"
    echo "$ETA_LINES" | sed 's/^/     /'
fi

echo ""
echo "🔧 建议修复措施:"
echo "----------------------------------------"

# 基于分析结果给出建议
ISSUES_FOUND=0

# 检查是否有错误
if grep -q -i "error\|failed\|panic" "$LOG_FILE"; then
    echo "❗ 发现错误信息 - 需要详细分析错误原因"
    ((ISSUES_FOUND++))
fi

# 检查处理效率
if [[ $PROGRESS_LINES -gt 1000 ]]; then
    echo "⚠️  进度更新频繁 ($PROGRESS_LINES次) - 可能影响性能"
    ((ISSUES_FOUND++))
fi

# 检查XMP处理
if [[ $XMP_COUNT -gt 100 ]]; then
    echo "📋 大量XMP文件 ($XMP_COUNT个) - 确保元数据正确处理"
fi

if [[ $ISSUES_FOUND -eq 0 ]]; then
    echo "✅ 未发现明显问题 - 日志看起来正常"
else
    echo "🔍 发现 $ISSUES_FOUND 个潜在问题需要关注"
fi

echo ""
echo "📝 详细分析建议:"
echo "----------------------------------------"
echo "1. 如需深入分析，可使用以下命令:"
echo "   grep -n 'error\\|fail' $LOG_FILE"
echo "   grep -A5 -B5 'Converting.*[5-9][0-9]%' $LOG_FILE"
echo ""
echo "2. 关注点:"
echo "   - 处理是否正常完成"
echo "   - 是否有重复的错误模式"
echo "   - 性能瓶颈在哪里"
echo "   - XMP元数据处理是否正确"

echo ""
echo "✅ 较旧版本日志分析完成"