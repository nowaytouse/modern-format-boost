#!/usr/bin/env bash
# Commit v7.5.0 - File Processing Optimization

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"

cd "$PROJECT_ROOT"

echo "🔧 v7.5.0 - File Processing Optimization"
echo ""

# 检查是否有未提交的更改
if ! git diff --quiet || ! git diff --cached --quiet; then
    echo "📝 Staging changes..."
    
    # 添加新文件和修改
    git add shared_utils/src/file_sorter.rs
    git add shared_utils/src/batch.rs
    git add shared_utils/src/lib.rs
    git add shared_utils/src/cli_runner.rs
    git add imgquality_hevc/src/main.rs
    git add imgquality_av1/src/main.rs
    git add CHANGELOG.md
    git add README.md
    git add v7.5.0_FILE_SORTING.md
    git add scripts/test_file_sorting.sh
    git add scripts/commit_v7.5.sh
    
    echo "✅ Files staged"
    echo ""
    
    # 显示将要提交的文件
    echo "📋 Files to commit:"
    git diff --cached --name-status
    echo ""
    
    # 提交
    echo "💾 Committing..."
    git commit -m "v7.5.0: File Processing Optimization - Small Files First

🎯 Feature: Intelligent File Sorting
- Created modular file_sorter.rs for flexible sorting strategies
- Implemented SortStrategy enum (SizeAscending, SizeDescending, NameAscending, None)
- Added convenience functions for common sorting patterns

✅ Benefits:
- Quick progress feedback (small files finish fast)
- Early problem detection (issues found sooner)
- Large files don't block the queue
- Better user experience during batch processing

🔧 Implementation:
- Updated batch.rs with collect_files_sorted() and collect_files_small_first()
- Updated all 5 tools to use file sorting
- Comprehensive unit tests with property-based validation

📦 Modified Files:
- shared_utils/src/file_sorter.rs (NEW - modular design)
- shared_utils/src/batch.rs (sorting functions)
- shared_utils/src/lib.rs (export new module)
- shared_utils/src/cli_runner.rs (use sorted collection)
- imgquality_hevc/src/main.rs (use sorted collection)
- imgquality_av1/src/main.rs (use sorted collection)
- CHANGELOG.md (v7.5.0 entry)
- README.md (v7.5.0 highlights)

✅ Testing:
- All tools compile successfully
- No warnings
- Unit tests pass
- Integration test script created"
    
    echo "✅ Committed"
    echo ""
    
    # 推送
    echo "🚀 Pushing to remote..."
    git push
    echo "✅ Pushed"
    echo ""
    
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    echo "✅ v7.5.0 committed and pushed successfully"
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
else
    echo "⚠️  No changes to commit"
fi
