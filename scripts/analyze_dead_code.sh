#!/bin/bash
# 分析死代码 - Dead Code Analysis Script
set -euo pipefail

cd "$(dirname "$0")/.."
OUTPUT_DIR="/tmp/dead_code_analysis_$(date +%Y%m%d_%H%M%S)"
mkdir -p "$OUTPUT_DIR"

echo "📊 死代码分析报告 - Dead Code Analysis Report" > "$OUTPUT_DIR/report.txt"
echo "生成时间: $(date)" >> "$OUTPUT_DIR/report.txt"
echo "========================================" >> "$OUTPUT_DIR/report.txt"
echo "" >> "$OUTPUT_DIR/report.txt"

# 1. 查找注释掉的代码
echo "1️⃣ 查找注释掉的代码块..." | tee -a "$OUTPUT_DIR/report.txt"
find . -name "*.rs" -type f ! -path "./target/*" ! -path "./.git/*" \
  -exec grep -Hn "^[[:space:]]*//.*\(fn \|struct \|impl \|pub fn\|pub struct\)" {} \; \
  > "$OUTPUT_DIR/commented_code.txt" 2>&1 || true

COMMENTED_COUNT=$(wc -l < "$OUTPUT_DIR/commented_code.txt" | tr -d ' ')
echo "   发现 $COMMENTED_COUNT 行注释代码" | tee -a "$OUTPUT_DIR/report.txt"

# 2. 查找未使用的导入
echo "" | tee -a "$OUTPUT_DIR/report.txt"
echo "2️⃣ 检查未使用的导入..." | tee -a "$OUTPUT_DIR/report.txt"
cargo clippy --all-targets 2>&1 | grep "unused import" > "$OUTPUT_DIR/unused_imports.txt" || true
IMPORT_COUNT=$(wc -l < "$OUTPUT_DIR/unused_imports.txt" | tr -d ' ')
echo "   发现 $IMPORT_COUNT 个未使用的导入" | tee -a "$OUTPUT_DIR/report.txt"

# 3. 查找大文件（可能需要重构）
echo "" | tee -a "$OUTPUT_DIR/report.txt"
echo "3️⃣ 查找大文件（>1000行）..." | tee -a "$OUTPUT_DIR/report.txt"
find . -name "*.rs" -type f ! -path "./target/*" ! -path "./.git/*" \
  -exec wc -l {} \; | sort -rn | head -20 > "$OUTPUT_DIR/large_files.txt"
echo "   前20个最大文件已列出" | tee -a "$OUTPUT_DIR/report.txt"

# 4. 统计信息
echo "" | tee -a "$OUTPUT_DIR/report.txt"
echo "📈 统计信息:" | tee -a "$OUTPUT_DIR/report.txt"
TOTAL_RS_FILES=$(find . -name "*.rs" -type f ! -path "./target/*" ! -path "./.git/*" | wc -l | tr -d ' ')
TOTAL_LINES=$(find . -name "*.rs" -type f ! -path "./target/*" ! -path "./.git/*" -exec wc -l {} \; | awk '{sum+=$1} END {print sum}')
echo "   总Rust文件数: $TOTAL_RS_FILES" | tee -a "$OUTPUT_DIR/report.txt"
echo "   总代码行数: $TOTAL_LINES" | tee -a "$OUTPUT_DIR/report.txt"

echo "" | tee -a "$OUTPUT_DIR/report.txt"
echo "✅ 分析完成！详细结果保存在: $OUTPUT_DIR" | tee -a "$OUTPUT_DIR/report.txt"
echo "   - report.txt: 总结报告"
echo "   - commented_code.txt: 注释代码位置"
echo "   - unused_imports.txt: 未使用的导入"
echo "   - large_files.txt: 大文件列表"
echo ""
echo "报告路径: $OUTPUT_DIR"
