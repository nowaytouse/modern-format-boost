# 🔥 如何确保使用最新二进制 / How to Use Latest Binary

## 问题 / Problem

如果你发现文件夹结构没有保留，可能是因为使用了**旧版本的二进制文件**。

If directory structure is not preserved, you may be using an **old binary**.

---

## ✅ 解决方案 / Solution

### 方法 1: 使用 Smart Build 脚本（推荐）

```bash
cd modern_format_boost
bash scripts/smart_build.sh
```

这会：
- 🧹 自动清理旧二进制文件
- 🔍 检查源代码时间戳
- 🔨 只在需要时重新编译
- ✅ 确保使用最新版本

### 方法 2: 强制重新编译

```bash
cd modern_format_boost
bash scripts/force_rebuild.sh
```

这会：
- 🧹 清理所有构建缓存
- 🔨 强制重新编译
- ✅ 生成全新的二进制文件

### 方法 3: 手动检查二进制时间戳

```bash
ls -lh target/release/imgquality-hevc
```

**最新版本时间戳**: `2026-01-18 16:13:43` 或更新

如果时间戳早于这个时间，请重新编译！

---

## 🧪 验证修复 / Verify Fix

运行测试脚本：

```bash
bash scripts/test_structure_preservation.sh
```

应该看到：
```
✅ SUCCESS: Directory structure preserved!
```

---

## 📋 使用拖放脚本 / Using Drag & Drop Script

拖放脚本会自动调用 `smart_build.sh`，确保使用最新二进制：

```bash
bash scripts/drag_and_drop_processor.sh /path/to/your/folder
```

或直接双击 `drag_and_drop_processor.sh`

---

## 🔍 如何确认问题已修复 / How to Confirm Fix

1. **检查二进制时间戳** >= `2026-01-18 16:13:43`
2. **运行测试脚本** 看到 ✅ SUCCESS
3. **实际测试** 处理包含子目录的文件夹

---

## 📞 仍然有问题？ / Still Having Issues?

如果按照以上步骤操作后仍然有问题：

1. 删除所有旧二进制：
   ```bash
   find . -name "imgquality-hevc" -not -path "*/target/*" -delete
   ```

2. 清理并重新编译：
   ```bash
   cargo clean
   cargo build --release --manifest-path imgquality_hevc/Cargo.toml
   ```

3. 检查代码是否包含修复：
   ```bash
   grep "strip_prefix" imgquality_hevc/src/main.rs
   ```
   应该看到 `let rel_path = input.strip_prefix(base_dir)`
