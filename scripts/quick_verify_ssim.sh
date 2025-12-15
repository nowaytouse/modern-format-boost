#!/bin/bash
# Quick SSIM verification (skip build)

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"
VIDQUALITY_HEVC="$PROJECT_ROOT/vidquality_hevc/target/release/vidquality-hevc"

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
CYAN='\033[0;36m'
BOLD='\033[1m'
NC='\033[0m'

TEST_DIR=$(mktemp -d)
trap "rm -rf $TEST_DIR" EXIT

echo -e "${CYAN}${BOLD}v5.69.4 Quick SSIM Verification${NC}"
echo ""

# Test VP8
echo -e "${YELLOW}Test 1: VP8${NC}"
VP8_TEST="$TEST_DIR/test_vp8.webm"
ffmpeg -y -f lavfi -i "testsrc=duration=2:size=320x240:rate=15" -c:v libvpx -b:v 500k -an "$VP8_TEST" 2>/dev/null
echo "Created: $VP8_TEST"
"$VIDQUALITY_HEVC" auto "$VP8_TEST" --explore --match-quality true --compress --apple-compat 2>&1 | grep -E "(SSIM|codec|method|FAILED)" || true
echo ""

# Test H264
echo -e "${YELLOW}Test 2: H.264${NC}"
H264_TEST="$TEST_DIR/test_h264.mp4"
ffmpeg -y -f lavfi -i "testsrc=duration=2:size=320x240:rate=15" -c:v libx264 -crf 28 -an "$H264_TEST" 2>/dev/null
echo "Created: $H264_TEST"
"$VIDQUALITY_HEVC" auto "$H264_TEST" --explore --match-quality true --compress --apple-compat 2>&1 | grep -E "(SSIM|codec|method|FAILED)" || true
echo ""

echo -e "${GREEN}Done!${NC}"
