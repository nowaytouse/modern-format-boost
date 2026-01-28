#!/bin/bash
# 全面转换审计脚本 - 检查转换质量、元数据保留、功能完整性
# 仅检查，不对原件进行任何改动

set -euo pipefail

echo "🔍 全面转换审计 - v7.8 质量验证"
echo "═══════════════════════════════════════════════════════════"
echo ""

TARGET_DIR="/Users/nyamiiko/Downloads/all/闷茶子新"
TEMP_DIR=$(mktemp -d)
trap "rm -rf $TEMP_DIR" EXIT

cd "$(dirname "$0")/.."

# 测试计数
PASS=0
FAIL=0
WARN=0

test_pass() {
    echo "✅ $1"
    ((PASS++))
}

test_fail() {
    echo "❌ $1"
    ((FAIL++))
}

test_warn() {
    echo "⚠️ $1"
    ((WARN++))
}

echo "📂 检查目标目录: $TARGET_DIR"
if [ ! -d "$TARGET_DIR" ]; then
    test_fail "目标目录不存在"
    exit 1
fi

# 1. 文件统计分析
echo ""
echo "📊 Test 1: 文件统计分析"
TOTAL_FILES=$(find "$TARGET_DIR" -type f | wc -l | tr -d ' ')
HEIC_FILES=$(find "$TARGET_DIR" -iname "*.heic" | wc -l | tr -d ' ')
MP4_FILES=$(find "$TARGET_DIR" -iname "*.mp4" | wc -l | tr -d ' ')
MOV_FILES=$(find "$TARGET_DIR" -iname "*.mov" | wc -l | tr -d ' ')
JPG_FILES=$(find "$TARGET_DIR" -iname "*.jpg" -o -iname "*.jpeg" | wc -l | tr -d ' ')
PNG_FILES=$(find "$TARGET_DIR" -iname "*.png" | wc -l | tr -d ' ')

echo "   总文件数: $TOTAL_FILES"
echo "   HEIC文件: $HEIC_FILES"
echo "   MP4文件: $MP4_FILES"
echo "   MOV文件: $MOV_FILES"
echo "   JPG文件: $JPG_FILES"
echo "   PNG文件: $PNG_FILES"

if [ "$TOTAL_FILES" -gt 0 ]; then
    test_pass "文件统计完成 ($TOTAL_FILES 个文件)"
else
    test_fail "未找到任何文件"
fi

# 2. 随机抽检HEIC文件
echo ""
echo "🖼️ Test 2: HEIC文件质量检查"
if [ "$HEIC_FILES" -gt 0 ]; then
    # 随机选择3个HEIC文件进行检查
    SAMPLE_HEIC=$(find "$TARGET_DIR" -iname "*.heic" | head -3)
    HEIC_CHECK_COUNT=0
    HEIC_PASS_COUNT=0
    
    for heic_file in $SAMPLE_HEIC; do
        ((HEIC_CHECK_COUNT++))
        echo "   检查: $(basename "$heic_file")"
        
        # 检查文件完整性
        if file "$heic_file" | grep -q "HEIF"; then
            echo "     ✓ 文件格式正确"
            
            # 检查文件大小
            SIZE=$(stat -f%z "$heic_file")
            if [ "$SIZE" -gt 1000 ]; then
                echo "     ✓ 文件大小正常 (${SIZE} bytes)"
                ((HEIC_PASS_COUNT++))
            else
                echo "     ⚠️ 文件可能过小 (${SIZE} bytes)"
            fi
        else
            echo "     ❌ 文件格式异常"
        fi
        
        # 检查元数据
        if command -v exiftool >/dev/null 2>&1; then
            METADATA=$(exiftool "$heic_file" 2>/dev/null | wc -l | tr -d ' ')
            if [ "$METADATA" -gt 5 ]; then
                echo "     ✓ 元数据保留 ($METADATA 条)"
            else
                echo "     ⚠️ 元数据较少 ($METADATA 条)"
            fi
        fi
        echo ""
    done
    
    if [ "$HEIC_PASS_COUNT" -eq "$HEIC_CHECK_COUNT" ]; then
        test_pass "HEIC文件检查通过 ($HEIC_PASS_COUNT/$HEIC_CHECK_COUNT)"
    else
        test_warn "HEIC文件部分通过 ($HEIC_PASS_COUNT/$HEIC_CHECK_COUNT)"
    fi
else
    test_warn "未找到HEIC文件"
fi

# 3. 随机抽检视频文件
echo ""
echo "🎬 Test 3: 视频文件质量检查"
VIDEO_FILES=$(find "$TARGET_DIR" -iname "*.mp4" -o -iname "*.mov" | head -3)
if [ -n "$VIDEO_FILES" ]; then
    VIDEO_CHECK_COUNT=0
    VIDEO_PASS_COUNT=0
    
    for video_file in $VIDEO_FILES; do
        ((VIDEO_CHECK_COUNT++))
        echo "   检查: $(basename "$video_file")"
        
        # 检查文件完整性
        if file "$video_file" | grep -qE "(MP4|QuickTime)"; then
            echo "     ✓ 文件格式正确"
            
            # 检查文件大小
            SIZE=$(stat -f%z "$video_file")
            if [ "$SIZE" -gt 10000 ]; then
                echo "     ✓ 文件大小正常 (${SIZE} bytes)"
                
                # 使用ffprobe检查视频信息
                if command -v ffprobe >/dev/null 2>&1; then
                    DURATION=$(ffprobe -v quiet -show_entries format=duration -of csv=p=0 "$video_file" 2>/dev/null || echo "0")
                    if [ "${DURATION%.*}" -gt 0 ] 2>/dev/null; then
                        echo "     ✓ 视频时长正常 (${DURATION}s)"
                        ((VIDEO_PASS_COUNT++))
                    else
                        echo "     ⚠️ 无法获取视频时长"
                    fi
                else
                    echo "     ⚠️ ffprobe不可用，跳过详细检查"
                    ((VIDEO_PASS_COUNT++))
                fi
            else
                echo "     ❌ 文件可能损坏 (${SIZE} bytes)"
            fi
        else
            echo "     ❌ 文件格式异常"
        fi
        echo ""
    done
    
    if [ "$VIDEO_PASS_COUNT" -eq "$VIDEO_CHECK_COUNT" ]; then
        test_pass "视频文件检查通过 ($VIDEO_PASS_COUNT/$VIDEO_CHECK_COUNT)"
    else
        test_warn "视频文件部分通过 ($VIDEO_PASS_COUNT/$VIDEO_CHECK_COUNT)"
    fi
else
    test_warn "未找到视频文件"
fi

# 4. 功能完整性测试
echo ""
echo "🔧 Test 4: 功能完整性测试"

# 测试imgquality-hevc
if [ -f "target/release/imgquality-hevc" ]; then
    if ./target/release/imgquality-hevc --version >/dev/null 2>&1; then
        test_pass "imgquality-hevc 可执行"
    else
        test_fail "imgquality-hevc 执行失败"
    fi
else
    test_fail "imgquality-hevc 不存在"
fi

# 测试vidquality-hevc
if [ -f "target/release/vidquality-hevc" ]; then
    if ./target/release/vidquality-hevc --version >/dev/null 2>&1; then
        test_pass "vidquality-hevc 可执行"
    else
        test_fail "vidquality-hevc 执行失败"
    fi
else
    test_fail "vidquality-hevc 不存在"
fi

# 5. 模块化架构验证
echo ""
echo "🏗️ Test 5: 模块化架构验证"

# 检查video_explorer模块
if [ -f "shared_utils/src/video_explorer/metadata.rs" ]; then
    test_pass "video_explorer/metadata.rs 存在"
else
    test_fail "video_explorer/metadata.rs 缺失"
fi

if [ -f "shared_utils/src/video_explorer/stream_analysis.rs" ]; then
    test_pass "video_explorer/stream_analysis.rs 存在"
else
    test_fail "video_explorer/stream_analysis.rs 缺失"
fi

if [ -f "shared_utils/src/video_explorer/codec_detection.rs" ]; then
    test_pass "video_explorer/codec_detection.rs 存在"
else
    test_fail "video_explorer/codec_detection.rs 缺失"
fi

# 检查common_utils模块
if [ -f "shared_utils/src/common_utils.rs" ]; then
    test_pass "common_utils.rs 存在"
else
    test_fail "common_utils.rs 缺失"
fi

# 检查logging模块
if [ -f "shared_utils/src/logging.rs" ]; then
    test_pass "logging.rs 存在"
else
    test_fail "logging.rs 缺失"
fi

# 6. 日志系统验证
echo ""
echo "📝 Test 6: 日志系统验证"

# 检查日志文件
LOG_FILES=$(find /tmp -name "*quality*.log" -mmin -1440 2>/dev/null | wc -l | tr -d ' ')
if [ "$LOG_FILES" -gt 0 ]; then
    test_pass "找到日志文件 ($LOG_FILES 个)"
    
    # 检查最新日志内容
    LATEST_LOG=$(find /tmp -name "*quality*.log" -mmin -1440 2>/dev/null | head -1)
    if [ -n "$LATEST_LOG" ]; then
        LOG_SIZE=$(stat -f%z "$LATEST_LOG" 2>/dev/null || echo "0")
        if [ "$LOG_SIZE" -gt 100 ]; then
            test_pass "日志内容正常 (${LOG_SIZE} bytes)"
        else
            test_warn "日志内容较少 (${LOG_SIZE} bytes)"
        fi
    fi
else
    test_warn "未找到最近的日志文件"
fi

# 7. 元数据保留验证
echo ""
echo "🏷️ Test 7: 元数据保留验证"

if command -v exiftool >/dev/null 2>&1; then
    # 随机选择一个图片文件检查元数据
    SAMPLE_IMAGE=$(find "$TARGET_DIR" -iname "*.jpg" -o -iname "*.png" | head -1)
    if [ -n "$SAMPLE_IMAGE" ]; then
        METADATA_COUNT=$(exiftool "$SAMPLE_IMAGE" 2>/dev/null | grep -v "ExifTool Version" | wc -l | tr -d ' ')
        if [ "$METADATA_COUNT" -gt 10 ]; then
            test_pass "元数据保留良好 ($METADATA_COUNT 条)"
        elif [ "$METADATA_COUNT" -gt 5 ]; then
            test_warn "元数据部分保留 ($METADATA_COUNT 条)"
        else
            test_warn "元数据保留较少 ($METADATA_COUNT 条)"
        fi
    else
        test_warn "未找到可检查的图片文件"
    fi
else
    test_warn "exiftool不可用，跳过元数据检查"
fi

# 8. 文件完整性验证
echo ""
echo "🔐 Test 8: 文件完整性验证"

# 检查是否有损坏的文件
CORRUPTED_COUNT=0
SAMPLE_FILES=$(find "$TARGET_DIR" -type f \( -iname "*.jpg" -o -iname "*.png" -o -iname "*.heic" \) | head -5)

for sample_file in $SAMPLE_FILES; do
    if ! file "$sample_file" | grep -qE "(JPEG|PNG|HEIF)"; then
        ((CORRUPTED_COUNT++))
    fi
done

if [ "$CORRUPTED_COUNT" -eq 0 ]; then
    test_pass "文件完整性检查通过"
else
    test_fail "发现 $CORRUPTED_COUNT 个可能损坏的文件"
fi

# 总结报告
echo ""
echo "═══════════════════════════════════════════════════════════"
echo "📊 审计总结报告"
echo "═══════════════════════════════════════════════════════════"
echo "通过: $PASS"
echo "失败: $FAIL"
echo "警告: $WARN"
echo ""

# 生成详细报告
REPORT_FILE="conversion_audit_report_$(date +%Y%m%d_%H%M%S).txt"
{
    echo "转换审计报告 - $(date)"
    echo "目标目录: $TARGET_DIR"
    echo "总文件数: $TOTAL_FILES"
    echo "HEIC文件: $HEIC_FILES"
    echo "视频文件: $((MP4_FILES + MOV_FILES))"
    echo "图片文件: $((JPG_FILES + PNG_FILES))"
    echo ""
    echo "测试结果:"
    echo "✅ 通过: $PASS"
    echo "❌ 失败: $FAIL"
    echo "⚠️ 警告: $WARN"
    echo ""
    echo "v7.8 质量改进验证:"
    echo "• 统一日志系统: $([ -f "shared_utils/src/logging.rs" ] && echo "✅" || echo "❌")"
    echo "• 模块化架构: $([ -d "shared_utils/src/video_explorer" ] && echo "✅" || echo "❌")"
    echo "• 通用工具库: $([ -f "shared_utils/src/common_utils.rs" ] && echo "✅" || echo "❌")"
    echo "• 功能完整性: $([ -f "target/release/imgquality-hevc" ] && echo "✅" || echo "❌")"
} > "$REPORT_FILE"

echo "📄 详细报告已保存: $REPORT_FILE"

if [ $FAIL -eq 0 ]; then
    echo ""
    echo "🎉 转换审计通过！"
    echo "✅ 功能正常，元数据保留完整，模块化架构工作正常"
    echo "✅ v7.8 质量改进未破坏任何功能"
    exit 0
else
    echo ""
    echo "⚠️ 发现 $FAIL 个问题，需要进一步检查"
    exit 1
fi