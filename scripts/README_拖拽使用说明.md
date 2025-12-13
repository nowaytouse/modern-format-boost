# Modern Format Boost - 拖拽式一键处理 v4.3

## 🆕 v4.3 新功能

### 🧪 测试模式（随机采样）
- **随机采样**: 每次运行选择不同的文件组合，避免重复测试相同文件
- **多样性覆盖**: 每种格式最多采样2个文件（特殊字符+普通命名）
- **边缘案例**: 自动采样小文件(<100KB)和大文件(>5MB)
- **最多20个文件**: 覆盖更多场景，快速验证工具功能

### � 断方点续传
- 处理中断后可继续，不会重复处理已完成的文件
- 原子操作保护，确保数据安全

### 🍎 Apple 兼容模式
- 自动将 AV1/VP9 等现代格式转换为 HEVC
- 动态图片智能处理：长动画→HEVC视频，短动画→GIF

### 📋 XMP 元数据合并
- 自动检测并合并 `.xmp` sidecar 元数据文件
- 合并成功后自动删除 XMP 文件
- 保留原始时间戳

## 🚀 使用方法

### 方法一：双击应用（推荐）
1. 双击 `Modern Format Boost.app`
2. 选择运行模式：
   - `[1] 测试模式` - 安全预览，输出到临时目录
   - `[2] 正式模式` - 原地转换，删除原文件
3. 选择要处理的文件夹
4. 确认后开始处理

### 方法二：拖拽文件夹
将文件夹拖拽到 `Modern Format Boost.app` 图标上

### 方法三：命令行
```bash
./scripts/drag_and_drop_processor.sh /path/to/folder
```

## 🧪 测试模式详解

测试模式会：
1. 从目标目录随机采样文件（每次不同）
2. 复制到临时目录处理
3. 不修改原文件
4. 生成详细日志

采样策略：
- XMP: 最多3个（优先特殊字符文件名）
- 图像: 每种格式最多2个
- 视频: 每种格式最多2个
- 额外: 小文件和大文件各1个

## ⚙️ 处理参数

### 图像处理
- `--in-place`: 原地转换
- `--recursive`: 递归子目录
- `--match-quality`: 算法预测质量
- `--explore`: 二分搜索最优CRF
- `--apple-compat`: Apple兼容模式

### 视频处理
- `--in-place`: 原地转换
- `--recursive`: 递归子目录
- `--match-quality`: 算法预测质量
- `--explore`: 二分搜索最优CRF
- `--apple-compat`: Apple兼容模式

## 🛡️ 安全特性

- **测试模式**: 首次使用推荐，不修改原文件
- **危险目录检测**: 阻止处理系统目录
- **断点续传**: 中断后可继续
- **原子操作**: 只有输出有效才删除原文件

## 📊 支持格式

| 类型 | 输入格式 | 输出格式 |
|------|----------|----------|
| 元数据 | XMP | 合并到媒体文件 |
| 静态图 | JPG, PNG, BMP, TIFF, WebP, HEIC | JXL |
| 动态图 | GIF, WebP动图, AVIF动图 | HEVC MP4 或 GIF |
| 视频 | MP4, MOV, AVI, MKV, WebM | HEVC MP4 |

## 🔧 依赖安装

```bash
# macOS (Homebrew)
brew install jpeg-xl ffmpeg exiftool

# 首次运行会自动编译 Rust 工具
```

## 🆘 故障排除

### 权限问题
```bash
chmod +x scripts/drag_and_drop_processor.sh
chmod +x "Modern Format Boost.app/Contents/MacOS/Modern Format Boost"
```

### 依赖检查
```bash
which ffmpeg cjxl exiftool
```
