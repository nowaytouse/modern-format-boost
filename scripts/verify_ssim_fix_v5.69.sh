#!/bin/bash
# v5.69.4 SSIM Validation Fix Verification Script
# 使用双击 app 的参数: --explore --match-quality --compress --apple-compat

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"
VIDQUALITY_HEVC="$PROJECT_ROOT/vidquality_hevc/target/release/vidquality-hevc"

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
CYAN='\033[0;36m'
BOLD='\033[1m'
NC='\033[0m'

echo -e "${CYAN}${BOLD}═══════════════════════════════════════════════════════════════${NC}"
echo -e "${CYAN}${BOLD}  v5.69.4 SSIM Validation Fix Verification${NC}"
echo -e "${CYAN}${BOLD}═══════════════════════════════════════════════════════════════${NC}"
echo ""

# Build first
echo -e "${YELLOW}📦 Building...${NC}"
"$PROJECT_ROOT/build_all.sh" > /dev/null 2>&1 || {
    echo -e "${RED}❌ Build failed${NC}"
    exit 1
}
echo -e "${GREEN}✅ Build successful${NC}"
echo ""

# Create test directory
TEST_DIR=$(mktemp -d)
trap "rm -rf $TEST_DIR" EXIT

# Test 1: Create VP8 test video (the problematic codec)
echo -e "${CYAN}═══════════════════════════════════════════════════════════════${NC}"
echo -e "${BOLD}Test 1: VP8 Video (previously failed SSIM)${NC}"
echo -e "${CYAN}═══════════════════════════════════════════════════════════════${NC}"

VP8_TEST="$TEST_DIR/test_vp8.webm"
echo -e "${YELLOW}Creating VP8 test video...${NC}"
ffmpeg -y -f lavfi -i "testsrc=duration=3:size=640x480:rate=30" \
    -c:v libvpx -b:v 1M -an "$VP8_TEST" 2>/dev/null

if [[ -f "$VP8_TEST" ]]; then
    echo -e "${GREEN}✅ VP8 test video created${NC}"
    ls -lh "$VP8_TEST"
    echo ""
    
    echo -e "${YELLOW}Running vidquality-hevc with app parameters...${NC}"
    echo -e "${CYAN}Command: vidquality-hevc auto $VP8_TEST --explore --match-quality --compress --apple-compat${NC}"
    echo ""
    
    "$VIDQUALITY_HEVC" auto "$VP8_TEST" --explore --match-quality true --compress --apple-compat 2>&1 || true
    echo ""
else
    echo -e "${RED}❌ Failed to create VP8 test video${NC}"
fi

# Test 2: Create VP9 test video
echo ""
echo -e "${CYAN}═══════════════════════════════════════════════════════════════${NC}"
echo -e "${BOLD}Test 2: VP9 Video${NC}"
echo -e "${CYAN}═══════════════════════════════════════════════════════════════${NC}"

VP9_TEST="$TEST_DIR/test_vp9.webm"
echo -e "${YELLOW}Creating VP9 test video...${NC}"
ffmpeg -y -f lavfi -i "testsrc=duration=3:size=640x480:rate=30" \
    -c:v libvpx-vp9 -b:v 1M -an "$VP9_TEST" 2>/dev/null

if [[ -f "$VP9_TEST" ]]; then
    echo -e "${GREEN}✅ VP9 test video created${NC}"
    ls -lh "$VP9_TEST"
    echo ""
    
    echo -e "${YELLOW}Running vidquality-hevc...${NC}"
    "$VIDQUALITY_HEVC" auto "$VP9_TEST" --explore --match-quality true --compress --apple-compat 2>&1 || true
    echo ""
else
    echo -e "${RED}❌ Failed to create VP9 test video${NC}"
fi

# Test 3: Standard H.264 video (should work with standard method)
echo ""
echo -e "${CYAN}═══════════════════════════════════════════════════════════════${NC}"
echo -e "${BOLD}Test 3: H.264 Video (standard codec)${NC}"
echo -e "${CYAN}═══════════════════════════════════════════════════════════════${NC}"

H264_TEST="$TEST_DIR/test_h264.mp4"
echo -e "${YELLOW}Creating H.264 test video...${NC}"
ffmpeg -y -f lavfi -i "testsrc=duration=3:size=640x480:rate=30" \
    -c:v libx264 -crf 23 -an "$H264_TEST" 2>/dev/null

if [[ -f "$H264_TEST" ]]; then
    echo -e "${GREEN}✅ H.264 test video created${NC}"
    ls -lh "$H264_TEST"
    echo ""
    
    echo -e "${YELLOW}Running vidquality-hevc...${NC}"
    "$VIDQUALITY_HEVC" auto "$H264_TEST" --explore --match-quality true --compress --apple-compat 2>&1 || true
    echo ""
else
    echo -e "${RED}❌ Failed to create H.264 test video${NC}"
fi

echo ""
echo -e "${CYAN}${BOLD}═══════════════════════════════════════════════════════════════${NC}"
echo -e "${GREEN}${BOLD}  Verification Complete!${NC}"
echo -e "${CYAN}${BOLD}═══════════════════════════════════════════════════════════════${NC}"
echo ""
echo -e "${BOLD}Expected Results:${NC}"
echo -e "  1. VP8/VP9: SSIM should be calculated (not 'SSIM CALCULATION FAILED')"
echo -e "  2. H.264: SSIM should use 'standard' method"
echo -e "  3. All: Should show 'SSIM calculated using XXX method: X.XXXX'"
echo ""
