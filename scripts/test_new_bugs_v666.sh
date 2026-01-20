#!/bin/bash

# 🔍 测试666日志中发现的新BUG
# 安全副本测试，验证x265编码和CJXL兼容性问题

set -e

echo "🔍 测试666日志发现的新BUG"
echo "========================================"

# 创建测试目录
TEST_DIR="test_new_bugs_v666"
mkdir -p "$TEST_DIR"

echo "📋 测试概述:"
echo "   1. CPU x265编码失败问题"
echo "   2. CJXL编码器兼容性问题"
echo "   3. 使用副本测试，保护原件"

echo ""
echo "🎬 1. 测试CPU x265编码问题"
echo "----------------------------------------"

# 查找测试视频文件
TEST_VIDEO=""
for ext in mp4 avi mov mkv; do
    if find test_media -name "*.$ext" -type f | head -1 | read video_file; then
        TEST_VIDEO="$video_file"
        break
    fi
done

if [[ -n "$TEST_VIDEO" ]]; then
    echo "   找到测试视频: $(basename "$TEST_VIDEO")"
    
    # 创建副本
    VIDEO_COPY="$TEST_DIR/test_video_copy.$(basename "$TEST_VIDEO" | cut -d. -f2-)"
    cp "$TEST_VIDEO" "$VIDEO_COPY"
    echo "   创建副本: $VIDEO_COPY"
    
    # 测试x265编码
    echo "   测试x265编码..."
    if timeout 60s ffmpeg -i "$VIDEO_COPY" -c:v libx265 -crf 20 -y "$TEST_DIR/x265_test_output.mp4" 2>&1 | tee "$TEST_DIR/x265_test.log"; then
        echo "   ✅ x265编码测试通过"
    else
        echo "   ❌ x265编码失败 - 需要修复"
        echo "   错误日志保存在: $TEST_DIR/x265_test.log"
    fi
else
    echo "   ⚠️  未找到测试视频文件，跳过x265测试"
fi

echo ""
echo "🖼️  2. 测试CJXL编码器兼容性"
echo "----------------------------------------"

# 检查CJXL版本
echo "   检查CJXL版本:"
if command -v cjxl >/dev/null 2>&1; then
    CJXL_VERSION=$(cjxl --version 2>&1 || echo "版本检测失败")
    echo "   CJXL版本: $CJXL_VERSION"
    
    # 查找测试图片
    TEST_IMAGE=""
    for ext in jpg jpeg png; do
        if find test_media -name "*.$ext" -type f | head -1 | read image_file; then
            TEST_IMAGE="$image_file"
            break
        fi
    done
    
    if [[ -n "$TEST_IMAGE" ]]; then
        echo "   找到测试图片: $(basename "$TEST_IMAGE")"
        
        # 创建副本
        IMAGE_COPY="$TEST_DIR/test_image_copy.$(basename "$TEST_IMAGE" | cut -d. -f2-)"
        cp "$TEST_IMAGE" "$IMAGE_COPY"
        echo "   创建副本: $IMAGE_COPY"
        
        # 测试CJXL编码
        echo "   测试CJXL编码..."
        if timeout 30s cjxl "$IMAGE_COPY" "$TEST_DIR/cjxl_test_output.jxl" 2>&1 | tee "$TEST_DIR/cjxl_test.log"; then
            echo "   ✅ CJXL编码测试通过"
            
            # 检查输出文件
            if [[ -f "$TEST_DIR/cjxl_test_output.jxl" ]]; then
                OUTPUT_SIZE=$(stat -f%z "$TEST_DIR/cjxl_test_output.jxl" 2>/dev/null || stat -c%s "$TEST_DIR/cjxl_test_output.jxl" 2>/dev/null || echo "0")
                echo "   输出文件大小: $OUTPUT_SIZE bytes"
            fi
        else
            echo "   ❌ CJXL编码失败 - 需要修复"
            echo "   错误日志保存在: $TEST_DIR/cjxl_test.log"
        fi
    else
        echo "   ⚠️  未找到测试图片文件，跳过CJXL测试"
    fi
else
    echo "   ❌ CJXL未安装或不在PATH中"
fi

echo ""
echo "🔧 3. 系统环境检查"
echo "----------------------------------------"

echo "   FFmpeg版本:"
if command -v ffmpeg >/dev/null 2>&1; then
    ffmpeg -version | head -1
    
    # 检查x265编码器支持
    if ffmpeg -encoders 2>/dev/null | grep -q libx265; then
        echo "   ✅ FFmpeg支持libx265编码器"
    else
        echo "   ❌ FFmpeg不支持libx265编码器"
    fi
else
    echo "   ❌ FFmpeg未安装"
fi

echo ""
echo "   系统信息:"
echo "   操作系统: $(uname -s)"
echo "   架构: $(uname -m)"

echo ""
echo "📊 4. 测试结果汇总"
echo "----------------------------------------"

ISSUES_FOUND=0

# 检查x265测试结果
if [[ -f "$TEST_DIR/x265_test.log" ]]; then
    if grep -q "error\|failed\|Error" "$TEST_DIR/x265_test.log"; then
        echo "❌ x265编码存在问题"
        ((ISSUES_FOUND++))
    else
        echo "✅ x265编码正常"
    fi
fi

# 检查CJXL测试结果
if [[ -f "$TEST_DIR/cjxl_test.log" ]]; then
    if grep -q "error\|failed\|Error" "$TEST_DIR/cjxl_test.log"; then
        echo "❌ CJXL编码存在问题"
        ((ISSUES_FOUND++))
    else
        echo "✅ CJXL编码正常"
    fi
fi

echo ""
echo "🎯 修复建议"
echo "----------------------------------------"

if [[ $ISSUES_FOUND -gt 0 ]]; then
    echo "发现 $ISSUES_FOUND 个问题需要修复:"
    
    if [[ -f "$TEST_DIR/x265_test.log" ]] && grep -q "error\|failed" "$TEST_DIR/x265_test.log"; then
        echo ""
        echo "🎬 x265编码问题修复建议:"
        echo "   1. 检查FFmpeg和libx265版本兼容性"
        echo "   2. 在x265_encoder.rs中添加错误处理"
        echo "   3. 实现备用编码器fallback机制"
        echo "   4. 添加输入格式预检查"
    fi
    
    if [[ -f "$TEST_DIR/cjxl_test.log" ]] && grep -q "error\|failed" "$TEST_DIR/cjxl_test.log"; then
        echo ""
        echo "🖼️  CJXL编码问题修复建议:"
        echo "   1. 升级CJXL编码器到稳定版本"
        echo "   2. 在lossless_converter.rs中添加版本检查"
        echo "   3. 实现CJXL编码参数优化"
        echo "   4. 添加编码失败时的fallback机制"
    fi
else
    echo "✅ 未发现明显问题，当前环境可能已修复相关BUG"
fi

echo ""
echo "🧹 清理测试文件"
echo "----------------------------------------"

# 清理测试文件
if [[ -d "$TEST_DIR" ]]; then
    rm -rf "$TEST_DIR"
    echo "✅ 测试文件已清理"
fi

echo ""
echo "✅ 666日志新BUG测试完成"
echo ""
echo "📝 后续步骤:"
echo "   1. 根据测试结果修复发现的问题"
echo "   2. 在相关源码中实现错误处理改进"
echo "   3. 创建针对性的单元测试"
echo "   4. 验证修复效果并更新文档"