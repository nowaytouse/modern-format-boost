#![allow(unused_imports)]

use foundation::{
    log_anomaly, log_corruption, log_debug, log_detail, log_failure, log_fatal, log_hint,
    log_ignore, log_info, log_skip, log_success,
};

use anyhow::{Context, Result};
use clap::Parser;
use dev::media::index::MediaIndex;
use foundation::image_analyzer::get_recommendation_from_row;
use foundation::video_detection::get_video_recommendation_from_row;
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(
    author,
    version,
    about = "Runs decision logic against the Media Index database (Instant Regression)."
)]
struct Args {
    /// Path to the `media_index.sqlite`
    #[arg(short, long, default_value = "debug/media_index.sqlite")]
    db: PathBuf,

    /// Save the current decisions as a snapshot with this tag (e.g. v1.0, baseline)
    #[arg(short, long)]
    save: Option<String>,
}

fn main() -> Result<()> {
    let args = Args::parse();

    let db = MediaIndex::open(&args.db)?;
    let count = db.count_records()?;

    let db_display = args.db.display();
    log_detail!("🧪 Testing decisions against Media Index: {db_display}");
    log_detail!("📊 Total records in DB: {count}");
    log_detail!("--------------------------------------------------");

    let mut total = 0;
    let mut image_conversions = 0;
    let mut video_conversions = 0;

    let sql = "SELECT blake3 FROM media_entries";
    let mut stmt = db
        .conn_prepare(sql)
        .map_err(|e| anyhow::anyhow!("SQL Error: {e}"))?;

    let blake3_hashes: Vec<String> = stmt
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;

    for hash in blake3_hashes {
        if let Some(row) = db.get_record(&hash)? {
            total += 1;
            match row.media_type.as_str() {
                "image" => {
                    let rec = get_recommendation_from_row(&row)?;
                    if rec.recommended_format != rec.current_format {
                        image_conversions += 1;
                        let rel_path = &row.rel_path;
                        let rec_format = &rec.recommended_format;
                        let reason = &rec.reason;
                        log_detail!("📸 [Img] {rel_path} -> {rec_format} ({reason})");
                    }
                }
                "video" => {
                    let rec = get_video_recommendation_from_row(&row)?;
                    if rec.is_archival_upgrade {
                        video_conversions += 1;
                        let rel_path = &row.rel_path;
                        let rec_codec = &rec.recommended_codec;
                        let reason = &rec.reason;
                        log_detail!("🎞️ [Vid] {rel_path} -> {rec_codec} ({reason})");
                    }
                }
                _ => {}
            }
        }
    }

    log_detail!("--------------------------------------------------");
    log_detail!("✅ Instant Regression Complete!");
    log_detail!("   - Total Files Checked: {total}");
    log_detail!("   - Image Upgrades:     {image_conversions}");
    log_detail!("   - Video Upgrades:     {video_conversions}");

    if let Some(tag) = args.save {
        db.save_snapshot(&tag)
            .context("Failed to save decision snapshot")?;
        log_detail!("📸 Snapshot saved with tag: {tag}");
    }

    Ok(())
}
