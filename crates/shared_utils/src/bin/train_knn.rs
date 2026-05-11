use anyhow::{Context, Result};
use clap::{Parser, ValueEnum};
use shared_utils::database::batch_ingest_samples;
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(name = "train_knn")]
#[command(about = "Train the dynamic content (Loop Intent) KNN model", long_about = None)]
struct Cli {
    /// Directory containing training assets
    input: PathBuf,

    /// Explicitly label the intent of these assets
    #[arg(short, long)]
    #[arg(value_enum)]
    label: Option<IntentLabel>,

    /// `PostgreSQL` connection string
    #[arg(
        short,
        long,
        default_value = "host=localhost dbname=modern_format_boost"
    )]
    conn: String,
}

#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, ValueEnum, Debug)]
enum IntentLabel {
    /// Pure Loop (e.g. Meme stickers, simple animations, GIF/WebP loops)
    Loop,
    /// Non-Looping dynamic content (e.g. Full animations, screen recordings, video clips)
    NonLoop,
    /// Video-encapsulated Loop (e.g. Telegram Video Stickers, short MP4 loops)
    VideoLoop,
}

impl IntentLabel {
    const fn to_db_label(self) -> &'static str {
        match self {
            Self::Loop | Self::VideoLoop => "high",
            Self::NonLoop => "video",
        }
    }
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    if !cli.input.exists() {
        anyhow::bail!("❌ Training directory not found: {}", cli.input.display());
    }

    shared_utils::progress_mode::emit_stderr("🧠 Starting Dynamic Content KNN Training...");
    shared_utils::progress_mode::emit_stderr(&format!("📂 Source: {}", cli.input.display()));
    if let Some(l) = cli.label {
        shared_utils::progress_mode::emit_stderr(&format!(
            "🏷️  Manual Override Label: {:?} (mapped to '{}')",
            l,
            l.to_db_label()
        ));
    } else {
        shared_utils::progress_mode::emit_stderr("🔍 Mode: Automatic Heuristic Labeling");
    }

    // Pass the label override to the batch ingestion engine
    let label_str = cli.label.map(IntentLabel::to_db_label);
    let count = batch_ingest_samples(&cli.input, label_str).context("Batch ingestion failed")?;

    shared_utils::progress_mode::emit_stderr(&format!(
        "✅ Success! Ingested {count} dynamic samples."
    ));
    shared_utils::progress_mode::emit_stderr(
        "📊 Global feature stats and baselines have been auto-refreshed.",
    );

    Ok(())
}
