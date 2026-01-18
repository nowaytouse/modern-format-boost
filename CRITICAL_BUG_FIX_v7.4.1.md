# 🚨 CRITICAL BUG FIX v7.4.1

## 问题描述

**BUG 再次复现！** 文件 `4h8uh4vkss9clo2wfiy30kach.gif` 被复制到根目录而不是 `1/参考/内容 猎奇/`

## 根本原因

`lossless_converter.rs` line 967 的失败场景复制代码**没有使用 base_dir**，导致文件被复制到根目录。

```rust
// ❌ WRONG (line 967 - 旧代码)
let file_name = input.file_name().unwrap_or_default();
let dest = out_dir.join(file_name);  // 丢失目录结构！
```

## 已修复的位置

1. ✅ `imgquality_hevc/src/lossless_converter.rs` - line 62 (copy_original_on_skip)
2. ✅ `imgquality_hevc/src/lossless_converter.rs` - line 967 (失败场景)
3. ✅ 使用统一的 `smart_file_copier` 模块

## 修复方案

使用 `shared_utils::copy_on_skip_or_fail()` 统一处理所有文件复制：

```rust
// ✅ CORRECT
shared_utils::copy_on_skip_or_fail(
    input,
    options.output_dir.as_deref(),
    options.base_dir.as_deref(),
    verbose
)?;
```

## 需要验证的其他文件

- `imgquality_hevc/src/conversion_api.rs:168`
- `imgquality_hevc/src/main.rs` (copy_original_if_adjacent_mode)
- `shared_utils/src/cli_runner.rs:143`

## 测试步骤

1. 重新编译：`bash scripts/smart_build.sh --hevc --force`
2. 测试：`bash scripts/test_structure_preservation.sh`
3. 实际测试：处理包含子目录的文件夹

## 预防措施

- ✅ 创建了 `smart_file_copier` 模块作为单一真相来源
- ✅ 所有文件复制必须使用此模块
- ⚠️  需要代码审查确保所有地方都使用了此模块
