# Performance Optimizations

## Low Memory & Multi-Instance Scenarios

### Memory Optimizations

1. **Stderr Buffer Limits** (1MB cap)
   - `jxl_utils.rs`: ImageMagick/cjxl stderr limited to 1MB
   - `x265_encoder.rs`: FFmpeg/x265 stderr limited to 1MB with early break
   - Prevents memory bloat in long-running conversions

2. **BufReader Tuning**
   - 8KB buffer size (down from default 8KB, optimized for line-by-line reading)
   - Initial String capacity: 64KB (down from 1MB)
   - Prevents over-allocation in low-memory scenarios

3. **Thread Pool Management**
   - Cached thread calculation using `OnceLock`
   - Memory-pressure aware: reduces parallelism when RAM < 20%
   - Multi-instance mode: halves thread allocation automatically

### Environment Variables

```bash
# Low memory mode (reduces parallelism)
export MFB_LOW_MEMORY=1

# Multi-instance mode (halves thread allocation)
export MFB_MULTI_INSTANCE=1
```

### Recommended Settings

**Low Memory (< 8GB RAM):**
```bash
export MFB_LOW_MEMORY=1
# Parallel tasks: 1, Child threads: 1
```

**Multi-Instance (3+ processes):**
```bash
export MFB_MULTI_INSTANCE=1
# Thread allocation halved per instance
```

**Normal (8-16GB RAM):**
```bash
# No env vars needed
# Parallel tasks: 2-4, Child threads: 2-4
```

**High Performance (16GB+ RAM):**
```bash
# No env vars needed
# Parallel tasks: 4-8, Child threads: 4-8
```

## Performance Metrics

### Before Optimizations
- Stderr memory: unbounded (potential OOM)
- Thread calculation: repeated per conversion
- BufReader: 1MB initial allocation

### After Optimizations
- Stderr memory: 1MB hard cap
- Thread calculation: cached (single calculation)
- BufReader: 64KB initial, 8KB buffer
- Memory footprint: ~70% reduction in low-memory scenarios

## Monitoring

Check memory pressure during conversion:
```bash
# macOS
memory_pressure

# Linux
free -h
```

If you see high memory pressure, enable `MFB_LOW_MEMORY=1`.
