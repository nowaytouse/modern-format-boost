#!/bin/bash
# 调试 FFmpeg 命令 - 查看实际执行的命令

set -x

TEST_GIF="/Users/user/Downloads/1/参考/内容 猎奇/4h8uh4vkss9clo2wfiy30kach.gif"
TEST_OUTPUT="/tmp/debug_test.mp4"

# 清理
rm -f "$TEST_OUTPUT"

# 模拟 CPU 编码命令（基于代码分析）
# 这是修复前的命令（有重复的 -preset）
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "🔴 BEFORE FIX (with duplicate -preset):"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

ffmpeg -y \
    -threads 4 \
    -i "$TEST_GIF" \
    -c:v libx265 \
    -crf 19.9 \
    -preset medium \
    -progress pipe:1 \
    -stats_period 0.5 \
    -preset medium \
    -tag:v hvc1 \
    -x265-params "log-level=error:pools=4" \
    -vf "scale='if(mod(iw,2),iw+1,iw)':'if(mod(ih,2),ih+1,ih)'" \
    "$TEST_OUTPUT" 2>&1 | head -20

echo ""
echo "Result: $?"
rm -f "$TEST_OUTPUT"

echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "🟢 AFTER FIX (no duplicate -preset):"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

ffmpeg -y \
    -threads 4 \
    -i "$TEST_GIF" \
    -c:v libx265 \
    -crf 19.9 \
    -progress pipe:1 \
    -stats_period 0.5 \
    -preset medium \
    -tag:v hvc1 \
    -x265-params "log-level=error:pools=4" \
    -vf "scale='if(mod(iw,2),iw+1,iw)':'if(mod(ih,2),ih+1,ih)'" \
    "$TEST_OUTPUT" 2>&1 | head -20

echo ""
echo "Result: $?"

if [ -f "$TEST_OUTPUT" ]; then
    echo "✅ Output created successfully"
    ls -lh "$TEST_OUTPUT"
    rm -f "$TEST_OUTPUT"
else
    echo "❌ Output not created"
fi
