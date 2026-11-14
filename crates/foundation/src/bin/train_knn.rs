use anyhow::{Context, Result};
use clap::{Parser, ValueEnum};
use foundation::database::batch_ingest_loop_intent_samples;
use foundation::modern_ui::symbols;
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(name = "train_knn")]
#[command(
    about = "Ingest loop-intent clustering samples for the dynamic-content KNN index",
    long_about = None
)]
struct Cli {
    /// Directory containing training assets
    input: PathBuf,

    /// Explicitly label the intent of these assets
    #[arg(short, long)]
    #[arg(value_enum)]
    label: Option<IntentLabel>,

    /// `PostgreSQL` connection string
    #[arg(short, long)]
    conn: Option<String>,
}

#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, ValueEnum, Debug)]
enum IntentLabel {
    /// Pure Loop (e.g. Meme stickers, simple animations, GIF/WebP loops)
    Loop,
    /// Non-Looping dynamic content (e.g. Full animations, screen recordings,
    /// video clips)
    NonLoop,
    /// Video-encapsulated Loop (e.g. Telegram Video Stickers, short MP4 loops)
    VideoLoop,
    /// Weak / low loop-intent label (matches C API `low` override)
    Low,
}

impl IntentLabel {
    const fn to_db_label(self) -> &'static str {
        match self {
            Self::Loop | Self::VideoLoop => "high",
            Self::NonLoop => "video",
            Self::Low => "low",
        }
    }
}

#[allow(clippy::too_many_lines)]
fn main() -> Result<()> {
    foundation::training_entry_guard::assert_train_knn_entry().context("train_knn entry guard")?;
    foundation::progress_mode::configure_terminal_ux(false);
    let cli = Cli::parse();

    if !cli.input.exists() {
        anyhow::bail!(
            "{} Training directory not found: {}",
            symbols::pick(symbols::ERROR, symbols::plain::ERROR),
            cli.input.display()
        );
    }

    foundation::ui_stderr::line(
        symbols::BRAIN,
        symbols::plain::BRAIN,
        "Starting Loop-Intent Clustering Ingestion...",
    );
    foundation::ui_stderr::line(
        symbols::FOLDER,
        symbols::plain::FOLDER,
        format!("Source: {}", cli.input.display()),
    );
    foundation::ui_stderr::line(
        symbols::TARGET,
        symbols::plain::TARGET,
        "Task Family: loop_clustering",
    );
    if let Some(l) = cli.label {
        foundation::ui_stderr::line(
            symbols::LABEL_TAG,
            symbols::plain::LABEL_TAG,
            format!(
                "Manual Override Label: {:?} (mapped to '{}')",
                l,
                l.to_db_label()
            ),
        );
    } else {
        foundation::ui_stderr::line(
            symbols::SEARCH,
            symbols::plain::SEARCH,
            "Mode: Automatic Heuristic Labeling",
        );
    }

    let conn = foundation::media_conversion_gate::delivery_training_pg_connstr_or_default(cli.conn);

    let label_str = cli.label.map(IntentLabel::to_db_label);
    let count = batch_ingest_loop_intent_samples(&cli.input, label_str, &conn)
        .context("Batch ingestion failed")?;

    foundation::ui_stderr::line(
        symbols::SUCCESS,
        symbols::plain::SUCCESS,
        format!("Success! Ingested {count} dynamic samples."),
    );
    foundation::ui_stderr::line(
        symbols::CHART,
        symbols::plain::CHART,
        "Global feature stats and baselines have been auto-refreshed.",
    );

    Ok(())
}
