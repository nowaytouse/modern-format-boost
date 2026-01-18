# 文件夹结构保留 - 完整审计报告

## 🎯 审计目标
确保所有文件复制操作都正确保留目录结构，避免文件被复制到输出根目录。

## ✅ 已修复的位置（使用 smart_file_copier）

### imgquality_hevc
1. ✅ `lossless_converter.rs:62` - skip 场景
2. ✅ `lossless_converter.rs:967` - 失败场景
3. ✅ `conversion_api.rs:168` - NoConversion 场景
4. ✅ `main.rs` - copy_original_if_adjacent_mode()

### vidquality_hevc
1. ✅ `conversion_api.rs:170` - NoConversion 场景
2. ✅ `conversion_api.rs:424` - 失败场景 (GPU)
3. ✅ `conversion_api.rs:475` - 失败场景 (CPU)
4. ✅ `conversion_api.rs:565` - 失败场景 (x265)

## ⚠️ 需要修复的位置

### imgquality_av1
**文件**: `imgquality_av1/src/conversion_api.rs:179`
**场景**: NoConversion skip
**当前代码**:
```rust
let file_name = input_path.file_name().unwrap_or_default();
out_dir.join(file_name)  // ❌ 丢失目录结构
```
**状态**: 有 base_dir 逻辑但未使用 smart_file_copier
**优先级**: 🔴 高

### vidquality_av1
**文件**: `vidquality_av1/src/conversion_api.rs:176`
**场景**: NoConversion skip
**当前代码**:
```rust
let file_name = input.file_name().unwrap_or_default();
out_dir.join(file_name)  // ❌ 丢失目录结构
```
**状态**: 有 base_dir 逻辑但未使用 smart_file_copier
**优先级**: 🔴 高

### shared_utils/cli_runner.rs
**文件**: `shared_utils/src/cli_runner.rs:144`
**场景**: 转换失败时的 fallback 复制
**当前代码**:
```rust
let file_name = file.file_name().unwrap_or_default();
let dest = out_dir.join(file_name);  // ❌ 丢失目录结构
```
**状态**: 没有 base_dir 逻辑
**优先级**: 🔴 高

## 📋 修复计划

### 方案1: 统一使用 smart_file_copier（推荐）
- 优点: 代码一致性好，维护简单
- 缺点: 需要确保所有地方都传递 base_dir

### 方案2: 保留现有 base_dir 逻辑
- 优点: 改动最小
- 缺点: 代码重复，不利于维护

**建议**: 采用方案1，统一使用 `smart_file_copier` 模块

## 🔍 检查清单

- [x] imgquality_hevc - 所有复制操作
- [x] vidquality_hevc - 所有复制操作
- [ ] imgquality_av1 - NoConversion 场景
- [ ] vidquality_av1 - NoConversion 场景
- [ ] cli_runner.rs - 失败 fallback 场景

## 🧪 测试建议

创建测试用例：
```
test_dir/
├── subdir1/
│   └── file1.jpg
├── subdir2/
│   └── file2.jpg
└── file3.jpg
```

预期输出：
```
output_dir/
├── subdir1/
│   └── file1.avif
├── subdir2/
│   └── file2.avif
└── file3.avif
```

## 📝 注意事项

1. **base_dir 必须正确传递**: 确保 ConversionConfig 包含 base_dir 字段
2. **相对路径计算**: 使用 `strip_prefix(base_dir)` 计算相对路径
3. **目录创建**: 使用 `create_dir_all()` 创建父目录
4. **元数据保留**: 使用 `copy_metadata()` 保留时间戳等
5. **XMP 合并**: 使用 `merge_xmp_for_copied_file()` 合并 XMP 文件
