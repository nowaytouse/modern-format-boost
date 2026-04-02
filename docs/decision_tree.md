# 循环意图判断树

**适用范围**：GIF、视频、Telegram 动图贴图的统一入口。  
**输出**：`LOOP_STRONG` / `LOOP_WEAK` / `UNCERTAIN`，后接动作路由。  
**原则**：越靠前的节点越便宜、越确定；越靠后越贵、越模糊。不引入硬编码魔法数字。

---

## 前置：格式预路由（提取信号集，不做判断）

```
输入文件
├── Telegram TGS / WebM-sticker → 注入 loop_count=0, platform=TELEGRAM
├── APNG                        → 注入 format_loop_semantic=true
├── 普通 GIF                    → 读取 loop_count, app_extensions, palette_size...
└── 普通视频 (MP4/MOV/WebM...)  → 读取 has_audio, container, duration...
```

预路由不做任何判断，只负责把信号统一填入 `SignalBundle`，供下面的树消费。  
同时初始化 `WeightedScore`（初始值 0.0，范围 [-1.0, +1.0]），贯穿第三至第五层持续累积。

---

## 第一层：格式物理硬约束（100% 确定，零歧义）

> 命中即强制出口，`WeightedScore` 不参与，不需要继续。

### 节点 1-A：有音轨？

- 信号：`has_audio == true`
- 是 → **直接出口：LOOP_WEAK**（GIF 物理上不支持音频，强制）
- 否 → 下一节点

### 节点 1-B：有透明通道且无音轨？

- 信号：`has_alpha == true`
- 是 → **直接出口：LOOP_STRONG**（视频透明通道处理成本极高，强烈偏向 GIF）
- 否 → 进入第二层

---

## 第二层：显式自我声明（创作者 / 平台直接声明意图）

> 命中即强制出口，`WeightedScore` 不参与，不需要继续。

### 节点 2-A：无限循环标记？

- 信号：`loop_count == 0`
- 是 → **直接出口：LOOP_STRONG**（文件自己声明"我要无限循环"）
- 否 → 下一节点

### 节点 2-B：明确不循环标记？

- 信号：`loop_count == 1`（播完停止）
- 是 → **直接出口：LOOP_WEAK**（文件自己声明"我只播一次"）
- 否 → 下一节点

### 节点 2-C：平台来源标记？

- 信号：`app_extensions` 含 `GIPHY` / `TENOR` / `STICKER` / `TELEGRAM`
- 是 → **直接出口：LOOP_STRONG**（平台语义声明内容性质）
- 否 → 下一节点

### 节点 2-D：容器格式语义？

- 信号：`container == WebM AND has_audio == false`
- 是 → **直接出口：LOOP_STRONG**（WebM 无音轨是 Web 动图的标准载体，格式本身即循环语义）
- 否 → 进入第三层

---

## 第三层：自参照结构信号（内容与自身比较，无外部阈值）

> 本层开始进入 `WeightedScore` 累积区间。每个节点计算完毕后继续，  
> 不单独触发出口（除非分数已在本层达到饱和，见层末说明）。

### 节点 3-A：首尾帧自参照闭合比

- 信号：
  ```
  closure_ratio = 首尾帧视觉距离 / 帧间平均视觉距离
  ```
- `closure_ratio ≈ 1.0`（首尾跳变与普通帧间跳变相当）
  → `WeightedScore += 0.35`（权重最高，自参照无外部常数）
- `closure_ratio >> 1.0`（首尾有突变）
  → `WeightedScore -= 0.35`
- 信号缺失 / 内容整体变化过大导致分母虚高（已知 edge case）
  → 跳过，`WeightedScore` 不变

> Edge case 处理原则：跳过而非误判。当帧间平均距离本身就很大时，
> closure_ratio 的参照基准失效，强制跳过优于产生错误信号。

### 节点 3-B：节奏均匀性

- 信号：`interval_consistency_score`（帧间隔变异系数，自参照）
- 分数高（帧间隔高度均匀）→ `WeightedScore += 0.20`
- 分数低（帧间隔杂乱） → `WeightedScore -= 0.15`
- 中间区域 → `WeightedScore` 不变

**层末检查**：若 `WeightedScore ≥ 0.55` → **直接出口：LOOP_STRONG**  
　　　　　　若 `WeightedScore ≤ -0.55` → **直接出口：LOOP_WEAK**  
　　　　　　否则 → 进入第四层（携带当前分数继续累积）

---

## 第四层：内容特征信号（需要采样计算，成本较高）

### 节点 4-A：调色板大小

- 信号：`palette_size`
- `≤ 64`（典型合成内容，像素风、贴纸）→ `WeightedScore += 0.25`
- `65–128`（中性） → `WeightedScore` 不变
- `> 128`（接近自然内容色彩空间） → `WeightedScore -= 0.15`

### 节点 4-B：帧内容可压缩性（WebP 压缩比）

- 信号：对采样帧做 WebP 有损压缩，测量 `raw_size / webp_size`
  ```
  比值 > 15x → 合成内容（色块平坦、信息熵低）→ WeightedScore += 0.20
  比值 < 5x  → 自然内容（噪点丰富、信息熵高）→ WeightedScore -= 0.25
  中间区域   → WeightedScore 不变
  ```

> 这是判断"合成 vs 自然"的最直接代理——直接测量 GIF 的 LZW 压缩会带来多大收益。

### 节点 4-C：compression_efficiency_score

- 信号：`compression_efficiency_score`（现有实现）
- `> 0.7` → `WeightedScore += 0.15`
- `< 0.3` → `WeightedScore -= 0.10`
- 中间区域 → `WeightedScore` 不变

**层末检查**：若 `WeightedScore ≥ 0.55` → **直接出口：LOOP_STRONG**  
　　　　　　若 `WeightedScore ≤ -0.55` → **直接出口：LOOP_WEAK**  
　　　　　　否则 → 进入第五层

---

## 第五层：上下文语义信号（最弱，仅辅助）

> 本层所有节点权重刻意压低，绝不会单独扭转方向，只作为细微修正。  
> 本层末不设检查点，所有未出口的情况统一进入第六层。

### 节点 5-A：目录 / 文件名语义

- 信号：`directory_meme_score`、`filename_score`
- 两者均 `> 0.8` → `WeightedScore += 0.10`
- 任意一项 `> 0.8` → `WeightedScore += 0.05`
- 否则 → `WeightedScore` 不变

### 节点 5-B：fps 异常

- 信号：`fps_anomaly_score`
- 异常值偏高（非标准帧率，典型动图特征）→ `WeightedScore += 0.05`
- 否则 → `WeightedScore` 不变

### 节点 5-C：时长（仅作为弱修正，不硬编码绝对值）

- 信号：`duration_secs / avg_frame_duration`（即总帧数的自参照表达）
- 总帧数极少（内容极短）→ `WeightedScore += 0.05`
- 总帧数极多（内容极长）→ `WeightedScore -= 0.10`
- 中间区域 → `WeightedScore` 不变

> 时长以自参照的总帧数形式进入，而非硬编码秒数；
> 同时作为 KNN 特征维度的一部分，让训练集自己学习分布。

---

## 第六层：KNN + WeightedScore 综合判断

到达本层时，`SignalBundle` 已包含：

- 第三至五层计算的所有原始信号（供 KNN 特征空间使用，不重算）
- 时长（帧数形式）
- 当前累积的 `WeightedScore`（作为 KNN 的一个额外特征维度传入）

```
KNN 输出：keep_probability, confidence
综合判断逻辑：

final_score = keep_probability * 0.6 + normalize(WeightedScore) * 0.4

confidence > 0.75 AND final_score > 0.6  → LOOP_STRONG
confidence > 0.75 AND final_score ≤ 0.4  → LOOP_WEAK
其余所有情况                              → UNCERTAIN，进兜底
```

> `WeightedScore` 在此不是独立判断者，而是 KNN 的加权修正项。
> 两者融合的权重比（0.6 / 0.4）可根据 KNN 训练集质量调整：
> 训练集越大越可信，KNN 权重应越高；训练集稀薄时，WeightedScore 权重应上调。

---

## 第七层：保守兜底

```
输入是现代动图格式（TGS / APNG / WebP 动图）→ 转 GIF（最小损失）
输入已经是 GIF                               → 保留原样，跳过
输入已经是视频                               → 保留原样，跳过

所有兜底情况 → 写入 low_confidence 标记到数据库
```

低置信度标记的价值：这些文件日后可经人工复核后作为新的 KNN 训练样本，  
盲区随时间推移自然收窄，不需要一次性解决。

---

## 后置：动作路由

```
判断树输出
├── LOOP_STRONG
│   ├── 输入是视频 → 转 GIF
│   └── 输入是 GIF → 保留
└── LOOP_WEAK
    ├── 输入是 GIF → 转视频
    └── 输入是视频 → 保留
```

---

## 各层设计原则对照

| 层级                     | 触发方式          | WeightedScore              | 可靠性               | 计算成本     |
| ------------------------ | ----------------- | -------------------------- | -------------------- | ------------ |
| 第一层：物理硬约束       | 强制出口          | 不参与                     | 100%                 | 极低         |
| 第二层：显式声明         | 强制出口          | 不参与                     | ~99%                 | 极低         |
| 第三层：自参照结构       | 层末检查点 / 累积 | 权重 0.35 / 0.20           | 高，有已知 edge case | 低           |
| 第四层：内容特征         | 层末检查点 / 累积 | 权重 0.25 / 0.20 / 0.15    | 中                   | 中（需采样） |
| 第五层：上下文语义       | 仅累积            | 权重 ≤ 0.10                | 弱                   | 低           |
| 第六层：KNN + Score 融合 | 概率出口          | 作为 KNN 特征维度 + 修正项 | 取决于训练集         | 高           |
| 第七层：保守兜底         | 保守默认          | 不参与                     | 最小损失             | 零           |
