//! Smart Thread Manager for Apple Silicon optimization
//!
//! Provides intelligent thread allocation that:
//! - Maximizes performance on Apple Silicon chips
//! - Prevents system overload during multi-instance scenarios
//! - Allows environment-based configuration

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;

/// Global flag indicating if multiple instances are running
static MULTI_INSTANCE_MODE: AtomicBool = AtomicBool::new(false);

/// Cached optimal thread count for this system
static OPTIMAL_THREADS: OnceLock<usize> = OnceLock::new();

/// Configuration for thread allocation
#[derive(Debug, Clone)]
pub struct ThreadConfig {
    /// Percentage of cores to use (0-100)
    pub core_percentage: usize,
    /// Minimum threads to allocate
    pub min_threads: usize,
    /// Maximum threads to allocate
    pub max_threads: usize,
    /// Whether to account for multi-instance scenarios
    pub multi_instance_aware: bool,
}

impl Default for ThreadConfig {
    fn default() -> Self {
        Self {
            core_percentage: 70, // 70% of cores by default
            min_threads: 2,
            max_threads: 16,
            multi_instance_aware: true,
        }
    }
}

impl ThreadConfig {
    /// Conservative config for background processing
    pub fn conservative() -> Self {
        Self {
            core_percentage: 50,
            min_threads: 1,
            max_threads: 8,
            multi_instance_aware: true,
        }
    }

    /// Aggressive config for maximum performance (single instance)
    pub fn aggressive() -> Self {
        Self {
            core_percentage: 90,
            min_threads: 4,
            max_threads: 32,
            multi_instance_aware: false,
        }
    }

    /// Config optimized for ffmpeg/video processing
    pub fn video_processing() -> Self {
        Self {
            core_percentage: 60,
            min_threads: 2,
            max_threads: 12,
            multi_instance_aware: true,
        }
    }
}

/// Calculate optimal thread count based on system capabilities
/// 
/// # Arguments
/// * `config` - Thread configuration settings
/// 
/// # Returns
/// Optimal number of threads to use
pub fn calculate_optimal_threads(config: &ThreadConfig) -> usize {
    let cpu_count = num_cpus::get();
    
    // Check for multi-instance mode
    let effective_percentage = if config.multi_instance_aware && is_multi_instance() {
        config.core_percentage / 2 // Halve resources in multi-instance mode
    } else {
        config.core_percentage
    };
    
    // Calculate based on percentage
    let calculated = (cpu_count * effective_percentage / 100).max(1);
    
    // Apply min/max bounds
    calculated.clamp(config.min_threads, config.max_threads)
}

/// Get optimal threads for general processing (cached)
pub fn get_optimal_threads() -> usize {
    *OPTIMAL_THREADS.get_or_init(|| {
        calculate_optimal_threads(&ThreadConfig::default())
    })
}

/// Get threads optimized for ffmpeg operations
pub fn get_ffmpeg_threads() -> usize {
    calculate_optimal_threads(&ThreadConfig::video_processing())
}

/// Check if running in multi-instance mode
pub fn is_multi_instance() -> bool {
    // Check environment variable
    if std::env::var("MFB_MULTI_INSTANCE").is_ok() {
        return true;
    }
    
    // Check atomic flag
    MULTI_INSTANCE_MODE.load(Ordering::Relaxed)
}

/// Enable multi-instance mode (reduces thread allocation)
pub fn enable_multi_instance_mode() {
    MULTI_INSTANCE_MODE.store(true, Ordering::Relaxed);
}

/// Disable multi-instance mode
pub fn disable_multi_instance_mode() {
    MULTI_INSTANCE_MODE.store(false, Ordering::Relaxed);
}

/// Get the path to brew rsync if available
/// 
/// Returns the path to the Homebrew-installed rsync (v3.4+) if available,
/// otherwise returns the system rsync path
pub fn get_rsync_path() -> &'static str {
    static RSYNC_PATH: OnceLock<String> = OnceLock::new();
    
    RSYNC_PATH.get_or_init(|| {
        // Check for Homebrew rsync on Apple Silicon
        let brew_rsync = "/opt/homebrew/opt/rsync/bin/rsync";
        if std::path::Path::new(brew_rsync).exists() {
            return brew_rsync.to_string();
        }
        
        // Check for Homebrew rsync on Intel Mac
        let intel_brew_rsync = "/usr/local/opt/rsync/bin/rsync";
        if std::path::Path::new(intel_brew_rsync).exists() {
            return intel_brew_rsync.to_string();
        }
        
        // Fall back to system rsync
        "rsync".to_string()
    })
}

/// Get rsync version info
pub fn get_rsync_version() -> Option<String> {
    use std::process::Command;
    
    let output = Command::new(get_rsync_path())
        .arg("--version")
        .output()
        .ok()?;
    
    if output.status.success() {
        let version_line = String::from_utf8_lossy(&output.stdout)
            .lines()
            .next()?
            .to_string();
        Some(version_line)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_thread_calculation() {
        let threads = get_optimal_threads();
        assert!(threads >= 2);
        assert!(threads <= 16);
    }

    #[test]
    fn test_ffmpeg_threads() {
        let threads = get_ffmpeg_threads();
        assert!(threads >= 2);
        assert!(threads <= 12);
    }

    #[test]
    fn test_thread_config() {
        let config = ThreadConfig::conservative();
        let threads = calculate_optimal_threads(&config);
        assert!(threads >= config.min_threads);
        assert!(threads <= config.max_threads);
    }

    #[test]
    fn test_rsync_path() {
        let path = get_rsync_path();
        assert!(!path.is_empty());
    }
}
