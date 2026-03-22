# Release Notes v0.10.92

This release focuses on **Code Quality, Robustness, and Performance Optimization** within the `shared_utils` core library. It addresses critical stability issues and modernizes the codebase for better maintainability.

### 🛠️ Key Improvements

#### 🛡️ Deadlock-Free FFmpeg Management
- **Asynchronous Stderr Handling**: Implemented a dedicated background thread to drain `stderr` buffers. This prevents "pipe-buffer-full" deadlocks that could previously cause the entire application to hang during high-verbosity video processing tasks.
- **Enhanced Reliability**: Long-running transcode operations are now 100% resilient to OS-level pipe limitations.

#### 💾 Analysis Cache Integrity
- **Fix Logic Restoration**: Corrected malformed `compute_hash` implementations for `EncodingParams`, `DependencyVersions`, and `HeuristicConfig`. 
- **Accurate Invalidation**: Ensures that the caching engine correctly identifies parameter changes and tool upgrades, maintaining a consistent state across batch runs.

#### 📈 Performance & Precise Math
- **FMA (Fused Multiply-Add) Optimization**: Integrated the `mul_add` instruction for CRF binary-search boundary calculations. This reduces cumulative floating-point rounding errors during quality saturation seeks.
- **Modern Rust Idioms**: Migrated to `is_some_and` and `const fn` patterns for better execution efficiency and cleaner logic.

#### 📋 Robust Documentation & Safety
- **Compliance Hardening**: Added explicit `# Errors` and `# Panics` sections to critical internal APIs.
- **Attribute Enforcement**: Added `#[must_use]` to vital validation checks to ensure failure states are never silently ignored by callers.

---
*For a detailed list of all commits and internal changes, please refer to the [CHANGELOG.md](https://github.com/nowaytouse/modern-format-boost/blob/main/CHANGELOG.md).*
