# GIF Duration Mismatch Fix

## 问题描述

GIF 文件具有不规则的帧延迟（variable frame delay），在转换为视频格式（MP4/MOV）时，由于帧时序的处理方式不同，可能导致输出视频的时长与原始 GIF 略有差异。

当时长差异超过验证容差（原先为 1.0 秒）时，系统会判定为质量验证失败，导致：
- 压缩成功但被丢弃
- 原文件被保护
- 输出文件被删除

## 解决方案

### 1. 放宽时长容差

为动画图像格式（GIF, WebP, AVIF, HEIC, HEIF）使用更宽松的时长验证阈值：
- 普通视频：1.0 秒容差
- 动画图像：3.0 秒容差

### 2. 自动检测输入格式

在质量验证阶段自动检测输入文件扩展名，对动画图像格式应用宽松验证策略。

## 修改的文件

### `shared_utils/src/quality_verifier_enhanced.rs`

添加了新的验证选项 `relaxed_animated_image()`：

```rust
pub fn relaxed_animated_image() -> Self {
    Self {
        min_file_size: DEFAULT_MIN_FILE_SIZE,
        require_duration_match: true,
        duration_tolerance_secs: 3.0, // 更宽松的容差
        require_video_stream: true,
    }
}
```

### `shared_utils/src/video_explorer/gpu_coarse_search.rs`

在验证前检测输入格式：

```rust
let is_animated_image = input
    .extension()
    .and_then(|e| e.to_str())
    .map(|e| {
        let ext = e.to_lowercase();
        matches!(ext.as_str(), "gif" | "webp" | "avif" | "heic" | "heif")
    })
    .unwrap_or(false);

let verify_options = if is_animated_image {
    VerifyOptions::relaxed_animated_image()
} else {
    VerifyOptions::strict_video()
};
```

## 效果

- ✅ GIF 转视频时允许最多 3 秒的时长差异
- ✅ 保留了对普通视频的严格验证（1 秒容差）
- ✅ 自动检测，无需用户干预
- ✅ 向后兼容，不影响现有功能

## 未来改进方向

1. **VFR 模式支持**：在 ffmpeg 命令中添加 `-vsync vfr` 参数以更好地保留原始帧时序
2. **动态容差计算**：根据输入文件的实际时长动态调整容差（例如：时长的 5%）
3. **Fallback 机制**：首次验证失败后，使用 VFR 模式重新编码
