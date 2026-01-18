# Metadata Preservation - 元数据保留机制

## ✅ 状态：已实现并验证

### 功能概述
所有转换工具都会自动保留原始文件的元数据，包括：

1. **文件时间戳**
   - 修改时间 (mtime)
   - 访问时间 (atime)

2. **文件权限**
   - Unix 权限位
   - 所有者信息

3. **扩展属性**
   - macOS: Finder 标签、备注等
   - Linux: xattrs

4. **内部元数据**
   - Exif 信息
   - ICC 颜色配置文件
   - MakerNotes

5. **XMP 边车文件**
   - 自动检测并合并 `.xmp` 文件
   - 支持 `photo.jpg.xmp` 和 `photo.xmp`

### 实现细节

#### Rust 工具
所有转换后自动调用 `copy_metadata()`：
```rust
// shared_utils/src/conversion.rs:486
if let Err(e) = crate::preserve_metadata(input, output) {
    eprintln!("⚠️ Failed to preserve metadata: {}", e);
}
```

关键修复（v5.76）：
```rust
// 先保存时间戳
let src_times = std::fs::metadata(src).ok().map(|m| {
    (filetime::FileTime::from_last_access_time(&m),
     filetime::FileTime::from_last_modification_time(&m))
});

// ... 执行 exiftool 等操作 ...

// 最后恢复时间戳（exiftool 会修改时间戳）
if let Some((atime, mtime)) = src_times {
    let _ = filetime::set_file_times(dst, atime, mtime);
}
```

#### 双击脚本
使用 `rsync -av` 复制非媒体文件：
```bash
rsync -av --ignore-existing "${excludes[@]}" "$TARGET_DIR/" "$OUTPUT_DIR/"
```

`-a` (archive) 参数保留所有元数据。

### 测试验证

```bash
# 创建文件并设置时间戳为 2020-01-01
touch -t 202001011200 test.png

# 转换
./imgquality-hevc auto test.png --output out/

# 验证
stat test.png      # 修改时间: Jan 1 12:00:00 2020
stat out/test.jxl  # 修改时间: Jan 1 12:00:00 2020 ✅
```

### 状态
✅ 时间戳保留：正常
✅ 权限保留：正常
✅ XMP 合并：正常
✅ 内部元数据：正常
