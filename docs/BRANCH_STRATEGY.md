# Branch Strategy

## Overview

This project uses a dual-branch strategy to balance production stability with bleeding-edge development.

## Branches

### Main Branch (Production)
- **Purpose**: Stable, production-ready releases
- **Dependencies**: All from crates.io registry (stable versions)
- **Target Users**: Production deployments, stable releases
- **Update Frequency**: Regular releases after thorough testing

**Dependency Sources:**
- `image`: crates.io (0.25.x)
- `rusqlite`: crates.io (0.32.x)
- `blake3`: crates.io (1.5.x)
- `mp4parse`: crates.io (0.17.x)
- `rand`: crates.io (0.8.5)
- All other dependencies: crates.io

### Nightly Branch (Development)
- **Purpose**: Bleeding-edge development and testing
- **Dependencies**: GitHub sources for fastest upstream iterations
- **Target Users**: Developers, testers, early adopters
- **Update Frequency**: Continuous integration with upstream

**Dependency Sources:**
- `image`: GitHub (rust-lang/image)
- `rusqlite`: GitHub (rusqlite/rusqlite)
- `blake3`: GitHub (BLAKE3-team/BLAKE3)
- `mp4parse`: GitHub (mozilla/mp4parse-rust)
- `getrandom`: GitHub (rust-random/getrandom)
- `hashbrown`: GitHub (rust-lang/hashbrown)
- `rand`: GitHub (rust-random/rand) - 0.10.0
- `rand_core`: GitHub (rust-random/rand_core)

## When to Use Which Branch

### Use Main Branch If:
- ✅ You need production-ready, stable code
- ✅ You want predictable dependency versions
- ✅ You're deploying to production environments
- ✅ You prefer crates.io ecosystem compatibility

### Use Nightly Branch If:
- ✅ You want the latest features and bug fixes
- ✅ You're contributing to development
- ✅ You want to test upcoming changes
- ✅ You need bleeding-edge dependency versions

## Version Synchronization

Both branches maintain the same version number (e.g., 0.10.85) but differ in:
- Dependency sources (crates.io vs GitHub)
- API compatibility (stable vs nightly)
- Feature availability (stable vs experimental)

## Merging Strategy

- **Main ← Nightly**: Only stable, tested features
- **Nightly ← Main**: Bug fixes and critical patches
- **Dependency Updates**: 
  - Main: Manual updates to stable versions
  - Nightly: Automatic tracking of GitHub HEAD

## Building

### Main Branch
```bash
git checkout main
cargo build --release
```

### Nightly Branch
```bash
git checkout nightly
cargo build --release
```

## Testing

Both branches pass the same test suite, ensuring feature parity while using different dependency sources.

## Contributing

- **Bug Fixes**: Submit to main branch
- **New Features**: Develop in nightly branch
- **Dependency Updates**: 
  - Main: Update to latest stable crates.io versions
  - Nightly: Already tracks GitHub HEAD automatically

## Cache Version Binding

Both branches use the same cache versioning strategy:
- Cache version = Program version (e.g., 0.10.85 → 1085)
- Automatic invalidation on version updates
- Ensures consistency between code and cached data
