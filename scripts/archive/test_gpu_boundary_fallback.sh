#!/bin/bash
# 测试 GPU boundary 验证失败时的 CPU fallback 机制

set -e

echo "🧪 Testing GPU Boundary Fallback Mechanism"
echo "=========================================="
echo ""

# 测试文件
TEST_GIF="/Users/nyamiiko/Downloads/1/参考/内容 猎奇/4h8uh4vkss9clo2wfiy30kach.gif"
OUTPUT_DIR="/tmp/test_gpu_fallback"
mkdir -p "$OUTPUT_DIR"

if [ ! -f "$TEST_GIF" ]; then
    echo "❌ Test file not found: $TEST_GIF"
    exit 1
fi

echo "📁 Input: $TEST_GIF"
echo "📂 Output: $OUTPUT_DIR"
echo ""

# 运行转换
echo "🔄 Running conversion..."
./target/release/vidquality-hevc \
    --input "$TEST_GIF" \
    --output "$OUTPUT_DIR/output.mp4" \
    --explore \
    --match-quality \
    --compress \
    2>&1 | tee "$OUTPUT_DIR/test.log"

echo ""
echo "📊 Checking results..."

# 检查是否有 GPU boundary 失败的日志
if grep -q "GPU boundary verification failed" "$OUTPUT_DIR/test.log"; then
    echo "✅ GPU boundary failure detected"
    
    # 检查是否触发了 CPU fallback
    if grep -q "Retrying with CPU encoding" "$OUTPUT_DIR/test.log"; then
        echo "✅ CPU fallback triggered"
        
        # 检查 CPU 编码是否成功
        if grep -q "CPU encoding succeeded" "$OUTPUT_DIR/test.log"; then
            echo "✅ CPU encoding succeeded"
        else
            echo "❌ CPU encoding failed"
            exit 1
        fi
    else
        echo "❌ CPU fallback NOT triggered"
        exit 1
    fi
else
    echo "ℹ️  No GPU boundary failure (test may have passed without fallback)"
fi

# 检查输出文件
if [ -f "$OUTPUT_DIR/output.mp4" ]; then
    SIZE=$(stat -f%z "$OUTPUT_DIR/output.mp4")
    echo "✅ Output file created: $SIZE bytes"
else
    echo "❌ Output file not created"
    exit 1
fi

echo ""
echo "✅ All tests passed!"
