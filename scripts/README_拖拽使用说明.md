# Modern Format Boost - 拖拽式一键处理 v3.9

## 🆕 v3.9 新功能：XMP 元数据合并

自动检测并合并 `.xmp` sidecar 元数据文件：
- 📋 在格式转换前自动扫描 XMP 文件
- 🔄 将元数据合并到对应的媒体文件
- 🗑️ 合并成功后自动删除 XMP 文件
- ⏱️ 保留原始时间戳

## 🚀 使用方法

### 方法一：双击应用（推荐）
1. 双击 `Modern Format Boost.app`
2. 在弹出的对话框中选择要处理的文件夹
3. 确认处理参数后开始自动处理

### 方法二：拖拽文件夹
1. 将要处理的文件夹拖拽到 `Modern Format Boost.app` 图标上
2. 会自动打开终端开始处理

### 方法三：命令行脚本
```bash
# 直接运行脚本
./scripts/drag_and_drop_processor.sh

# 或指定目录
./scripts/drag_and_drop_processor.sh /path/to/your/folder
```

## ⚙️ 处理参数

基于之前测试的最佳参数配置：

### 图像处理
- `--in-place`: 原地转换（删除原文件）
- `--recursive`: 递归处理所有子目录
- `--match-quality`: AI质量匹配（仅动图）
- `--explore`: 二分搜索最优CRF（仅动图）

### 视频处理
- `--in-place`: 原地转换（删除原文件）
- `--recursive`: 递归处理所有子目录
- `--match-quality`: AI质量匹配
- `--explore`: 二分搜索最优CRF

## 🛡️ 安全特性

- **危险目录检测**: 自动阻止处理系统目录（`/`, `/System`, `~`等）
- **用户确认**: 处理前需要用户明确确认
- **文件统计**: 显示将要处理的文件数量
- **进度显示**: 实时显示处理进度

## 📊 支持格式

### 元数据文件
- **XMP**: 自动检测并合并到对应媒体文件

### 图像格式
- **输入**: JPG, PNG, GIF, BMP, TIFF, WebP, HEIC
- **输出**: JXL (静态), HEVC MP4 (动图)

### 视频格式
- **输入**: MP4, MOV, AVI, MKV, WebM, M4V, FLV
- **输出**: HEVC MP4

## ⚠️ 重要提醒

1. **原地处理**: 会删除原文件，请确保有备份
2. **处理时间**: 大文件夹可能需要较长时间
3. **系统要求**: 需要安装 `ffmpeg`, `cjxl`, `exiftool`
4. **首次运行**: 会自动编译工具（需要几分钟）

## 🔧 依赖安装

```bash
# macOS
brew install jpeg-xl ffmpeg exiftool

# 或使用 MacPorts
sudo port install jpeg-xl ffmpeg exiftool
```

## 📝 处理日志

所有处理过程都会在终端中显示详细日志，包括：
- 文件检测和分析
- PNG量化检测结果
- CRF计算过程
- 转换进度和结果
- 最终统计报告

## 🆘 故障排除

### 工具未找到
如果提示工具未找到，会自动尝试编译。确保已安装 Rust：
```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

### 权限问题
如果遇到权限问题：
```bash
chmod +x scripts/drag_and_drop_processor.sh
chmod +x "Modern Format Boost.app/Contents/MacOS/Modern Format Boost"
```

### 依赖缺失
确保安装了所有依赖：
```bash
# 检查依赖
which ffmpeg cjxl exiftool

# 如果缺失，使用 Homebrew 安装
brew install jpeg-xl ffmpeg exiftool
```