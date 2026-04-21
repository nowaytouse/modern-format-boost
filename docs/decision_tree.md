# Loop Intent Decision Tree — Architecture Reference

> **Source**: `crates/shared_utils/src/loop_intent.rs`  
> **Entry point**: `assess_loop_intent_from_meta` → `evaluate_loop_tree`  
> **Version**: v2.1 (Zero-Trust Metadata + Anti-Cliff Architecture)

---

## Core Design Principle: Duration is Ground Truth

The loop intent system is built around one axiom:

> **Duration alone has veto power at the extremes. All other signals — including file size,
> resolution, transparency, loop_count, platform markers, and audio presence — are weighted
> evidence that accumulates in a log-odds pipeline.**

This means:
- A 500 MB, 4K resolution file with `loop_count=0` and a GIPHY platform marker that is 4s
  long **is an animated image**. Full stop.
- A 50 KB WebP file with perfect loop-closure scores and genuine transparency that is 20s
  long **is a video**. Full stop.
- A 10s file with `loop_count=0` **might** be an animated image if enough other signals
  confirm it, but the metadata alone cannot decide.

---

## Duration Zones and Authority Model

```
0s ──────────────────────────────────────────── 15s ─── ∞
     │← Hard Veto →│← Prox Ramp →│← Gray Zone →│← Prox Ramp →│← Hard Veto →│
     0             6s            8s            13s            15s           ∞

ZONE              DURATION       AUTHORITY                DEFAULT VERDICT
────────────────────────────────────────────────────────────────────────────────────────
Extreme Short     ≤ 6.0s (silent) Absolute veto           LoopStrong — no exceptions
Short Proximity   6.0–8.0s (silent) Tier bias + proximity  Very strong pro-loop (decaying)
Gray Zone         8.0–13.0s      Tier bias only            Depends on full pipeline
Long Proximity    13.0–15.0s     Tier bias + proximity     Very strong anti-loop (growing)
Extreme Long      ≥ 15.0s        Absolute veto            LoopWeak — no exceptions
```

---

## Anti-Cliff Defense: The Proximity Ramp

A naive hard boundary creates a **behavioral cliff**:
```
5.9s → Hard Veto → LoopStrong (certain, log-odds irrelevant)
6.1s → Only tier bias → much weaker prior
```

The **Proximity Ramp** eliminates this discontinuity with linear interpolation:

```
proximity_factor = 1.0 - (duration - veto_limit) / buffer_width
additional_bias  = proximity_factor × MAX_BIAS  (decays from MAX to 0)
```

**Short side (silent assets, 6.0–8.0s):**
| Duration | Proximity | Additional Bias | Total Effective Bias |
|---|---|---|---|
| 5.9s | (veto) | — | LoopStrong (absolute) |
| 6.1s | 0.95 | +2.375 | Very strong LoopStrong prior |
| 7.0s | 0.50 | +1.25 | Strong LoopStrong prior |
| 8.0s | 0.00 | +0.00 | Tier bias only (MediumLong: -0.25) |

**Long side (all assets, 13.0–15.0s):**
| Duration | Proximity | Additional Penalty | Total Effective Bias |
|---|---|---|---|
| 13.0s | 0.00 | -0.00 | Tier bias only (Long: -1.0) |
| 14.0s | 0.50 | -1.25 | Strong LoopWeak prior |
| 14.9s | 0.95 | -2.375 | Very strong LoopWeak prior |
| 15.0s | (veto) | — | LoopWeak (absolute) |

---

## Layer Architecture

### Layer 0-EX: Extreme Duration Hard Veto

**The only two conditions with one-shot authority.**

```
IF duration ≤ 6.0s AND no audible audio:
    → LoopStrong (Hard Veto) — exits immediately, no further analysis

IF duration ≥ 15.0s:
    → LoopWeak (Hard Veto) — exits immediately, no further analysis
```

**Why these boundaries?**
- **6.0s**: Empirically covers all real-world stickers, reactions, short memes, and looping
  UI animations. Screen recordings intended for sharing as GIFs rarely exceed this. Audible
  audio is explicitly excluded because a real short video with audio should be classified
  as video even at 3s.
- **15.0s**: The practical upper bound for any real-world looping animated image. GIF/WebP
  sticker platforms (Tenor, GIPHY, Telegram) enforce strict duration limits well below this.
  Above 15s, content is unambiguously video regardless of container or metadata.

### Layer 0: Degenerate Input Guard (Error)

```
IF frame_count ≤ 1:              → Error (cannot loop, physical impossibility)
IF duration < 0.01s (non-GIF):  → Error (degenerate duration)
```

### Layer 0: Duration Bias Dispatcher + Proximity Ramp

For assets in the 6–15s gray zone:
1. **Tier-proportional base bias** applied (once, at the top level — not repeated in sub-trees)
2. **Proximity ramp** applied for assets within 2s of either veto boundary

| Tier | Duration | Base Bias |
|---|---|---|
| UltraShort | ≤ 2.0s | +1.5 |
| Short | 2.0–5.0s | +0.5 |
| MediumLong | 5.0–8.0s | -0.25 |
| Long | 8.0–15.0s | -1.0 |
| VeryLong | 15.0–18.0s | -2.0 |
| DefinitivelyLong | > 18.0s | -3.0 |

> **Note**: The tier bias is applied **once** at the top-level dispatcher. Sub-trees
> (`evaluate_image_tree`, `evaluate_video_tree`) do **not** re-apply it.

---

## Stage 1: Specialized Tree Dispatch

After Layer 0 bias injection, the asset is routed to one of two sub-trees:
- **Image Tree** (`evaluate_image_tree`): for `is_native_gif` or image-family extensions
  (WebP, AVIF, APNG, JXL, HEIC, HEIF)
- **Video Tree** (`evaluate_video_tree`): for all other containers

---

## Layer 1: Hard Physical Constraints (Weighted, Not Absolute)

Under the zero-trust architecture, physical signals are **weighted contributions** rather
than immediate exits (with exceptions noted).

### Layer 1-A: Audio Track (Video Tree)
```
IF audible audio (mean_volume > -70 dB):
    penalty = match tier {
        UltraShort  → -SCENE_CUT_NEGATIVE_LOG_ODDS × 0.6
        Short       → -SCENE_CUT_NEGATIVE_LOG_ODDS
        _           → -LOG_ODDS_BIAS_DEFINITIVELY_LONG  (overwhelmingly strong)
    }
    log_odds.add(penalty)  # Continues to full pipeline
```

### Layer 1-B: Transparency (Image Tree)
```
IF no audible audio AND has_transparency:
    log_odds.add(TRANSPARENCY_POSITIVE_LOG_ODDS × 2.0)
    # Weighted bonus — a 20s transparent WebP is still classified as video (it gets veto'd)
```

### Layer 1-B2: Sticker-Class Native GIF
```
IF gif AND silent AND (in short tier) AND canvas ≤ 512px AND pixels ≤ 200,000:
    log_odds.add(COMPACT_SILENT_POSITIVE_LOG_ODDS)
```

### Layer 1-B3: Dimensional Sticker (Video Tree)
```
IF UltraShort AND canvas ≤ 512px AND sparse packet data:
    log_odds.add(COMPACT_SILENT_POSITIVE_LOG_ODDS)
```

### Layer 1-B4: Micro-Clip (Video Tree)
```
IF tier == UltraShort AND duration > 0.0:
    → LoopStrong (duration-grounded exit — consistent with 0-EX philosophy)
```

---

## Layer 2: Explicit Declarations (Weighted Signals Only)

Formerly "immediate exit" signals, now reduced to weighted bonuses/penalties.

```
loop_count == 0:    log_odds.add(loop_count_zero_bonus(meta, thresholds))
                    # Bonus decays as duration increases

loop_count == 1:    log_odds.add(-PLAY_ONCE_NEGATIVE_LOG_ODDS)

Platform marker:    log_odds.add(PLATFORM_MARKER_POSITIVE_LOG_ODDS)
(GIPHY, TENOR, ...)

Short silent WebM:  log_odds.add(COMPACT_SILENT_POSITIVE_LOG_ODDS)
```

**Security note**: All these signals are trivially forgeable in container metadata. Under
the zero-trust architecture, they cannot produce a verdict alone. For gray-zone assets,
they accumulate as evidence that must overcome the duration-based prior.

---

## Layers 3–5: Physical Signal Fusion

### Layer 3: Structural Kinetics (Checkpoint at ±0.55)

Physical motion and loop-structure signals:
- **Loop closure score**: How similar is the first frame to the last?
- **Motion periodicity**: Does the motion repeat rhythmically?
- **Loop frequency**: Does the frame cadence suggest looping animation?
- **Sparse cadence**: Low-FPS animation style?
- **Temporal jitter**: Irregular frame delays typical of GIF encoders?
- **Scene cut**: Hard cuts strongly imply non-looping video.
- **Compactness / large media signals**: File size × canvas size priors.

**Checkpoint** at ±0.55: exits if structural evidence is already decisive.

### Layer 4: Content Envelope (Checkpoint at ±0.78)

- WebP compression ratio, motion Gini, palette depth, temporal flatness.
- Format bonuses, aspect ratio signals, long-silent penalty.
- Directory and filename context (low weight: 0.12 and 0.10).

**Checkpoint** at ±0.78.

### Layer 5: Final Arbitration

```
IF log_odds >= decision_threshold (0.95):   → LoopStrong
IF log_odds <= -decision_threshold (0.95):  → LoopWeak
ELSE:                                        → Uncertain
```

---

## Layers 6–7: KNN Fusion and Fallback

**Layer 6**: For `Uncertain` verdicts, K-Nearest-Neighbor lookup against the labeled sample
database. Final verdict is a weighted fusion of tree probability and KNN probability.

**Layer 7**: If KNN data is insufficient (cold start), conservative fallback based on raw
log-odds value and asset type.

---

## Metadata Trust Decay (Gray Zone Defense)

To prevent forged metadata (e.g., a 14s video declaring `loop_count=0` and `GIPHY` tags) from
overwhelming the duration-based priors, the system implements **Metadata Trust Decay**.

Soft metadata signals are multiplied by a `metadata_trust` factor in the 6.0–15.0s range:
- **Trust = 1.0** at 6.0s (Full confidence)
- **Trust = 0.0** at 15.0s (Zero confidence in metadata)

**Attenuated Signals:**
- `loop_count == 0` bonus
- Platform markers (`GIPHY`, `TENOR`, etc.)
- Transparency flag bonus

**Non-Attenuated Signals (Physical Reality):**
- All Layer 3–5 structural and content signals (Loop closure, periodicity, etc.)
- Audio presence/silence signals
- Tier-based log-odds bias

### Effect on Gray Zone Forgery
At 12.0s, the `metadata_trust` is approximately **0.33**. A forged `GIPHY` marker (+0.52)
only contributes **+0.17** to the log-odds. This ensures that metadata alone cannot "pull"
 a long asset back into a `LoopStrong` verdict; it **must** be supported by genuine physical
 evidence from the frames themselves.

---

## Anti-Forgery Guarantees

| Scenario | Outcome | Reason |
|---|---|---|
| `loop_count=0` + GIPHY at 14.9s | LoopWeak | Proximity ramp (-2.375) + Trust Decay (~0.01) overwhelms all forgery |
| `loop_count=0` + GIPHY at 16s | LoopWeak (Hard Veto) | Extreme long veto fires first |
| 500 MB 4K file at 4s silent | LoopStrong (Hard Veto) | Extreme short veto fires first; file size has no vote |
| Transparent WebP at 14s | LoopWeak | Long bias (-1.0) + Ramp (-1.25) >> Trust-decayed transparency |
| Silent WebM at 12s (gray zone) | Physical Reality | Metadata trust is low (~0.33); physical loop signals must prove the intent |

---

## Constants Reference

| Constant | Value | Purpose |
|---|---|---|
| `EXTREME_SHORT_ABSOLUTE_LIMIT_SECS` | **6.0s** | Hard veto boundary (GIF side) |
| `EXTREME_LONG_ABSOLUTE_LIMIT_SECS` | **15.0s** | Hard veto boundary (Video side) |
| `EXTREME_SHORT_PROXIMITY_BUFFER_SECS` | 2.0s | Width of anti-cliff ramp above 6s |
| `EXTREME_SHORT_PROXIMITY_MAX_BIAS` | +2.5 | Max bonus at 6.0+ε (decays to 0 at 8s) |
| `EXTREME_LONG_PROXIMITY_BUFFER_SECS` | 2.0s | Width of anti-cliff ramp below 15s |
| `EXTREME_LONG_PROXIMITY_MAX_BIAS` | -2.5 | Max penalty at 15.0-ε (decays to 0 at 13s) |
| `LOG_ODDS_BIAS_ULTRA_SHORT` | +1.5 | Tier bias for ≤ 2s |
| `LOG_ODDS_BIAS_SHORT` | +0.5 | Tier bias for 2–5s |
| `LOG_ODDS_BIAS_MEDIUM_LONG` | -0.25 | Tier bias for 5–8s |
| `LOG_ODDS_BIAS_LONG` | -1.0 | Tier bias for 8–15s |
| `LOG_ODDS_BIAS_VERY_LONG` | -2.0 | Tier bias for 15–18s |
| `LOG_ODDS_BIAS_DEFINITIVELY_LONG` | -3.0 | Tier bias for 18+s |
| `TREE_STRUCTURAL_CHECKPOINT_LOG_ODDS_THRESHOLD` | 0.55 | Layer 3 checkpoint |
| `TREE_CONTENT_CHECKPOINT_LOG_ODDS_THRESHOLD` | 0.78 | Layer 4 checkpoint |
| `TREE_DECISION_LOG_ODDS_THRESHOLD` | 0.95 | Layer 5 final arbitration |
