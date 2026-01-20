#!/bin/bash

# 深度日志分析脚本 - 识别BUG模式和问题
# 分析check日志文件中的所有错误、警告和异常情况

LOG_FILE="../check"
REPORT_FILE="deep_log_analysis_report_$(date +%Y%m%d_%H%M%S).txt"

echo "🔍 深度日志分析报告" > "$REPORT_FILE"
echo "分析时间: $(date)" >> "$REPORT_FILE"
echo "日志文件: $LOG_FILE" >> "$REPORT_FILE"
echo "总行数: $(wc -l < "$LOG_FILE")" >> "$REPORT_FILE"
echo "========================================" >> "$REPORT_FILE"
echo "" >> "$REPORT_FILE"

# 1. GIF像素格式不兼容错误
echo "1. GIF像素格式不兼容错误分析:" >> "$REPORT_FILE"
echo "----------------------------------------" >> "$REPORT_FILE"
gif_errors=$(grep -c "Pixel format incompatibility" "$LOG_FILE")
echo "总计GIF像素格式错误: $gif_errors 次" >> "$REPORT_FILE"
echo "" >> "$REPORT_FILE"

# 提取具体的GIF错误信息
echo "具体GIF错误详情:" >> "$REPORT_FILE"
grep -A2 -B2 "Pixel format incompatibility" "$LOG_FILE" | head -20 >> "$REPORT_FILE"
echo "" >> "$REPORT_FILE"

# 2. MS-SSIM计算失败
echo "2. MS-SSIM质量计算失败分析:" >> "$REPORT_FILE"
echo "----------------------------------------" >> "$REPORT_FILE"
msssim_failures=$(grep -c "ALL QUALITY CALCULATIONS FAILED" "$LOG_FILE")
echo "MS-SSIM完全失败次数: $msssim_failures 次" >> "$REPORT_FILE"

channel_failures=$(grep -c "Channel.*MS-SSIM failed" "$LOG_FILE")
echo "单通道MS-SSIM失败次数: $channel_failures 次" >> "$REPORT_FILE"
echo "" >> "$REPORT_FILE"

# 3. 质量验证失败 - SSIM低于阈值
echo "3. 质量验证失败分析:" >> "$REPORT_FILE"
echo "----------------------------------------" >> "$REPORT_FILE"
quality_failures=$(grep -c "Quality validation FAILED" "$LOG_FILE")
echo "质量验证失败次数: $quality_failures 次" >> "$REPORT_FILE"

protected_files=$(grep -c "Original file PROTECTED" "$LOG_FILE")
echo "原文件保护次数: $protected_files 次" >> "$REPORT_FILE"
echo "" >> "$REPORT_FILE"

# 4. 压缩失败 - 输出大于输入
echo "4. 压缩失败分析:" >> "$REPORT_FILE"
echo "----------------------------------------" >> "$REPORT_FILE"
compression_failures=$(grep -c "Cannot compress even at max CRF" "$LOG_FILE")
echo "无法压缩文件数: $compression_failures 次" >> "$REPORT_FILE"

skipped_larger=$(grep -c "output larger than input" "$LOG_FILE")
echo "输出大于输入跳过: $skipped_larger 次" >> "$REPORT_FILE"
echo "" >> "$REPORT_FILE"

# 5. 心跳重复警告
echo "5. 心跳系统警告:" >> "$REPORT_FILE"
echo "----------------------------------------" >> "$REPORT_FILE"
heartbeat_warnings=$(grep -c "Multiple heartbeats with same name" "$LOG_FILE")
echo "心跳重复警告: $heartbeat_warnings 次" >> "$REPORT_FILE"
echo "" >> "$REPORT_FILE"

# 6. HEIC分析失败
echo "6. HEIC文件分析失败:" >> "$REPORT_FILE"
echo "----------------------------------------" >> "$REPORT_FILE"
heic_failures=$(grep -c "Deep HEIC analysis failed" "$LOG_FILE")
echo "HEIC分析失败: $heic_failures 次" >> "$REPORT_FILE"

if [ $heic_failures -gt 0 ]; then
    echo "HEIC错误详情:" >> "$REPORT_FILE"
    grep -A1 "Deep HEIC analysis failed" "$LOG_FILE" >> "$REPORT_FILE"
fi
echo "" >> "$REPORT_FILE"

# 7. 统计信息分析
echo "7. 转换统计分析:" >> "$REPORT_FILE"
echo "----------------------------------------" >> "$REPORT_FILE"
total_files=$(grep -o "Total Files: [0-9]*" "$LOG_FILE" | head -1 | grep -o "[0-9]*")
image_files=$(grep -o "Images:.*[0-9]*" "$LOG_FILE" | head -1 | grep -o "[0-9]*")
video_files=$(grep -o "Videos:.*[0-9]*" "$LOG_FILE" | head -1 | grep -o "[0-9]*")

echo "总文件数: $total_files" >> "$REPORT_FILE"
echo "图片文件: $image_files" >> "$REPORT_FILE"
echo "视频文件: $video_files" >> "$REPORT_FILE"
echo "" >> "$REPORT_FILE"

# 8. 成功转换分析
echo "8. 成功转换分析:" >> "$REPORT_FILE"
echo "----------------------------------------" >> "$REPORT_FILE"
successful_conversions=$(grep -c "✅ RESULT.*Size.*%" "$LOG_FILE")
echo "成功转换次数: $successful_conversions 次" >> "$REPORT_FILE"

# 分析压缩率
echo "压缩率分布:" >> "$REPORT_FILE"
grep "✅ RESULT.*Size.*%" "$LOG_FILE" | grep -o "Size [+-][0-9.]*%" | sort | uniq -c >> "$REPORT_FILE"
echo "" >> "$REPORT_FILE"

# 9. 错误模式总结
echo "9. 错误模式总结:" >> "$REPORT_FILE"
echo "----------------------------------------" >> "$REPORT_FILE"
echo "主要问题:" >> "$REPORT_FILE"
echo "- GIF文件像素格式不兼容: $gif_errors 次" >> "$REPORT_FILE"
echo "- MS-SSIM质量计算失败: $msssim_failures 次" >> "$REPORT_FILE"
echo "- 质量验证失败保护原文件: $quality_failures 次" >> "$REPORT_FILE"
echo "- 无法压缩的文件: $compression_failures 次" >> "$REPORT_FILE"
echo "- HEIC分析失败: $heic_failures 次" >> "$REPORT_FILE"
echo "- 心跳重复警告: $heartbeat_warnings 次" >> "$REPORT_FILE"
echo "" >> "$REPORT_FILE"

# 10. 建议修复措施
echo "10. 建议修复措施:" >> "$REPORT_FILE"
echo "----------------------------------------" >> "$REPORT_FILE"
echo "1. GIF像素格式问题:" >> "$REPORT_FILE"
echo "   - 已修复: 在video_explorer.rs和msssim_parallel.rs中添加GIF格式检测" >> "$REPORT_FILE"
echo "   - 建议: 为GIF文件使用替代质量指标" >> "$REPORT_FILE"
echo "" >> "$REPORT_FILE"

echo "2. MS-SSIM计算失败:" >> "$REPORT_FILE"
echo "   - 原因: libvmaf不可用或像素格式不兼容" >> "$REPORT_FILE"
echo "   - 建议: 改进fallback机制，使用更可靠的质量指标" >> "$REPORT_FILE"
echo "" >> "$REPORT_FILE"

echo "3. 质量验证过于严格:" >> "$REPORT_FILE"
echo "   - 问题: SSIM阈值0.95可能过高" >> "$REPORT_FILE"
echo "   - 建议: 根据文件类型调整质量阈值" >> "$REPORT_FILE"
echo "" >> "$REPORT_FILE"

echo "4. 心跳重复警告:" >> "$REPORT_FILE"
echo "   - 问题: x265 CLI编码时出现重复心跳" >> "$REPORT_FILE"
echo "   - 建议: 改进心跳管理机制" >> "$REPORT_FILE"
echo "" >> "$REPORT_FILE"

echo "5. HEIC内存限制:" >> "$REPORT_FILE"
echo "   - 问题: SecurityLimitExceeded错误" >> "$REPORT_FILE"
echo "   - 建议: 增加HEIC解析的内存限制或使用替代解析器" >> "$REPORT_FILE"
echo "" >> "$REPORT_FILE"

echo "========================================" >> "$REPORT_FILE"
echo "分析完成时间: $(date)" >> "$REPORT_FILE"

echo "✅ 深度日志分析完成"
echo "📊 报告已保存到: $REPORT_FILE"
echo ""
echo "🔍 主要发现:"
echo "- GIF像素格式错误: $gif_errors 次"
echo "- MS-SSIM计算失败: $msssim_failures 次" 
echo "- 质量验证失败: $quality_failures 次"
echo "- 压缩失败: $compression_failures 次"
echo "- HEIC分析失败: $heic_failures 次"
echo "- 心跳重复警告: $heartbeat_warnings 次"