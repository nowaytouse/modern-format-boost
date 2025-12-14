# v5.54 → v5.60 改进路线图

**开始时间**: 2025-12-14
**目标**: 融合旧版本的鲁棒性 + 新版本的速度
**策略**: 三层改进，逐步实施，每步都可验证

## 🎯 改进目标

### 丢失的功能分析

| 功能 | v5.2 (旧版) | v5.54 (新版) | 状态 |
|------|-----------|-----------|------|
| 三阶段结构 | ✅ 清晰 | ❌ 简化 | 需要恢复 |
| 智能提前终止 | ✅ 有 | ❌ 无 | 需要恢复 |
| 采样 vs 完整编码 | ✅ 分离 | ❌ 混合 | 需要分离 |
| GPU→CPU 校准 | ✅ 有 | ❌ 无 | 需要添加 |
| 精度控制 | ✅ 0.1 步进 | ❌ 0.1 步进 | 需要优化 |

### 改进收益预期

| 改进 | 预期收益 | 难度 | 优先级 |
|------|---------|------|--------|
| 精度调整 (0.25 步进) | 速度 +2-3x | ⭐ | 1️⃣ |
| 三阶段结构恢复 | 鲁棒性 +50% | ⭐⭐⭐ | 1️⃣ |
| 预检查增强 | UX 改进 | ⭐ | 2️⃣ |
| GPU→CPU 校准 | 精度 +20% | ⭐⭐ | 2️⃣ |
| 最坏情况采样 | 可靠性 +30% | ⭐⭐⭐ | 3️⃣ |
| 时间预算机制 | 可控性 +100% | ⭐⭐ | 3️⃣ |
| 置信度输出 | 透明度 +100% | ⭐⭐ | 3️⃣ |

## 📋 第一层：精度调整 (v5.55)

### 目标
将 CPU 阶段的 0.1 步进改为 0.25 步进，速度提升 2-3 倍

### 改动位置

#### 位置 1: 常量定义 (shared_utils/src/gpu_accel.rs, ~280 行)
```rust
// 当前
pub const ULTRA_FINE_STEP: f32 = 0.1;

// 改为
pub const ULTRA_FINE_STEP: f32 = 0.25;
```

#### 位置 2: 缓存精度 (shared_utils/src/gpu_accel.rs, ~5000 行)
```rust
// 当前 - 支持 0.01 精度
let key = (crf * 100.0).round() as i32;

// 改为 - 支持 0.25 精度
let key = (crf * 4.0).round() as i32;
```

#### 位置 3: 搜索步长 (shared_utils/src/gpu_accel.rs, ~6500 行)
```rust
// 当前
test_crf -= 0.1;

// 改为
test_crf -= 0.25;
```

### 验证方法
```bash
# 编译
cargo build --release

# 测试速度
time ./vidquality_hevc/target/release/vidquality-hevc auto test.mp4 --explore --match-quality --compress

# 预期: 速度提升 2-3 倍
```

### 回滚方案
```bash
git revert <commit-hash>
```

---

## 📋 第二层：结构化改进 (v5.56-v5.57)

### 目标
恢复旧版本的三阶段结构，融合到新版本中

### 新的搜索流程

```
Phase 1: GPU 粗搜索 (explore_with_gpu_coarse_search)
  ├─ 目标: 用 GPU 快速排除不可能的 CRF 范围
  ├─ 输出: 压缩边界的大致位置 (如 CRF 35-40)
  └─ 时间: 2-5 分钟

Phase 2: CPU 边界定位 (cpu_boundary_search)
  ├─ 目标: 用 CPU 精确找到压缩边界
  ├─ 策略: 从 GPU 边界开始，0.5 步进二分搜索
  ├─ 输出: 最低能压缩的 CRF (如 CRF 38.5)
  └─ 时间: 1-2 分钟

Phase 3: CPU 精细化 (cpu_fine_tune_v2)
  ├─ 目标: 在边界 ±1.0 范围内用 0.25 步进找最优点
  ├─ 策略: 向下探索 (更高质量) → 向上确认 (边界验证)
  ├─ 输出: 最优 CRF (如 CRF 38.0)
  └─ 时间: 1-2 分钟

Phase 4: SSIM 验证 (ssim_validation)
  ├─ 目标: 验证最优点的质量
  ├─ 输出: SSIM 值和质量等级
  └─ 时间: 30-60 秒
```

### 代码改动

#### 新增函数 1: cpu_boundary_search
```rust
fn cpu_boundary_search(
    input: &Path,
    gpu_boundary_crf: f32,
    target_size: u64,
) -> Result<f32> {
    // 从 GPU 边界开始，用 0.5 步进二分搜索
    // 找到最低能压缩的 CRF
    
    let mut low = gpu_boundary_crf;
    let mut high = gpu_boundary_crf + 5.0;
    
    while high - low > 0.5 {
        let mid = (low + high) / 2.0;
        let size = encode_and_measure(input, mid)?;
        
        if size < target_size {
            high = mid;  // 能压缩，继续向下
        } else {
            low = mid;   // 不能压缩，向上
        }
    }
    
    Ok(high)  // 返回最低能压缩的 CRF
}
```

#### 新增函数 2: cpu_fine_tune_v2
```rust
fn cpu_fine_tune_v2(
    input: &Path,
    boundary_crf: f32,
    target_size: u64,
) -> Result<(f32, u64)> {
    // Stage B-1: 向下探索 (更高质量)
    let mut best_crf = boundary_crf;
    let mut best_size = encode_and_measure(input, best_crf)?;
    
    for offset in [0.25, 0.5, 0.75, 1.0] {
        let test_crf = boundary_crf - offset;
        let size = encode_and_measure(input, test_crf)?;
        
        if size < target_size {
            best_crf = test_crf;
            best_size = size;
        } else {
            break;  // 不能压缩，停止向下
        }
    }
    
    // Stage B-2: 向上确认 (边界验证)
    for offset in [0.25, 0.5] {
        let test_crf = best_crf + offset;
        let size = encode_and_measure(input, test_crf)?;
        
        if size < target_size {
            best_crf = test_crf;
            best_size = size;
        }
    }
    
    Ok((best_crf, best_size))
}
```

#### 修改函数: explore_with_gpu_coarse_search
```rust
// 在函数末尾添加
if let Some(gpu_boundary) = gpu_result.boundary_crf {
    // 调用新的 CPU 阶段
    let boundary_crf = cpu_boundary_search(input, gpu_boundary, target_size)?;
    let (final_crf, final_size) = cpu_fine_tune_v2(input, boundary_crf, target_size)?;
    
    // 更新结果
    result.final_crf = final_crf;
    result.final_size = final_size;
}
```

### 验证方法
```bash
# 编译
cargo build --release

# 测试结构化搜索
./vidquality_hevc/target/release/vidquality-hevc auto test.mp4 --explore --match-quality --compress

# 检查日志输出
RUST_LOG=debug ./vidquality_hevc/target/release/vidquality-hevc auto test.mp4 --explore --match-quality --compress 2>&1 | grep "Phase"

# 预期: 看到 Phase 1, 2, 3, 4 的清晰日志
```

---

## 📋 第三层：高级功能增强 (v5.58-v5.60)

### 优先级 1: 预检查增强

**文件**: shared_utils/src/video_explorer.rs
**函数**: 在 `analyze_video` 中添加

```rust
fn calculate_bpp(width: u32, height: u32, frame_count: u64, file_size: u64) -> f64 {
    let total_pixels = (width as u64) * (height as u64) * frame_count;
    (file_size as f64 * 8.0) / (total_pixels as f64)
}

fn assess_compressibility(bpp: f64) -> &'static str {
    match bpp {
        x if x < 0.15 => "low",
        x if x < 0.30 => "medium",
        _ => "high",
    }
}
```

**输出示例**:
```
⚠️ 低 BPP (0.12): 文件已高度优化
   建议: 压缩空间有限，可能需要降低质量预期

✅ 高 BPP (0.35): 有较大压缩空间
   建议: 可以使用 --explore --match-quality --compress
```

### 优先级 2: GPU→CPU 自适应校准

**文件**: shared_utils/src/gpu_accel.rs
**新增结构**:

```rust
pub struct CalibrationPoint {
    pub gpu_crf: f32,
    pub gpu_size: u64,
    pub gpu_ssim: Option<f64>,
    pub predicted_cpu_crf: f32,
    pub confidence: f64,
}

fn calculate_calibration(
    gpu_crf: f32,
    gpu_size: u64,
    input_size: u64,
) -> CalibrationPoint {
    let size_ratio = gpu_size as f64 / input_size as f64;
    
    let predicted_cpu_crf = if size_ratio < 0.95 {
        gpu_crf + 1.0  // GPU 压缩余量大
    } else if size_ratio < 1.0 {
        gpu_crf + 0.5  // GPU 刚好压缩
    } else {
        gpu_crf - 1.0  // GPU 没压缩
    };
    
    CalibrationPoint {
        gpu_crf,
        gpu_size,
        gpu_ssim: None,
        predicted_cpu_crf,
        confidence: 0.8,
    }
}
```

### 优先级 3: 最坏情况采样

**文件**: shared_utils/src/video_explorer.rs
**新增函数**:

```rust
fn detect_worst_case_segments(
    input: &Path,
    num_segments: usize,
) -> Result<Vec<(f32, f32)>> {
    // 使用 ffmpeg 检测场景复杂度
    // 返回最难压缩的 N 个片段
    
    // 实现细节:
    // 1. 用 ffmpeg -filter:v select='gt(scene,0.3)' 检测场景切换
    // 2. 用 framestats 计算每帧的运动向量
    // 3. 选择复杂度最高的片段
    
    Ok(vec![])  // 占位符
}
```

### 优先级 4: 时间预算机制

**文件**: shared_utils/src/gpu_accel.rs
**修改结构**:

```rust
pub struct ExploreConfig {
    // ... 现有字段 ...
    pub time_budget_seconds: Option<u64>,
    pub gpu_time_ratio: f32,  // 默认 0.3
}

// 在搜索过程中
let start = Instant::now();
if let Some(budget) = config.time_budget_seconds {
    let gpu_limit = (budget as f32 * config.gpu_time_ratio) as u64;
    
    if start.elapsed().as_secs() > gpu_limit {
        eprintln!("⏰ GPU 阶段超时，切换到 CPU");
        break;
    }
}
```

### 优先级 5: 置信度输出

**文件**: shared_utils/src/gpu_accel.rs
**新增结构**:

```rust
pub struct ConfidenceBreakdown {
    pub sampling_coverage: f64,
    pub prediction_accuracy: f64,
    pub margin_safety: f64,
    pub ssim_confidence: f64,
}

pub struct ExploreResult {
    // ... 现有字段 ...
    pub confidence: f64,
    pub confidence_detail: ConfidenceBreakdown,
}

fn calculate_confidence(
    sampling_coverage: f64,
    prediction_accuracy: f64,
    margin_safety: f64,
    ssim_confidence: f64,
) -> f64 {
    (sampling_coverage * 0.3
        + prediction_accuracy * 0.3
        + margin_safety * 0.2
        + ssim_confidence * 0.2)
        .min(1.0)
}
```

---

## 📅 实施时间表

| 阶段 | 版本 | 任务 | 预计时间 | 状态 |
|------|------|------|---------|------|
| 第一层 | v5.55 | 精度调整 (0.25 步进) | 1 天 | ⏳ 待开始 |
| 第二层 | v5.56 | 三阶段结构恢复 | 2-3 天 | ⏳ 待开始 |
| 第二层 | v5.57 | 智能提前终止恢复 | 1-2 天 | ⏳ 待开始 |
| 第三层 | v5.58 | 预检查增强 | 1 天 | ⏳ 待开始 |
| 第三层 | v5.59 | GPU→CPU 校准 + 最坏情况采样 | 2 天 | ⏳ 待开始 |
| 第三层 | v5.60 | 时间预算 + 置信度输出 | 2 天 | ⏳ 待开始 |

---

## ✅ 验证清单

### 每个版本的验证步骤

```bash
# 1. 编译检查
cargo build --release

# 2. 功能测试
./vidquality_hevc/target/release/vidquality-hevc auto test.mp4 --explore --match-quality --compress

# 3. 性能测试
time ./vidquality_hevc/target/release/vidquality-hevc auto test.mp4 --explore --match-quality --compress

# 4. 日志检查
RUST_LOG=debug ./vidquality_hevc/target/release/vidquality-hevc auto test.mp4 --explore --match-quality --compress 2>&1 | head -50

# 5. 结果验证
# - 输出文件大小 < 输入文件大小
# - SSIM >= 0.95
# - 耗时合理 (< 15 分钟)
```

### 回滚方案

```bash
# 如果某个版本有问题，回滚到上一个版本
git revert <commit-hash>

# 或者回到稳定版本
git checkout v5.54-stable
cargo build --release
```

---

## 🎯 成功标准

### v5.55 成功标准
- [ ] 编译无错误
- [ ] CPU 搜索速度提升 2-3 倍
- [ ] 输出质量不变 (SSIM >= 0.95)
- [ ] 缓存精度正确 (0.25 步进)

### v5.56-v5.57 成功标准
- [ ] 三阶段结构清晰可见
- [ ] 日志输出显示 Phase 1-4
- [ ] 搜索结果更稳定
- [ ] 鲁棒性提升 50%

### v5.58-v5.60 成功标准
- [ ] 预检查信息有用
- [ ] GPU→CPU 校准准确度 > 90%
- [ ] 最坏情况采样覆盖率 > 80%
- [ ] 时间预算机制有效
- [ ] 置信度评分准确

---

## 🚨 风险管理

### 风险 1: 精度调整导致质量下降
**缓解**: 保持 0.25 步进在 ±1.0 CRF 范围内，不影响最终精度

### 风险 2: 三阶段结构引入 BUG
**缓解**: 每个函数单独测试，逐步集成

### 风险 3: 高级功能增加复杂度
**缓解**: 所有新功能都是可选的，不影响核心流程

### 风险 4: 性能回退
**缓解**: 每个版本都进行性能基准测试

---

## 📊 预期收益

### 性能提升
- CPU 搜索速度: +2-3x
- 总耗时: -30-40%
- 内存使用: -10-15%

### 质量改进
- 鲁棒性: +50%
- 可靠性: +30%
- 用户体验: +100%

### 代码质量
- 可维护性: +40%
- 可读性: +50%
- 测试覆盖: +20%

---

**开始日期**: 2025-12-14
**目标完成**: 2025-12-28
**状态**: 🟢 准备开始
