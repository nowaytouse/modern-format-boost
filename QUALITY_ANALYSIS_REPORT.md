# Modern Format Boost - 质量匹配与探索模式精度分析报告

**生成日期**: 2025-12-11
**分析版本**: v3.0+

---

## 1. 当前实现概述

### 1.1 质量匹配算法 (`quality_matcher.rs`)

核心公式：
```
effective_bpp = raw_bpp × gop_factor × chroma_factor × hdr_factor × aspect_factor
              × complexity_factor × grain_factor × mode_adjustment × resolution_factor
              × alpha_factor / codec_factor / color_depth_factor / target_adjustment
```

**CRF 计算**:
- AV1: `CRF = 50 - 6 × log2(effective_bpp × 100)`, 范围 [15, 40]
- HEVC: `CRF = 46 - 5 × log2(effective_bpp × 100)`, 范围 [0, 35]
- JXL: `distance = (100 - quality) / 10`, 范围 [0.0, 5.0]

**已实现的调整因子**:
| 因子 | 状态 | 影响范围 |
|------|------|----------|
| GOP 结构 | ✅ 完整 | 0.70-1.25 |
| 色度子采样 | ✅ 完整 | 1.0-1.20 |
| HDR 检测 | ✅ 基础 | 1.0-1.25 |
| 内容类型 | ✅ 完整 | -3 到 +5 CRF |
| 宽高比 | ✅ 完整 | 1.0-1.08 |
| 胶片颗粒 | ⚠️ 手动 | 1.0-1.20 |
| SI/TI 复杂度 | ⚠️ 可选 | 未强制 |

### 1.2 探索模式 (`video_explorer.rs`)

| 模式 | 策略 | 迭代次数 | 质量验证 |
|------|------|----------|----------|
| `--explore` | 二分搜索 | 8 | 无 |
| `--match-quality` | 单次编码 | 1 | SSIM ≥ 0.95 |
| `--explore --match-quality` | 二分+SSIM | 8 | SSIM ≥ min_ssim |

**当前质量指标**:
- SSIM: 标准 Wang et al. 2004 算法, 11×11 高斯窗口
- PSNR: 标准 MSE 计算, 阈值 35dB

---

## 2. 精度提升空间分析

### 2.1 🔴 高优先级: 集成 VMAF

**现状**: 项目未使用 VMAF，仅依赖 SSIM/PSNR

**问题**:
- SSIM 0.95 ≠ VMAF 70（相关性不完美）
- SSIM 对运动模糊不敏感
- PSNR 与人眼感知相关性较弱

**建议方案**:
```rust
// 在 video_explorer.rs 中添加
pub struct QualityThresholds {
    pub min_ssim: f64,      // 快速预检
    pub min_vmaf: f64,      // 主要裁判 (建议 >= 85)
    pub use_vmaf: bool,     // 启用 VMAF
}
```

**实现方式**:
1. 通过 FFmpeg 调用 libvmaf: `ffmpeg -i ref.mp4 -i dist.mp4 -lavfi libvmaf -f null -`
2. 或集成 Rust vmaf crate

**预期收益**: 在相同感知质量下减少 5-15% 文件大小

### 2.2 🔴 高优先级: 自动 SI/TI 计算

**现状**: SI/TI 为可选字段，未强制计算

**建议方案**:
```rust
// 在 video_quality_detector.rs 中添加
pub fn calculate_si(frame: &[u8], width: u32, height: u32) -> f64 {
    // Sobel 滤波器计算空间复杂度
    // SI = std(Sobel(frame))
}

pub fn calculate_ti(frame_curr: &[u8], frame_prev: &[u8]) -> f64 {
    // 帧间差异计算时间复杂度
    // TI = std(frame_curr - frame_prev)
}
```

**预期收益**: 减少 3-8% CRF 误差

### 2.3 🟡 中优先级: 胶片颗粒自动检测

**现状**: `has_film_grain` 需手动标记

**建议方案**:
```rust
pub fn detect_film_grain(frames: &[Frame]) -> f64 {
    // 1. 计算高频能量 (Laplacian)
    // 2. 分析帧间高频差异
    // 3. 返回颗粒强度 [0.0, 1.0]
}
```

**预期收益**: 胶片内容 CRF 精度 ±1

### 2.4 🟡 中优先级: 增强 HDR 检测

**现状**: 基于 BT.2020 + is_hdr 标志

**建议增强**:
- 检测 PQ (SMPTE 2084) 转移函数
- 检测 HLG 转移函数
- 解析 MaxCLL/MaxFALL 元数据
- 检测 Dolby Vision 配置

**预期收益**: HDR 内容 CRF 精度 ±2

### 2.5 🟢 低优先级: GOP 因子细化

**现状**: 基于 GOP 大小和 B 帧数的静态因子

**可增强**:
- B 帧金字塔深度检测
- 参考帧数量分析
- CABAC vs CAVLC 检测

**预期收益**: 减少 2-5% CRF 误差

---

## 3. VMAF 集成建议

### 3.1 是否有必要加入 VMAF?

**结论: 建议加入，但作为可选功能**

**理由**:
1. VMAF 与人眼感知相关性更高 (Pearson 0.93 vs SSIM 0.85)
2. Netflix 等大规模视频平台的标准指标
3. 对运动、模糊、压缩伪影更敏感

**权衡**:
| 方面 | SSIM | VMAF |
|------|------|------|
| 计算速度 | 快 (~10ms/帧) | 慢 (~100ms/帧) |
| 依赖 | 无 | libvmaf |
| 精度 | 良好 | 优秀 |
| 适用场景 | 快速预检 | 最终验证 |

### 3.2 推荐实现策略

```
阶段 1: SSIM 快速筛选 (当前)
    ↓ SSIM < 0.90 → 直接拒绝
阶段 2: VMAF 精确验证 (新增)
    ↓ VMAF < 85 → 降低 CRF 重试
阶段 3: 输出最终结果
```

### 3.3 VMAF 阈值建议

| 质量等级 | VMAF 分数 | 适用场景 |
|----------|-----------|----------|
| 优秀 | ≥ 93 | 存档、专业 |
| 良好 | ≥ 85 | 流媒体、日常 |
| 可接受 | ≥ 75 | 移动端、低带宽 |
| 较差 | < 75 | 不推荐 |

---

## 4. 精度验证现状

项目已包含 100+ 精度测试，覆盖:
- 标准分辨率 (720p, 1080p, 4K, 8K)
- 不同比特率 (500kbps - 100Mbps)
- 内容类型 (动画、胶片、HDR)
- GOP 结构 (all-intra, long-gop)
- 色度采样 (420, 422, 444)

**当前精度**: CRF ±1-2 范围内

---

## 5. 优先级排序总结

| 优先级 | 改进项 | 预期收益 | 实现难度 |
|--------|--------|----------|----------|
| 🔴 P0 | VMAF 集成 | 5-15% | 中 |
| 🔴 P0 | 自动 SI/TI | 3-8% | 低 |
| 🟡 P1 | 胶片颗粒检测 | 3-6% | 中 |
| 🟡 P1 | HDR 增强 | 2-4% | 低 |
| 🟢 P2 | GOP 细化 | 2-5% | 中 |
| 🟢 P2 | 分辨率因子 | 1-2% | 低 |

---

## 6. 代码审查：可疑 Fallback 和"作弊"行为分析

### 6.1 🔴 发现的 Fallback 行为

| 位置 | 行为 | 风险等级 | 说明 |
|------|------|----------|------|
| `quality_matcher.rs:527` | `.max(0.001)` 安全回退 | 🟡 中 | 当 BPP 接近 0 时使用 0.001，可能掩盖数据问题 |
| `quality_matcher.rs:525-527` | CRF 上限 35 | 🟢 低 | 极低 BPP 时限制 CRF，合理的边界保护 |
| `quality_matcher.rs:820` | B-frame 假设 | 🟡 中 | `unwrap_or(if has_b_frames { 2 } else { 0 })` 假设 B 帧数量 |
| `quality_matcher.rs:966-974` | FPS 默认值 | 🟢 低 | 根据编码器类型推断 FPS，合理的回退 |
| `quality_matcher.rs:1009` | GOP 未知时假设 1.0 | 🟢 低 | 使用中等 GOP 作为默认，保守策略 |
| `quality_matcher.rs:1207-1210` | 复杂度因子硬编码 | 🟡 中 | 当 SI/TI 不可用时使用 BPP 比率估计 |

### 6.2 🟢 无"作弊"行为

经审查，代码中**未发现明显的作弊行为**：
- ✅ 所有计算步骤都有执行，无跳过
- ✅ 使用精确的对数公式而非查表
- ✅ 浮点计算保持 f64 精度直到最终舍入
- ✅ 质量验证失败时正确返回 false，不静默通过

### 6.3 精度损失点

| 位置 | 问题 | 影响 |
|------|------|------|
| `quality_matcher.rs:550` | `round()` 舍入 | ±0.5 CRF |
| `video_explorer.rs:481` | `(low + high) / 2` 整数除法 | 无法探索 CRF 23.5 |
| `video_explorer.rs:786` | SSIM epsilon = 0.0001 | 精度足够，无问题 |

---

## 7. 小数点级别 CRF 精度可行性分析

### 7.1 当前限制

```rust
// 当前实现
let crf = (crf_with_bias.round() as i32).clamp(15, 40) as u8;
```

CRF 被强制转换为 `u8`，无法表示 23.5 这样的值。

### 7.2 FFmpeg 支持情况

FFmpeg **支持浮点 CRF**：
```bash
ffmpeg -i input.mp4 -c:v libsvtav1 -crf 23.5 output.mp4  # ✅ 有效
ffmpeg -i input.mp4 -c:v libx265 -crf 22.3 output.mp4   # ✅ 有效
```

### 7.3 实现方案

```rust
// 方案 A: 使用 f32 存储 CRF
pub struct MatchedQuality {
    pub crf: f32,  // 允许 23.5
    // ...
}

// 方案 B: 使用量化值 (0-255 表示 0-63.75)
pub struct MatchedQuality {
    pub crf_q8: u8,  // 实际 CRF = crf_q8 / 4.0
    // ...
}
```

### 7.4 二分搜索改进

```rust
// 当前：整数搜索
let mid = (low + high) / 2;  // 18, 19 → mid = 18

// 改进：浮点搜索 (步长 0.5)
let mid = ((low + high) / 2.0 * 2.0).round() / 2.0;  // 支持 18.5
```

### 7.5 预期收益

| 精度 | 当前 | 改进后 |
|------|------|--------|
| CRF 步长 | 1 | 0.5 |
| 文件大小变化 | ~5-8% | ~2-4% |
| 质量匹配精度 | ±1 CRF | ±0.5 CRF |

---

## 8. 进一步提升建议

### 8.1 立即可做 (无需架构改动)

1. **移除 `.max(0.001)` 回退**，改为返回错误
2. **验证 B-frame 数量**，不使用假设值
3. **添加详细日志**记录所有 fallback 情况

### 8.2 短期改进 (1-2 周)

1. **实现浮点 CRF 支持** (0.5 步长)
2. **修复二分搜索整数截断**
3. **使用连续函数替代离散阈值**

### 8.3 中期改进 (2-4 周)

1. **VMAF 已集成** ✅ (v3.3)
2. **自动 SI/TI 计算**
3. **胶片颗粒自动检测**

---

## 9. 自动模式路由精度分析

### 9.1 视频自动路由 (`conversion_api.rs`)

**路由流程**:
```
输入视频 → detect_video() → determine_strategy() → auto_convert()
```

**跳过检测** (已完善 ✅):
- H.265/HEVC → SKIP
- AV1 → SKIP
- VP9 → SKIP
- VVC/H.266 → SKIP
- AV2 → SKIP

**压缩类型判断** (`detection_api.rs:188-221`):
| BPP 范围 | 压缩类型 | 默认 CRF |
|----------|----------|----------|
| Lossless 编码器 | Lossless | 0 (无损) |
| ProRes/DNxHD | VisuallyLossless | 18 |
| > 2.0 | VisuallyLossless | 18 |
| 0.5 - 2.0 | HighQuality | 20 |
| 0.1 - 0.5 | Standard | 20 |
| < 0.1 | LowQuality | 20 |

**发现的问题**:

| 问题 | 位置 | 影响 | 建议 |
|------|------|------|------|
| BPP 使用总比特率 | `detection_api.rs:257` | 高估 10-30% | 优先使用 `video_bitrate` |
| 未传递 GOP 信息 | `conversion_api.rs:382-393` | GOP 因子始终为 1.0 | 添加 GOP 检测 |
| 未传递 pix_fmt | `from_video_detection()` | 色度因子始终为 1.0 | 传递 pix_fmt |
| 未传递 color_space | `from_video_detection()` | HDR 因子始终为 1.0 | 传递 color_space |
| 内容类型未自动检测 | 全局 | 无法优化动画/屏幕录制 | 添加自动检测 |

### 9.2 图像自动路由 (`image_quality_detector.rs`)

**路由流程**:
```
输入图像 → analyze_image_quality() → classify_content_type() → make_routing_decision()
```

**内容类型分类** (已完善 ✅):
| 类型 | 检测条件 | 质量调整 |
|------|----------|----------|
| Animation | `is_animated` | 0 |
| Icon | 小尺寸 + alpha + 低复杂度 | +6 |
| Screenshot | 屏幕比例 + 低色彩多样性 + 锐利边缘 | +8 |
| Graphic | 低色彩多样性 + 定义边缘 | +5 |
| Photo | 高复杂度 + 高色彩多样性 | -2 |
| Artwork | 中等复杂度 + 定义边缘 | +2 |

**格式推荐** (已完善 ✅):
| 内容类型 | 推荐格式 |
|----------|----------|
| Photo | avif, jxl, webp, jpeg |
| Artwork | avif, webp, jxl, png |
| Screenshot | webp, png, avif |
| Icon | webp, png, avif |
| Animation | webp, avif, gif |

**跳过检测** (已完善 ✅):
- avif, jxl, heic, heif → SKIP (避免代际损失)

### 9.3 路由精度提升空间

#### 🔴 高优先级

1. **视频 BPP 计算改进**
   ```rust
   // 当前：使用总比特率
   let bits_per_pixel = (probe.bit_rate as f64) / pixels_per_second;

   // 改进：优先使用视频比特率
   let bits_per_pixel = if let Some(vbr) = probe.video_bit_rate {
       (vbr as f64) / pixels_per_second
   } else {
       (probe.bit_rate as f64) / pixels_per_second
   };
   ```

2. **传递完整的视频分析数据**
   ```rust
   // 当前：from_video_detection() 缺少关键字段
   // 改进：使用 VideoAnalysisBuilder
   let analysis = VideoAnalysisBuilder::new()
       .basic(codec, width, height, fps, duration)
       .video_bitrate(video_bitrate)
       .gop(gop_size, b_frames)
       .pix_fmt(pix_fmt)
       .color(color_space, is_hdr)
       .build();
   ```

#### 🟡 中优先级

3. **视频内容类型自动检测**
   - 基于 SI/TI 指标区分动画/实景
   - 基于帧间差异检测屏幕录制
   - 基于色彩分布检测动画

4. **图像分类阈值优化**
   - 当前阈值基于经验值
   - 可通过机器学习优化

#### 🟢 低优先级

5. **动态质量阈值**
   ```rust
   let min_ssim = match content_type {
       ContentType::Animation => 0.92,
       ContentType::FilmGrain => 0.97,
       _ => 0.95,
   };
   ```

---

## 10. v3.4 改进确认

### 10.1 已确认的改进 ✅

| 改进项 | 位置 | 状态 |
|--------|------|------|
| CRF 类型改为 f32 | `conversion_api.rs:105` | ✅ 已实现 |
| VMAF 验证支持 | `video_explorer.rs:814-827` | ✅ 已实现 |
| 质量指标三重验证 | `video_explorer.rs:602-622` | ✅ 已实现 |
| VideoAnalysisBuilder | `quality_matcher.rs:1600-1701` | ✅ 已实现 |
| 精度常量模块 | `video_explorer.rs:993-1123` | ✅ 已实现 |

### 10.2 待改进项

| 改进项 | 优先级 | 预期收益 |
|--------|--------|----------|
| 视频 BPP 使用 video_bitrate | 🔴 P0 | 10-30% 精度提升 |
| 传递 GOP/pix_fmt/color_space | 🔴 P0 | 5-15% 精度提升 |
| 视频内容类型自动检测 | 🟡 P1 | 3-8% 精度提升 |
| 浮点 CRF 二分搜索 | 🟡 P1 | ±0.5 CRF 精度 |
| 动态质量阈值 | 🟢 P2 | 2-5% 精度提升 |

---

## 11. 结论

1. **质量匹配算法**已相当完善，主要提升空间在自动化检测
2. **探索模式**的二分搜索策略有效，VMAF 已集成 (v3.3)
3. **CRF 已改为 f32** (v3.4)，支持小数点精度
4. **无明显作弊行为**，代码逻辑正确
5. **自动路由**图像部分完善，视频部分需补充字段传递
6. **最大精度提升空间**在于视频 BPP 计算和完整字段传递
