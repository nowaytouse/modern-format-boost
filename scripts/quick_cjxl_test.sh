#!/bin/bash

# 🔧 快速CJXL修复验证
# 直接测试修复后的代码

set -e

echo "🔧 快速CJXL修复验证"
echo "========================"

# 创建测试目录
TEST_DIR="quick_cjxl_test"
mkdir -p "$TEST_DIR"

# 使用大图片测试
TEST_IMAGE="test_media/very_large_test.png"
if [[ ! -f "$TEST_IMAGE" ]]; then
    echo "❌ 测试图片不存在: $TEST_IMAGE"
    exit 1
fi

# 创建副本
IMAGE_COPY="$TEST_DIR/test_copy.png"
cp "$TEST_IMAGE" "$IMAGE_COPY"
echo "✅ 创建测试副本: $IMAGE_COPY"

# 测试直接CJXL (应该失败)
echo ""
echo "🔧 测试直接CJXL编码..."
if cjxl "$IMAGE_COPY" "$TEST_DIR/direct.jxl" 2>&1 | tee "$TEST_DIR/direct.log"; then
    echo "✅ 直接CJXL成功"
else
    echo "❌ 直接CJXL失败 (预期)"
    if grep -q "Getting pixel data failed" "$TEST_DIR/direct.log"; then
        echo "🎯 确认错误: Getting pixel data failed"
    fi
fi

# 测试FFmpeg fallback
echo ""
echo "🔄 测试FFmpeg fallback..."
if ffmpeg -i "$IMAGE_COPY" -f png -pix_fmt rgba - 2>/dev/null | \
   cjxl - "$TEST_DIR/ffmpeg.jxl" -d 1.0 -e 7 -j 2 2>&1 | tee "$TEST_DIR/ffmpeg.log"; then
    echo "✅ FFmpeg fallback成功"
    if [[ -f "$TEST_DIR/ffmpeg.jxl" ]]; then
        SIZE=$(stat -f%z "$TEST_DIR/ffmpeg.jxl" 2>/dev/null || stat -c%s "$TEST_DIR/ffmpeg.jxl" 2>/dev/null)
        echo "   输出大小: $SIZE bytes"
    fi
else
    echo "❌ FFmpeg fallback失败"
fi

# 测试实际程序
echo ""
echo "🧪 测试修复后的程序..."
mkdir -p "$TEST_DIR/output"

if ./target/release/imgquality-hevc auto \
    "$IMAGE_COPY" \
    --output "$TEST_DIR/output" \
    --verbose 2>&1 | tee "$TEST_DIR/program.log"; then
    echo "✅ 程序转换成功"
    
    # 检查输出
    OUTPUT_FILE="$TEST_DIR/output/test_copy.jxl"
    if [[ -f "$OUTPUT_FILE" ]]; then
        SIZE=$(stat -f%z "$OUTPUT_FILE" 2>/dev/null || stat -c%s "$OUTPUT_FILE" 2>/dev/null)
        echo "   输出文件: $OUTPUT_FILE"
        echo "   输出大小: $SIZE bytes"
        
        # 检查是否使用了fallback
        if grep -q "FALLBACK" "$TEST_DIR/program.log"; then
            echo "🎯 检测到fallback机制被触发"
            if grep -q "FFmpeg" "$TEST_DIR/program.log"; then
                echo "   ✅ 使用了FFmpeg fallback"
            fi
        fi
    fi
else
    echo "❌ 程序转换失败"
fi

# 清理
rm -rf "$TEST_DIR"
echo ""
echo "✅ 测试完成"