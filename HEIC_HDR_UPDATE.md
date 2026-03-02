# Modern Format Boost - HEIC HDR/杜比视界支持更新

## 更新内容

### 1. HEIC HDR/杜比视界检测与跳过

项目现已支持自动检测并跳过包含 HDR 或杜比视界元数据的 HEIC 文件，以保护高动态范围内容。

#### 实现细节

- **HDR 检测**: 扫描 HEIC 文件中的 `hvcC` (HEVC配置) 和 `colr`/`nclx` (颜色信息) box
- **杜比视界检测**: 检测 `dvcC` 和 `dvvC` box
- **自动跳过**: 检测到 HDR 或杜比视界时自动跳过转换，保留原始文件

#### 检测标准

1. **传输特性 (Transfer Characteristics)**:
   - PQ (SMPTE 2084) = 16
   - HLG (Hybrid Log-Gamma) = 18

2. **色域 (Color Primaries)**:
   - BT.2020 = 9

3. **杜比视界配置**:
   - dvcC box (Dolby Vision Configuration)
   - dvvC box (Dolby Vision Enhancement Layer)

#### 代码位置

- `shared_utils/src/image_heic_analysis.rs`: HEIC 分析和 HDR/DV 检测
- `shared_utils/src/image_analyzer.rs`: 图像分析器集成
- `shared_utils/src/img_errors.rs`: 新增 `SkipFile` 错误类型

### 2. Eagle 插件二进制自动更新系统

为 Eagle 插件添加了基于 git 的二进制依赖自动更新系统，无需使用 GitHub API。

#### 特性

- ✅ **Git Sparse Checkout**: 仅下载二进制文件，高效节省带宽
- ✅ **浅克隆 (Shallow Clone)**: 仅获取最新提交，速度快
- ✅ **自动更新**: 插件每周自动检查更新
- ✅ **手动更新**: 支持手动运行更新脚本
- ✅ **无 API 依赖**: 直接使用 git，不依赖 GitHub API

#### 文件结构

```
plugin/
├── update_binaries.sh          # 二进制更新脚本
├── .binary_source.example      # 配置示例
├── .binary_source              # 实际配置（需创建）
├── BINARY_UPDATE.md            # 更新系统文档
└── bin/
    ├── img-hevc                # HEIC/HEIF 图像处理器
    ├── vid-hevc                # HEVC 视频处理器
    ├── img-av1                 # AV1 图像处理器
    ├── vid-av1                 # AV1 视频处理器
    └── .last_update            # 上次更新时间戳
```

#### 配置方法

1. 复制配置示例:
   ```bash
   cd /Users/nyamiiko/Downloads/GitHub/modern_format_boost_Eaglecool/plugin
   cp .binary_source.example .binary_source
   ```

2. 编辑 `.binary_source` 设置仓库地址:
   ```bash
   REPO_URL="https://github.com/your-username/modern_format_boost.git"
   BRANCH="main"
   ```

3. 手动运行更新（可选）:
   ```bash
   ./update_binaries.sh
   ```

#### 自动更新机制

插件启动时会自动检查:
- 如果距离上次更新超过 7 天，自动在后台更新二进制文件
- 更新过程不会中断用户操作
- 更新日志显示在插件日志区域

#### 工作原理

```bash
# 1. 浅克隆仓库（仅最新提交）
git clone --depth 1 --filter=blob:none --sparse <repo>

# 2. 配置稀疏检出（仅二进制文件）
git sparse-checkout set target/release/img-hevc target/release/vid-hevc ...

# 3. 复制到 bin 目录并设置执行权限
cp target/release/* bin/
chmod +x bin/*
```

## 使用说明

### HEIC HDR/杜比视界处理

当处理包含 HDR 或杜比视界的 HEIC 文件时，工具会自动:

1. 检测文件中的 HDR/DV 元数据
2. 显示跳过消息: `⏭️ HEIC with Dolby Vision - skipping to preserve HDR metadata`
3. 保留原始文件不做任何修改

### 二进制更新

#### 自动更新
- 插件每周自动检查更新
- 无需手动干预

#### 手动更新
```bash
cd /Users/nyamiiko/Downloads/GitHub/modern_format_boost_Eaglecool/plugin
./update_binaries.sh
```

输出示例:
```
🔄 Modern Format Boost Binary Updater
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
📦 Repository: https://github.com/...
🌿 Branch: main

📥 Fetching latest binaries...
🎯 Configuring sparse checkout...

📦 Installing binaries...
✅ img-hevc (5.4M)
✅ vid-hevc (2.7M)
✅ img-av1 (6.1M)
✅ vid-av1 (3.2M)

✨ Binary update complete!
```

## 技术细节

### HDR 检测算法

```rust
fn detect_heic_hdr_dv(path: &Path) -> (bool, bool) {
    // 读取文件前 64KB
    // 扫描 ISO BMFF box 结构
    // 检测:
    // - hvcC: HEVC 配置，包含传输特性
    // - dvcC/dvvC: 杜比视界配置
    // - colr/nclx: 颜色信息（色域、传输特性）
    // 返回 (is_hdr, is_dolby_vision)
}
```

### Git Sparse Checkout 优势

1. **带宽节省**: 仅下载需要的二进制文件（~20MB），而非整个仓库（可能数百MB）
2. **速度快**: 浅克隆 + 稀疏检出，几秒内完成
3. **无 API 限制**: 不受 GitHub API 速率限制
4. **简单可靠**: 标准 git 命令，兼容所有 git 托管平台

## 测试

### 编译测试
```bash
cd /Users/nyamiiko/Downloads/GitHub/modern_format_boost
cargo check --workspace
```

### 功能测试

1. **HDR 检测测试**:
   - 准备包含 HDR 的 HEIC 文件
   - 运行 `img-hevc run <file>`
   - 验证是否显示跳过消息

2. **二进制更新测试**:
   ```bash
   cd /Users/nyamiiko/Downloads/GitHub/modern_format_boost_Eaglecool/plugin
   ./update_binaries.sh
   ```

## 注意事项

1. **仓库配置**: 确保 `.binary_source` 中的仓库地址正确
2. **Git 依赖**: 系统需要安装 git
3. **网络访问**: 更新需要访问 git 仓库
4. **权限**: 确保插件目录有写入权限

## 故障排除

### HDR 检测问题
- 检查文件是否真的包含 HDR 元数据
- 查看日志中的详细错误信息

### 二进制更新失败
1. 检查 git 是否安装: `git --version`
2. 验证仓库地址: 在 `.binary_source` 中
3. 检查网络连接
4. 手动运行脚本查看详细错误: `./update_binaries.sh`

## 相关文件

- `/Users/nyamiiko/Downloads/GitHub/modern_format_boost/shared_utils/src/image_heic_analysis.rs`
- `/Users/nyamiiko/Downloads/GitHub/modern_format_boost/shared_utils/src/image_analyzer.rs`
- `/Users/nyamiiko/Downloads/GitHub/modern_format_boost_Eaglecool/plugin/update_binaries.sh`
- `/Users/nyamiiko/Downloads/GitHub/modern_format_boost_Eaglecool/plugin/js/plugin.js`
