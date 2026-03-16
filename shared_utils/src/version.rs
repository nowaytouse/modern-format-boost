//! 🔢 Unified Version Management
//!
//! This module provides a single source of truth for all version numbers in the project.
//! All versions are automatically derived from CARGO_PKG_VERSION at compile time.
//!
//! ## Version Binding Strategy
//!
//! 1. **Program Version**: From `Cargo.toml` → `[workspace.package]` → `version`
//! 2. **Cache Algorithm Version**: Auto-calculated from program version
//! 3. **Schema Versions**: Manually incremented only when structure changes
//!
//! ## Usage
//!
//! ```rust
//! use shared_utils::version::{PROGRAM_VERSION, cache_algorithm_version, CACHE_SCHEMA_VERSION};
//!
//! println!("Program: {}", PROGRAM_VERSION);
//! println!("Cache Algorithm: {}", cache_algorithm_version());
//! println!("Cache Schema: {}", CACHE_SCHEMA_VERSION);
//! ```

use std::sync::LazyLock;
use tracing::info;

/// 📦 Program Version (from Cargo.toml)
///
/// This is the single source of truth for the program version.
/// Format: "MAJOR.MINOR.PATCH" (e.g., "0.10.70")
pub const PROGRAM_VERSION: &str = env!("CARGO_PKG_VERSION");

/// 🧬 Cache Algorithm Version - Automatically bound to program version
///
/// This value is automatically calculated from CARGO_PKG_VERSION at program initialization.
/// Version Format: Major.Minor.Patch → MajorMinorPatch (e.g., 0.10.70 → 10070)
///
/// **Purpose**: Automatic cache invalidation on ANY program update
///
/// **CRITICAL**: If version parsing fails, the program will panic at startup.
/// This is intentional - we must never silently use a wrong version number.
///
/// **Version History**:
/// - v1: Original HEIC lossless detection
/// - v2: Fixed HEIC lossless detection + improved box parsing
/// - v10060: Bound to program version 0.10.60 (automatic invalidation on updates)
/// - v10061: Cache version binding mechanism
/// - v10062: Dependency unification (GitHub nightly sources)
/// - v10063: HEIC security limits increased (6GB, 10k ipco children)
/// - v10064: Git history cleanup (AI tool configs removed for privacy)
/// - v10065: HEIC security limits fix (apply before reading, 7GB memory)
/// - v10066: HEIC security limits increased to 15GB + feature flag fix
/// - v10067: Log output debug metadata removed + file creation time preservation
/// - v10068: Comprehensive metadata preservation (Windows/Linux/macOS)
/// - v10069: Metadata preservation enabled by default + creation time fix
/// - v10070: Creation time preservation fix + cache version auto-binding + unified version management
static CACHE_ALGORITHM_VERSION: LazyLock<i32> = LazyLock::new(|| {
    parse_version_to_code(PROGRAM_VERSION, "Cache Algorithm")
});

/// 🔢 Cache Schema Version - Increment ONLY when database structure changes
///
/// **Current**: v3 (added content_fingerprint_hash and data_checksum columns)
///
/// **Update Policy**: Increment manually ONLY when:
/// - Adding/removing database columns
/// - Changing column types
/// - Modifying table structure
/// - Altering indexes
///
/// **Migration**: Add migration logic in `analysis_cache.rs::check_and_migrate_schema()`
///
/// **History**:
/// - v1: Initial schema
/// - v2: Added algorithm_version column + enhanced file signature tracking
/// - v3: Added content_fingerprint_hash (BLOB) and data_checksum (INTEGER) for integrity verification
pub const CACHE_SCHEMA_VERSION: i32 = 3;

/// 📊 Get cache algorithm version
///
/// Returns the auto-calculated cache algorithm version based on program version.
/// This function is lazy-initialized and will panic if version parsing fails.
pub fn cache_algorithm_version() -> i32 {
    *CACHE_ALGORITHM_VERSION
}

/// 🔧 Parse semantic version string to integer code
///
/// Converts "MAJOR.MINOR.PATCH" to MajorMinorPatch integer.
/// Example: "0.10.70" → 10070
///
/// **Panics** if:
/// - Version format is not "MAJOR.MINOR.PATCH"
/// - Any component is not a valid integer
///
/// This is intentional - we must never silently use a wrong version number.
fn parse_version_to_code(version: &str, context: &str) -> i32 {
    let parts: Vec<&str> = version.split('.').collect();
    
    if parts.len() != 3 {
        panic!(
            "FATAL [{}]: Invalid version format: '{}'. Expected format: 'major.minor.patch'",
            context, version
        );
    }
    
    let major = parts[0].parse::<i32>().unwrap_or_else(|e| {
        panic!("FATAL [{}]: Failed to parse major version from '{}': {}", context, parts[0], e);
    });
    
    let minor = parts[1].parse::<i32>().unwrap_or_else(|e| {
        panic!("FATAL [{}]: Failed to parse minor version from '{}': {}", context, parts[1], e);
    });
    
    let patch = parts[2].parse::<i32>().unwrap_or_else(|e| {
        panic!("FATAL [{}]: Failed to parse patch version from '{}': {}", context, parts[2], e);
    });
    
    let version_code = major * 10000 + minor * 100 + patch;
    
    info!(
        "{} version initialized: {} (from program version: {})",
        context, version_code, version
    );
    
    version_code
}

/// 📋 Version Information - For display and debugging
#[derive(Debug, Clone)]
pub struct VersionInfo {
    /// Program version string (e.g., "0.10.70")
    pub program_version: String,
    
    /// Cache algorithm version code (e.g., 10070)
    pub cache_algorithm_version: i32,
    
    /// Cache schema version (e.g., 3)
    pub cache_schema_version: i32,
}

impl VersionInfo {
    /// Get current version information
    pub fn current() -> Self {
        Self {
            program_version: PROGRAM_VERSION.to_string(),
            cache_algorithm_version: cache_algorithm_version(),
            cache_schema_version: CACHE_SCHEMA_VERSION,
        }
    }
    
    /// Display version information
    pub fn display(&self) -> String {
        format!(
            "Program: {} | Cache Algorithm: {} | Cache Schema: v{}",
            self.program_version,
            self.cache_algorithm_version,
            self.cache_schema_version
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_version_parsing() {
        assert_eq!(parse_version_to_code("0.10.70", "Test"), 10070);
        assert_eq!(parse_version_to_code("1.2.3", "Test"), 10203);
        assert_eq!(parse_version_to_code("10.20.30", "Test"), 102030);
    }

    #[test]
    #[should_panic(expected = "Invalid version format")]
    fn test_invalid_version_format() {
        parse_version_to_code("1.2", "Test");
    }

    #[test]
    #[should_panic(expected = "Failed to parse major version")]
    fn test_invalid_major_version() {
        parse_version_to_code("abc.2.3", "Test");
    }

    #[test]
    fn test_version_info() {
        let info = VersionInfo::current();
        assert!(!info.program_version.is_empty());
        assert!(info.cache_algorithm_version > 0);
        assert_eq!(info.cache_schema_version, CACHE_SCHEMA_VERSION);
    }
}
