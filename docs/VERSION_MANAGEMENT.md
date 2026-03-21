# Version Management Guide

## Overview

This project uses a **unified version management system** where all version numbers are automatically synchronized with the main program version defined in `Cargo.toml`.

## Version Number Binding

### Primary Version Source
- **Location**: `Cargo.toml` → `[workspace.package]` → `version`
- **Format**: `MAJOR.MINOR.PATCH` (e.g., `0.10.85`)
- **Scope**: Applies to ALL workspace crates

### Centralized Version Management

All version numbers are managed through the `shared_utils::version` module:

```rust
use shared_utils::version::{PROGRAM_VERSION, cache_algorithm_version, CACHE_SCHEMA_VERSION, VersionInfo};

// Program version (from Cargo.toml)
println!("Program: {}", PROGRAM_VERSION);  // "0.10.85"

// Cache algorithm version (auto-calculated)
println!("Cache Algorithm: {}", cache_algorithm_version());  // 1085

// Cache schema version (manual)
println!("Cache Schema: {}", CACHE_SCHEMA_VERSION);  // 3

// Display all versions
let info = VersionInfo::current();
println!("{}", info.display());
```

### Automatically Bound Versions

The following version numbers are **automatically derived** from `CARGO_PKG_VERSION` at compile time:

1. **Program Version** (`PROGRAM_VERSION`)
   - **Location**: `shared_utils/src/version.rs`
   - **Format**: String (e.g., `"0.10.85"`)
   - **Source**: `env!("CARGO_PKG_VERSION")`
   - **Purpose**: Single source of truth for program version

2. **Cache Algorithm Version** (`cache_algorithm_version()`)
   - **Location**: `shared_utils/src/version.rs`
   - **Format**: `MajorMinorPatch` (e.g., `0.10.85` → `1085`)
   - **Calculation**: `Major * 10000 + Minor * 100 + Patch`
   - **Purpose**: Automatic cache invalidation on ANY program update
   - **Implementation**: Uses `LazyLock` to parse version at runtime
   - **Failure Mode**: Panics if version parsing fails (intentional - ensures correctness)

3. **Database Schema Version** (`CACHE_SCHEMA_VERSION`)
   - **Location**: `shared_utils/src/version.rs`
   - **Format**: Integer (e.g., `3`)
   - **Purpose**: Track database structure changes
   - **Update Policy**: Increment manually ONLY when database schema changes
   - **Current**: v3 (added content_fingerprint_hash and data_checksum columns)

## Branch Strategy

### Main Branch
- **Dependencies**: Stable crates.io versions
- **Purpose**: Production-ready releases
- **Stability**: High - uses tested, published crates
- **Example**:
  ```toml
  image = { version = "0.25", features = [...] }
  rusqlite = { version = "0.32", features = ["bundled"] }
  blake3 = "1.5"
  mp4parse = "0.17"
  ```

### Nightly Branch
- **Dependencies**: GitHub HEAD sources
- **Purpose**: Latest upstream iterations and fast bug fixes
- **Stability**: Medium - uses bleeding-edge code
- **Example**:
  ```toml
  image = { git = "https://github.com/image-rs/image", features = [...] }
  rusqlite = { git = "https://github.com/rusqlite/rusqlite", features = ["bundled"] }
  blake3 = { git = "https://github.com/BLAKE3-team/BLAKE3" }
  mp4parse = { git = "https://github.com/mozilla/mp4parse-rust" }
  ```
- **Patch Section**: Forces all transitive dependencies to use GitHub sources

## Version Update Workflow

### For Regular Updates (Bug Fixes, Features)

1. **Update ONLY ONE place**: `Cargo.toml` → `[workspace.package]` → `version`
   ```toml
   [workspace.package]
   version = "0.10.85"  # ← Change this ONLY
   ```

2. **All other versions auto-sync**:
   - Cache algorithm version: `1085` (automatic)
   - All workspace crates: `0.10.85` (via `version.workspace = true`)

3. **Update CHANGELOG.md** with changes

4. **Commit and push to BOTH branches**:
   ```bash
   git checkout nightly
   git add Cargo.toml CHANGELOG.md
   git commit -m "chore: bump version to 0.10.85"
   git push origin nightly
   
   git checkout main
   git cherry-pick <commit-hash>  # Or merge if appropriate
   git push origin main
   ```

### For Schema Changes (Database Structure)

1. **Update `CACHE_SCHEMA_VERSION`** in `shared_utils/src/version.rs`:
   ```rust
   const CACHE_SCHEMA_VERSION: i32 = 4;  // ← Increment this
   ```

2. **Add migration logic** in `check_and_migrate_schema()`:
   ```rust
   if v == 3 {
       info!("🔄 [Cache] Migrating schema from v3 to v4");
       // Add ALTER TABLE statements here
   }
   ```

3. **Follow regular update workflow** above

## Benefits of This System

1. **Single Source of Truth**: Only `Cargo.toml` needs manual updates
2. **No Version Drift**: Cache version always matches program version
3. **Automatic Invalidation**: Old cache entries auto-expire on updates
4. **Type Safety**: Compile-time version parsing with panic on failure
5. **Branch Isolation**: Main and nightly can have different dependencies

## Verification

### Check Current Versions
```bash
# Program version
grep '^version' Cargo.toml

# Cache algorithm version (at runtime)
cargo run --bin img-av1 -- --version

# All versions (programmatically)
cargo run --example show_versions  # If example exists

# Schema version (in database)
sqlite3 .cache/image_analysis_v2.db "SELECT value FROM cache_metadata WHERE key = 'schema_version'"
```

### Verify Auto-Binding
```bash
# Build and check logs
RUST_LOG=info cargo build --release 2>&1 | grep "version initialized"

# Or use the version module directly
cargo run --bin img-av1 -- --help | head -1
```

## Troubleshooting

### Cache Version Mismatch
**Symptom**: Old analysis results persist after update
**Solution**: Cache auto-invalidates on startup. If not, delete `.cache/` directory.

### Version Parsing Failure
**Symptom**: Panic at startup with "FATAL: Invalid CARGO_PKG_VERSION format"
**Solution**: Ensure `Cargo.toml` version follows `MAJOR.MINOR.PATCH` format exactly.

### Branch Dependency Conflicts
**Symptom**: Compilation fails after switching branches
**Solution**: 
```bash
cargo clean
cargo update
cargo build --release
```

## Future Enhancements

The following cache key components are **defined but not yet integrated**:
- `EncodingParams`: CRF, quality, preset, effort, feature flags
- `DependencyVersions`: FFmpeg, libjxl, libavif versions
- `EncoderBackend`: CPU, GPU, VideoToolbox, QuickSync, NVENC, AMF
- `HeuristicConfig`: JXL/HEVC thresholds, entropy, size tolerance

These will be integrated in future versions when needed for more precise cache invalidation.
