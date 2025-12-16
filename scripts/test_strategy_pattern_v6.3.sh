#!/bin/bash
# 🔥 v6.3 Strategy Pattern 实际测试脚本
# 验证重构没有破坏功能
# 使用双击 app 的参数: --explore --match-quality true --compress --apple-compat

set -e

echo "═══════════════════════════════════════════════════════════════"
echo "🔥 Strategy Pattern v6.3 - 实际文件测试"
echo "═══════════════════════════════════════════════════════════════"

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
WORKSPACE_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
# 使用较大的测试视频（1.2MB），避免元数据余量问题
TEST_VIDEO="$WORKSPACE_ROOT/test_videos/test_60s.mp4"

# 每个测试使用独立的输出目录
TMP_BASE="/tmp/strategy_test_v6.3"
rm -rf "$TMP_BASE"

cleanup() { rm -rf "$TMP_BASE"; }
trap cleanup EXIT

echo ""
echo "📁 测试文件: $TEST_VIDEO"

if [ ! -f "$TEST_VIDEO" ]; then
    echo "❌ 测试视频不存在"
    exit 1
fi

CODEC=$(ffprobe -v error -select_streams v:0 -show_entries stream=codec_name -of csv=p=0 "$TEST_VIDEO" 2>/dev/null)
echo "   Codec: $CODEC"
echo ""

echo "🔨 构建 vidquality_hevc..."
cargo build --release --manifest-path "$WORKSPACE_ROOT/vidquality_hevc/Cargo.toml" 2>&1 | tail -3
VIDQUALITY="$WORKSPACE_ROOT/vidquality_hevc/target/release/vidquality-hevc"
[ ! -f "$VIDQUALITY" ] && echo "❌ 构建失败" && exit 1
echo "✅ 构建成功"
echo ""

INPUT_SIZE=$(stat -f%z "$TEST_VIDEO" 2>/dev/null || stat -c%s "$TEST_VIDEO")
echo "📊 输入文件大小: $INPUT_SIZE bytes"
echo ""

# 测试 1: 双击 app 默认参数
echo "═══════════════════════════════════════════════════════════════"
echo "📊 测试 1: --explore --match-quality true --compress --apple-compat"
echo "═══════════════════════════════════════════════════════════════"
TMP1="$TMP_BASE/test1"
mkdir -p "$TMP1"
$VIDQUALITY auto "$TEST_VIDEO" --explore --match-quality true --compress --apple-compat -o "$TMP1" 2>&1 | tail -20

OUTPUT1=$(find "$TMP1" -name "*.mp4" -type f 2>/dev/null | head -1)
if [ -n "$OUTPUT1" ] && [ -f "$OUTPUT1" ]; then
    OUTPUT_SIZE=$(stat -f%z "$OUTPUT1" 2>/dev/null || stat -c%s "$OUTPUT1")
    echo ""
    echo "📊 输出文件: $OUTPUT1"
    echo "📊 输出大小: $OUTPUT_SIZE bytes"
    if [ "$OUTPUT_SIZE" -lt "$INPUT_SIZE" ]; then
        RATIO=$((100 - OUTPUT_SIZE * 100 / INPUT_SIZE))
        echo "✅ 测试 1 通过: $INPUT_SIZE → $OUTPUT_SIZE bytes (压缩 ${RATIO}%)"
    else
        echo "❌ 测试 1 失败: 输出 $OUTPUT_SIZE >= 输入 $INPUT_SIZE (--compress 应保证压缩!)"
    fi
else
    echo "❌ 测试 1 失败: 输出文件不存在"
fi
echo ""

# 测试 2: 极限探索模式
echo "═══════════════════════════════════════════════════════════════"
echo "📊 测试 2: --explore --match-quality true --compress --ultimate"
echo "═══════════════════════════════════════════════════════════════"
TMP2="$TMP_BASE/test2"
mkdir -p "$TMP2"
$VIDQUALITY auto "$TEST_VIDEO" --explore --match-quality true --compress --ultimate -o "$TMP2" 2>&1 | tail -25

OUTPUT2=$(find "$TMP2" -name "*.mp4" -type f 2>/dev/null | head -1)
if [ -n "$OUTPUT2" ] && [ -f "$OUTPUT2" ]; then
    OUTPUT_SIZE=$(stat -f%z "$OUTPUT2" 2>/dev/null || stat -c%s "$OUTPUT2")
    echo ""
    echo "📊 输出文件: $OUTPUT2"
    echo "📊 输出大小: $OUTPUT_SIZE bytes"
    if [ "$OUTPUT_SIZE" -lt "$INPUT_SIZE" ]; then
        RATIO=$((100 - OUTPUT_SIZE * 100 / INPUT_SIZE))
        echo "✅ 测试 2 通过: $INPUT_SIZE → $OUTPUT_SIZE bytes (压缩 ${RATIO}%)"
    else
        echo "❌ 测试 2 失败: 输出 $OUTPUT_SIZE >= 输入 $INPUT_SIZE (--compress 应保证压缩!)"
    fi
else
    echo "❌ 测试 2 失败: 输出文件不存在"
fi
echo ""

# 测试 3: 简单模式 (无压缩保证)
echo "═══════════════════════════════════════════════════════════════"
echo "📊 测试 3: simple 模式 (无探索，无压缩保证)"
echo "═══════════════════════════════════════════════════════════════"
TMP3="$TMP_BASE/test3"
mkdir -p "$TMP3"
$VIDQUALITY simple "$TEST_VIDEO" -o "$TMP3" 2>&1 | tail -10

OUTPUT3=$(find "$TMP3" -name "*.mp4" -type f 2>/dev/null | head -1)
if [ -n "$OUTPUT3" ] && [ -f "$OUTPUT3" ]; then
    OUTPUT_SIZE=$(stat -f%z "$OUTPUT3" 2>/dev/null || stat -c%s "$OUTPUT3")
    echo ""
    echo "📊 输出文件: $OUTPUT3"
    echo "📊 输出大小: $OUTPUT_SIZE bytes"
    echo "✅ 测试 3 通过: 输出 $OUTPUT_SIZE bytes (simple 模式不保证压缩)"
else
    echo "❌ 测试 3 失败: 输出文件不存在"
fi
echo ""

echo "═══════════════════════════════════════════════════════════════"
echo "🎉 Strategy Pattern v6.3 测试完成"
echo "═══════════════════════════════════════════════════════════════"
