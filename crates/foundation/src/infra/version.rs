//! 🔢 Unified Version Management
//!
//! This module provides a single source of truth for all version numbers in the
//! project. All versions are automatically derived from `CARGO_PKG_VERSION` at
//! compile time.
//!
//! ## Version Binding Strategy
//!
//! 1. **Program Version**: From `Cargo.toml` → `[workspace.package]` →
//!    `version`
//! 2. **Cache Algorithm Version**: Auto-calculated from program version
//! 3. **Schema Versions**: Manually incremented only when structure changes
//!
//! ## Usage
//!
//! ```rust
//! use foundation::version::{CACHE_SCHEMA_VERSION, PROGRAM_VERSION, cache_algorithm};
//!
//! println!("Program: {}", PROGRAM_VERSION);
//! println!("Cache Algorithm: {}", cache_algorithm());
//! println!("Cache Schema: {}", CACHE_SCHEMA_VERSION);
//! ```
//!
//! ensures that the project remains complete for all builds.

use std::sync::LazyLock;
// no-op (removed tracing::info)

/// 📦 Program Version (from Cargo.toml)
///
/// This is the single source of truth for the program version.
/// Format: "MAJOR.MINOR.PATCH" (e.g., "0.11.1")
pub const PROGRAM_VERSION: &str = env!("CARGO_PKG_VERSION");

/// 🧬 Cache Algorithm Version - Automatically bound to program version
///
/// This value is automatically calculated from `CARGO_PKG_VERSION` at program
/// initialization. Version Format: Major.Minor.Patch → `MajorMinorPatch` (e.g.,
/// 0.11.1 → 1101)
///
/// **Purpose**: Automatic cache invalidation on ANY program update
///
/// **CRITICAL**: If version parsing fails, `cache_algorithm()` audits and
/// returns `0` so mis-keyed caches are visible in logs rather than silently
/// using a wrong code.
///
/// **Version History**:
/// - v1: Original HEIC lossless detection
/// - v2: Fixed HEIC lossless detection + improved box parsing
/// - v1060: Bound to program version 0.10.60 (automatic invalidation on
///   updates)
/// - v1061: Cache version binding mechanism
/// - v1062: Dependency unification (GitHub nightly sources)
/// - v1063: HEIC security limits increased (6GB, 10k ipco children)
/// - v1064: Git history cleanup (AI tool configs removed for privacy)
/// - v1065: HEIC security limits fix (apply before reading, 7GB memory)
/// - v1066: HEIC security limits increased to 15GB + feature flag fix
/// - v1067: Log output debug metadata removed + file creation time preservation
/// - v1068: Comprehensive metadata preservation (Windows/Linux/macOS)
/// - v1069: Metadata preservation enabled by default + creation time fix
/// - v1070: Creation time preservation fix + cache version auto-binding +
///   unified version management
/// - v1084: Perceived-speed scheduling, progress refresh, and louder runtime
///   failure reporting
/// - v1085: GUI/script launch hardening and narrow-terminal progress adaptation
/// - v1089: HDR10+ metadata retention and MS-SSIM chroma channel resolution
///   guard
/// - v1090: Intelligent checkpoint reset on output directory deletion
/// - v1091: Documentation binding (removed in v1097)
/// - v1102: Zero-warning state & EXR/JP2 detection (v0.10.102)
/// - v1108: Scanner fortification & Bit-depth hardening (v0.10.108)
/// - v1100: Unified Production Consolidation (v0.11.0)
/// - v1101: Sprint acceleration fix + cjxl signal-kill retry (v0.11.1)
/// - v1102: Granular Cache Cleanup & Selective Build (v0.11.2)
/// - v1103: Hardened Numeric Casts & Strict Clippy Compliance (v0.11.3)
static CACHE_ALGORITHM_VERSION: LazyLock<i32> =
    LazyLock::new(|| parse_version_to_code(PROGRAM_VERSION, "Cache Algorithm"));

/// 🔢 Cache Schema Version - Increment ONLY when database structure changes
///
/// **Current**: v4 (strict cache cutover after `ImageAnalysis` payload changes)
///
/// **Update Policy**: Increment manually ONLY when:
/// - Adding/removing database columns
/// - Changing column types
/// - Modifying table structure
/// - Altering indexes
///
/// **Migration**: Add migration logic in
/// `analysis_cache.rs::check_and_migrate_schema()`
///
/// **History**:
/// - v1: Initial schema
/// - v2: Added `algorithm_version` column + enhanced file signature tracking
/// - v3: Added `content_fingerprint_hash` (BLOB) and `data_checksum` (INTEGER)
///   for integrity verification
/// - v4: Forced destructive cache cutover for strict `ImageAnalysis` payload
///   changes
pub const CACHE_SCHEMA_VERSION: i32 = crate::constants::CACHE_SCHEMA_VERSION;

/// 📊 Get cache algorithm version
///
/// Returns the auto-calculated cache algorithm version based on program
/// version. This function is lazy-initialized; parse failures are audited once
/// at first use.
#[must_use]
pub fn cache_algorithm() -> i32 {
    *CACHE_ALGORITHM_VERSION
}

/// 🔧 Parse semantic version string to integer code
///
/// Converts "MAJOR.MINOR.PATCH" to `MajorMinorPatch` integer.
/// Example: "0.10.102" → 1102
fn try_parse_version_to_code(version: &str, context: &str) -> Result<i32, String> {
    let parts: Vec<&str> = version.split('.').collect();

    let [major_str, minor_str, patch_str] = parts[..] else {
        return Err(format!(
            "Invalid version format: '{version}'. Expected format: 'major.minor.patch'"
        ));
    };

    let major: u32 = major_str
        .parse()
        .map_err(|_| format!("Failed to parse major version: '{major_str}'"))?;

    let minor: u32 = minor_str
        .parse()
        .map_err(|_| format!("Failed to parse minor version: '{minor_str}'"))?;

    let patch: u32 = patch_str
        .parse()
        .map_err(|_| format!("Failed to parse patch version: '{patch_str}'"))?;

    let version_code = major * 10000 + minor * 100 + patch;

    tracing::debug!(
        "{} version initialized: {} (from program version: {})",
        context,
        version_code,
        version
    );

    Ok(crate::numeric_cast::u32_to_i32_sat(version_code))
}

fn parse_version_to_code(version: &str, context: &str) -> i32 {
    match try_parse_version_to_code(version, context) {
        Ok(code) => code,
        Err(detail) => {
            crate::media_conversion_gate::delivery_pipeline_batch_audit(
                "version_parse",
                format!("FATAL [{context}]: {detail} — using version code 0"),
            );
            0
        }
    }
}

/// 📋 Version Information - For display and debugging
#[derive(Debug, Clone)]
pub struct Info {
    /// Program version string (e.g.)
    pub program_version: String,

    /// Cache algorithm version code (e.g., 1102)
    pub cache_algorithm_version: i32,

    /// Cache schema version (e.g., 3)
    pub cache_schema_version: i32,
}

impl Info {
    /// Get current version information
    #[must_use]
    pub fn current() -> Self {
        Self {
            program_version: PROGRAM_VERSION.to_string(),
            cache_algorithm_version: cache_algorithm(),
            cache_schema_version: CACHE_SCHEMA_VERSION,
        }
    }

    /// Display version information
    #[must_use]
    pub fn display(&self) -> String {
        format!(
            "Program: {} | Cache Algorithm: {} | Cache Schema: v{}",
            self.program_version, self.cache_algorithm_version, self.cache_schema_version
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_version_parsing() {
        assert_eq!(parse_version_to_code("0.11.3", "Test"), 1_103_i32);
        assert_eq!(parse_version_to_code("0.11.2", "Test"), 1_102_i32);
        assert_eq!(parse_version_to_code("0.11.1", "Test"), 1_101_i32);
        assert_eq!(parse_version_to_code("0.11.0", "Test"), 1_100_i32);
        assert_eq!(parse_version_to_code("0.10.102", "Test"), 1_102_i32);
        assert_eq!(parse_version_to_code("1.2.3", "Test"), 10_203_i32);
        assert_eq!(parse_version_to_code("10.20.30", "Test"), 102_030_i32);
    }

    #[test]
    fn test_invalid_version_format() {
        assert_eq!(parse_version_to_code("1.2", "Test"), 0);
        assert!(try_parse_version_to_code("1.2", "Test").is_err());
    }

    #[test]
    fn test_invalid_major_version() {
        assert_eq!(parse_version_to_code("abc.2.3", "Test"), 0);
        assert!(try_parse_version_to_code("abc.2.3", "Test").is_err());
    }

    #[test]
    fn test_version_info() {
        let info = Info::current();
        assert!(!info.program_version.is_empty());
        assert!(info.cache_algorithm_version > 0_i32);
        assert_eq!(info.cache_schema_version, CACHE_SCHEMA_VERSION);
    }
}
