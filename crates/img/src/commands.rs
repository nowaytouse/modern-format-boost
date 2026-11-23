#![allow(
    clippy::large_unopt_stack_frames,
    reason = "Clap generates one unoptimized parser factory for this command tree"
)]

use std::path::PathBuf;

use clap::Subcommand;

use super::runtime_cli;

#[derive(Subcommand)]
pub(crate) enum Commands {
    /// Inspect persistent runtime preferences without accessing media or databases.
    Config {
        #[command(subcommand)]
        command: runtime_cli::ConfigCommand,
    },
    #[command(name = "run")]
    Run {
        #[arg(short, long)]
        output: Option<PathBuf>,

        #[arg(long)]
        base_dir: Option<PathBuf>,

        #[arg(value_name = "INPUT")]
        input: PathBuf,

        #[arg(short, long)]
        force: bool,

        #[arg(short, long, default_value_t = true)]
        recursive: bool,

        #[arg(long)]
        delete_original: bool,

        #[arg(long)]
        in_place: bool,

        #[arg(long, default_value_t = true)]
        explore: bool,

        #[arg(long, default_value_t = true)]
        match_quality: bool,

        #[arg(long, default_value_t = true)]
        compress: bool,

        #[arg(long, default_value_t = true)]
        apple_compat: bool,

        #[arg(long)]
        no_apple_compat: bool,

        #[arg(long, default_value_t = false)]
        ultimate: bool,

        /// Archive mode uses the archival effort and fidelity search domains.
        #[arg(long, default_value_t = false)]
        archive: bool,

        /// Enable explicitly gated lab-only encoder/decoder fallbacks. Final verification is unchanged.
        #[arg(long = "allow_expert_options", default_value_t = false)]
        allow_expert_options: bool,

        /// Frozen advanced opt-in: disable JXL progressive/responsive transforms and generated noise.
        /// By default libjxl selects its own optional features. Does not affect JPEG reconstruction.
        #[arg(long = "jxl-fixed-features", default_value_t = false)]
        jxl_fixed_features: bool,

        #[arg(long, default_value_t = true)]
        preserve_timestamps: bool,

        #[arg(long, default_value_t = true)]
        preserve: bool,

        #[arg(short, long, default_value_t = true)]
        verbose: bool,

        /// ASCII symbols, no decorative ANSI (also respects `NO_COLOR` / `MODERN_FORMAT_PLAIN_UI=1`).
        #[arg(long, default_value_t = false)]
        plain: bool,

        /// Resume from last run: skip files already in progress file.
        #[arg(long, default_value_t = false)]
        resume: bool,

        /// Start fresh: ignore previous progress file, process all files.
        #[arg(long)]
        no_resume: bool,

        /// Static still delivery: `hevc`→JXL. `av1` is rejected; AVIF is reserved for fast-img Meme Mode.
        #[arg(long, value_parser = ["hevc", "av1"], default_value = "hevc")]
        codec: String,
    },

    Verify {
        original: PathBuf,

        converted: PathBuf,
    },

    RestoreTimestamps {
        #[arg(value_name = "SOURCE_DIR")]
        source: PathBuf,

        #[arg(value_name = "OUTPUT_DIR")]
        output: PathBuf,
    },

    /// Display cache statistics
    CacheStats,

    /// Internal: Check if a directory is already locked by MFB
    LockCheck {
        #[arg(value_name = "INPUT")]
        input: PathBuf,
    },

    /// Internal: Get the lock hash for a directory
    PathHash {
        #[arg(value_name = "INPUT")]
        input: PathBuf,
    },

    /// Batch ingest unannotated static image samples into `SQLite` database for Active Learning
    IngestSamples {
        #[arg(value_name = "INPUT_DIR")]
        input: PathBuf,

        #[arg(short, long)]
        label: Option<String>,
    },

    /// Perform deep diagnostic scan of the database infrastructure and data integrity
    DbHealth,

    /// Fast still-image encoding with reversible JPEG, lossless pixel JXL, or AVIF.
    ///
    /// Detects true JPEGs via content identity, requires byte-identical JPEG
    /// reconstruction after all metadata work, and deletes sources only after
    /// that final proof. External XMP is appended without rewriting the
    /// reconstruction-owned JXL boxes, then reconstruction is proved again.
    /// Static PNG and proven lossless WebP/AVIF use d=0 pixel encoding with effort
    /// exploration up to E10. A verified complete JXL at or below the original size wins.
    /// Confirmed-lossy modern stills can be delivered to Photos in their original format.
    ///
    /// Locked decisions: D1=Photos import only with shortest-path,
    /// D2=abort on delete failure, D3=exact JBRD plus orientation audit,
    /// D4=Rust-only, D5=subcommand,
    /// D6=verified source delete mandatory, D7=content-based admission.
    #[command(name = "fast-img")]
    FastImg {
        /// Input directory or static image. JXL accepts JPEG, reconstructible JXL,
        /// static PNG, and confirmed lossless WebP or AVIF.
        #[arg(value_name = "INPUT")]
        input: PathBuf,

        /// Exact working copy selected by the launcher. It must match the current
        /// source, delivery mode and marker state; direct CLI use may omit it.
        #[arg(short, long)]
        output: Option<PathBuf>,

        /// Deprecated for fast-img; verified source cleanup is mandatory.
        #[arg(long, default_value_t = false)]
        delete_source: bool,

        /// Dry-run: no writes, no deletions.
        #[arg(long, default_value_t = false)]
        dry_run: bool,

        /// Recurse into subdirectories.
        #[arg(short, long, default_value_t = true)]
        recursive: bool,

        /// Import verified outputs after Gate 1, then run fresh final Gate 2 before cleanup.
        #[arg(long = "shortest-path", default_value_t = false)]
        shortest_path: bool,

        /// Archive mode uses the archival search policy. JPEG comparison and retries follow runtime configuration.
        #[arg(long, default_value_t = false)]
        archive: bool,

        /// Explicitly resume a verified matching interrupted task. Changed inputs are rejected.
        #[arg(long, default_value_t = false)]
        retry: bool,

        /// Retry only retained failures from a completed task in an independent child attempt.
        #[arg(long, default_value_t = false, conflicts_with_all = ["retry", "no_resume"])]
        retry_failed: bool,

        /// Explicit completed parent for a new failed-only attempt; never inferred from --output.
        #[arg(long, value_name = "WORKING_COPY", requires = "retry_failed")]
        retry_failed_parent: Option<PathBuf>,

        /// Start in a new working copy without consuming prior state.
        #[arg(long, default_value_t = false, conflicts_with_all = ["retry", "retry_failed"])]
        no_resume: bool,

        /// Enable explicitly gated lab-only encoder/decoder fallbacks. Final verification is unchanged.
        #[arg(long = "allow_expert_options", default_value_t = false)]
        allow_expert_options: bool,

        /// Meme mode strategy. "jxl" (default) or "avif" (Meme Mode).
        #[arg(long, value_parser = ["jxl", "avif"], default_value = "jxl")]
        strategy: String,

        /// Accepted for CLI compatibility. Meme Mode keeps its bounded coarse-plus-binary search.
        #[arg(long = "extreme-precision", default_value_t = false)]
        extreme_precision: bool,
    },

    /// List native Photos folders/albums with the UUIDs accepted by restore-jpeg.
    #[command(name = "photos-albums")]
    PhotosAlbums {
        /// Photos library package to inspect.
        #[arg(value_name = "LIBRARY")]
        library: PathBuf,

        /// Emit machine-readable JSON for the native GUI.
        #[arg(long, default_value_t = false)]
        json: bool,
    },

    /// Restore exact JPEGs and isolate non-reversible JXL automatically. A
    /// Photos-library input audits live assets without rewriting media bytes.
    #[command(name = "restore-jpeg")]
    RestoreJpeg {
        /// JXL file/directory, Photos library, or concrete asset inside one.
        #[arg(value_name = "INPUT")]
        input: PathBuf,

        /// Output directory for local exact JPEG restoration.
        #[arg(short, long)]
        output: Option<PathBuf>,

        /// Recurse into subdirectories.
        #[arg(short, long, default_value_t = true)]
        recursive: bool,

        /// Overwrite existing restored JPEGs.
        #[arg(short, long, default_value_t = false)]
        force: bool,

        /// Preserve source JXL files after verified JPEG reconstruction.
        #[arg(long, default_value_t = false)]
        keep_source: bool,

        /// Audit only one native Photos album UUID.
        #[arg(long, conflicts_with = "photos_folder_id")]
        photos_album_id: Option<String>,

        /// Audit every album below one native Photos folder UUID.
        #[arg(long, conflicts_with = "photos_album_id")]
        photos_folder_id: Option<String>,
    },
}
