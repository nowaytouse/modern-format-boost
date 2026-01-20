#!/bin/bash

# 创建大图片用于测试CJXL fallback

echo "🖼️  创建测试图片..."

# 使用ImageMagick创建一个大图片
if command -v magick >/dev/null 2>&1; then
    magick -size 4096x4096 xc:white -fill black -pointsize 144 \
        -draw "rectangle 100,200 1000,1200" \
        -fill red -draw "rectangle 500,600 1500,1600" \
        -fill blue -draw "circle 2000,2000 2500,2500" \
        -fill green -draw "rectangle 2500,100 3500,1100" \
        test_media/large_test.png
    
    if [[ -f "test_media/large_test.png" ]]; then
        echo "✅ 创建大测试图片: test_media/large_test.png"
        SIZE=$(stat -f%z "test_media/large_test.png" 2>/dev/null || stat -c%s "test_media/large_test.png" 2>/dev/null)
        echo "   大小: $SIZE bytes"
    else
        echo "❌ 创建失败"
    fi
else
    echo "❌ ImageMagick不可用"
fi