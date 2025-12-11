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

## 6. 结论

1. **质量匹配算法**已相当完善，主要提升空间在自动化检测
2. **探索模式**的二分搜索策略有效，但裁判指标可升级
3. **VMAF 建议作为可选功能加入**，用于高精度场景
4. **短期收益最大的改进**是自动 SI/TI 计算和胶片颗粒检测
