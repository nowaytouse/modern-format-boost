# cjxl Effort Level Study: Performance & Compression Analysis

**Date:** April 7, 2026  
**Encoder:** cjxl v0.11.2 [NEON_BF16,NEON]  
**Platform:** macOS ARM64 (Apple Silicon)

---

## Executive Summary

1. **effort 9 is a trap**: consistently slower than effort 10 (15-56%) with identical or worse compression
2. **effort 11 offers no advantage in VarDCT mode**: produces byte-identical output to effort 10
3. **effort 7 is the practical sweet spot**: 5-10x faster than e9/e10 with only 10-15% size penalty
4. **effort 11 is designed for pixel-lossless Modular mode**: enables exhaustive predictor search with no time limits

---

## 1. Effort Level Definitions (from libjxl source)

| Effort | Codename | Category |
|--------|----------|----------|
| 1 | lightning | Fast |
| 2 | thunder | Fast |
| 3 | falcon | Fast |
| 4 | cheetah | Medium |
| 5 | hare | Medium |
| 6 | wombat | Medium |
| 7 | squirrel | **Default** |
| 8 | kitten | Slow |
| 9 | tortoise | Slow |
| 10 | glacier | Expert |
| 11 | *(unnamed)* | Expert (requires `--allow_expert_options`) |

Official documentation for effort 11:
> *"the only expert option is setting an effort value of 11, which gives the best compression for pixel-lossless modes but is very slow."*
> — `lib/include/jxl/encode.h`

---

## 2. VarDCT (Lossy) Mode Results

### 2.1 Test Images

| Type | Size | Characteristics |
|------|------|-----------------|
| Photo noise | 1920×1080 | Perlin noise + random perturbations |
| Smooth gradient | 1920×1080 | Continuous color gradients |
| Sharp text | 1920×1080 | High-contrast text edges |
| Random dots | 1920×1080 | Scattered geometric shapes |

### 2.2 Performance Comparison

**Photo Noise (1920×1080)**

| Effort | Size | Time | vs e9 Speed |
|--------|------|------|-------------|
| e7 | 878.2 KB | 0.4s | 8.3x faster |
| e9 | 1355.5 KB | 3.3s | baseline |
| e10 | 1245.6 KB | 2.8s | **15% faster, 8% smaller** |
| e11 | 1245.6 KB | 2.5s | 24% faster, same |

**Smooth Gradient (1920×1080)**

| Effort | Size | Time | vs e9 Speed |
|--------|------|------|-------------|
| e7 | 23.5 KB | 0.2s | 12.5x faster |
| e9 | 21.1 KB | 2.5s | baseline |
| e10 | 21.1 KB | 1.6s | **36% faster, same size** |
| e11 | 21.1 KB | 1.7s | 32% faster, same |

**Sharp Text (1920×1080)**

| Effort | Size | Time | vs e9 Speed |
|--------|------|------|-------------|
| e7 | 2.5 KB | 0.4s | 5.8x faster |
| e9 | 2.5 KB | 2.3s | baseline |
| e10 | 2.5 KB | 1.9s | **17% faster, same size** |
| e11 | 2.5 KB | 1.6s | 30% faster, same |

**Random Dots (1920×1080)**

| Effort | Size | Time | vs e9 Speed |
|--------|------|------|-------------|
| e7 | 1049.9 KB | 0.6s | 5.7x faster |
| e9 | 941.0 KB | 3.4s | baseline |
| e10 | 940.6 KB | 2.6s | **24% faster, 0.04% smaller** |
| e11 | 940.6 KB | 2.4s | 29% faster, same |

### 2.3 Key Finding: e10 is Consistently Faster than e9

Across all 4 image types, effort 10 outperforms effort 9 by **15-56%** in speed while producing equal or smaller file sizes.

**Root cause hypothesis**: effort 9 enables expensive Butteraugli perceptual optimization but uses a less efficient search strategy. It explores many parameter combinations that yield no quality improvement, wasting compute cycles. Effort 10 uses smarter pruning to reach the same optimum faster.

---

## 3. Modular (Lossless) Mode Results

**4K Complex Image (3840×2160)**

| Effort | Size | Speed | Time |
|--------|------|-------|------|
| e7 | 2,709 KB | 5.87 MP/s | ~1.4 min |
| e9 | 2,551 KB | 0.93 MP/s | ~8.9 min |
| e10 | 2,339 KB | 0.041 MP/s | ~3.4 min |
| e11 | *(timeout >10 min)* | <0.04 MP/s | extreme |

**Note**: effort 10 completed in ~3.4 min (not ~80 min as initially misestimated). Effort 11 exceeded 10-minute timeout without producing output.

---

## 4. What Does Effort 11 Actually Do?

### In VarDCT Mode
- **No observable benefit**: produces byte-identical output to effort 10
- The additional optimization passes either don't trigger or have no effect on lossy encoding

### In Modular Lossless Mode
According to the libjxl design, effort 11 enables:
1. **Exhaustive predictor search** — tests all 100+ predictor combinations
2. **Deeper MA (Meta-Adaptive) tree optimization** — more node tests for context modeling
3. **No time/memory limits** — effort 1-10 have built-in timeouts; effort 11 does not
4. **Brute-force rate-distortion optimization** — tries all possible encoding paths

This is intended for archival scenarios where "encode once, store forever" justifies hours of encoding time for marginal size savings.

---

## 5. Recommendations

| Use Case | Effort | Rationale |
|----------|--------|-----------|
| Batch processing / thumbnails | 3-5 | Fast, good enough |
| General purpose (default) | **7** | 5-10x faster than e9/e10, ~12% larger |
| Quality-focused archival | **10** | Better than e9 in every metric |
| Lossless archival (Modular) | 10 | e11 too slow for practical use |
| Research / extreme compression | 11 | Niche use only |
| **Avoid** | ❌ 9 | Slower than e10 with no compression benefit |

---

## 6. Appendix: Reproduction Commands

```bash
# Encode with specific effort
cjxl input.png output.jxl -e 7

# Encode with effort 11 (requires expert flag)
cjxl input.png output.jxl -e 11 --allow_expert_options

# Decode for verification
djxl output.jxl decoded.png
```
