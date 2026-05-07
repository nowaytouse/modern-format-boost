use anyhow::{Context, Result};
use clap::Parser;
use dev::media_index::MediaIndex;
use shared_utils::image_recommender::get_recommendation_from_row;
use shared_utils::video_recommender::get_video_recommendation_from_row;
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
    println!("🧪 Testing decisions against Media Index: {db_display}");
    println!("📊 Total records in DB: {count}");
    println!("--------------------------------------------------");

    let mut total = 0;
    let mut image_conversions = 0;
    let mut video_conversions = 0;

    let sql = "SELECT blake3 FROM media_entries";
    let mut stmt = db
        .conn_prepare(sql)
        .map_err(|e| anyhow::anyhow!("SQL Error: {e}"))?;

    let blake3_hashes: Vec<String> = stmt
        .query_map([], |row| row.get::<_, String>(0))?
        .filter_map(core::result::Result::ok)
        .collect();

    for hash in blake3_hashes {
        if let Some(row) = db.get_record(&hash)? {
            total += 1;
            match row.media_type.as_str() {
                "image" => {
                    if let Ok(rec) = get_recommendation_from_row(&row)
                        && rec.recommended_format != rec.current_format
                    {
                        image_conversions += 1;
                        let rel_path = &row.rel_path;
                        let rec_format = &rec.recommended_format;
                        let reason = &rec.reason;
                        println!("📸 [Img] {rel_path} -> {rec_format} ({reason})");
                    }
                }
                "video" => {
                    if let Ok(rec) = get_video_recommendation_from_row(&row)
                        && rec.is_archival_upgrade
                    {
                        video_conversions += 1;
                        let rel_path = &row.rel_path;
                        let rec_codec = &rec.recommended_codec;
                        let reason = &rec.reason;
                        println!("🎞️ [Vid] {rel_path} -> {rec_codec} ({reason})");
                    }
                }
                _ => {}
            }
        }
    }

    println!("--------------------------------------------------");
    println!("✅ Instant Regression Complete!");
    println!("   - Total Files Checked: {total}");
    println!("   - Image Upgrades:     {image_conversions}");
    println!("   - Video Upgrades:     {video_conversions}");

    if let Some(tag) = args.save {
        db.save_snapshot(&tag)
            .context("Failed to save decision snapshot")?;
        println!("📸 Snapshot saved with tag: {tag}");
    }

    Ok(())
}
