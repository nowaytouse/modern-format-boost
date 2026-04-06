**English Translation:**

```markdown
# Loop Intent Judgment System

**Core Question**: Is the creation intent of this content to loop/play in a loop?  
**Output**: `LOOP_STRONG` (keep GIF / convert to GIF) or `LOOP_WEAK` (convert to video / keep video) or `UNCERTAIN`  
**Principle**: Single-axis judgment. Do not introduce engineering concerns such as file size, encoding efficiency, etc. File size is an encoding parameter issue, not a decision tree issue.

---

## Known Design Flaws (Issues Fixed in This Document)

The existing tree is too lenient toward `LOOP_STRONG`:

- `Layer 1-C`: `is_image AND duration ≤ 10s` → unconditional hard pass, but 10-second live-action video clips also satisfy this condition.
- `Layer 1-D`: `width ≤ 512` → unconditional hard pass, but low-resolution live-action clips also satisfy this.
- `loop_count == 0` in `Layer 2-A` gives positive weight, but in the GIF specification, the default value is 0, which also applies to a large number of GIFs without loop intent.

The fundamental problem is: **the signals for `LOOP_WEAK` are severely missing**. A large number of GIFs are hard-passed or pulled away by positive weights in the first two layers and never reach the layers capable of judging `LOOP_WEAK`.

---

## Preprocessing: Format Pre-Routing

```
Input File
├── Telegram TGS / WebM-sticker → inject loop_count=0, platform=TELEGRAM
├── APNG                        → inject format_loop_semantic=true
├── Normal GIF                  → read loop_count, app_extensions, palette_size...
└── Normal Video (MP4/MOV/WebM...) → read has_audio, container, duration...
```

Initialize `WeightedScore = 0.0`, range `[-1.0, +1.0]`, which continues to accumulate from Layer 2 through Layer 4.  
Simultaneously initialize `veto_flags`: record veto signals used to block hard-pass conditions.

---

## Layer 1: Physical Hard Constraints (Forced Exit, Does Not Participate in WeightedScore)

This layer only handles cases that are **physically impossible to be loop intent** or **physically must be loop format**. Conditions are extremely strict.

### Node 1-A: Video Container with Audio Track
- `has_audio == true AND is_video_container`
- → **LOOP_WEAK** (GIF physically does not support audio; presence of audio track means it is definitely not created with looping animation intent)

### Node 1-B: Has Alpha Channel
- `has_alpha == true AND NOT has_audio`
- → **LOOP_STRONG** (Alpha channel is exclusive semantics for GIF/stickers; video transparency handling paths are completely different)

### Node 1-C: Extremely Short Content Hard Pass (with Veto Conditions)

**Base Condition**: `is_image AND duration ≤ 10s`  
**But if any of the following veto conditions are also met, skip the hard pass and continue evaluation**:

```
Veto Conditions (veto_flags):
├── detect_scene_cut == true       → Narrative structure, not loop design (typical feature of live-action clips)
└── webp_compression_ratio < 5x    → Naturally captured content (rich in noise, not synthetic animation)
```

- No veto → **LOOP_STRONG** (extremely short + synthetic content; looping cost is negligible)
- Has veto → Skip and proceed to Layer 2 for further evaluation

### Node 1-D: Small Size Hard Pass (with Veto Conditions)

**Base Condition**: `is_image AND width ≤ 512 AND height ≤ 512`  
**But if any of the following veto conditions are also met, skip**:

```
Veto Conditions:
├── detect_scene_cut == true
└── webp_compression_ratio < 5x
```

- No veto → **LOOP_STRONG** (sticker/icon semantics; small-size content has very strong loop intent)
- Has veto → Skip and proceed to Layer 2

---

## Layer 2: Explicit Self-Declaration (Strong Signal, Direct Exit)

The file’s own self-declaration has extremely high credibility. If matched, exit immediately. WeightedScore does not participate.

### Node 2-A: Platform Source Tag
- `app_extensions` contains `GIPHY / TENOR / STICKER / TELEGRAM / TIKTOK / DISCORD`
- → **LOOP_STRONG** (the platform’s own semantics are looping animations)

### Node 2-B: WebM without Audio Track
- `container == webm AND has_audio == false`
- → **LOOP_STRONG** (WebM without audio track is the standard carrier for web looping animations; format itself is semantics)

### Node 2-C: Explicit Non-Loop Declaration
- `loop_count == 1` (stop after one play)
- → **LOOP_WEAK** (the file actively declares “I only play once”)

> **Note**: `loop_count == 0` is no longer used as a direct exit.  
> Reason: In the GIF specification, `loop_count == 0` means infinite looping, but because this field defaults to 0, a large number of GIFs without loop intent also carry this value. It has been downgraded to a weighted signal in WeightedScore (Layer 3) and is evaluated together with other signals instead of deciding the result alone.

---

## Layer 3: Structural Signals (WeightedScore Accumulation, Checkpoint at End of Layer)

Signals in this layer are objective measurements of content structure, with no external thresholds—all are self-referential.

### Node 3-A: First-to-Last Frame Closure Ratio (Most Direct Evidence of Loop Design)

```
closure_ratio = visual distance between first and last frame / average visual distance between frames
```

- `closure_ratio ≈ 1.0` (jump from first to last frame is comparable to normal inter-frame jumps) → `WeightedScore += 0.35`
- `closure_ratio >> 1.0` (sudden jump from first to last frame; looping would cause obvious frame skip) → `WeightedScore -= 0.35`
- Denominator inflated (overall content changes extremely; reference baseline invalid) → **Skip**, no score modification

> Edge case handling: When the average inter-frame distance itself is extremely large, the reference baseline for closure_ratio becomes invalid. In this case, force skip instead of generating erroneous signals.

### Node 3-B: Frame Rhythm Uniformity

```
frame_delay_variation (coefficient of variation of frame intervals, self-referential):
< 0.10 → WeightedScore += 0.20 (highly uniform, typical looping animation rhythm)
< 0.25 → WeightedScore += 0.10
> 0.60 → WeightedScore -= 0.15 (chaotic rhythm, not designed looping)
```

### Node 3-C: Scene Cut Detection (Core Supplementary Signal for LOOP_WEAK)

```
detect_scene_cut (I-frame mutation >5x in frame packet size stream):
true  → WeightedScore -= 0.30 (narrative structure, not loop design)
false → No modification
```

> This is the most critical node for fixing the missing LOOP_WEAK signals.  
> Scene cuts are a typical feature of narrative content (live-action video clips) and are strongly mutually exclusive with loop intent.

### Node 3-D: Motion Vector Distribution (Structural Distinction Between Synthetic vs. Natural)

```
motion_gini (Gini coefficient of motion vector magnitudes):
High Gini (motion concentrated in a few regions) → WeightedScore += 0.15 (local motion, synthetic animation feature)
Low Gini (motion evenly distributed across the entire frame) → WeightedScore -= 0.15 (global motion, natural capture feature)
Proportion of zero values in mv_magnitudes > 70% → WeightedScore += 0.10 (mostly static, typical sticker/looping animation)
```

### Node 3-E: `loop_count` Weighted Signal (Downgraded from Direct Exit to Structural Signal)

```
loop_count == 0, with progressive decay based on duration:
duration ≤ 18s  → WeightedScore += 0.25
18s < d ≤ 35s  → Linear decay to +0.10
duration > 35s  → WeightedScore += 0.05 (very low credibility, only slight positive)
```

> Downgrade reason: `loop_count == 0` is the default value in the GIF specification; its credibility is insufficient to decide the result alone.  
> It only becomes reliable when evaluated together with signals such as first-to-last closure ratio and rhythm uniformity.

**Layer-End Checkpoint**:  
`WeightedScore ≥ 0.55` → **LOOP_STRONG**  
`WeightedScore ≤ -0.55` → **LOOP_WEAK**  
Otherwise → Proceed to Layer 4, carrying the current score for further accumulation

---

## Layer 4: Content Feature Signals (Higher Cost, Continue Accumulation)

### Node 4-A: Frame Content Compressibility (WebP Compression Ratio)

```
raw_size / webp_size (measured on sampled frames):
> 15x → WeightedScore += 0.20 (synthetic content, flat color blocks, typical looping animation)
< 5x  → WeightedScore -= 0.25 (natural content, rich in noise, not synthetic looping intent)
5x–15x → Neutral
```

> This is the most direct proxy for judging “synthetic vs. natural”.  
> Naturally captured content (live-action) almost never has loop design intent.

### Node 4-B: Palette Size

```
palette_size ≤ 64   → WeightedScore += 0.20 (typical synthetic content)
palette_size > 128  → WeightedScore -= 0.15 (close to natural color space)
```

### Node 4-C: compression_efficiency_score

```
> 0.7 → WeightedScore += 0.10
< 0.3 → WeightedScore -= 0.10
```

**Layer-End Checkpoint**:  
`WeightedScore ≥ 0.55` → **LOOP_STRONG**  
`WeightedScore ≤ -0.55` → **LOOP_WEAK**  
Otherwise → Proceed to Layer 5

---

## Layer 5: Contextual Semantic Signals (Auxiliary Only, Extremely Low Weight)

Weights in this layer are deliberately kept very low. **They absolutely cannot reverse the direction alone** and only provide minor corrections. This layer has no checkpoint.

### Node 5-A: Directory / Filename Semantics
```
directory_meme_score > 0.8 AND filename_score > 0.8 → WeightedScore += 0.08
Any one > 0.8                                       → WeightedScore += 0.04
```

### Node 5-B: FPS Anomaly
```
fps_anomaly_score > 0.6 (non-standard frame rate, typical animation feature) → WeightedScore += 0.04
```

### Node 5-C: Total Frame Count (Self-Referential Expression of Duration)
```
frame_count ≤ 8 (extremely short content)   → WeightedScore += 0.04
frame_count > 500 (long-form content)       → WeightedScore -= 0.08
```

### Node 5-D: Aspect Ratio
```
width == height (1:1 square) → WeightedScore += 0.03 (typical ratio for stickers/emojis)
Close to 16:9 (±5%)          → WeightedScore -= 0.04 (typical ratio for film/TV content)
```

---

## Layer 6: KNN + WeightedScore Fusion

When reaching this layer, the `SignalBundle` contains all signals calculated from Layers 3 to 5 (no repeated sampling).

```
KNN Output: keep_probability, confidence

final_score = keep_probability × 0.6 + normalize(WeightedScore) × 0.4

confidence > 0.75 AND final_score > 0.60  → LOOP_STRONG
confidence > 0.75 AND final_score < 0.40  → LOOP_WEAK
Otherwise                                 → UNCERTAIN, proceed to Layer 7
```

> KNN weight (0.6) > WeightedScore weight (0.4):  
> The training set consists of annotated real cases, which is more credible than rule-based inference.  
> When the training set is sparse, the WeightedScore weight can be increased.

---

## Layer 7: Conservative Fallback

```
Input is a modern animation format (TGS / APNG / WebP animation) → Convert to GIF (minimal loss)
Input is already a GIF                               → Keep as is, write low_confidence flag
Input is already a video                             → Keep as is, write low_confidence flag
```

Low-confidence files will later be manually reviewed and used as KNN training samples, gradually narrowing blind spots over time.

---

## Post-processing: Action Routing

```
Decision Tree Output
├── LOOP_STRONG
│   ├── Input is video → Convert to GIF
│   └── Input is GIF → Keep
└── LOOP_WEAK
    ├── Input is GIF → Convert to video
    └── Input is video → Keep
```

---

## Design Principle Comparison by Layer

| Layer | Trigger Method | WeightedScore | Reliability | Computational Cost |
|-------|----------------|---------------|-------------|--------------------|
| Layer 1: Physical Hard Constraints | Forced exit (with veto conditions) | Not involved | 100% (downgraded after veto) | Low |
| Layer 2: Explicit Declaration | Forced exit | Not involved | ~99% | Extremely Low |
| Layer 3: Structural Signals | Layer-end checkpoint / accumulation | Core weight range | High, edge cases handled | Low |
| Layer 4: Content Features | Layer-end checkpoint / accumulation | Medium weight | Medium (requires sampling) | Medium |
| Layer 5: Contextual Semantics | Accumulation only, no checkpoint | Weight ≤ 0.08 | Weak | Extremely Low |
| Layer 6: KNN Fusion | Probabilistic exit | Used as correction term | Depends on training set | High |
| Layer 7: Conservative Fallback | Conservative default | Not involved | Minimal loss | Zero |

---

## Core Differences from Previous Version

| Issue | Previous Version | This Version |
|-------|------------------|--------------|
| Layer 1-C/1-D too broad | Unconditional hard pass | With veto conditions (scene cut / natural content) |
| `loop_count=0` too strong | Direct exit LOOP_STRONG | Downgraded to structural signal with progressive decay weighting |
| LOOP_WEAK signal missing | Almost no active LOOP_WEAK signals | New additions: scene cut (-0.30), motion vector distribution, WebP compression ratio penalty |
| Hard-coded duration | Some nodes use absolute seconds | All changed to self-referential (total frame count, duration decay ratio) |
```
