# Loop Intent Decision Tree

**Scope**: Unified entry point for GIFs, videos, and Telegram animated stickers.
**Output**: `LOOP_STRONG` / `LOOP_WEAK` / `UNCERTAIN`, followed by action routing.
**Principles**: Earlier nodes are cheaper and more certain; later nodes are more expensive and fuzzy. Avoid hard-coded magic numbers.

---

## Pre-routing: Format Signal Extraction (Extraction only, no judgment)

```text
Input File
├── Telegram TGS / WebM-sticker → Inject loop_count=0, platform=TELEGRAM
├── APNG                        → Inject format_loop_semantic=true
├── Standard GIF                → Read loop_count, app_extensions, palette_size...
└── Standard Video (MP4/MOV...) → Read has_audio, container, duration...
```

Pre-routing does not make any decisions. it is only responsible for populating the `SignalBundle` for consumption by the tree below.
At the same time, `WeightedScore` is initialized (initial value 0.0, range [-1.0, +1.0]), which accumulates through layers 3 to 5.

---

## Layer 0: Duration Dispatcher (Fast-path gating)

> The first entry point of the tree. Dispatches assets based purely on duration to decouple simple short-assets from complex video-pattern analysis.

### Node 0-A: Short/Medium Assets
- Signal: `duration_tier ∈ {UltraShort, Short, MediumLong}` (Duration < 8s)
- **Direct Exit**:
    - `has_audio == true` → **LOOP_WEAK** ("Layer 0: short asset with audio")
    - `has_audio == false` → **LOOP_STRONG** ("Layer 0: silent short asset prior")

### Node 0-B: Long Assets
- Signal: `duration_tier ∈ {Long, VeryLong, DefinitivelyLong}` (Duration ≥ 8s)
- **Proceed to Layer 1** (Enter "Further Judgment" phase)

---

## Stage 1: Further Judgment (Complex Signal Fusion)

> Only reached when Layer 0 determines an asset is "Long" (≥ 8s) and requires deeper analysis to distinguish between a long animation and a standard video.

## Layer 1: Physical Format Constraints (100% certainty, zero ambiguity)


> If a hit occurs, exit immediately; `WeightedScore` is not involved.

### Node 1-A: Has Audio?

- Signal: `has_audio == true`
- Yes → **Direct Exit: LOOP_WEAK** (GIF does not physically support audio, forced)
- No → Next node

### Node 1-B: Has Transparency and No Audio?

- Signal: `has_alpha == true`
- Yes → **Direct Exit: LOOP_STRONG** (Processing video transparency is extremely costly; strongly prefer GIF)
- No → Next node

### Node 1-B3: Dimensional Sticker (Micro-Resolution)
- Signal: `duration_tier ∈ {UltraShort, Short} AND max(width, height) ≤ 320`
- Yes → **Direct Exit: LOOP_STRONG** (Too small to be a regular video, clear sticker intent)
- No → Next node

### Node 1-B4: Micro-Clip (Dimension-Agnostic)
- Signal: `duration_tier == UltraShort`
- Yes → **Direct Exit: LOOP_STRONG** (Any highly transient burst, like a screen recording snippet, is essentially an animated image)
- No → Proceed to Layer 2

---

## Layer 2: Explicit Self-Declaration (Creator / Platform declared intent)

> If a hit occurs, exit immediately; `WeightedScore` is not involved.

### Node 2-A: Infinite Loop Marker?

- Signal: `loop_count == 0`
- Yes → **Direct Exit: LOOP_STRONG** (File explicitly declares "I want to loop infinitely")
- No → Next node

### Node 2-B: Explicit Non-Loop Marker?

- Signal: `loop_count == 1` (Stop after playing once)
- Yes → **Direct Exit: LOOP_WEAK** (File explicitly declares "I only play once")
- No → Next node

### Node 2-C: Platform Source Marker?

- Signal: `app_extensions` contains `GIPHY` / `TENOR` / `STICKER` / `TELEGRAM`
- Yes → **Direct Exit: LOOP_STRONG** (Platform semantic declaration of content nature)
- No → Next node

### Node 2-D: Container Format Semantics?

- Signal: `container == WebM AND has_audio == false AND duration_tier ∈ {UltraShort, Short, MediumLong}`
- Yes → **Direct Exit: LOOP_STRONG** (WebM without audio is a standard carrier for web animations; format itself implies loop semantics for short assets)
- No → Proceed to Layer 3

---

## Layer 3: Self-Referential Structural Signals (Content compared with itself, no external thresholds)

> From this layer onwards, the `WeightedScore` accumulation zone is entered. Each node continues after calculation.
> No individual exit is triggered (unless the score has reached saturation at the end of the layer).

### Node 3-A: First-Last Frame Closure Ratio

- Signal:

```text
closure_ratio = Visual distance between first and last frames / Average inter-frame visual distance
```

- `closure_ratio ≈ 1.0` (First-last jump is comparable to normal inter-frame jumps)
  → `WeightedScore += 0.35` (Highest weight, self-referential without external constants)
- `closure_ratio >> 1.0` (Sudden jump between first and last frames)
  → `WeightedScore -= 0.35`
- Signal missing / Content too overall variable leading to inflated denominator (known edge case)
  → Skip, `WeightedScore` unchanged

> Edge case handling principle: Skip rather than misjudge. When the average inter-frame distance itself is very large,
> the reference baseline for `closure_ratio` is invalid; forcing a skip is better than generating a false signal.

### Node 3-B: Rhythmic Uniformity

- Signal: `interval_consistency_score` (Coefficient of variation of frame intervals, self-referential)
- High Score (Highly uniform intervals) → `WeightedScore += 0.20`
- Low Score (Messy intervals) → `WeightedScore -= 0.15`
- Middle Area → `WeightedScore` unchanged

**End-of-Layer Check**: If `WeightedScore ≥ 0.55` → **Direct Exit: LOOP_STRONG**
　　　　　　If `WeightedScore ≤ -0.55` → **Direct Exit: LOOP_WEAK**
　　　　　　Otherwise → Proceed to Layer 4 (Continue accumulating with current score)

---

## Layer 4: Content Feature Signals (Requires sampling and calculation, higher cost)

### Node 4-A: Palette Size

- Signal: `palette_size`
- `≤ 64` (Typical synthetic content, pixel art, stickers) → `WeightedScore += 0.25`
- `65–128` (Neutral) → `WeightedScore` unchanged
- `> 128` (Approaching natural content color space) → `WeightedScore -= 0.15`

### Node 4-B: Frame Content Compressibility (WebP Compression Ratio)

- Signal: Perform lossy WebP compression on sampled frames, measure `raw_size / webp_size`

```text
Ratio > 15x → Synthetic content (Flat color areas, low entropy) → WeightedScore += 0.20
Ratio < 5x → Natural content (Noisy, high entropy) → WeightedScore -= 0.25
Middle Area → WeightedScore unchanged
```

> This is the most direct proxy for judging "Synthetic vs Natural"—directly measuring how much benefit LZW compression of the GIF provides.

### Node 4-C: compression_efficiency_score

- Signal: `compression_efficiency_score` (Existing implementation)
- `> 0.7` → `WeightedScore += 0.15`
- `< 0.3` → `WeightedScore -= 0.10`
- Middle Area → `WeightedScore` unchanged

**End-of-Layer Check**: If `WeightedScore ≥ 0.55` → **Direct Exit: LOOP_STRONG**
　　　　　　If `WeightedScore ≤ -0.55` → **Direct Exit: LOOP_WEAK**
　　　　　　Otherwise → Proceed to Layer 5

---

## Layer 5: Contextual Semantic Signals (Weakest, auxiliary only)

> All node weights in this layer are intentionally low; they will never reverse the direction on their own, serving only as fine corrections.
> No check points are set at the end of this layer; all non-exited cases proceed to Layer 6.

### Node 5-A: Directory / Filename Semantics

- Signal: `directory_meme_score`, `filename_score`
- Both `> 0.8` → `WeightedScore += 0.10`
- Either `> 0.8` → `WeightedScore += 0.05`
- Otherwise → `WeightedScore` unchanged

### Node 5-B: FPS Anomaly

- Signal: `fps_anomaly_score`
- High Anomaly Value (Non-standard frame rates, typical animation feature) → `WeightedScore += 0.05`
- Otherwise → `WeightedScore` unchanged

### Node 5-C: Duration (Tiered Bias Logic)

- Signal: `duration_tier` (Centralized classification from `constants.rs`)
- **UltraShort** (≤ 2.0s) → `log_odds += 1.5` (Massive prior)
- **Short** (2.0s - 5.0s) → `log_odds += 0.5`
- **MediumLong** (5.0s - 8.0s) → `log_odds -= 0.25`
- **Long** (8.0s - 15.0s) → `log_odds -= 1.0`
- **VeryLong** (15.0s - 18.0s) → `log_odds -= 2.0`
- **DefinitivelyLong** (> 18.0s) → `log_odds -= 3.0` (Definitively rejects loop intent by default)

> Duration enters via discrete Tier classification rather than unstable raw-second thresholds.
> In the modern architecture, primary duration gating happens in Layer 0.


---

## Layer 6: Integrated KNN + WeightedScore Judgment

When reaching this layer, the `SignalBundle` already contains:

- All raw signals calculated in Layers 3 to 5 (used for KNN feature space, not re-calculated)
- Duration (in frame count form)
- Current accumulated `WeightedScore` (passed as an additional KNN feature dimension)

```text
KNN Output: keep_probability, confidence
Integrated Judgment Logic:

final_score = keep_probability * 0.6 + normalize(WeightedScore) * 0.4

confidence > 0.75 AND final_score > 0.6  → LOOP_STRONG
confidence > 0.75 AND final_score ≤ 0.4  → LOOP_WEAK
All other cases                           → Proceed to Layer 6-B directional arbitration
```

> `WeightedScore` is not an independent judge here, but a weighted correction term for KNN.
> The fusion weight ratio (0.6 / 0.4) can be adjusted based on the KNN training set quality:
> Larger training sets are more credible, so KNN weight should be higher; when the set is thin, `WeightedScore` weight should be increased.

---

## Layer 6-B: Directional Arbitration

- Purpose: convert borderline-but-readable cases into an explicit retain/convert decision instead of overusing Layer 7.
- Evidence sources:
  - Tree direction (`log_odds`, `tree_probability`)
  - KNN direction (`keep_probability`, `confidence`, fused score) when available
  - Envelope priors (short silent asset, transparency, square canvas, widescreen, large-video shape)
  - Structural anchors (`loop_closure`, `motion_periodicity`, `loop_frequency`)
- Exit rule:
  - If one side has a clear evidence margin → **Direct Exit: LOOP_STRONG / LOOP_WEAK**
  - Otherwise → Proceed to Layer 7

---

## Layer 7: Conservative Fallback

```text
Only reached when Tree + Layer 6 + Layer 6-B still cannot establish a dominant direction

All fallback cases → Write low_confidence flag to the database and yield UNCERTAIN
```

Value of low confidence markers: These files can be manually reviewed later as new KNN training samples,
allowing the blind spot to narrow naturally over time without needing a one-time solution.

---

## Post-processing: Action Routing

```text
Judgment Tree Output
├── LOOP_STRONG
│   ├── Input is Video → Convert to GIF
│   └── Input is GIF   → Keep as GIF
├── LOOP_WEAK / UNCERTAIN
│   ├── Input is GIF   → Convert to Video (HEVC/AV1)
│   └── Input is Video → Optimize as Video (HEVC/AV1)
└── ERROR
    └── Drop / Skip File entirely (e.g., 1 frame video)
```

---

## Design Principles Comparison by Layer

| Layer                               | Trigger Mechanism                 | WeightedScore             | Reliability             | Computation Cost  |
| :---------------------------------- | :-------------------------------- | :------------------------ | :---------------------- | :---------------- |
| Layer 1: Physical Constraints       | Forced Exit                       | Not Involved              | 100%                    | Extremely Low     |
| Layer 2: Explicit Declaration       | Forced Exit                       | Not Involved              | ~99%                    | Extremely Low     |
| Layer 3: Self-Referential structure | End-of-Layer Check / Accumulation | Weight 0.35 / 0.20        | High, known edge cases  | Low               |
| Layer 4: Content Features           | End-of-Layer Check / Accumulation | Weight 0.25 / 0.20 / 0.15 | Medium                  | Medium (sampling) |
| Layer 5: Contextual Semantics       | Accumulation Only                 | Weight ≤ 0.10             | Weak                    | Low               |
| Layer 6: KNN + Score Fusion         | Probabilistic Exit                | As feature + Correction   | Depends on Training set | High              |
| Layer 6-B: Directional Arbitration  | Explicit tie-break / final route  | Consumes accumulated bias | Medium-High             | Low               |
| Layer 7: Conservative Fallback      | Conservative Default              | Not Involved              | Minimal loss            | Zero              |
