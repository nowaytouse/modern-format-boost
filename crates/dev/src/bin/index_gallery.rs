use foundation::log_detail;

use anyhow::{Context, Result};
use blake3::Hasher;
use clap::Parser;
use dev::media::index::{MediaIndex, now_unix};
use foundation::image_analyzer::analyze_image;
use foundation::media_index_types::MediaIndexRow;
use foundation::video_detection::detect_video;
use std::io::Read;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

#[derive(Parser, Debug)]
#[command(
    author,
    version,
    about = "Indexes a media gallery into a SQLite database for accelerated development."
)]
struct Args {
    /// Path to the media gallery (iCloud export, etc.)
    #[arg(required = true)]
    gallery_path: PathBuf,

    /// Path to the `media_index.sqlite` (defaults to debug directory)
    #[arg(short, long, default_value = "debug/media_index.sqlite")]
    db: PathBuf,

    /// Re-extract features for files that already exist in the `SQLite` index.
    #[arg(long)]
    refresh_existing: bool,
}

fn main() -> Result<()> {
    foundation::entry_guard::assert_dev_tool_entry("index_gallery")?;
    let args = Args::parse();

    // Ensure debug directory exists
    if let Some(parent) = args.db.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let db = MediaIndex::open(&args.db)?;
    let db_display = args.db.display();
    log_detail!("📂 Initialized Media Index at {db_display}");

    let mut new_records = 0;
    let mut refreshed_existing = 0;
    let mut skipped = 0;
    let mut errors = 0;

    for entry in WalkDir::new(&args.gallery_path) {
        let entry = match entry {
            Ok(entry) => entry,
            Err(err) => {
                log_detail!(" Failed to walk gallery entry: {err}");
                errors += 1;
                continue;
            }
        };
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();

        // 1. Calculate BLAKE3
        let Ok(b3) = calculate_blake3(path) else {
            errors += 1;
            continue;
        };
        let b3_str = b3.to_string();

        // 2. Check if exists
        let existed = db.get_record(&b3_str)?.is_some();
        if existed && !args.refresh_existing {
            skipped += 1;
            continue;
        }

        // 3. Extract features
        match extract_record(path, &b3_str, &args.gallery_path) {
            Ok(record) => {
                db.upsert_extraction(&record)?;
                if existed {
                    refreshed_existing += 1;
                } else {
                    new_records += 1;
                }
                let processed = new_records + refreshed_existing;
                if processed % 100 == 0 {
                    let total = skipped + processed;
                    log_detail!(" Indexed {processed}/{total} files...");
                }
            }
            Err(e) => {
                let path_display = path.display();
                log_detail!(" Failed to index {path_display}: {e}");
                errors += 1;
            }
        }
    }

    log_detail!("\n Indexing Complete!");
    log_detail!(" - New Records: {new_records}");
    log_detail!(" - Refreshed Existing: {refreshed_existing}");
    log_detail!(" - Skipped Existing: {skipped}");
    log_detail!(" - Errors: {errors}");
    let total_rows = db.count_records()?;
    log_detail!(" - Total Rows: {total_rows}");

    if errors > 0 {
        anyhow::bail!("Gallery indexing finished with {errors} error(s)");
    }

    Ok(())
}

fn calculate_blake3(path: &Path) -> Result<blake3::Hash> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Hasher::new();
    let mut buffer = vec![0u8; 65536];
    loop {
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        hasher.update(buffer.get(..n).ok_or_else(|| {
            anyhow::anyhow!(
                "Required byte slice missing (out of bounds) at index {} with length {}",
                n,
                buffer.len()
            )
        })?);
    }
    Ok(hasher.finalize())
}

fn extract_record(path: &Path, b3: &str, root: &Path) -> Result<MediaIndexRow> {
    let rel_path = path.strip_prefix(root)?.to_string_lossy().to_string();
    let file_size = std::fs::metadata(path)?.len();

    // Determine type by extension/content (Simplified for tool)
    let ext = path
        .extension()
        .and_then(|s| s.to_str())
        .map_or_else(String::new, str::to_lowercase);
    let is_video = matches!(ext.as_str(), "mp4" | "mov" | "m4v" | "avi" | "mkv");

    let mut row = MediaIndexRow {
        blake3: b3.to_string(),
        rel_path,
        media_type: if is_video {
            "video".to_string()
        } else {
            "image".to_string()
        },
        width: 0,
        height: 0,
        format: String::new(),
        file_size,
        has_hdr: false,
        has_alpha: false,
        duration: 0.0,
        raw_features_json: String::new(),
        decided_format: None,
        decided_params_json: None,
        decision_reason: None,
        flagged_issue: None,
        last_extracted_at: now_unix(),
    };

    if is_video {
        let v = detect_video(path).context("Video ffprobe failed")?;
        // 🚨 Filter: ONLY long videos (1 minute minimum)
        let dur = v
            .duration_secs
            .ok_or_else(|| anyhow::anyhow!("Skipping video: ffprobe returned no duration"))?;
        if dur < 60.0 {
            anyhow::bail!("Skipping video: shorter than 1 minute (Current: {dur:.2}s)");
        }
        row.width = v
            .width
            .ok_or_else(|| anyhow::anyhow!("Skipping video: ffprobe returned no width"))?;
        row.height = v
            .height
            .ok_or_else(|| anyhow::anyhow!("Skipping video: ffprobe returned no height"))?;
        row.format.clone_from(&v.format);
        row.duration = dur;
        row.has_hdr = v.is_hdr();
        row.raw_features_json = serde_json::to_string(&v)?;
    } else {
        let img = analyze_image(path).context("Image analysis failed")?;
        // 🚨 Filter: ONLY static images
        if img.is_animated {
            anyhow::bail!("Skipping non-static image (Animated/Sequence)");
        }
        row.width = img.width;
        row.height = img.height;
        row.format.clone_from(&img.format);
        row.has_alpha = img.has_alpha;
        row.raw_features_json = serde_json::to_string(&img)?;
        row.has_hdr = img.has_true_hdr_metadata();
    }

    Ok(row)
}
