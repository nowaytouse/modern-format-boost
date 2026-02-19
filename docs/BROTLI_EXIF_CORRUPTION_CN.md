# Brotli EXIF 损坏问题

## 问题描述

20 个 JXL 文件无法导入 iCloud 照片，错误信息：
```
无法读取元数据。文件可能已损坏。
```

## 根本原因

**JXL 容器格式中的 Brotli 压缩 EXIF 数据损坏**

### 技术细节

JXL 格式允许使用 Brotli 压缩元数据以节省空间。损坏发生在：

1. 源工具写入 Brotli 压缩的 EXIF 数据
2. 压缩流格式错误或被截断
3. exiftool 可以读取（高容错性）
4. iCloud Photos 解析器拒绝（严格验证）

### 检测方法

```bash
exiftool -validate -warning file.jxl
```

损坏文件的输出：
```
Validate: 1 Warning
Warning: Corrupted Brotli 'Exif' data
```

## 为什么会发生

损坏是由上游工具（可能是图像转换器或下载器）在 Modern Format Boost 处理文件之前引入的。Modern Format Boost 的元数据保留功能（`exiftool -tagsfromfile`）按原样复制元数据，包括损坏的 Brotli 流。

## 解决方案：元数据重建

### 工作原理

```bash
exiftool -all= -tagsfromfile @ -all:all -overwrite_original file.jxl
```

**逐步过程：**

1. `-all=` - 清空目标文件的所有元数据
2. `-tagsfromfile @` - 从同一文件读取元数据（清空前）
3. `-all:all` - 将所有元数据标签复制回来
4. exiftool 使用标准格式重新编码元数据（不使用 Brotli）

**为什么能修复：**

- exiftool 的**读取**操作容错性强（可以解码损坏的 Brotli）
- exiftool 的**写入**操作使用标准编码（默认不使用 Brotli）
- 结果：损坏的压缩数据 → 干净的未压缩数据

### 文件大小影响

极小。Brotli 压缩每个文件节省约 10-50 字节。示例：
- 修复前：367,843 字节（损坏的 Brotli）
- 修复后：367,830 字节（标准编码）
- 差异：-13 字节

## 修复工具

### 使用方法

```bash
./modern_format_boost/scripts/fix_brotli_exif.sh <目录>
```

### 功能特性

- 仅检测有 Brotli 损坏的文件
- 在 `.brotli_exif_backups/` 创建备份
- 保留所有元数据：
  - 文件大小（字节级精确）
  - 时间戳（修改时间、创建时间）
  - 扩展属性（xattr）
  - 所有 EXIF/XMP 数据
- 修复后验证
- 修复失败时恢复备份

### 输出示例

```
📦 77570528_p0-2.jxl
   ✓ 已修复

总计：检测到 20 个文件，修复 20 个，失败 0 个
```

## 预防措施

### 为什么无法预防？

**我们无法预防上游损坏。** 文件到达时已经损坏，来源包括：
- 图像下载工具
- 格式转换器
- 云同步服务

Modern Format Boost 的元数据保留功能按原样复制数据以保持保真度。

### 检测策略

用户可以在处理后验证文件：

```bash
exiftool -validate -warning -q -ext jxl -r <目录> 2>&1 | \
  grep "Corrupted Brotli"
```

如果输出为空，所有文件都是干净的。

## 统计数据

从 993 个 JXL 文件的调查中：
- **问题文件**：20 个（2.0%）
- **检测准确率**：100%（20/20 匹配 iCloud 错误）
- **修复成功率**：100%（在测试文件上验证）
- **元数据保留**：100%（所有字段完整）

## 参考资料

- 问题跟踪：`??BUG`
- 调查报告：`INVESTIGATION_SUMMARY.md`
- 修复工具：`scripts/fix_brotli_exif.sh`
- 测试脚本：`test_brotli_fix.sh`、`validate_metadata_corruption.sh`

## 日期

2026-02-20
