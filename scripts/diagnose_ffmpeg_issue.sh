#!/bin/bash
# 诊断 FFmpeg x265 编码问题

echo "🔍 检查 FFmpeg 和 x265 安装状态..."

# 检查 FFmpeg
if command -v ffmpeg &> /dev/null; then
    echo "✅ FFmpeg 已安装"
    ffmpeg -version | head -n 1
    
    # 检查 libx265 支持
    if ffmpeg -encoders 2>/dev/null | grep -q libx265; then
        echo "✅ FFmpeg 支持 libx265"
    else
        echo "❌ FFmpeg 不支持 libx265"
    fi
else
    echo "❌ FFmpeg 未安装"
fi

# 检查 x265 命令行工具
if command -v x265 &> /dev/null; then
    echo "✅ x265 命令行工具已安装"
    x265 --version 2>&1 | head -n 1
else
    echo "❌ x265 命令行工具未安装"
    echo "   安装命令: brew install x265"
fi

echo ""
echo "🔧 建议的修复方案:"
echo "1. 如果 FFmpeg 不支持 libx265，需要重新安装:"
echo "   brew uninstall ffmpeg"
echo "   brew install ffmpeg"
echo ""
echo "2. 如果 x265 命令行工具未安装:"
echo "   brew install x265"
