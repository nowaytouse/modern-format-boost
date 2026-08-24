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

fn adjacent_dir(target_dir: &Path, suffix_name: &str) -> PathBuf {
    let mut file_name = target_dir.file_name().unwrap_or_default().to_os_string();
    file_name.push(format!("_{suffix_name}"));
    target_dir.with_file_name(file_name)
}

fn unique_adjacent_dir(target_dir: &Path, suffix_name: &str) -> PathBuf {
    let base = adjacent_dir(target_dir, suffix_name);
    let mut candidate = base.clone();
    let mut suffix = 2;
    while candidate.exists() {
        let mut file_name = base.file_name().unwrap_or_default().to_os_string();
        file_name.push(format!("_{suffix}"));
        candidate = base.with_file_name(file_name);
        suffix += 1;
    }
    candidate
}

/// Resolve the adjacent JPEG restoration output directory.
#[must_use]
pub fn fast_img_restore_output_dir_for_target(target_dir: &Path) -> PathBuf {
    adjacent_dir(target_dir, "restored_jpeg")
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
    output_dir: &Path,
    shortest_path: bool,
    archive: bool,
    retry: bool,
    fresh: bool,
    strategy: Option<&str>,
    extreme_precision: bool,
) -> Vec<String> {
    let mut command = vec![
        img_binary.to_string_lossy().to_string(),
        "fast-img".to_string(),
        target_dir.to_string_lossy().to_string(),
        "--output".to_string(),
        output_dir.to_string_lossy().to_string(),
        "--recursive".to_string(),
    ];
    if retry {
        command.push("--retry".to_string());
    } else if fresh {
        command.push("--no-resume".to_string());
    }
    if archive {
        command.push("--archive".to_string());
    }
    if shortest_path {
        command.push("--shortest-path".to_string());
    }
    if extreme_precision && strategy == Some("avif") {
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
    output_dir: Option<&Path>,
    photos_album_id: Option<&str>,
    photos_folder_id: Option<&str>,
) -> Vec<String> {
    debug_assert!(photos_album_id.is_none() || photos_folder_id.is_none());
    let mut command = vec![
        img_binary.to_string_lossy().to_string(),
        "restore-jpeg".to_string(),
        target_dir.to_string_lossy().to_string(),
        "--recursive".to_string(),
    ];
    if let Some(output_dir) = output_dir {
        command.push("--output".to_string());
        command.push(output_dir.to_string_lossy().to_string());
    }
    if let Some(id) = photos_album_id {
        command.extend(["--photos-album-id".to_string(), id.to_string()]);
    }
    if let Some(id) = photos_folder_id {
        command.extend(["--photos-folder-id".to_string(), id.to_string()]);
    }
    command
}

/// Build the Rust animated-image/video `FastMode` command.
#[must_use]
pub fn build_fast_vid_command(
    vid_binary: &Path,
    target_dir: &Path,
    output_dir: &Path,
    shortest_path: bool,
    strategy: Option<&str>,
) -> Vec<String> {
    let mut command = vec![
        vid_binary.to_string_lossy().to_string(),
        "fast-gif".to_string(),
        target_dir.to_string_lossy().to_string(),
        "--output".to_string(),
        output_dir.to_string_lossy().to_string(),
        "--recursive".to_string(),
        "--apple-compat".to_string(),
    ];
    if shortest_path {
        command.push("--shortest-path".to_string());
    }
    command.push("--strategy".to_string());
    command.push(
        if strategy == Some("avif") {
            "avif"
        } else {
            "default"
        }
        .to_string(),
    );
    command
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

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
            Path::new("/Users/example/Pictures/Album_optimized"),
            false,
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
                "--output".to_string(),
                "/Users/example/Pictures/Album_optimized".to_string(),
                "--recursive".to_string(),
            ]
        );
    }

    #[test]
    fn test_fastmode_archive_command_requests_archive_quality() {
        let command = build_fast_img_command(
            Path::new("/opt/mfb/img"),
            Path::new("/Users/example/Pictures/Album"),
            Path::new("/Users/example/Pictures/Album_optimized"),
            false,
            true,
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
                "--output".to_string(),
                "/Users/example/Pictures/Album_optimized".to_string(),
                "--recursive".to_string(),
                "--archive".to_string(),
            ]
        );
    }

    #[test]
    fn test_fastmode_avif_shortest_path_command_uses_single_delivery_flag() {
        let command = build_fast_img_command(
            Path::new("/opt/mfb/img"),
            Path::new("/Users/example/Pictures/Album"),
            Path::new("/Users/example/Pictures/Album_optimized"),
            true,
            true,
            false,
            false,
            Some("avif"),
            false,
        );
        assert_eq!(
            command,
            vec![
                "/opt/mfb/img".to_string(),
                "fast-img".to_string(),
                "/Users/example/Pictures/Album".to_string(),
                "--output".to_string(),
                "/Users/example/Pictures/Album_optimized".to_string(),
                "--recursive".to_string(),
                "--archive".to_string(),
                "--shortest-path".to_string(),
                "--strategy".to_string(),
                "avif".to_string(),
            ]
        );
    }

    #[test]
    fn test_fastmode_jxl_shortest_path_reaches_verified_delivery() {
        let command = build_fast_img_command(
            Path::new("/opt/mfb/img"),
            Path::new("/Users/example/Pictures/Album"),
            Path::new("/Users/example/Pictures/Album_optimized"),
            true,
            true,
            false,
            false,
            Some("jxl"),
            true,
        );
        assert_eq!(
            command,
            vec![
                "/opt/mfb/img".to_string(),
                "fast-img".to_string(),
                "/Users/example/Pictures/Album".to_string(),
                "--output".to_string(),
                "/Users/example/Pictures/Album_optimized".to_string(),
                "--recursive".to_string(),
                "--archive".to_string(),
                "--shortest-path".to_string(),
                "--strategy".to_string(),
                "jxl".to_string(),
            ]
        );
    }

    #[test]
    fn test_fastmode_retry_flag_is_explicit() {
        let command = build_fast_img_command(
            Path::new("/opt/mfb/img"),
            Path::new("/Users/example/Pictures/Album"),
            Path::new("/Users/example/Pictures/Album_optimized"),
            false,
            false,
            true,
            false,
            None,
            false,
        );
        assert!(command.contains(&"--retry".to_string()));
    }

    #[test]
    fn test_fastmode_fresh_flag_does_not_consume_resume_state() {
        let command = build_fast_img_command(
            Path::new("/opt/mfb/img"),
            Path::new("/Users/example/Pictures/Album"),
            Path::new("/Users/example/Pictures/Album_optimized"),
            false,
            false,
            false,
            true,
            None,
            false,
        );
        assert!(command.contains(&"--no-resume".to_string()));
        assert!(!command.contains(&"--retry".to_string()));
    }

    #[test]
    fn test_fastmode_extreme_precision_flag_is_passed() {
        let command = build_fast_img_command(
            Path::new("/opt/mfb/img"),
            Path::new("/Users/example/Pictures/Album"),
            Path::new("/Users/example/Pictures/Album_optimized"),
            false,
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
    fn test_fastmode_restore_jpeg_dir_is_stable_when_output_exists() {
        let temp = tempfile::tempdir().unwrap();
        let src = temp.path().join("Album");
        let expected = temp.path().join("Album_restored_jpeg");
        std::fs::create_dir_all(&expected).unwrap();

        assert_eq!(fast_img_restore_output_dir_for_target(&src), expected);
    }

    #[test]
    fn test_fastmode_restore_jpeg_command_uses_rust_restore_subcommand() {
        let command = build_fast_img_restore_command(
            Path::new("/opt/mfb/img"),
            Path::new("/Users/example/Pictures/Album_optimized"),
            Some(Path::new("/Users/example/Pictures/Album_restored_jpeg")),
            None,
            None,
        );
        assert_eq!(
            command,
            vec![
                "/opt/mfb/img".to_string(),
                "restore-jpeg".to_string(),
                "/Users/example/Pictures/Album_optimized".to_string(),
                "--recursive".to_string(),
                "--output".to_string(),
                "/Users/example/Pictures/Album_restored_jpeg".to_string(),
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
    fn test_fast_vid_command_uses_fast_gif_pipeline_and_maps_jxl_strategy() {
        let command = build_fast_vid_command(
            Path::new("/opt/mfb/vid"),
            Path::new("/Users/example/Movies/Clips"),
            Path::new("/Users/example/Movies/Clips_optimized"),
            false,
            Some("jxl"),
        );
        assert_eq!(
            command,
            vec![
                "/opt/mfb/vid".to_string(),
                "fast-gif".to_string(),
                "/Users/example/Movies/Clips".to_string(),
                "--output".to_string(),
                "/Users/example/Movies/Clips_optimized".to_string(),
                "--recursive".to_string(),
                "--apple-compat".to_string(),
                "--strategy".to_string(),
                "default".to_string(),
            ]
        );
    }

    #[test]
    fn test_fast_vid_shortest_path_enables_verified_import_and_avif_strategy() {
        let command = build_fast_vid_command(
            Path::new("/opt/mfb/vid"),
            Path::new("/Users/example/Movies/Clips"),
            Path::new("/Users/example/Movies/Clips_optimized"),
            true,
            Some("avif"),
        );
        assert!(command.contains(&"fast-gif".to_string()));
        assert!(command.contains(&"--shortest-path".to_string()));
        assert!(!command.contains(&"--auto-import".to_string()));
        assert_eq!(
            command
                .windows(2)
                .find(|pair| pair[0] == "--strategy")
                .map(|pair| pair[1].as_str()),
            Some("avif")
        );
    }
}
