#!/bin/bash
# 测试FFmpeg filter参数问题

# 测试文件（从日志中找到的失败文件）
TEST_FILES=(
    "/Users/nyamiiko/Downloads/1/参考/内容 动态/53ruq7spdtna5vh1avyp2xeoh.gif"
    "/Users/nyamiiko/Downloads/1/参考/内容 动态/-69c559cc367f48f1.gif"
)

echo "🧪 测试FFmpeg filter参数..."
echo ""

for file in "${TEST_FILES[@]}"; do
    if [ ! -f "$file" ]; then
        echo "⏭️  文件不存在: $file"
        continue
    fi
    
    echo "测试文件: $(basename "$file")"
    
    # 获取尺寸
    dims=$(ffprobe -v error -select_streams v:0 -show_entries stream=width,height -of csv=p=0 "$file" 2>/dev/null)
    width=$(echo "$dims" | cut -d',' -f1)
    height=$(echo "$dims" | cut -d',' -f2)
    
    echo "  尺寸: ${width}x${height}"
    
    # 测试调色板生成命令
    echo "  测试调色板生成..."
    ffmpeg -y -i "$file" \
        -vf "fps=10,scale=${width}:${height}:flags=lanczos,palettegen=max_colors=256:stats_mode=diff" \
        /tmp/test_palette.png 2>&1 | grep -i "error\|option"
    
    if [ $? -eq 0 ]; then
        echo "  ❌ 调色板生成失败"
    else
        echo "  ✅ 调色板生成成功"
        rm -f /tmp/test_palette.png
    fi
    
    echo ""
done

echo "✅ 测试完成"
