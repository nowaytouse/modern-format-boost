//! Fast image/video mode path policy and command builders.

use anyhow::Result;
use std::path::{Path, PathBuf};

pub const FAST_IMG_FORCE_SMART_BUILD: bool = false;

/// Return the persistent user state root.
///
/// # Errors
/// Returns an error if the MFB root cannot be resolved.
pub fn default_mfb_state_root() -> Result<PathBuf> {
    foundation::process_lock::get_mfb_root()
}

/// Resolve fastmode's adjacent JXL-only output directory.
pub fn fast_img_output_dir_for_target(
    target_dir: &Path,
    has_resume_marker: Option<&dyn Fn(&Path) -> bool>,
) -> PathBuf {
    let target = target_dir.to_path_buf();
    let file_name = target.file_name().and_then(|f| f.to_str()).unwrap_or("");
    let base = target.with_file_name(format!("{file_name}_optimized"));
    let mut candidate = base.clone();
    let mut suffix = 2;
    while candidate.exists()
        && !(candidate.join(".mfb_wc").exists())
        && !has_resume_marker.is_some_and(|f| f(&candidate))
    {
        candidate = base.with_file_name(format!("{file_name}_optimized_{suffix}"));
        suffix += 1;
    }
    candidate
}

fn unique_adjacent_dir(target_dir: &Path, suffix_name: &str) -> PathBuf {
    let target = target_dir.to_path_buf();
    let file_name = target.file_name().and_then(|f| f.to_str()).unwrap_or("");
    let base = target.with_file_name(format!("{file_name}_{suffix_name}"));
    let mut candidate = base.clone();
    let mut suffix = 2;
    while candidate.exists() {
        candidate = base.with_file_name(format!("{file_name}_{suffix_name}_{suffix}"));
        suffix += 1;
    }
    candidate
}

/// Resolve the adjacent JPEG restoration output directory.
#[must_use]
pub fn fast_img_restore_output_dir_for_target(target_dir: &Path) -> PathBuf {
    unique_adjacent_dir(target_dir, "restored_jpeg")
}

/// Resolve the adjacent full-pipeline output directory for vid `FastMode`.
#[must_use]
pub fn fast_vid_output_dir_for_target(target_dir: &Path) -> PathBuf {
    unique_adjacent_dir(target_dir, "optimized")
}

/// Build the Rust fast-img command for drag-and-drop launches.
#[must_use]
pub fn build_fast_img_command(
    img_binary: &Path,
    target_dir: &Path,
    shortest_path: bool,
    archive: bool,
    retry: bool,
    strategy: Option<&str>,
    extreme_precision: bool,
) -> Vec<String> {
    let mut command = vec![
        img_binary.to_string_lossy().to_string(),
        "fast-img".to_string(),
        target_dir.to_string_lossy().to_string(),
        "--recursive".to_string(),
    ];
    if retry {
        command.push("--retry".to_string());
    }
    if archive {
        command.push("--archive".to_string());
    }
    if shortest_path {
        command.push("--shortest-path".to_string());
        command.push("--auto-import".to_string());
    }
    if extreme_precision {
        command.push("--extreme-precision".to_string());
    }
    if let Some(strat) = strategy {
        command.push("--strategy".to_string());
        command.push(strat.to_string());
    }
    command
}

/// Build the Rust JXL-to-JPEG restore command for drag-and-drop launches.
#[must_use]
pub fn build_fast_img_restore_command(
    img_binary: &Path,
    target_dir: &Path,
    output_dir: &Path,
) -> Vec<String> {
    vec![
        img_binary.to_string_lossy().to_string(),
        "restore-jpeg".to_string(),
        target_dir.to_string_lossy().to_string(),
        "--output".to_string(),
        output_dir.to_string_lossy().to_string(),
        "--recursive".to_string(),
    ]
}

/// Build the Rust full loop-intent video `FastMode` command.
#[must_use]
pub fn build_fast_vid_command(
    vid_binary: &Path,
    target_dir: &Path,
    output_dir: &Path,
    _shortest_path: bool,
    strategy: Option<&str>,
) -> Vec<String> {
    let mut command = vec![
        vid_binary.to_string_lossy().to_string(),
        "run".to_string(),
        target_dir.to_string_lossy().to_string(),
        "--output".to_string(),
        output_dir.to_string_lossy().to_string(),
        "--base-dir".to_string(),
        target_dir.to_string_lossy().to_string(),
        "--recursive".to_string(),
        "--apple-compat".to_string(),
        "--ultimate".to_string(),
        "--archive".to_string(),
    ];
    if let Some(strat) = strategy {
        command.push("--strategy".to_string());
        command.push(strat.to_string());
    }
    command
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use std::fs;

    #[test]
    fn test_fastmode_output_dir_uses_adjacent_optimized_suffix() {
        let src = Path::new("/Users/example/Pictures/Album");
        assert_eq!(
            fast_img_output_dir_for_target(src, None),
            PathBuf::from("/Users/example/Pictures/Album_optimized")
        );
    }

    #[test]
    fn test_fastmode_output_collision_uses_numbered_optimized_suffix() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("Album");
        fs::create_dir(&src).unwrap();
        let opt = tmp.path().join("Album_optimized");
        fs::create_dir(&opt).unwrap();

        assert_eq!(
            fast_img_output_dir_for_target(&src, None),
            tmp.path().join("Album_optimized_2")
        );
    }

    #[test]
    fn test_fastmode_output_dir_allows_legacy_mfb_wc_resume_for_cleanup() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("Album");
        fs::create_dir(&src).unwrap();
        let optimized = tmp.path().join("Album_optimized");
        fs::create_dir(&optimized).unwrap();
        fs::write(optimized.join(".mfb_wc"), "legacy marker").unwrap();

        assert_eq!(fast_img_output_dir_for_target(&src, None), optimized);
    }

    #[test]
    #[serial]
    fn test_fastmode_state_root_defaults_to_user_home_for_app_launch() {
        let tmp = tempfile::tempdir().unwrap();
        unsafe {
            std::env::set_var("HOME", tmp.path());
            std::env::set_var("FROM_APP", "1");
            std::env::remove_var("MFB_HOME_ROOT");
        }
        let root = default_mfb_state_root().unwrap();
        assert_eq!(root, tmp.path().join(".modern_format_boost"));
        unsafe {
            std::env::remove_var("FROM_APP");
            std::env::remove_var("HOME");
        }
    }

    #[test]
    fn test_fastmode_uses_smart_build_without_force() {
        const {
            assert!(!FAST_IMG_FORCE_SMART_BUILD);
        };
    }

    #[test]
    fn test_fastmode_normal_command_uses_local_jxl_delivery() {
        let command = build_fast_img_command(
            Path::new("/opt/mfb/img"),
            Path::new("/Users/example/Pictures/Album"),
            false,
            false,
            false,
            None,
            false,
        );
        assert_eq!(
            command,
            vec![
                "/opt/mfb/img".to_string(),
                "fast-img".to_string(),
                "/Users/example/Pictures/Album".to_string(),
                "--recursive".to_string(),
            ]
        );
    }

    #[test]
    fn test_fastmode_archive_command_requests_archive_quality() {
        let command = build_fast_img_command(
            Path::new("/opt/mfb/img"),
            Path::new("/Users/example/Pictures/Album"),
            false,
            true,
            false,
            None,
            false,
        );
        assert_eq!(
            command,
            vec![
                "/opt/mfb/img".to_string(),
                "fast-img".to_string(),
                "/Users/example/Pictures/Album".to_string(),
                "--recursive".to_string(),
                "--archive".to_string(),
            ]
        );
    }

    #[test]
    fn test_fastmode_shortest_path_command_auto_imports_after_shared_delivery() {
        let command = build_fast_img_command(
            Path::new("/opt/mfb/img"),
            Path::new("/Users/example/Pictures/Album"),
            true,
            true,
            false,
            None,
            false,
        );
        assert_eq!(
            command,
            vec![
                "/opt/mfb/img".to_string(),
                "fast-img".to_string(),
                "/Users/example/Pictures/Album".to_string(),
                "--recursive".to_string(),
                "--archive".to_string(),
                "--shortest-path".to_string(),
                "--auto-import".to_string(),
            ]
        );
    }

    #[test]
    fn test_fastmode_retry_flag_is_explicit() {
        let command = build_fast_img_command(
            Path::new("/opt/mfb/img"),
            Path::new("/Users/example/Pictures/Album"),
            false,
            false,
            true,
            None,
            false,
        );
        assert!(command.contains(&"--retry".to_string()));
    }

    #[test]
    fn test_fastmode_extreme_precision_flag_is_passed() {
        let command = build_fast_img_command(
            Path::new("/opt/mfb/img"),
            Path::new("/Users/example/Pictures/Album"),
            false,
            false,
            false,
            Some("avif"),
            true,
        );
        assert!(command.contains(&"--extreme-precision".to_string()));
        assert!(command.contains(&"--strategy".to_string()));
        assert!(command.contains(&"avif".to_string()));
    }

    #[test]
    fn test_fastmode_restore_jpeg_dir_uses_adjacent_suffix() {
        let src = Path::new("/Users/example/Pictures/Album");
        assert_eq!(
            fast_img_restore_output_dir_for_target(src),
            PathBuf::from("/Users/example/Pictures/Album_restored_jpeg")
        );
    }

    #[test]
    fn test_fastmode_restore_jpeg_command_uses_rust_restore_subcommand() {
        let command = build_fast_img_restore_command(
            Path::new("/opt/mfb/img"),
            Path::new("/Users/example/Pictures/Album_optimized"),
            Path::new("/Users/example/Pictures/Album_restored_jpeg"),
        );
        assert_eq!(
            command,
            vec![
                "/opt/mfb/img".to_string(),
                "restore-jpeg".to_string(),
                "/Users/example/Pictures/Album_optimized".to_string(),
                "--output".to_string(),
                "/Users/example/Pictures/Album_restored_jpeg".to_string(),
                "--recursive".to_string(),
            ]
        );
    }

    #[test]
    fn test_fast_vid_dirs_use_adjacent_suffixes() {
        let src = Path::new("/Users/example/Movies/Clips");
        assert_eq!(
            fast_vid_output_dir_for_target(src),
            PathBuf::from("/Users/example/Movies/Clips_optimized")
        );
    }

    #[test]
    fn test_fast_vid_command_uses_full_loop_intent_run_pipeline() {
        let command = build_fast_vid_command(
            Path::new("/opt/mfb/vid"),
            Path::new("/Users/example/Movies/Clips"),
            Path::new("/Users/example/Movies/Clips_optimized"),
            false,
            None,
        );
        assert_eq!(
            command,
            vec![
                "/opt/mfb/vid".to_string(),
                "run".to_string(),
                "/Users/example/Movies/Clips".to_string(),
                "--output".to_string(),
                "/Users/example/Movies/Clips_optimized".to_string(),
                "--base-dir".to_string(),
                "/Users/example/Movies/Clips".to_string(),
                "--recursive".to_string(),
                "--apple-compat".to_string(),
                "--ultimate".to_string(),
                "--archive".to_string(),
            ]
        );
    }

    #[test]
    fn test_fast_vid_shortest_path_command_stays_on_full_run_pipeline() {
        let command = build_fast_vid_command(
            Path::new("/opt/mfb/vid"),
            Path::new("/Users/example/Movies/Clips"),
            Path::new("/Users/example/Movies/Clips_optimized"),
            true,
            None,
        );
        assert!(command.contains(&"run".to_string()));
        assert!(!command.contains(&"fast-gif".to_string()));
        assert!(!command.contains(&"--shortest-path".to_string()));
        assert!(!command.contains(&"--auto-import".to_string()));
    }
}
