# 算法策略深度代码复盘 (Strict Code Analysis v4.7)

> **版本**: v4.7 (基于源代码逻辑分析)
> **状态**: 去除一切文档/注释中的"AI"营销词汇，仅展现代码实现的真实逻辑。
> **警告**: 发现 `explore_size_only` 存在潜在逻辑 Bug。

本文档基于 `shared_utils/src/video_explorer.rs` 和 `shared_utils/src/quality_matcher.rs` 的实际代码实现编写。

---

## 1. 核心机制：启发式预测 (Heuristic Prediction)

代码中所谓的"预测"实际上是**基于 BPP (Bits Per Pixel) 的数学公式计算**，而非神经网络或 AI 模型。

*   **代码位置**: `shared_utils/src/quality_matcher.rs`
*   **计算逻辑**:
    1.  提取视频元数据：码率、分辨率、帧率。
    2.  计算 `Effective BPP` (有效每像素比特数)，并根据编码格式 (HEVC/AV1) 进行加权。
    3.  **公式 (AV1 示例)**:
        $$ CRF = 50 - 6 \times \log_2(EffectiveBPP \times 100) $$
    4.  **修正**: 根据内容类型 (如动画 +4 CRF, 颗粒 -3 CRF) 和 HDR 状态进行微调。

**结论**: 这是一个纯数学的静态映射系统，完全确定性，无 AI 参与。

---

## 2. 策略算法拆解 (Strategy Analysis)

### 2.1. `--compress` (CompressOnly)
**代码逻辑**: `video_explorer.rs` (Lines 571-667)

*   **目标**: 找到能让 $Size < Input$ 的**最低 CRF** (即最佳画质)。
*   **流程**:
    1.  **快速通道**: 计算出的 `initial_crf` 直接试跑。如果 $Size < Input$，**立即返回成功**。
        *   *目的*: 极速处理，只要能压小就不折腾。
    2.  **二分搜索**:
        *   区间: `[initial_crf, max_crf]`
        *   若 `Size < Input`: 有效。记录为候选，尝试降低 CRF (`high = mid`) 以提升画质。
        *   若 `Size >= Input`: 无效。必须提高 CRF (`low = mid`) 以减小体积。
    3.  **结论**: 逻辑正确，是一个寻找"压缩边界左侧最优解"的算法。

### 2.2. `--explore` (SizeOnly)
**代码逻辑**: `video_explorer.rs` (Lines 405-515)

*   **目标**: 找到能让 $Size < Input$ 的**最高 CRF** (即最小体积)。
*   **流程**:
    1.  **最大值探底**: 先测 `max_crf`。若 $Size \ge Input$，直接失败。
    2.  **二分搜索 (存在逻辑 Bug)**:
        *   代码 L459: `if size < input` (有效压缩):
            *   记录最佳值。
            *   `low = mid` (尝试更高 CRF/更小体积)。 -> **逻辑正确**。
        *   代码 L464: `else` (即 `size >= input`, 体积太大):
            *   `high = mid` (搜索下半区 `[low, mid]`)。 -> **逻辑错误!**
            *   *分析*: 如果当前 CRF 导致体积太大，我们需要**增加 CRF** (降低画质) 来减小体积。正常应当搜索 `[mid, high]`。
            *   *后果*: 代码实际上在向"更大体积"的方向搜索，导致无法找到中间的可行解。
    3.  **精细化**: 在 `best_crf` 附近尝试 `+0.5` 等微调。

### 2.3. `--compress --match-quality` (CompressWithQuality)
**代码逻辑**: `video_explorer.rs` (Lines 669-785)

*   **目标**: $Size < Input$ 且 $SSIM \ge 0.95$。
*   **流程**:
    1.  **二分搜索边界**:
        *   寻找**刚刚好能压缩** (Size < Input) 的 CRF 临界点。
    2.  **线性回溯 (Linear Backtrack)**:
        *   从临界点 (`compress_boundary`) 开始。
        *   代码 L726: `while crf >= initial` (向下循环)。
        *   每次 `crf -= 1.0` (提高画质/增大体积)。
        *   **检测**:
            *   如果 $Size \ge Input$: 立即停止 (体积超标)。
            *   如果 $SSIM \ge 0.95$: 立即停止 (目标达成)。
    3.  **结论**: 利用"体积余量"购买画质。一旦体积不够用或画质达标即停止。

### 2.4. `--explore --match-quality` (PreciseQualityMatch)
**代码逻辑**: `video_explorer.rs` (Lines 799-958)

*   **目标**: 寻找 SSIM 曲线的高效拐点 ("Visual Lossless Knee")。
*   **流程**:
    1.  **平台检测**: 计算 Min/Max CRF 的 SSIM 差值。如果太小，说明调节 CRF 无效，直接用最大 CRF。
    2.  **黄金分割搜索 (Golden Section Search)**:
        *   使用 $\phi = 0.618$ 选点。
        *   **决策逻辑**:
            *   比较 `Previous SSIM` (Good Quality) 和 `Current SSIM`。
            *   如果差值 > `Threshold`: 画质掉太快了，必须回退 (`high = mid`)。
            *   如果差值小: 这里是平原，我们可以贪心地选更大的 CRF (`low = mid`)。
    3.  **微调**: 在找到的区域进行 `±0.5` 修正。
    4.  **特点**: 并不严格锁定 $SSIM=0.95$，而是寻找"性价比最高"的点。

### 2.5. `--explore --match-quality --compress` (PreciseQualityWithCompression)
**代码逻辑**: `video_explorer.rs` (Lines 969-1104)

*   **目标**: 在 $Size < Input$ 的前提下，穷举找到最高 SSIM。
*   **流程**:
    1.  **二分搜索**: 找到压缩边界。
    2.  **向下搜索 (Search Down)**:
        *   从边界开始，向下尝试更低的 CRF。
        *   只要 $Size < Input$，就不断刷新 `Best SSIM`。
        *   **区别**: 即使 SSIM 到了 0.99 也不停，直到 $Size \ge Input$ 撞墙为止。
    3.  **结论**: 这是最耗时的模式，为了画质愿意牺牲所有体积缩减量。

---

## 3. Bug 报告摘要

**严重性**: 高
**位置**: `video_explorer.rs` : `explore_size_only` 函数 (L466)
**现象**: 当中间值 `mid` 导致文件过大时，算法错误地向"更低 CRF"方向搜索（导致文件更大），而非"更高 CRF"方向。这将导致 `explore` 模式在某些边缘情况下无法找到最优解，甚至在本来能压缩的情况下报告失败（如果 `initial` 和 `max` 之间存在解，但第一次二分走错方向）。

---

*文末备注: 本文档完全依据代码逻辑生成，未参考任何原有文档描述。*
