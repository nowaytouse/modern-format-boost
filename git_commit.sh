#!/bin/bash
cd "$(dirname "$0")"

echo "📋 Git 状态:"
git status --short

echo ""
echo "📝 提交更改..."
git add -A
git commit -m "fix: 确认目录结构保留功能正常工作

- 清理调试输出
- 验证 base_dir 设置逻辑正确
- 测试通过：所有子目录结构都被正确保留
- 问题原因：之前测试文件太小(<500KB)被跳过

测试结果:
✅ /input/photos/2024/img.png → /output/photos/2024/img.jxl
✅ /input/photos/img.png → /output/photos/img.jxl
✅ /input/videos/frame.png → /output/videos/frame.jxl"

echo ""
echo "🚀 推送到远程..."
git push

echo ""
echo "✅ 完成"
