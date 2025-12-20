# VMAF → MS-SSIM 迁移总结

## 概述

本次更新将项目中的视频质量评估指标从 **VMAF** (Video Multimethod Assessment Fusion) 改为 **MS-SSIM** (Multi-Scale Structural Similarity)。

## 主要原因

1. **MS-SSIM 更适合评估一致性**：本项目的目标是追求处理前后的视觉一致性，而 MS-SSIM 更专注于结构相似性
2. **计算效率**：MS-SSIM 比 VMAF 计算速度更快
3. **短内容自动启用**：默认对短内容（<5分钟）自动使用 MS-SSIM，用户可以强制对长内容启用

## 主要变更

### 1. 核心数据结构 (shared_utils/src/video_explorer.rs)

#### QualityThresholds 结构体
```rust
// 旧版本
pub struct QualityThresholds {
    pub min_vmaf: f64,          // 0-100 范围
    pub validate_vmaf: bool,
    pub force_vmaf_long: bool,
}

// 新版本
pub struct QualityThresholds {
    pub min_ms_ssim: f64,       // 0-1 范围
    pub validate_ms_ssim: bool,
    pub force_ms_ssim_long: bool,
}
```

#### ExploreResult 结构体
```rust
// 旧版本
pub struct ExploreResult {
    pub vmaf: Option<f64>,  // 0-100
}

// 新版本
pub struct ExploreResult {
    pub ms_ssim: Option<f64>,  // 0-1
}
```

#### 默认阈值更新
```rust
// 旧版本
pub const EXPLORE_DEFAULT_MIN_VMAF: f64 = 85.0;
pub const DEFAULT_MIN_VMAF: f64 = 85.0;
pub const HIGH_QUALITY_MIN_VMAF: f64 = 93.0;
pub const ACCEPTABLE_MIN_VMAF: f64 = 75.0;

// 新版本
pub const EXPLORE_DEFAULT_MIN_MS_SSIM: f64 = 0.90;
pub const DEFAULT_MIN_MS_SSIM: f64 = 0.90;
pub const HIGH_QUALITY_MIN_MS_SSIM: f64 = 0.95;
pub const ACCEPTABLE_MIN_MS_SSIM: f64 = 0.85;
```

### 2. CLI 参数更新

#### vidquality_hevc/src/main.rs
```bash
# 旧版本
--vmaf                    # 启用 VMAF 验证
--vmaf-threshold 85.0     # VMAF 阈值 (0-100)
--force-vmaf-long         # 强制对长视频启用

# 新版本
--ms-ssim                 # 启用 MS-SSIM 验证
--ms-ssim-threshold 0.90  # MS-SSIM 阈值 (0-1)
--force-ms-ssim-long      # 强制对长视频启用
```

### 3. 函数更新

#### 计算函数
```rust
// 旧版本
pub fn calculate_vmaf(input: &Path, output: &Path) -> Option<f64>
pub fn is_valid_vmaf(vmaf: f64) -> bool
pub fn vmaf_quality_grade(vmaf: f64) -> &'static str
pub fn format_vmaf(vmaf: f64) -> String

// 新版本
pub fn calculate_ms_ssim(input: &Path, output: &Path) -> Option<f64>
pub fn is_valid_ms_ssim(ms_ssim: f64) -> bool
pub fn ms_ssim_quality_grade(ms_ssim: f64) -> &'static str
pub fn format_ms_ssim(ms_ssim: f64) -> String
```

#### FFmpeg 滤镜命令更新
```bash
# 旧版本
ffmpeg -lavfi "libvmaf=log_fmt=json:log_path=/dev/stdout"

# 新版本
ffmpeg -lavfi "libvmaf=feature=name=ms_ssim:log_fmt=json:log_path=/dev/stdout"
```

### 4. MS-SSIM 分数解读

```rust
if ms_ssim >= 0.95 {
    "Excellent (几乎无法区分)"
} else if ms_ssim >= 0.90 {
    "Good (流媒体质量)"
} else if ms_ssim >= 0.85 {
    "Acceptable (移动端质量)"
} else if ms_ssim >= 0.80 {
    "Fair (可见差异)"
} else {
    "Poor (明显质量损失)"
}
```

### 5. 归一化逻辑更新

```rust
// 旧版本 - VMAF 是 0-100 范围，需要归一化
let vmaf_norm = (vmaf / 100.0).clamp(0.0, 1.0);

// 新版本 - MS-SSIM 本身就是 0-1 范围
let ms_ssim_norm = ms_ssim.clamp(0.0, 1.0);
```

### 6. 质量评分权重

```rust
// 三重验证权重保持不变
if validate_ms_ssim && validate_psnr {
    // SSIM 50%, MS-SSIM 35%, PSNR 15%
    ssim_norm * 0.50 + ms_ssim_norm * 0.35 + psnr_norm * 0.15
}
```

## 文件变更清单

### Rust 源代码
- ✅ shared_utils/src/video_explorer.rs
- ✅ shared_utils/src/explore_strategy.rs
- ✅ vidquality_hevc/src/main.rs
- ✅ vidquality_hevc/src/conversion_api.rs
- ✅ vidquality_av1/src/main.rs
- ✅ vidquality_av1/src/conversion_api.rs

### 测试文件
- ✅ shared_utils/src/video_explorer_tests.rs
- ✅ imgquality_hevc/tests/test_explorer.rs

### Shell 脚本
- ✅ scripts/test_ms_ssim_ssim_v5.75.sh (重命名自 test_vmaf_ssim_v5.75.sh)
- ✅ scripts/test_long_video_cpu_stepping.sh
- ✅ scripts/drag_and_drop_processor.sh
- ✅ 所有其他相关脚本

## 编译验证

所有模块编译通过：
```bash
✅ cargo check --manifest-path shared_utils/Cargo.toml
✅ cargo check --manifest-path vidquality_hevc/Cargo.toml
✅ cargo check --manifest-path vidquality_av1/Cargo.toml
✅ cargo check --manifest-path imgquality_hevc/Cargo.toml
```

## 使用示例

### 命令行使用
```bash
# 启用 MS-SSIM 验证（短视频自动启用）
./vidquality-hevc auto input.mp4 --ms-ssim --ms-ssim-threshold 0.90

# 强制对长视频启用 MS-SSIM
./vidquality-hevc auto long_video.mp4 --ms-ssim --force-ms-ssim-long

# 配合探索模式使用
./vidquality-hevc auto input.mp4 --explore --match-quality --ms-ssim
```

### 代码使用
```rust
use shared_utils::{QualityThresholds, ExploreConfig};

let thresholds = QualityThresholds {
    min_ssim: 0.95,
    min_psnr: 35.0,
    min_ms_ssim: 0.90,      // 使用 MS-SSIM 而不是 VMAF
    validate_ssim: true,
    validate_psnr: false,
    validate_ms_ssim: true,  // 启用 MS-SSIM 验证
    force_ms_ssim_long: false,
};
```

## 向后兼容性

⚠️ **不兼容变更**：
- 旧的 `--vmaf` 参数已移除，必须使用 `--ms-ssim`
- VMAF 相关的配置字段已全部重命名
- 阈值范围从 0-100 改为 0-1

## 技术细节

### MS-SSIM vs VMAF

| 特性 | MS-SSIM | VMAF |
|------|---------|------|
| 范围 | 0-1 | 0-100 |
| 速度 | 快 | 慢 |
| 重点 | 结构相似性 | 感知质量 |
| 多尺度 | ✅ 是 | ❌ 否 |
| 适用场景 | 一致性评估 | 主观质量评估 |

### FFmpeg 依赖

MS-SSIM 通过 libvmaf 的 feature 功能实现：
```bash
ffmpeg -lavfi "libvmaf=feature=name=ms_ssim:log_fmt=json:log_path=/dev/stdout"
```

需要 ffmpeg 编译时包含 libvmaf 支持。

## 测试建议

1. **短视频测试** (< 5分钟)
   - 验证 MS-SSIM 自动启用
   - 检查阈值是否正确应用 (0.90)

2. **长视频测试** (> 5分钟)
   - 验证默认跳过 MS-SSIM
   - 测试 --force-ms-ssim-long 标志

3. **质量验证**
   - 对比 MS-SSIM 与 SSIM 的相关性
   - 验证分数在合理范围 (0.85-0.99)

## 迁移日期

2025-12-20
