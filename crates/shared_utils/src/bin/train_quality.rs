use anyhow::{Context, Result};
use clap::{Parser, ValueEnum};
use postgres::{Client, NoTls};
use shared_utils::image_quality_db::{ingest_quality_sample, init_quality_schema};
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(name = "train_quality")]
#[command(about = "Train the static image quality KNN model", long_about = None)]
struct Cli {
    /// Directory containing sample images
    input: PathBuf,

    /// Semantic label for these samples
    #[arg(short, long)]
    #[arg(value_enum)]
    label: QualityLabel,

    /// `PostgreSQL` connection string
    #[arg(
        short,
        long,
        default_value = "host=localhost dbname=modern_format_boost"
    )]
    conn: String,
}

#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, ValueEnum, Debug)]
enum QualityLabel {
    /// High-quality PNG/Lossless
    PngHigh,
    /// Low-quality/Compressed PNG
    PngLow,
    /// High-quality Modern format (WebP/AVIF)
    ModernHigh,
    /// Low-quality Modern format
    ModernLow,
}

impl QualityLabel {
    fn as_str(self) -> &'static str {
        match self {
            Self::PngHigh => "png-high",
            Self::PngLow => "png-low",
            Self::ModernHigh => "modern-high",
            Self::ModernLow => "modern-low",
        }
    }
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    let mut client =
        Client::connect(&cli.conn, NoTls).context("Failed to connect to PostgreSQL")?;

    init_quality_schema(&mut client)?;

    println!("🎨 Training Static Image Quality Model...");
    println!("📂 Input: {} (Label: {:?}", cli.input.display(), cli.label);

    let mut count = 0;
    let supported_extensions = [
        "jpg", "jpeg", "jpe", "png", "webp", "gif", "tiff", "tif", "bmp", "ico", "avif", "heic",
        "heif", "hif", "jxl",
    ];

    for entry in walkdir::WalkDir::new(&cli.input).into_iter().flatten() {
        if entry.file_type().is_file() {
            let path = entry.path();
            let ext = path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_lowercase();

            if !supported_extensions.contains(&ext.as_str()) {
                continue;
            }

            if let Err(e) =
                ingest_quality_sample(&mut client, path, cli.label.as_str(), "manual_training")
            {
                eprintln!("⚠️ Failed to ingest {}: {}", path.display(), e);
            } else {
                count += 1;
            }
        }
    }

    println!("✅ Finished! Ingested {count} samples.");
    Ok(())
}
