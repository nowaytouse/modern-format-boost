# Media Encoding Optimization Research Summary: JPEG XL & HEVC

## 1\. JPEG XL (cjxl) Core Research and Findings

### 1.1 Deep Dive into Effort Values

In the `cjxl` encoder, the effort value represents the amount of computational resources the encoder is willing to invest to find the optimal compression scheme.

| Level      | Name             | Characteristics                                 | Use Case                               |
| :--------- | :--------------- | :---------------------------------------------- | :------------------------------------- |
| **1 \- 3** | Lightning/Falcon | High-speed encoding                             | Real-time generation                   |
| **7**      | **Squirrel**     | **Default sweet spot**                          | Batch processing, library optimization |
| **8 \- 9** | Kitten/Tortoise  | Deep compression, significant time increase     | Static resource distribution           |
| **10**     | Glacier          | Expert-level optimization with advanced pruning | High-quality archiving, avoids e9 trap |
| **11**     | **Experimental** | Brute-force; slight gains only in lossless mode | Extreme experiments, benchmarking      |

### 1.2 The Algorithm Paradox of Effort 9 and 10

Research indicates that in **VarDCT (lossy mode)**, Effort 9 (Tortoise) suffers from inefficient search strategies:

- **Phenomenon**: Effort 10 is often faster (15% \- 56% faster) and produces smaller or equal file sizes compared to Effort 9\.
- **Reason**: Effort 9 tends to "blindly iterate" within low-level heuristic searches; Effort 10 introduces advanced mathematical prediction models and precise pruning logic.
- **Conclusion**: Effort 9 should be **skipped entirely** in any production environment.

### 1.3 Relationship Between Encoding Mode and Effort

- **VarDCT (Lossy)**: Algorithms saturate quickly between Effort 7-10; 11 yields almost no extra benefit.
- **Modular (Lossless/Near-lossless)**: Effort 11 enables Global MA-tree search, the ultimate choice for absolute pixel lossless.

---

## 2\. HEVC (x265) Performance and Preset Research

### 2.1 Preset Tiers

HEVC provides 10 standard presets, which are combinations of parameters for motion estimation, reference frames, and Rate-Distortion Optimization (RDO).

| Preset        | Core Mechanism                             | Benefit/Cost                    |
| :------------ | :----------------------------------------- | :------------------------------ |
| **ultrafast** | Limited CTU splitting, no B-frames         | Ultra-low latency, bulky size   |
| **medium**    | **Official default**, excellent balance    | All-rounder                     |
| **slow**      | Enables UMH motion search, more ref frames | **Top choice for archiving**    |
| **slower**    | Enables `tesa` (SATD frequency evaluation) | Extreme quality, anti-aliasing  |
| **placebo**   | Full-range search, similar to JXL e11      | Psychologically comforting only |

### 2.2 Motion Estimation: SAD vs SATD (tesa)

- **SAD (Sum of Absolute Differences)**: Based on pure pixel comparison. Fast but cannot distinguish high-frequency noise, leading to blocking.
- **SATD (Sum of Absolute Transformed Differences)**: Evaluated in the frequency domain via Hadamard transform. It identifies texture features precisely, reducing artifacts.

---

## 3\. "modern-format-boost" Project Engineering Strategy

To optimize iCloud libraries and batch media transcoding, the following tiered strategy is recommended:

### 3.1 Recommended Preset Profiles

| Mode         | JPEG XL Config | HEVC (x265) Config | Positioning                        |
| :----------- | :------------- | :----------------- | :--------------------------------- |
| **Balanced** | `effort 7`     | `preset slow`      | Default; balance space and time    |
| **Archive**  | `effort 10`    | `preset slower`    | Expert; chasing the final 1% limit |
| **Preview**  | `effort 3`     | `preset faster`    | Validation only                    |

### 3.2 Engineering Implementation Notes

1. **Avoidance Guide**: Hard-disable `cjxl -e 9` and `x265 --preset placebo` in code logic.
2. **Asymmetric Complexity**: High effort only impacts encoding; viewing (decoding) speed remains unaffected.
3. **Hardware Acceleration Trade-offs**: Mac's `VideoToolbox` is fast but less efficient than `libx265 slow` software encoding.

---

## References

- [cjxl Effort Level Study: Performance & Compression Analysis](./cjxl_effort_study.md) — Detailed benchmark data and algorithm analysis

---

## 媒体编码优化研究总结：JPEG XL 与 HEVC

## 1\. JPEG XL (cjxl) 核心研究与发现

### 1.1 努力值 (Effort) 深度解析

在 `cjxl` 编码器中，努力值代表了编码器为寻找最优压缩方案所愿意投入的计算资源。

| 级别       | 名称             | 特点                                 | 适用场景                 |
| :--------- | :--------------- | :----------------------------------- | :----------------------- |
| **1 \- 3** | Lightning/Falcon | 追求极速编码                         | 实时生成、高吞吐量场景   |
| **7**      | **Squirrel**     | **默认平衡点**，性价比最高的“甜点”位 | 日常批量处理、图库优化   |
| **8 \- 9** | Kitten/Tortoise  | 深度压缩，耗时显著增加               | 静态资源分发             |
| **10**     | Glacier          | 专家级优化，使用更高级的剪枝模型     | 高质量归档，避开 e9 陷阱 |
| **11**     | **Experimental** | 暴力穷举，仅在无损模式下有微弱增益   | 极限实验、基准测试       |

### 1.2 Effort 9 与 10 的算法悖论

研究发现，在 **VarDCT (有损模式)** 下，努力值 9 (Tortoise) 存在算法效率低下的问题：

- **现象**：努力值 10 往往比 9 处理速度更快（快 15% \- 56%），且体积更小或持平。
- **原因**：努力值 9 倾向于在低级启发式搜索中“盲目迭代”；而努力值 10 引入了更高级的数学预测模型和精准剪枝逻辑。
- **结论**：在任何生产环境中都应**直接跳过努力值 9**。

### 1.3 编码模式与努力值的关系

- **VarDCT (有损)**：算法在努力值 7-10 之间迅速饱和，11 几乎不产生额外收益。
- **Modular (无损/近无损)**：努力值 11 开启全局 MA 树搜索，是追求绝对像素无损时的极致选择。

---

## 2\. HEVC (x265) 性能与预设研究

### 2.1 预设档位 (Presets) 体系

HEVC 提供了 10 个标准预设，本质是运动搜索、参考帧和率失真优化 (RDO) 参数的组合。

| 预设          | 核心机制                      | 收益代价                       |
| :------------ | :---------------------------- | :----------------------------- |
| **ultrafast** | 限制 CTU 分割，关闭 B 帧      | 极低延迟，体积臃肿             |
| **medium**    | **官方默认值**，平衡性极佳    | 万金油选项                     |
| **slow**      | 开启 UMH 运动搜索，增加参考帧 | **高质量归档首选**，画质细腻   |
| **slower**    | 开启 `tesa` (SATD 频域评估)   | 极限画质，消除马赛克，耗时极大 |
| **placebo**   | 全遍历搜索，类似 JXL e11      | 仅具心理慰藉，无实战意义       |

### 2.2 运动搜索：SAD vs SATD (tesa)

- **SAD (绝对误差和)**：基于纯像素比对。虽然快，但无法分辨高频噪点，易产生色块。
- **SATD (变换绝对误差和)**：通过 Hadamard 变换在频域进行评估。它能精准识别纹理特征，不仅能显著减少马赛克，还能让残差数据更易被后续算法压缩。

---

## 3\. "modern-format-boost" 项目工程策略

针对 iCloud 图库优化及批量媒体重编码，建议采用以下分层策略：

### 3.1 预设模式建议

| 模式                    | JPEG XL 配置 | HEVC (x265) 配置 | 定位                         |
| :---------------------- | :----------- | :--------------- | :--------------------------- |
| **均衡模式 (Balanced)** | `effort 7`   | `preset slow`    | 默认选项，平衡空间与时间     |
| **极致归档 (Archive)**  | `effort 10`  | `preset slower`  | 专家选项，追求最后 1% 的极限 |
| **快速预览 (Preview)**  | `effort 3`   | `preset faster`  | 仅用于验证逻辑               |

### 3.2 工程实现注意事项

1. **避坑指南**：在代码逻辑中硬性禁用 `cjxl -e 9` 和 `x265 --preset placebo`。
2. **非对称复杂性**：由于 JXL 和 HEVC 的复杂性主要集中在编码端，高努力值不会影响用户的查看（解码）速度。
3. **硬件加速权衡**：如果使用 Mac 的 `VideoToolbox` 硬件编码，速度虽快，但压缩效率将无法达到 `libx265 slow` 的软编水平。

---

## 参考资料

- [cjxl Effort Level Study: Performance & Compression Analysis](./cjxl_effort_study.md) — 详细的实测数据与算法分析（英文）
