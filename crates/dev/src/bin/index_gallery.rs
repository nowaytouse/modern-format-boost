use anyhow::{Context, Result};
use clap::Parser;
use dev::media_index::{now_unix, MediaIndex};
use shared_utils::blake3::Hasher;
use shared_utils::image_detection::{detect_image, ImageType};
use shared_utils::media_index_types::MediaIndexRow;
use shared_utils::video_detection::detect_video;
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
}

fn main() -> Result<()> {
    let args = Args::parse();

    // Ensure debug directory exists
    if let Some(parent) = args.db.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let db = MediaIndex::open(&args.db)?;
    let db_display = args.db.display();
    println!("📂 Initialized Media Index at {db_display}");

    let mut count = 0;
    let mut skipped = 0;
    let mut errors = 0;

    for entry in WalkDir::new(&args.gallery_path)
        .into_iter()
        .filter_map(|e: std::result::Result<walkdir::DirEntry, walkdir::Error>| e.ok())
        .filter(|e: &walkdir::DirEntry| e.file_type().is_file())
    {
        let path = entry.path();

        // 1. Calculate BLAKE3
        let b3 = if let Ok(h) = calculate_blake3(path) { h } else {
            errors += 1;
            continue;
        };
        let b3_str = b3.to_string();

        // 2. Check if exists
        if let Ok(Some(_)) = db.get_record(&b3_str) {
            skipped += 1;
            continue;
        }

        // 3. Extract features
        match extract_record(path, &b3_str, &args.gallery_path) {
            Ok(record) => {
                db.upsert_extraction(&record)?;
                count += 1;
                if count % 100 == 0 {
                    let total = skipped + count;
                    println!("🚀 Indexed {count}/{total} files...");
                }
            }
            Err(e) => {
                let path_display = path.display();
                eprintln!("⚠️ Failed to index {path_display}: {e}");
                errors += 1;
            }
        }
    }

    println!("\n✅ Indexing Complete!");
    println!("   - New Records:  {count}");
    println!("   - Skipped Existing: {skipped}");
    println!("   - Errors:       {errors}");
    let total_rows = db.count_records()?;
    println!("   - Total Rows:   {total_rows}");

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
        hasher.update(&buffer[..n]);
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
        .unwrap_or_default()
        .to_lowercase();
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
        if v.duration_secs < 60.0 {
            let dur = v.duration_secs;
            anyhow::bail!("Skipping video: shorter than 1 minute (Current: {dur:.2}s)");
        }
        row.width = v.width;
        row.height = v.height;
        row.format.clone_from(&v.format);
        row.duration = v.duration_secs;
        row.has_hdr = v.is_hdr();
        row.raw_features_json = serde_json::to_string(&v)?;
    } else {
        let img = detect_image(path).context("Image analysis failed")?;
        // 🚨 Filter: ONLY static images
        if img.image_type != ImageType::Static {
            anyhow::bail!("Skipping non-static image (Animated/Sequence)");
        }
        row.width = img.width;
        row.height = img.height;
        row.format = img.format.as_str().to_string();
        row.has_alpha = img.has_alpha;
        row.raw_features_json = serde_json::to_string(&img)?;
        // Simple HDR check for images (if precision data has it, or based on bit depth)
        row.has_hdr = img.bit_depth > 8;
    }

    Ok(row)
}
