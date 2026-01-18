#!/bin/bash
# Update README with v7.2 changes
set -e

README="$(dirname "$0")/../README.md"

# 备份
cp "$README" "$README.bak"

# 创建临时更新内容
cat > /tmp/readme_update.txt << 'EOF'
## 🔥 Latest Updates (v7.2)

### Quality Verification Fix
- **✅ Standalone VMAF Integration**: Bypass ffmpeg libvmaf dependency using Netflix's official vmaf CLI tool
- **✅ Multi-layer Fallback**: Standalone vmaf → ffmpeg libvmaf → SSIM All → SSIM Y
- **✅ No Recompilation**: Works without rebuilding ffmpeg
- **✅ Loud Error Reporting**: All failures clearly reported, no silent fallbacks

### Installation
```bash
# Install standalone vmaf tool
brew install libvmaf  # macOS
apt install libvmaf   # Linux

# Verify
vmaf --version
```

### Previous Updates (v6.9.17)
- **✅ CPU Encoding Reliability**: x265 CLI tool for better compatibility
- **✅ GPU Fallback System**: Automatic CPU fallback on GPU failures
- **✅ GIF Format Support**: Fixed bgra pixel format handling
- **✅ Error Transparency**: Clear error messages and fallback notifications
EOF

# 使用 sed 替换（macOS 兼容）
sed -i.tmp '1,/^## 🔥 Latest Updates/d' "$README"
cat /tmp/readme_update.txt > /tmp/new_readme.md
echo "" >> /tmp/new_readme.md
cat "$README" >> /tmp/new_readme.md
mv /tmp/new_readme.md "$README"

# 清理
rm -f "$README.tmp" /tmp/readme_update.txt

echo "✅ README updated with v7.2 changes"
