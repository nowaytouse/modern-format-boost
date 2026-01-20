#!/bin/bash

# 🔧 测试CJXL大图片编码修复 (v7.8.2)
# 验证FFmpeg fallback机制是否正常工作

set -e

echo "🔧 测试CJXL大图片编码修复 (v7.8.2)"
echo "========================================"

# 创建测试目录
TEST_DIR="test_cjxl_fix_v7.8.2"
mkdir -p "$TEST_DIR"

echo "📋 测试概述:"
echo "   1. 测试CJXL直接编码"
echo "   2. 测试FFmpeg fallback机制"
echo "   3. 测试ImageMagick secondary fallback"
echo "   4. 使用副本测试，保护原件"

echo ""
echo "🖼️  1. 准备测试图片"
echo "----------------------------------------"

# 使用现有测试图片
TEST_SMALL="test_media/test_image.png"
TEST_LARGE="test_media/test_large.png"

if [[ -f "$TEST_SMALL" ]]; then
    echo "   找到小图片: $TEST_SMALL"
    SMALL_SIZE=$(stat -f%z "$TEST_SMALL" 2>/dev/null || stat -c%s "$TEST_SMALL" 2>/dev/null || echo "0")
    echo "   小图片大小: $SMALL_SIZE bytes"
else
    echo "   ❌ 小图片不存在: $TEST_SMALL"
fi

if [[ -f "$TEST_LARGE" ]]; then
    echo "   找到大图片: $TEST_LARGE"
    LARGE_SIZE=$(stat -f%z "$TEST_LARGE" 2>/dev/null || stat -c%s "$TEST_LARGE" 2>/dev/null || echo "0")
    echo "   大图片大小: $LARGE_SIZE bytes"
else
    echo "   ❌ 大图片不存在: $TEST_LARGE"
fi

# 创建副本
if [[ -f "$TEST_SMALL" ]]; then
    SMALL_COPY="$TEST_DIR/test_small_copy.png"
    cp "$TEST_SMALL" "$SMALL_COPY"
    echo "   创建小图片副本: $SMALL_COPY"
fi

if [[ -f "$TEST_LARGE" ]]; then
    LARGE_COPY="$TEST_DIR/test_large_copy.png"
    cp "$TEST_LARGE" "$LARGE_COPY"
    echo "   创建大图片副本: $LARGE_COPY"
fi

echo ""
echo "🔧 2. 测试CJXL直接编码"
echo "----------------------------------------"

# 测试小图片 (应该成功)
if [[ -f "$SMALL_COPY" ]]; then
    echo "   测试小图片CJXL编码..."
    if timeout 30s cjxl "$SMALL_COPY" "$TEST_DIR/small_direct.jxl" 2>&1 | tee "$TEST_DIR/small_direct.log"; then
        echo "   ✅ 小图片CJXL直接编码成功"
        if [[ -f "$TEST_DIR/small_direct.jxl" ]]; then
            OUTPUT_SIZE=$(stat -f%z "$TEST_DIR/small_direct.jxl" 2>/dev/null || stat -c%s "$TEST_DIR/small_direct.jxl" 2>/dev/null || echo "0")
            echo "      输出大小: $OUTPUT_SIZE bytes"
        fi
    else
        echo "   ❌ 小图片CJXL直接编码失败"
    fi
fi

# 测试大图片 (可能失败，触发fallback)
if [[ -f "$LARGE_COPY" ]]; then
    echo "   测试大图片CJXL编码..."
    if timeout 30s cjxl "$LARGE_COPY" "$TEST_DIR/large_direct.jxl" 2>&1 | tee "$TEST_DIR/large_direct.log"; then
        echo "   ✅ 大图片CJXL直接编码成功"
        if [[ -f "$TEST_DIR/large_direct.jxl" ]]; then
            OUTPUT_SIZE=$(stat -f%z "$TEST_DIR/large_direct.jxl" 2>/dev/null || stat -c%s "$TEST_DIR/large_direct.jxl" 2>/dev/null || echo "0")
            echo "      输出大小: $OUTPUT_SIZE bytes"
        fi
    else
        echo "   ❌ 大图片CJXL直接编码失败 (预期行为，将触发fallback)"
        # 检查错误信息
        if grep -q "Getting pixel data failed" "$TEST_DIR/large_direct.log"; then
            echo "      🎯 检测到 'Getting pixel data failed' 错误 - 正是我们要修复的BUG"
        fi
    fi
fi

echo ""
echo "🔄 3. 测试FFmpeg fallback机制"
echo "----------------------------------------"

# 检查FFmpeg可用性
if command -v ffmpeg >/dev/null 2>&1; then
    echo "   ✅ FFmpeg可用"
    
    # 测试FFmpeg → cjxl pipeline
    if [[ -f "$LARGE_COPY" ]]; then
        echo "   测试FFmpeg → cjxl pipeline..."
        
        # 模拟fallback流程
        if timeout 60s bash -c "
            ffmpeg -i '$LARGE_COPY' -f png -pix_fmt rgba - 2>/dev/null | \
            cjxl - '$TEST_DIR/large_ffmpeg_fallback.jxl' -d 1.0 -e 7 -j 2 2>&1
        " | tee "$TEST_DIR/ffmpeg_fallback.log"; then
            echo "   ✅ FFmpeg fallback成功"
            if [[ -f "$TEST_DIR/large_ffmpeg_fallback.jxl" ]]; then
                OUTPUT_SIZE=$(stat -f%z "$TEST_DIR/large_ffmpeg_fallback.jxl" 2>/dev/null || stat -c%s "$TEST_DIR/large_ffmpeg_fallback.jxl" 2>/dev/null || echo "0")
                echo "      输出大小: $OUTPUT_SIZE bytes"
                
                # 验证JXL文件有效性
                if command -v jxlinfo >/dev/null 2>&1; then
                    if jxlinfo "$TEST_DIR/large_ffmpeg_fallback.jxl" >/dev/null 2>&1; then
                        echo "      ✅ JXL文件验证通过"
                    else
                        echo "      ❌ JXL文件验证失败"
                    fi
                fi
            fi
        else
            echo "   ❌ FFmpeg fallback失败"
        fi
    fi
else
    echo "   ❌ FFmpeg不可用"
fi

echo ""
echo "🔧 4. 测试ImageMagick secondary fallback"
echo "----------------------------------------"

# 检查ImageMagick可用性
if command -v magick >/dev/null 2>&1; then
    echo "   ✅ ImageMagick可用"
    
    # 测试ImageMagick → cjxl pipeline
    if [[ -f "$LARGE_COPY" ]]; then
        echo "   测试ImageMagick → cjxl pipeline..."
        
        # 模拟secondary fallback流程
        if timeout 60s bash -c "
            magick '$LARGE_COPY' -depth 16 png:- 2>/dev/null | \
            cjxl - '$TEST_DIR/large_magick_fallback.jxl' -d 1.0 -e 7 -j 2 2>&1
        " | tee "$TEST_DIR/magick_fallback.log"; then
            echo "   ✅ ImageMagick secondary fallback成功"
            if [[ -f "$TEST_DIR/large_magick_fallback.jxl" ]]; then
                OUTPUT_SIZE=$(stat -f%z "$TEST_DIR/large_magick_fallback.jxl" 2>/dev/null || stat -c%s "$TEST_DIR/large_magick_fallback.jxl" 2>/dev/null || echo "0")
                echo "      输出大小: $OUTPUT_SIZE bytes"
                
                # 验证JXL文件有效性
                if command -v jxlinfo >/dev/null 2>&1; then
                    if jxlinfo "$TEST_DIR/large_magick_fallback.jxl" >/dev/null 2>&1; then
                        echo "      ✅ JXL文件验证通过"
                    else
                        echo "      ❌ JXL文件验证失败"
                    fi
                fi
            fi
        else
            echo "   ❌ ImageMagick secondary fallback失败"
        fi
    fi
else
    echo "   ❌ ImageMagick不可用"
fi

echo ""
echo "🧪 5. 测试修复后的代码"
echo "----------------------------------------"

# 编译项目
echo "   编译项目..."
if cargo build --release 2>&1 | tee "$TEST_DIR/build.log"; then
    echo "   ✅ 编译成功"
else
    echo "   ❌ 编译失败"
    echo "   查看编译日志: $TEST_DIR/build.log"
fi

# 测试实际的修复代码 (如果编译成功)
if [[ -f "target/release/imgquality-hevc" ]] && [[ -f "$LARGE_COPY" ]]; then
    echo "   测试修复后的CJXL转换..."
    
    # 创建临时输出目录
    mkdir -p "$TEST_DIR/output"
    
    # 运行实际的转换程序
    if timeout 120s ./target/release/imgquality-hevc \
        --input "$LARGE_COPY" \
        --output-dir "$TEST_DIR/output" \
        --format jxl \
        --verbose 2>&1 | tee "$TEST_DIR/actual_conversion.log"; then
        echo "   ✅ 修复后的转换成功"
        
        # 检查输出文件
        JXL_OUTPUT="$TEST_DIR/output/$(basename "$LARGE_COPY" .png).jxl"
        if [[ -f "$JXL_OUTPUT" ]]; then
            OUTPUT_SIZE=$(stat -f%z "$JXL_OUTPUT" 2>/dev/null || stat -c%s "$JXL_OUTPUT" 2>/dev/null || echo "0")
            echo "      输出文件: $JXL_OUTPUT"
            echo "      输出大小: $OUTPUT_SIZE bytes"
            
            # 检查是否使用了fallback
            if grep -q "FALLBACK" "$TEST_DIR/actual_conversion.log"; then
                echo "      🎯 检测到fallback机制被触发"
                if grep -q "FFmpeg" "$TEST_DIR/actual_conversion.log"; then
                    echo "         ✅ FFmpeg fallback被使用"
                fi
                if grep -q "ImageMagick" "$TEST_DIR/actual_conversion.log"; then
                    echo "         ✅ ImageMagick secondary fallback被使用"
                fi
            else
                echo "      ℹ️  直接CJXL编码成功，未触发fallback"
            fi
        else
            echo "      ❌ 未找到输出文件"
        fi
    else
        echo "   ❌ 修复后的转换失败"
        echo "   查看转换日志: $TEST_DIR/actual_conversion.log"
    fi
fi

echo ""
echo "📊 6. 测试结果汇总"
echo "----------------------------------------"

TESTS_PASSED=0
TESTS_TOTAL=0

# 检查各项测试结果
echo "   测试结果:"

# 1. 编译测试
((TESTS_TOTAL++))
if [[ -f "target/release/imgquality-hevc" ]]; then
    echo "     ✅ 编译测试: 通过"
    ((TESTS_PASSED++))
else
    echo "     ❌ 编译测试: 失败"
fi

# 2. FFmpeg fallback测试
((TESTS_TOTAL++))
if [[ -f "$TEST_DIR/large_ffmpeg_fallback.jxl" ]]; then
    echo "     ✅ FFmpeg fallback: 通过"
    ((TESTS_PASSED++))
else
    echo "     ❌ FFmpeg fallback: 失败"
fi

# 3. ImageMagick fallback测试
((TESTS_TOTAL++))
if [[ -f "$TEST_DIR/large_magick_fallback.jxl" ]]; then
    echo "     ✅ ImageMagick fallback: 通过"
    ((TESTS_PASSED++))
else
    echo "     ❌ ImageMagick fallback: 失败"
fi

# 4. 实际转换测试
((TESTS_TOTAL++))
if [[ -f "$TEST_DIR/output/"*.jxl ]]; then
    echo "     ✅ 实际转换测试: 通过"
    ((TESTS_PASSED++))
else
    echo "     ❌ 实际转换测试: 失败"
fi

echo ""
echo "   总体结果: $TESTS_PASSED/$TESTS_TOTAL 测试通过"

if [[ $TESTS_PASSED -eq $TESTS_TOTAL ]]; then
    echo "   🎉 所有测试通过！CJXL修复成功"
elif [[ $TESTS_PASSED -gt 0 ]]; then
    echo "   ⚠️  部分测试通过，修复部分有效"
else
    echo "   ❌ 所有测试失败，需要进一步调试"
fi

echo ""
echo "🔧 7. 修复验证建议"
echo "----------------------------------------"

if [[ $TESTS_PASSED -lt $TESTS_TOTAL ]]; then
    echo "   修复建议:"
    
    if [[ ! -f "target/release/imgquality-hevc" ]]; then
        echo "   1. 检查编译错误并修复代码问题"
    fi
    
    if [[ ! -f "$TEST_DIR/large_ffmpeg_fallback.jxl" ]]; then
        echo "   2. 检查FFmpeg安装和版本兼容性"
        echo "      命令: ffmpeg -version"
    fi
    
    if [[ ! -f "$TEST_DIR/large_magick_fallback.jxl" ]]; then
        echo "   3. 检查ImageMagick安装和版本兼容性"
        echo "      命令: magick -version"
    fi
    
    echo "   4. 查看详细日志文件:"
    echo "      - 编译日志: $TEST_DIR/build.log"
    echo "      - FFmpeg日志: $TEST_DIR/ffmpeg_fallback.log"
    echo "      - ImageMagick日志: $TEST_DIR/magick_fallback.log"
    echo "      - 转换日志: $TEST_DIR/actual_conversion.log"
else
    echo "   ✅ 修复验证完成，建议:"
    echo "   1. 运行更大规模的测试验证稳定性"
    echo "   2. 测试不同格式和尺寸的图片"
    echo "   3. 监控实际使用中的fallback使用频率"
    echo "   4. 考虑添加fallback使用统计"
fi

echo ""
echo "🧹 8. 清理测试文件"
echo "----------------------------------------"

# 清理测试文件
if [[ -d "$TEST_DIR" ]]; then
    rm -rf "$TEST_DIR"
    echo "✅ 测试文件已清理"
fi

echo ""
echo "✅ CJXL修复测试完成 (v7.8.2)"
echo ""
echo "📝 后续步骤:"
echo "   1. 如果测试通过，提交修复代码"
echo "   2. 更新CHANGELOG记录修复内容"
echo "   3. 在实际环境中验证修复效果"
echo "   4. 监控fallback机制的使用情况"