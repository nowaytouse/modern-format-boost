#!/bin/bash

# 🔍 快速测试666日志发现的新BUG
# 使用现有测试文件进行验证

set -e

echo "🔍 快速测试666日志新BUG"
echo "========================================"

# 创建测试目录
TEST_DIR="quick_bug_test"
mkdir -p "$TEST_DIR"

echo ""
echo "🎬 1. 测试CPU x265编码问题"
echo "----------------------------------------"

# 使用现有测试视频
TEST_VIDEO="test_media/short_test.mp4"
if [[ -f "$TEST_VIDEO" ]]; then
    echo "   使用测试视频: $TEST_VIDEO"
    
    # 创建副本
    VIDEO_COPY="$TEST_DIR/test_video_copy.mp4"
    cp "$TEST_VIDEO" "$VIDEO_COPY"
    echo "   创建副本: $VIDEO_COPY"
    
    # 测试x265编码 (CRF 20)
    echo "   测试x265编码 CRF 20..."
    if timeout 30s ffmpeg -i "$VIDEO_COPY" -c:v libx265 -crf 20 -preset fast -y "$TEST_DIR/x265_crf20_output.mp4" 2>&1 | tee "$TEST_DIR/x265_crf20.log"; then
        echo "   ✅ x265 CRF 20编码成功"
    else
        echo "   ❌ x265 CRF 20编码失败"
    fi
    
    # 测试x265编码 (CRF 18)
    echo "   测试x265编码 CRF 18..."
    if timeout 30s ffmpeg -i "$VIDEO_COPY" -c:v libx265 -crf 18 -preset fast -y "$TEST_DIR/x265_crf18_output.mp4" 2>&1 | tee "$TEST_DIR/x265_crf18.log"; then
        echo "   ✅ x265 CRF 18编码成功"
    else
        echo "   ❌ x265 CRF 18编码失败"
    fi
    
    # 测试x265编码 (CRF 22)
    echo "   测试x265编码 CRF 22..."
    if timeout 30s ffmpeg -i "$VIDEO_COPY" -c:v libx265 -crf 22 -preset fast -y "$TEST_DIR/x265_crf22_output.mp4" 2>&1 | tee "$TEST_DIR/x265_crf22.log"; then
        echo "   ✅ x265 CRF 22编码成功"
    else
        echo "   ❌ x265 CRF 22编码失败"
    fi
else
    echo "   ❌ 测试视频文件不存在: $TEST_VIDEO"
fi

echo ""
echo "🖼️  2. 测试CJXL编码器兼容性"
echo "----------------------------------------"

# 检查CJXL版本
if command -v cjxl >/dev/null 2>&1; then
    CJXL_VERSION=$(cjxl --version 2>&1 | head -1)
    echo "   CJXL版本: $CJXL_VERSION"
    
    # 检查是否是有问题的v0.11.1版本
    if echo "$CJXL_VERSION" | grep -q "v0.11.1"; then
        echo "   ⚠️  检测到v0.11.1版本 - 666日志中报告的问题版本"
    fi
    
    # 使用现有测试图片
    TEST_IMAGE="test_media/test_image.png"
    if [[ -f "$TEST_IMAGE" ]]; then
        echo "   使用测试图片: $TEST_IMAGE"
        
        # 创建副本
        IMAGE_COPY="$TEST_DIR/test_image_copy.png"
        cp "$TEST_IMAGE" "$IMAGE_COPY"
        echo "   创建副本: $IMAGE_COPY"
        
        # 测试CJXL编码 (默认参数)
        echo "   测试CJXL编码 (默认参数)..."
        if timeout 15s cjxl "$IMAGE_COPY" "$TEST_DIR/cjxl_default_output.jxl" 2>&1 | tee "$TEST_DIR/cjxl_default.log"; then
            echo "   ✅ CJXL默认参数编码成功"
            if [[ -f "$TEST_DIR/cjxl_default_output.jxl" ]]; then
                OUTPUT_SIZE=$(stat -f%z "$TEST_DIR/cjxl_default_output.jxl" 2>/dev/null || stat -c%s "$TEST_DIR/cjxl_default_output.jxl" 2>/dev/null || echo "0")
                echo "      输出文件大小: $OUTPUT_SIZE bytes"
            fi
        else
            echo "   ❌ CJXL默认参数编码失败"
        fi
        
        # 测试CJXL编码 (无损模式)
        echo "   测试CJXL编码 (无损模式)..."
        if timeout 15s cjxl "$IMAGE_COPY" "$TEST_DIR/cjxl_lossless_output.jxl" -d 0 2>&1 | tee "$TEST_DIR/cjxl_lossless.log"; then
            echo "   ✅ CJXL无损模式编码成功"
            if [[ -f "$TEST_DIR/cjxl_lossless_output.jxl" ]]; then
                OUTPUT_SIZE=$(stat -f%z "$TEST_DIR/cjxl_lossless_output.jxl" 2>/dev/null || stat -c%s "$TEST_DIR/cjxl_lossless_output.jxl" 2>/dev/null || echo "0")
                echo "      输出文件大小: $OUTPUT_SIZE bytes"
            fi
        else
            echo "   ❌ CJXL无损模式编码失败"
        fi
        
        # 测试更大的图片
        LARGE_IMAGE="test_media/test_large.png"
        if [[ -f "$LARGE_IMAGE" ]]; then
            echo "   测试大图片CJXL编码..."
            LARGE_COPY="$TEST_DIR/test_large_copy.png"
            cp "$LARGE_IMAGE" "$LARGE_COPY"
            
            if timeout 30s cjxl "$LARGE_COPY" "$TEST_DIR/cjxl_large_output.jxl" 2>&1 | tee "$TEST_DIR/cjxl_large.log"; then
                echo "   ✅ 大图片CJXL编码成功"
            else
                echo "   ❌ 大图片CJXL编码失败"
            fi
        fi
    else
        echo "   ❌ 测试图片文件不存在: $TEST_IMAGE"
    fi
else
    echo "   ❌ CJXL未安装或不在PATH中"
fi

echo ""
echo "📊 3. 测试结果分析"
echo "----------------------------------------"

ISSUES_FOUND=0
X265_ISSUES=0
CJXL_ISSUES=0

# 分析x265测试结果
echo "   x265编码结果:"
for crf in 20 18 22; do
    LOG_FILE="$TEST_DIR/x265_crf${crf}.log"
    if [[ -f "$LOG_FILE" ]]; then
        if grep -q "error\|failed\|Error" "$LOG_FILE"; then
            echo "     ❌ CRF $crf: 编码失败"
            ((X265_ISSUES++))
        else
            echo "     ✅ CRF $crf: 编码成功"
        fi
    fi
done

if [[ $X265_ISSUES -gt 0 ]]; then
    echo "   ⚠️  发现 $X265_ISSUES 个x265编码问题"
    ((ISSUES_FOUND++))
fi

# 分析CJXL测试结果
echo ""
echo "   CJXL编码结果:"
for mode in default lossless large; do
    LOG_FILE="$TEST_DIR/cjxl_${mode}.log"
    if [[ -f "$LOG_FILE" ]]; then
        if grep -q "error\|failed\|Error" "$LOG_FILE"; then
            echo "     ❌ $mode模式: 编码失败"
            ((CJXL_ISSUES++))
        else
            echo "     ✅ $mode模式: 编码成功"
        fi
    fi
done

if [[ $CJXL_ISSUES -gt 0 ]]; then
    echo "   ⚠️  发现 $CJXL_ISSUES 个CJXL编码问题"
    ((ISSUES_FOUND++))
fi

echo ""
echo "🎯 4. 修复建议"
echo "----------------------------------------"

if [[ $ISSUES_FOUND -eq 0 ]]; then
    echo "✅ 当前环境未复现666日志中的BUG"
    echo "   可能原因:"
    echo "   1. 系统环境已更新，BUG已修复"
    echo "   2. 测试文件与实际问题文件不同"
    echo "   3. 问题与特定文件格式或内容相关"
else
    echo "❌ 发现 $ISSUES_FOUND 类问题，需要修复:"
    
    if [[ $X265_ISSUES -gt 0 ]]; then
        echo ""
        echo "🎬 x265编码问题修复建议:"
        echo "   1. 检查FFmpeg输入解码器兼容性"
        echo "   2. 在x265_encoder.rs中添加解码失败处理"
        echo "   3. 实现备用编码参数或格式转换"
        echo "   4. 添加详细的错误日志记录"
    fi
    
    if [[ $CJXL_ISSUES -gt 0 ]]; then
        echo ""
        echo "🖼️  CJXL编码问题修复建议:"
        echo "   1. 升级CJXL到更稳定版本"
        echo "   2. 在lossless_converter.rs中添加版本兼容性检查"
        echo "   3. 实现CJXL编码失败时的fallback机制"
        echo "   4. 优化编码参数以提高成功率"
    fi
fi

echo ""
echo "🧹 5. 清理测试文件"
echo "----------------------------------------"

# 清理测试文件
if [[ -d "$TEST_DIR" ]]; then
    rm -rf "$TEST_DIR"
    echo "✅ 测试文件已清理"
fi

echo ""
echo "✅ 666日志新BUG快速测试完成"
echo ""
echo "📋 总结:"
if [[ $ISSUES_FOUND -eq 0 ]]; then
    echo "   当前环境未复现666日志中报告的BUG"
    echo "   建议在实际使用环境中进一步验证"
else
    echo "   发现问题需要在代码中实现相应修复"
    echo "   建议创建针对性的错误处理机制"
fi