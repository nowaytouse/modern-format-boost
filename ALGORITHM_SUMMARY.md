# Modern Format Boost v4.6 - 算法总结

## 🎯 一句话总结

**Modern Format Boost** 是一个集合级媒体归档工具，通过 AI 质量预测 + 四阶段 CRF 搜索（±0.1 精度）+ Flag 组合模块化，实现无损或视觉无损的媒体格式升级。

---

## 🔥 v4.6 三大创新

### 1️⃣ Flag 组合模块化

**问题**：四个工具中重复实现 flag 验证逻辑

**解决**：
```
flag_validator.rs 模块
├── 7 种有效组合
├── 1 种无效组合（--explore --compress）
├── 17 个单元测试
└── 所有工具共享
```

**有效组合**：
| 组合 | 目标 | 用途 |
|------|------|------|
| `--compress` | 输出 < 输入 | 快速压缩 |
| `--explore` | 最小输出 | 极限压缩 |
| `--match-quality` | 粗略质量 | 快速转换 |
| `--compress --match-quality` | 压缩 + 质量 | 平衡方案 |
| `--explore --match-quality` | 精确质量 | 质量优先 |
| `--explore --match-quality --compress` | 精确 + 压缩 | 完美方案 |

### 2️⃣ 精度提升到 ±0.1

**从 ±0.5 → ±0.1**：

```
Phase 1: 粗搜索 (步长 2.0)  → ±2.0
Phase 2: 细搜索 (步长 0.5)  → ±0.5
Phase 3: 精细搜索 (步长 0.1) → ±0.1
Phase 4: 验证 (步长 0.05)   → ±0.05
```

**为什么足够？**
- CRF 变化 0.1 → 文件大小变化 ~0.5-1%
- SSIM 变化 ~0.0001-0.0005（人眼无法区分）
- 编码器量化步长 > 0.1

### 3️⃣ 完整的算法文档

- [ALGORITHM_DEEP_DIVE_v4.6.md](./ALGORITHM_DEEP_DIVE_v4.6.md) - 深度复盘
- [README.md](./README.md) - 用户文档
- 352 个单元测试全部通过

---

## 🧠 核心算法

### AI 质量预测（v3.5）

**输入**：视频元数据（9 个因子）
```
视频专用码率 (55%) + GOP 结构 (15%) + 色度采样 (10%) + 
HDR 检测 (8%) + 内容类型 (7%) + 其他 (5%)
```

**输出**：最优 CRF 值（±92% 置信度）

**公式**：
```
CRF = 46 - 5 × log₂(effective_bpp × 100) + content_adjustment
```

### 精确质量匹配（v4.5）

**目标**：找到最高 SSIM（最接近源质量）

**三阶段搜索**：
```
Phase 1: 边界测试 (2 次)
  ├── 测试 min_crf（最高质量）
  └── 测试 max_crf（最低质量）

Phase 2: 平台搜索 (4-6 次)
  ├── 从 min_crf 向上搜索（步长 2.0）
  └── 检测 SSIM 平台

Phase 3: 精细调整 (2-4 次)
  ├── ±1 CRF 范围（步长 0.5）
  └── 选择最高 SSIM
```

**为什么高效？**
- SSIM 单调递减（CRF↑ → SSIM↓）
- 最高 SSIM 总在最低 CRF
- 但存在平台（继续降低 CRF 不再提升 SSIM）
- 三阶段快速定位平台边缘

### CRF 搜索（v4.6）

**四阶段搜索**：
```
粗搜索 (步长 2.0)
  ↓ 快速定位边界
细搜索 (步长 0.5)
  ↓ 精确定位最优
精细搜索 (步长 0.1)
  ↓ 达到 ±0.1 精度
验证 (步长 0.05)
  ↓ 确保最优
```

**迭代次数**：
- 粗搜索：5 次
- 细搜索：4 次
- 精细搜索：3 次
- 验证：2 次
- **总计**：~14 次（max_iterations=15）

---

## 📊 性能指标

| 指标 | 值 |
|------|-----|
| CRF 精度 | ±0.1 |
| SSIM 精度 | 0.0001 |
| 典型迭代次数 | 12-15 |
| 缓存命中率 | ~30% |
| 单元测试 | 352 个 |
| 编译警告 | 4 个（未使用方法） |

---

## 🏗️ 架构

### 模块化设计

```
shared_utils/
├── flag_validator.rs      ← Flag 组合验证（v4.6 新增）
├── video_explorer.rs      ← CRF 搜索算法
├── quality_matcher.rs     ← AI 质量预测
├── conversion.rs          ← 转换选项
├── checkpoint.rs          ← 断电保护
├── metadata.rs            ← 元数据保留
└── progress.rs            ← 进度显示

四个工具共享上述模块：
├── imgquality_hevc
├── imgquality_av1
├── vidquality_hevc
└── vidquality_av1
```

### 依赖关系

```
flag_validator.rs (独立)
    ↓
conversion.rs (使用 flag_validator)
    ↓
video_explorer.rs (使用 conversion)
    ↓
四个工具 (使用 video_explorer)
```

---

## 🔍 质量验证

### SSIM 质量等级

| SSIM | 等级 | 描述 |
|------|------|------|
| >= 0.98 | Excellent | 几乎无法区分 |
| >= 0.95 | Good | 视觉无损 |
| >= 0.90 | Acceptable | 轻微差异 |
| >= 0.85 | Fair | 可见差异 |
| < 0.85 | Poor | 明显质量损失 |

### 三重交叉验证

```
SSIM (50%) + VMAF (35%) + PSNR (15%)
    ↓
🟢 All Agree      → 高置信度，早期终止
🟡 Majority Agree → 良好置信度
🔴 Divergent      → 继续搜索
```

---

## 🚀 使用示例

### 默认模式（拖拽）

```bash
# macOS：双击 Modern Format Boost.app
# 或将文件夹拖拽到应用图标
# 自动启用：--explore --match-quality --compress --apple-compat
```

### 命令行

```bash
# 精确质量匹配 + 压缩（推荐）
./vidquality_hevc/target/release/vidquality-hevc auto /path/to/videos \
  --recursive \
  --explore \
  --match-quality \
  --compress \
  --apple-compat

# 仅压缩（快速）
./vidquality_hevc/target/release/vidquality-hevc auto /path/to/videos \
  --recursive \
  --compress

# 精确质量（质量优先）
./vidquality_hevc/target/release/vidquality-hevc auto /path/to/videos \
  --recursive \
  --explore \
  --match-quality
```

---

## 📈 版本演进

| 版本 | 主要改进 | 精度 |
|------|----------|------|
| v3.4 | CRF u8 → f32 | ±1.0 |
| v3.5 | AI 质量预测 | ±1.0 |
| v3.6 | 三阶段搜索 | ±0.5 |
| v4.1 | 三重交叉验证 | ±0.5 |
| v4.5 | 精确质量匹配 | ±0.5 |
| v4.6 | Flag 模块化 + 四阶段搜索 | **±0.1** |

---

## 🎓 关键洞察

### 为什么 ±0.1 精度足够？

1. **CRF 的非线性特性**
   - 0.1 CRF 变化 → 0.5-1% 文件大小变化
   - 0.1 CRF 变化 → 0.0001-0.0005 SSIM 变化
   - 人眼无法区分

2. **编码器的量化特性**
   - x265/SVT-AV1 内部量化步长 > 0.1
   - 0.1 精度已接近编码器最小可分辨单位

3. **SSIM 测量精度**
   - ffmpeg ssim 滤镜输出精度 0.0001
   - 0.1 CRF 变化导致 SSIM 变化 < 0.001
   - 远小于测量精度

### 为什么需要四阶段搜索？

1. **粗搜索**：快速定位大致范围
2. **细搜索**：精确定位最优点
3. **精细搜索**：达到 ±0.1 精度
4. **验证**：确保没有遗漏更优点

### Flag 组合的设计原则

1. **单一职责**：每个 flag 只做一件事
2. **组合的语义清晰**：组合后的行为易于理解
3. **无效组合的检测**：响亮报错，提供建议

---

## 📚 文档导航

- **[ALGORITHM_DEEP_DIVE_v4.6.md](./ALGORITHM_DEEP_DIVE_v4.6.md)** - 完整的算法复盘（推荐阅读）
- **[README.md](./README.md)** - 用户文档和使用指南
- **[flag_validator.rs](./shared_utils/src/flag_validator.rs)** - Flag 验证模块源码
- **[video_explorer.rs](./shared_utils/src/video_explorer.rs)** - CRF 搜索算法源码
- **[quality_matcher.rs](./shared_utils/src/quality_matcher.rs)** - AI 质量预测源码

---

## 🎯 下一步

- [ ] 支持 AV1 编码器的精度优化
- [ ] 添加 GPU 加速支持
- [ ] 实现分布式处理
- [ ] 支持更多编码器（VP9、VVC）

---

**最后更新**：2025-12-13 | **版本**：v4.6 | **精度**：±0.1 CRF
