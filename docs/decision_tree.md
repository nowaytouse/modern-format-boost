# Loop Intent Decision Tree — Architecture Reference

> **Source**: `crates/shared_utils/src/loop_intent.rs`  
> **Entry point**: `assess_loop_intent_from_meta` → `evaluate_loop_tree`  
> **Version**: v2 (Zero-Trust Metadata Architecture)

---

## Core Design Principle: Duration is Ground Truth

The loop intent system is built around one axiom:

> **Duration alone has veto power at the extremes. All other signals — including file size,
> resolution, transparency, loop_count, platform markers, and audio presence — are weighted
> evidence that accumulates in a log-odds pipeline.**

This means:
- A 500 MB, 4K resolution file with `loop_count=0` and a GIPHY platform marker that is 1.5s
  long **is an animated image**. Full stop.
- A 50 KB WebP file with perfect loop-closure scores and genuine transparency that is 35s
  long **is a video**. Full stop.
- A 10s file with `loop_count=0` **might** be an animated image if enough other signals
  confirm it, but the metadata alone cannot decide.

---

## Duration Zones and Authority Model

```
0s ───────────────────────────────────────────────────────── ∞
     │← Hard Veto →│← Transition →│← Gray Zone →│← Transition →│← Hard Veto →│
     0            5.5s           6.5s          14.5s          15.5s          ∞

ZONE          DURATION         AUTHORITY          DEFAULT VERDICT
────────────────────────────────────────────────────────────────────────────────────────
Extreme Short  ≤ 6.0s (silent)  Hard/Smooth Veto   LoopStrong
Short Transition 5.5s – 6.5s    Interpolated Veto  Graduated authority
Gray Zone      6.5s – 14.5s     Full pipeline only No default — all signals compete
Long Transition 14.5s – 15.5s   Interpolated Veto  Graduated authority
Extreme Long   ≥ 15.0s          Hard/Smooth Veto   LoopWeak
```

---

## Smooth Veto Architecture

To prevent "behavioral cliffs" (e.g., a 5.9s asset being forced to GIF while a 6.1s asset is 
forced to video), the system implements a **Smooth Veto** mechanism using a 1.0s transition window.

### Logic Flow:
1.  **Veto Injection**: A massive log-odds bias (`LOG_ODDS_EXTREME_VETO_STRENGTH = 15.0`) is calculated.
2.  **Linear Interpolation**: Inside the transition windows (5.5s-6.5s and 14.5s-15.5s), this bias 
    linearly ramps from 15.0 down to 0.0 (or vice versa).
3.  **Hard Veto Exit**: An immediate return only triggers if the asset is strictly outside the 
    transition window (i.e., ≤ 5.5s or ≥ 15.5s).
4.  **Soft Integration**: Assets within the transition window proceed to the full pipeline, 
    but with a very high starting bias, ensuring the resulting probability curve is continuous.

---

## Layer Architecture

### Layer 0-EX: Extreme Duration Smooth Veto

**The only two zones with absolute authority.**

```
IF duration ≤ 5.5s AND silent:   → LoopStrong (Hard Veto)
IF duration ≥ 15.5s:             → LoopWeak (Hard Veto)

IF in [5.5s, 6.5s] OR [14.5s, 15.5s]:
    → Inject interpolated Extreme Bias into Log-Odds
```

**Why these boundaries?**
- **6.0s**: The refined threshold for "expressive short-form animation". Silent content under 6s 
  is almost universally intended to be looped or treated as a sticker.
- **15.0s**: The practical upper limit for looping media. Any asset longer than 15s is 
  statistically much more likely to be standard video content.

### Layer 0: Degenerate Input Guard (Error)

```
IF frame_count ≤ 1:              → Error (cannot loop, physical impossibility)
IF duration < 0.01s (non-GIF):  → Error (degenerate duration)
```

### Layer 0: Duration Bias Dispatcher

For assets in the 2–30s gray zone, the dispatcher injects a tier-proportional log-odds bias
before dispatching to the specialized sub-tree. This is a **soft prior**, not a veto.

| Tier | Duration | Log-Odds Bias | Buffer Zone Bonus |
|---|---|---|---|
| UltraShort | ≤ 2.0s | +1.5 | +1.0 (if 2.0–4.0s) |
| Short | 2.0–5.0s | +0.5 | +1.0 (if 2.0–4.0s) |
| MediumLong | 5.0–8.0s | -0.25 | — |
| Long | 8.0–15.0s | -1.0 | — |
| VeryLong | 15.0–18.0s | -2.0 | — |
| DefinitivelyLong | 18.0–30.0s | -3.0 | -1.5 (if ≥ 20.0s) |

> **Note**: The tier bias is applied **once** at the top-level dispatcher and **not** repeated
> in the specialized image/video sub-trees.

---

## Stage 1: Specialized Tree Dispatch

After Layer 0 bias injection, the asset is routed to one of two sub-trees:
- **Image Tree** (`evaluate_image_tree`): for `is_native_gif` or image-family extensions
  (WebP, AVIF, APNG, JXL, HEIC, HEIF)
- **Video Tree** (`evaluate_video_tree`): for all other containers

---

## Layer 1: Hard Physical Constraints

These are strong physical signals. Under the zero-trust architecture, they are **weighted
signals** rather than immediate exits (except for audio at non-UltraShort tiers, which
becomes overwhelming).

### Layer 1-A: Audio Track (Video Tree)
```
IF audible audio (mean_volume > -70 dB) AND NOT silent:
    penalty = match tier {
        UltraShort  → -SCENE_CUT_NEGATIVE_LOG_ODDS × 0.6
        Short       → -SCENE_CUT_NEGATIVE_LOG_ODDS
        _           → -LOG_ODDS_BIAS_DEFINITIVELY_LONG  (very strong)
    }
    log_odds.add(penalty)
    # Continues to full pipeline — does NOT exit immediately
```

Rationale: An UltraShort video (already caught by 0-EX if silent) with a brief click sound
should still be evaluated by structural signals. A Long video with audible audio is practically
certain to be video, but the structural analysis still runs to provide a logged verdict.

### Layer 1-B: Transparency (Image Tree)
```
IF no audible audio AND has_transparency:
    log_odds.add(TRANSPARENCY_POSITIVE_LOG_ODDS × 2.0)
    # Does NOT exit immediately — a 25-minute transparent WebP is still a video
```

### Layer 1-B2: Sticker-Class Native GIF (Image Tree)
```
IF gif AND silent AND short tier AND canvas ≤ 512px AND pixels ≤ 200,000:
    log_odds.add(COMPACT_SILENT_POSITIVE_LOG_ODDS)
    # Weighted bonus, not immediate exit
```

### Layer 1-B3: Dimensional Sticker (Video Tree)
```
IF UltraShort AND canvas ≤ 512px AND sparse packet data:
    log_odds.add(COMPACT_SILENT_POSITIVE_LOG_ODDS)
    # Weighted bonus, not immediate exit
```

### Layer 1-B4: Micro-Clip (Video Tree)
```
IF tier == UltraShort AND duration > 0.0:
    → LoopStrong (this IS a duration-based exit, consistent with 0-EX philosophy)
```

---

## Layer 2: Explicit Declarations (Weighted Signals Only)

These are formerly "immediate exit" signals, now reduced to weighted bonuses/penalties.

```
loop_count == 0:    log_odds.add(loop_count_zero_bonus(meta, thresholds))
                    # Bonus decays as duration increases — large bonus at 3s, tiny at 15s

loop_count == 1:    log_odds.add(-PLAY_ONCE_NEGATIVE_LOG_ODDS)

Platform marker     log_odds.add(PLATFORM_MARKER_POSITIVE_LOG_ODDS)
(GIPHY, TENOR, ...):

Short silent WebM:  log_odds.add(COMPACT_SILENT_POSITIVE_LOG_ODDS)
```

**Security note**: These signals are trivially forgeable in container metadata. A malicious
file could declare `loop_count=0` and `GIPHY` platform markers while containing 10 minutes
of high-definition video. Under the zero-trust architecture, these signals alone cannot
produce a `LoopStrong` verdict for assets in the gray zone.

---

## Layers 3–5: Physical Signal Fusion (Log-Odds Accumulation)

These layers accumulate evidence from physical measurements of the actual media content.
They are the primary decision mechanism for gray-zone assets.

### Layer 3: Structural Kinetics (Checkpoint at ±0.55)

Physical motion and loop-structure signals:
- **Loop closure score** (`loop_closure_score`): How similar is the first frame to the last?
  High similarity → strong loop prior. Low similarity → anti-loop.
- **Motion periodicity** (`motion_periodicity`): Does the motion repeat rhythmically?
- **Loop frequency** (`score_loop_frequency`): Does the frame cadence suggest looping animation?
- **Sparse cadence** (`score_sparse_cadence`): Low-FPS animation style?
- **Temporal jitter** (`temporal_jitter`): Irregular frame delays typical of GIF encoders?
- **Scene cut** (`scene_cut`): Hard cuts strongly imply non-looping video.
- **Compactness signal**: Small file + small canvas = animation prior.
- **Large media signal**: Large file + large canvas = video prior.

After accumulation, a **checkpoint** fires at ±0.55 log-odds:
```
IF |log_odds| >= 0.55 THRESHOLD:
    → LoopStrong or LoopWeak (structural evidence is decisive)
```

### Layer 4: Content Envelope (Checkpoint at ±0.78)

Weaker content-level signals:
- WebP compression ratio, motion Gini coefficient, palette depth, temporal flatness.
- Format-specific bonuses (image container, square aspect ratio, widescreen penalty).
- Long-silent video penalty (asset is long + silent + non-image = suspicious).
- Directory and filename context (low weight: 0.12 and 0.10 respectively).

After accumulation, a **checkpoint** fires at ±0.78 log-odds.

### Layer 5: Final Arbitration

```
IF log_odds >= decision_threshold:   → LoopStrong
IF log_odds <= -decision_threshold:  → LoopWeak
ELSE:                                → Uncertain
```

---

## Layer 6: KNN + Fusion

For `Uncertain` verdicts, a K-Nearest-Neighbor lookup against the labeled sample database
provides additional classification confidence. The final verdict is a weighted fusion of the
tree probability and the KNN probability.

---

## Layer 7: Conservative Fallback

If KNN data is insufficient (cold start), the system falls back to a conservative verdict
based on the raw log-odds value and asset type.

---

## Anti-Forgery Guarantees

The system provides the following guarantees against metadata manipulation:

1. **`loop_count=0` cannot force `LoopStrong` at > 30s** — the hard veto fires first.
2. **`loop_count=0` + GIPHY marker cannot force `LoopStrong` at 25s** — the -3.0 bias from
   the long-duration tier plus the -1.5 buffer zone bonus requires physical evidence
   (loop closure, periodicity) to overcome.
3. **Extreme file size cannot force `LoopWeak` at < 2s** — the hard veto fires first.
4. **Transparency flag cannot force `LoopStrong` for long assets** — it only contributes
   `TRANSPARENCY_POSITIVE_LOG_ODDS × 2 = 0.68`, which is easily overcome by the -3.0+
   long-tier bias.

---

## Constants Reference

| Constant | Value | Purpose |
|---|---|---|
| `EXTREME_SHORT_ABSOLUTE_LIMIT_SECS` | 2.0s | Hard veto boundary (GIF side) |
| `EXTREME_LONG_ABSOLUTE_LIMIT_SECS` | 30.0s | Hard veto boundary (Video side) |
| `EXTREME_SHORT_BUFFER_UPPER_SECS` | 4.0s | Upper edge of pro-loop buffer zone |
| `EXTREME_SHORT_BUFFER_BIAS` | +1.0 | Additional bias in 2–4s buffer |
| `EXTREME_LONG_BUFFER_LOWER_SECS` | 20.0s | Lower edge of anti-loop buffer zone |
| `EXTREME_LONG_BUFFER_BIAS` | -1.5 | Additional penalty in 20–30s buffer |
| `LOG_ODDS_BIAS_ULTRA_SHORT` | +1.5 | Tier bias for ≤ 2s |
| `LOG_ODDS_BIAS_SHORT` | +0.5 | Tier bias for 2–5s |
| `LOG_ODDS_BIAS_MEDIUM_LONG` | -0.25 | Tier bias for 5–8s |
| `LOG_ODDS_BIAS_LONG` | -1.0 | Tier bias for 8–15s |
| `LOG_ODDS_BIAS_VERY_LONG` | -2.0 | Tier bias for 15–18s |
| `LOG_ODDS_BIAS_DEFINITIVELY_LONG` | -3.0 | Tier bias for 18–30s |
| `TREE_STRUCTURAL_CHECKPOINT_LOG_ODDS_THRESHOLD` | 0.55 | Layer 3 checkpoint |
| `TREE_CONTENT_CHECKPOINT_LOG_ODDS_THRESHOLD` | 0.78 | Layer 4 checkpoint |
| `TREE_DECISION_LOG_ODDS_THRESHOLD` | 0.95 | Layer 5 final arbitration |
