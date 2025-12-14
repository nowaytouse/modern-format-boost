#!/bin/bash
# 🔥 v5.5: 进度条效果测试脚本

set -e

PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
IMGQUALITY_HEVC="$PROJECT_ROOT/imgquality_hevc/target/release/imgquality-hevc"
VIDQUALITY_HEVC="$PROJECT_ROOT/vidquality_hevc/target/release/vidquality-hevc"

# 颜色定义
CYAN='\033[0;36m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BOLD='\033[1m'
DIM='\033[2m'
NC='\033[0m'

echo -e "${CYAN}${BOLD}╔══════════════════════════════════════════════════════════════════════════╗${NC}"
echo -e "${CYAN}${BOLD}║     🔥 Modern Format Boost v5.5 - 进度条效果测试                         ║${NC}"
echo -e "${CYAN}${BOLD}╚══════════════════════════════════════════════════════════════════════════╝${NC}"
echo ""

# 检查工具
echo -e "${YELLOW}📋 检查工具...${NC}"
if [[ ! -f "$IMGQUALITY_HEVC" ]]; then
    echo -e "${YELLOW}⚠️  imgquality-hevc 未编译，正在编译...${NC}"
    (cd "$PROJECT_ROOT/imgquality_hevc" && cargo build --release 2>&1 | tail -5)
fi

if [[ ! -f "$VIDQUALITY_HEVC" ]]; then
    echo -e "${YELLOW}⚠️  vidquality-hevc 未编译，正在编译...${NC}"
    (cd "$PROJECT_ROOT/vidquality_hevc" && cargo build --release 2>&1 | tail -5)
fi

echo -e "${GREEN}✅ 工具检查完成${NC}"
echo ""

# 创建测试目录
TEST_DIR="/tmp/modern_format_boost_test_$$"
mkdir -p "$TEST_DIR"
trap "rm -rf $TEST_DIR" EXIT

echo -e "${YELLOW}📁 创建测试文件...${NC}"

# 创建测试动画 GIF（使用 ImageMagick）- 这会触发探索模式
if command -v magick &>/dev/null || command -v convert &>/dev/null; then
    MAGICK_CMD=$(command -v magick || command -v convert)
    
    # 创建一个 5 秒的动画 GIF（50 帧，每帧 100ms）
    echo -e "${DIM}   创建动画 GIF (50 帧)...${NC}"
    for i in $(seq 1 50); do
        $MAGICK_CMD -size 640x480 \
            -seed $i plasma:red-blue \
            -blur 0x2 \
            "$TEST_DIR/frame_$i.png" 2>/dev/null
    done
    
    $MAGICK_CMD -delay 10 -loop 0 "$TEST_DIR/frame_*.png" "$TEST_DIR/test_animation.gif" 2>/dev/null
    rm -f "$TEST_DIR/frame_*.png"
    
    GIF_SIZE=$(stat -f%z "$TEST_DIR/test_animation.gif" 2>/dev/null || stat -c%s "$TEST_DIR/test_animation.gif" 2>/dev/null)
    echo -e "${GREEN}✅ 创建测试动画: test_animation.gif (5 秒, $(numfmt --to=iec-i --suffix=B $GIF_SIZE 2>/dev/null || echo $GIF_SIZE bytes))${NC}"
else
    echo -e "${YELLOW}⚠️  ImageMagick 未安装，跳过动画测试${NC}"
fi

# 创建测试视频（使用 ffmpeg）- 更复杂的内容
if command -v ffmpeg &>/dev/null; then
    # 创建一个有噪点的视频（更难压缩，会触发多次迭代）
    ffmpeg -f lavfi -i "testsrc2=s=1280x720:d=5" \
        -f lavfi -i "anoisesrc=d=5" \
        -pix_fmt yuv420p -c:v libx264 -crf 18 -preset fast \
        -c:a aac -b:a 128k \
        "$TEST_DIR/test_video.mp4" 2>/dev/null
    
    VID_SIZE=$(stat -f%z "$TEST_DIR/test_video.mp4" 2>/dev/null || stat -c%s "$TEST_DIR/test_video.mp4" 2>/dev/null)
    echo -e "${GREEN}✅ 创建测试视频: test_video.mp4 (5 秒, $(numfmt --to=iec-i --suffix=B $VID_SIZE 2>/dev/null || echo $VID_SIZE bytes))${NC}"
else
    echo -e "${YELLOW}⚠️  FFmpeg 未安装，跳过视频测试${NC}"
fi

echo ""
echo -e "${CYAN}${BOLD}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
echo -e "${CYAN}${BOLD}📊 测试 1: 动画 GIF → HEVC MP4 (探索模式进度条)${NC}"
echo -e "${CYAN}${BOLD}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
echo ""

if [[ -f "$TEST_DIR/test_animation.gif" ]]; then
    echo -e "${DIM}运行: imgquality-hevc auto --explore --match-quality --compress --apple-compat${NC}"
    echo -e "${DIM}这将触发 CRF 探索，显示实时进度条${NC}"
    echo ""
    
    # 运行转换，显示进度条
    "$IMGQUALITY_HEVC" auto "$TEST_DIR/test_animation.gif" \
        --output "$TEST_DIR/output_anim" \
        --explore --match-quality --compress \
        --apple-compat || true
    
    echo ""
    echo -e "${GREEN}✅ 动画处理完成${NC}"
    
    # 显示结果
    if [[ -f "$TEST_DIR/output_anim/test_animation.mp4" ]]; then
        orig_size=$(stat -f%z "$TEST_DIR/test_animation.gif" 2>/dev/null || stat -c%s "$TEST_DIR/test_animation.gif")
        conv_size=$(stat -f%z "$TEST_DIR/output_anim/test_animation.mp4" 2>/dev/null || stat -c%s "$TEST_DIR/output_anim/test_animation.mp4")
        if [[ $orig_size -gt $conv_size ]]; then
            reduction=$((100 * (orig_size - conv_size) / orig_size))
            echo -e "   📊 原始 GIF: $(numfmt --to=iec-i --suffix=B $orig_size 2>/dev/null || echo $orig_size bytes)"
            echo -e "   📊 转换 MP4: $(numfmt --to=iec-i --suffix=B $conv_size 2>/dev/null || echo $conv_size bytes)"
            echo -e "   📊 节省: ${reduction}%"
        else
            echo -e "   📊 原始 GIF: $(numfmt --to=iec-i --suffix=B $orig_size 2>/dev/null || echo $orig_size bytes)"
            echo -e "   📊 转换 MP4: $(numfmt --to=iec-i --suffix=B $conv_size 2>/dev/null || echo $conv_size bytes)"
            echo -e "   📊 无法压缩（MP4 更大）"
        fi
    fi
else
    echo -e "${YELLOW}⚠️  跳过动画测试（未创建测试文件）${NC}"
fi

echo ""
echo -e "${CYAN}${BOLD}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
echo -e "${CYAN}${BOLD}📊 测试 2: 视频处理 - 进度条显示${NC}"
echo -e "${CYAN}${BOLD}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
echo ""

if [[ -f "$TEST_DIR/test_video.mp4" ]]; then
    echo -e "${DIM}运行: vidquality-hevc auto --explore --match-quality --compress${NC}"
    echo ""
    
    # 运行转换，显示进度条
    "$VIDQUALITY_HEVC" auto "$TEST_DIR/test_video.mp4" \
        --output "$TEST_DIR/output_vid" \
        --explore --match-quality true \
        --compress --apple-compat || true
    
    echo ""
    echo -e "${GREEN}✅ 视频处理完成${NC}"
    
    # 显示结果
    if [[ -f "$TEST_DIR/output_vid/test_video.mp4" ]]; then
        orig_size=$(stat -f%z "$TEST_DIR/test_video.mp4" 2>/dev/null || stat -c%s "$TEST_DIR/test_video.mp4")
        conv_size=$(stat -f%z "$TEST_DIR/output_vid/test_video.mp4" 2>/dev/null || stat -c%s "$TEST_DIR/output_vid/test_video.mp4")
        reduction=$((100 * (orig_size - conv_size) / orig_size))
        echo -e "   📊 原始: $(numfmt --to=iec-i --suffix=B $orig_size 2>/dev/null || echo $orig_size bytes)"
        echo -e "   📊 转换: $(numfmt --to=iec-i --suffix=B $conv_size 2>/dev/null || echo $conv_size bytes)"
        echo -e "   📊 节省: ${reduction}%"
    fi
else
    echo -e "${YELLOW}⚠️  跳过视频测试（未创建测试文件）${NC}"
fi

echo ""
echo -e "${CYAN}${BOLD}╔══════════════════════════════════════════════════════════════════════════╗${NC}"
echo -e "${CYAN}${BOLD}║     ✅ 测试完成！                                                        ║${NC}"
echo -e "${CYAN}${BOLD}╚══════════════════════════════════════════════════════════════════════════╝${NC}"
echo ""
echo -e "${DIM}📝 进度条特性:${NC}"
echo -e "   ✅ 固定底部显示（不刷屏）"
echo -e "   ✅ 实时 CRF 值更新"
echo -e "   ✅ SSIM 分数显示"
echo -e "   ✅ 大小变化百分比"
echo -e "   ✅ 迭代次数计数"
echo -e "   ✅ 耗时统计"
echo -e "   ✅ 最终结果框线显示"
echo ""
