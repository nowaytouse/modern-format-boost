//! Generate synthetic media fixtures for edge tests.

use anyhow::{Context, Result};
use dev::infra::ui_tokens::pick_symbol;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

struct MediaJob {
    args: &'static [&'static str],
    output: &'static str,
}

const IMAGE_JOBS: &[MediaJob] = &[
    MediaJob {
        args: &[
            "-f",
            "lavfi",
            "-i",
            "color=c=blue:s=1920x1080:d=1",
            "-f",
            "lavfi",
            "-i",
            "sine=f=1000:d=1",
            "-c:v",
            "png",
            "-c:a",
            "pcm_s16le",
        ],
        output: "test_image_1080p.png",
    },
    MediaJob {
        args: &[
            "-f",
            "lavfi",
            "-i",
            "color=red:s=800x600:d=1",
            "-c:v",
            "png",
        ],
        output: "test_gradient_red.png",
    },
    MediaJob {
        args: &[
            "-f",
            "lavfi",
            "-i",
            "color=green:s=3840x2160:d=1",
            "-c:v",
            "png",
        ],
        output: "test_hd_4k.png",
    },
    MediaJob {
        args: &[
            "-f",
            "lavfi",
            "-i",
            "color=yellow:s=640x480:d=1",
            "-c:v",
            "png",
        ],
        output: "test_low_quality.png",
    },
];

const VIDEO_JOBS: &[MediaJob] = &[
    MediaJob {
        args: &[
            "-f",
            "lavfi",
            "-i",
            "color=c=blue:s=1280x720:d=10",
            "-f",
            "lavfi",
            "-i",
            "sine=f=440:d=10",
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
            "-crf",
            "23",
            "-c:a",
            "aac",
            "-b:a",
            "128k",
        ],
        output: "test_h264_10s.mp4",
    },
    MediaJob {
        args: &[
            "-f",
            "lavfi",
            "-i",
            "color=c=green:s=1920x1080:d=5",
            "-f",
            "lavfi",
            "-i",
            "sine=f=880:d=5",
            "-c:v",
            "libvpx-vp9",
            "-preset",
            "fast",
            "-crf",
            "28",
            "-c:a",
            "libopus",
            "-b:a",
            "128k",
        ],
        output: "test_vp9_5s.webm",
    },
    MediaJob {
        args: &[
            "-f",
            "lavfi",
            "-i",
            "color=c=red:s=1920x1080:d=8",
            "-f",
            "lavfi",
            "-i",
            "sine=f=660:d=8",
            "-c:v",
            "libx265",
            "-preset",
            "fast",
            "-crf",
            "28",
            "-c:a",
            "aac",
            "-b:a",
            "128k",
        ],
        output: "test_hevc_8s.mp4",
    },
    MediaJob {
        args: &[
            "-f",
            "lavfi",
            "-i",
            "color=c=yellow:s=1920x1080:d=6",
            "-f",
            "lavfi",
            "-i",
            "sine=f=1000:d=6",
            "-c:v",
            "libaom-av1",
            "-preset",
            "4",
            "-crf",
            "30",
            "-c:a",
            "libopus",
            "-b:a",
            "128k",
        ],
        output: "test_av1_6s.mkv",
    },
    MediaJob {
        args: &[
            "-f",
            "lavfi",
            "-i",
            "color=c=cyan:s=1920x1080:d=15",
            "-f",
            "lavfi",
            "-i",
            "sine=f=1200:d=15",
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
            "-crf",
            "18",
            "-c:a",
            "aac",
            "-b:a",
            "192k",
        ],
        output: "test_hq_source_15s.mp4",
    },
    MediaJob {
        args: &[
            "-f",
            "lavfi",
            "-i",
            "color=c=magenta:s=640x480:d=12",
            "-f",
            "lavfi",
            "-i",
            "sine=f=500:d=12",
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
            "-crf",
            "35",
            "-c:a",
            "aac",
            "-b:a",
            "64k",
        ],
        output: "test_lq_source_12s.mp4",
    },
    MediaJob {
        args: &[
            "-f",
            "lavfi",
            "-i",
            "color=c=white:s=1280x720:d=2",
            "-f",
            "lavfi",
            "-i",
            "sine=f=800:d=2",
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
            "-crf",
            "23",
            "-c:a",
            "aac",
            "-b:a",
            "128k",
        ],
        output: "test_short_2s.mp4",
    },
];

const GIF_JOBS: &[MediaJob] = &[
    MediaJob {
        args: &[
            "-f",
            "lavfi",
            "-i",
            "color=c=red:s=640x480:d=2",
            "-f",
            "lavfi",
            "-i",
            "sine=f=440:d=2",
            "-vf",
            "fps=10,scale=640:480:flags=lanczos",
        ],
        output: "test_simple.gif",
    },
    MediaJob {
        args: &[
            "-f",
            "lavfi",
            "-i",
            "testsrc=s=320x240:d=3",
            "-vf",
            "fps=10",
        ],
        output: "test_pattern.gif",
    },
];

fn project_root() -> Result<PathBuf> {
    let cwd = std::env::current_dir().context("read current directory")?;
    let mut dir = cwd.as_path();
    loop {
        if dir.join("Cargo.toml").is_file() && dir.join("crates").is_dir() {
            return Ok(dir.to_path_buf());
        }
        let Some(parent) = dir.parent() else {
            anyhow::bail!("cannot locate workspace root from {}", cwd.display());
        };
        dir = parent;
    }
}

fn run_ffmpeg(args: &[&str], output: &Path) -> bool {
    let status = Command::new("ffmpeg")
        .args(args)
        .args(["-y"])
        .arg(output)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
    matches!(status, Ok(status) if status.success())
}

fn summary_lines(
    test_dir: &Path,
    manifest_path: &Path,
    image_count: usize,
    video_count: usize,
    gif_count: usize,
    manifest_exists: bool,
) -> Vec<String> {
    let sep = "=".repeat(64);
    let mut lines = vec![
        String::new(),
        sep.clone(),
        format!(
            "{} Test media generation complete!",
            pick_symbol("✅", "[OK]")
        ),
        sep.clone(),
        String::new(),
        "Generated media files:".to_string(),
        format!(
            "{} Images:  {image_count} files",
            pick_symbol("📁", "[DIR]")
        ),
        format!(
            "{} Videos:  {video_count} files",
            pick_symbol("📁", "[DIR]")
        ),
        format!("{} GIFs:    {gif_count} files", pick_symbol("📁", "[DIR]")),
        String::new(),
    ];
    if manifest_exists {
        lines.push(format!(
            "For test specifications, see: {}",
            manifest_path.display()
        ));
    }
    lines.push(sep);
    debug_assert!(manifest_path.starts_with(test_dir));
    lines
}

fn main() -> Result<()> {
    let root = project_root()?;
    let test_dir = root.join("crates/dev/src/tests/edge");
    let images_dir = test_dir.join("images");
    let videos_dir = test_dir.join("videos");
    let gifs_dir = test_dir.join("gifs");
    fs::create_dir_all(&images_dir)?;
    fs::create_dir_all(&videos_dir)?;
    fs::create_dir_all(&gifs_dir)?;

    println!(
        "{} Generating test media in {}",
        pick_symbol("🎬", "[VID]"),
        test_dir.display()
    );
    for job in IMAGE_JOBS {
        let _ = run_ffmpeg(job.args, &images_dir.join(job.output));
    }

    for job in VIDEO_JOBS {
        let _ = run_ffmpeg(job.args, &videos_dir.join(job.output));
    }

    for job in GIF_JOBS {
        let _ = run_ffmpeg(job.args, &gifs_dir.join(job.output));
    }

    let manifest_path = test_dir.join("MEDIA_MANIFEST.md");
    for line in summary_lines(
        &test_dir,
        &manifest_path,
        fs::read_dir(&images_dir)?.count(),
        fs::read_dir(&videos_dir)?.count(),
        fs::read_dir(&gifs_dir)?.count(),
        manifest_path.is_file(),
    ) {
        println!("{line}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rust_fixture_manifest_matches_python_original_scope() {
        assert_eq!(IMAGE_JOBS.len(), 4);
        assert_eq!(VIDEO_JOBS.len(), 7);
        assert_eq!(GIF_JOBS.len(), 2);
        assert!(
            VIDEO_JOBS
                .iter()
                .any(|job| job.output == "test_hevc_8s.mp4")
        );
        assert!(VIDEO_JOBS.iter().any(|job| job.output == "test_av1_6s.mkv"));
        assert!(
            VIDEO_JOBS
                .iter()
                .any(|job| job.output == "test_hq_source_15s.mp4")
        );
        assert!(
            VIDEO_JOBS
                .iter()
                .any(|job| job.output == "test_lq_source_12s.mp4")
        );
        assert!(
            IMAGE_JOBS[0]
                .args
                .windows(2)
                .any(|args| args == ["-c:a", "pcm_s16le"])
        );
        assert!(
            GIF_JOBS[0]
                .args
                .windows(2)
                .any(|args| args == ["-i", "sine=f=440:d=2"])
        );
    }

    #[test]
    fn rust_summary_matches_python_output_shape() {
        let lines = summary_lines(
            Path::new("/tmp/mfb-edge"),
            Path::new("/tmp/mfb-edge/MEDIA_MANIFEST.md"),
            4,
            7,
            2,
            true,
        );

        assert!(
            lines
                .iter()
                .any(|line| line.contains("Generated media files:"))
        );
        assert!(lines.iter().any(|line| line.contains("Images:  4 files")));
        assert!(lines.iter().any(|line| line.contains("Videos:  7 files")));
        assert!(lines.iter().any(|line| line.contains("GIFs:    2 files")));
        assert!(lines
            .iter()
            .any(|line| line == "For test specifications, see: /tmp/mfb-edge/MEDIA_MANIFEST.md"));
    }
}
