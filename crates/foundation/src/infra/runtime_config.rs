//! Layered runtime preferences. Optional quality DB access is separate from
//! mandatory Photos custody verification.
use super::unified_error::BatchErrorMode;
use anyhow::{Context, Result, bail, ensure};
use clap::ValueEnum;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum PhotosBackend {
    Auto,
    #[default]
    #[serde(alias = "photokit")]
    #[value(alias = "photokit")]
    Native,
    Applescript,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum FallbackPolicy {
    #[default]
    Strict,
    #[serde(rename = "same-semantics", alias = "same_semantics")]
    SameSemantics,
    Repair,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum ToolPolicy {
    Single,
    #[default]
    Fallback,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum PerformanceMode {
    #[default]
    Adaptive,
    Relaxed,
    Balanced,
    Tight,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PerformancePolicy {
    pub mode: PerformanceMode,
}

#[derive(Debug, Clone, Default, clap::Args)]
pub struct PerformanceArgs {
    /// Adaptive scheduling is the default; fixed tiers still obey memory safety caps.
    #[arg(
        id = "performance_mode",
        long = "performance",
        global = true,
        value_enum
    )]
    mode: Option<PerformanceMode>,
}

impl PerformanceArgs {
    pub fn cli_arguments(&self) -> Vec<String> {
        self.mode.map_or_else(Vec::new, |mode| {
            vec![
                "--performance".into(),
                match mode {
                    PerformanceMode::Adaptive => "adaptive",
                    PerformanceMode::Relaxed => "relaxed",
                    PerformanceMode::Balanced => "balanced",
                    PerformanceMode::Tight => "tight",
                }
                .into(),
            ]
        })
    }

    pub fn apply_to(&self, loaded: &mut LoadedConfig) {
        if let Some(mode) = self.mode {
            loaded.config.performance.mode = mode;
            record_source(
                &mut loaded.sources,
                &mut loaded.source_chain,
                "performance.mode",
                "CLI",
            );
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ExecutionPolicy {
    pub error_mode: BatchErrorMode,
}

#[derive(Debug, Clone, Default, clap::Args)]
pub struct ExecutionArgs {
    /// Failure handling for IMG/VID conversion, Fast IMG, and shared Photos import.
    /// Record and continue is the default. Fast IMG also isolates fully identified
    /// file-local Gate 1 failures and re-verifies the remaining outputs.
    /// Fail-fast stops before the next Fast IMG wave after checkpointing started work.
    /// Shared safety failures always block; intentional skips remain skips.
    #[arg(long, global = true, value_enum)]
    error_mode: Option<BatchErrorMode>,
}

impl ExecutionArgs {
    pub fn cli_arguments(&self) -> Vec<String> {
        self.error_mode.map_or_else(Vec::new, |mode| {
            vec!["--error-mode".into(), mode.as_str().into()]
        })
    }

    pub fn apply_to(&self, loaded: &mut LoadedConfig) {
        if let Some(mode) = self.error_mode {
            loaded.config.execution.error_mode = mode;
            record_source(
                &mut loaded.sources,
                &mut loaded.source_chain,
                "execution.error_mode",
                "CLI",
            );
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ImgPolicy {
    pub allow_database: bool,
    pub quality_heuristic: bool,
    /// Standard IMG only: permit the configured pure-media growth budget.
    pub allow_size_tolerance: bool,
    /// Standard IMG only: inclusive pure-media byte allowance; metadata is excluded.
    pub size_tolerance_bytes: u64,
    /// Fixed AVIF quality used by Fast IMG AVIF; progressive AVIF requires at least Q10.
    pub avif_quality: u8,
    /// Final AVIF encoder speed used by Fast IMG AVIF delivery.
    pub avif_speed: u8,
    /// Fast AVIF's private removable-metadata contract, independent of JXL preservation.
    pub avif_metadata_policy: AvifMetadataPolicy,
    pub jpeg_effort: u8,
    /// Override distance only where the private media policy permits lossy encoding.
    pub jxl_distance: Option<f32>,
    /// None retains the normal/ultimate or PNG effort selected by the media policy.
    pub jxl_effort: Option<u8>,
    /// Compare verified encodes at fixed quality before distance exploration.
    pub jxl_effort_search: EffortSearch,
    /// Select reversible JPEG candidates independently of direct pixel encoding.
    pub jpeg_effort_search: JpegEffortSearch,
    pub quality_search: QualitySearch,
    pub fallback_policy: FallbackPolicy,
}

/// Supported metadata intent for the private Fast AVIF processing route.
///
/// This route is deliberately not an archival-preservation mode. Unsupported
/// policies must be rejected instead of changing encoder flags independently
/// of final verification and checkpoint identity.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub enum AvifMetadataPolicy {
    /// Remove removable embedded metadata and require the output clear-policy audit.
    #[default]
    Clear,
}

impl AvifMetadataPolicy {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Clear => "clear",
        }
    }

    #[must_use]
    pub const fn output_policy(self) -> crate::metadata::MetadataOutputPolicy {
        match self {
            Self::Clear => crate::metadata::MetadataOutputPolicy::Clear,
        }
    }

    /// Version of the existing cleanup contract required by reusable AVIF outputs.
    #[must_use]
    pub const fn version(self) -> u32 {
        match self {
            Self::Clear => 3,
        }
    }
}

/// Parse a final direct pixel JXL effort.
///
/// # Errors
/// Rejects values outside the encoder range (1 through 11).
pub fn parse_direct_jxl_effort(value: &str) -> std::result::Result<u8, String> {
    let effort = value.parse::<u8>().map_err(|error| error.to_string())?;
    if (1..=11).contains(&effort) {
        Ok(effort)
    } else {
        Err("direct JXL effort must be 1..11".into())
    }
}

/// Parse a finite JXL distance within the encoder range.
///
/// # Errors
/// Rejects non-finite values and distances outside 0 through 25.
pub fn parse_jxl_distance(value: &str) -> std::result::Result<f32, String> {
    let distance = value.parse::<f32>().map_err(|error| error.to_string())?;
    if distance.is_finite() && (0.0..=25.0).contains(&distance) {
        Ok(distance)
    } else {
        Err("JXL distance must be finite and within 0..25".into())
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub enum EffortSearch {
    Fixed,
    #[default]
    Adaptive,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub enum JpegEffortSearch {
    #[value(help = "Use the selected JPEG effort only")]
    Fixed,
    #[value(
        help = "Compare selected efforts up to the selected JPEG effort; not an exhaustive search"
    )]
    Adaptive,
    #[default]
    #[value(help = "Compare every JPEG effort E1 through E11; ignores the selected JPEG effort")]
    Full,
}

impl JpegEffortSearch {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Fixed => "fixed",
            Self::Adaptive => "adaptive",
            Self::Full => "full",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub enum QualitySearch {
    ModeDefault,
    Fixed,
    #[default]
    Adaptive,
}

impl QualitySearch {
    #[must_use]
    pub const fn enabled(self, default: bool) -> bool {
        match self {
            Self::ModeDefault => default,
            Self::Fixed => false,
            Self::Adaptive => true,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum VidCodec {
    #[default]
    Hevc,
    Av1,
}

impl VidCodec {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Hevc => "hevc",
            Self::Av1 => "av1",
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct VidPolicy {
    pub codec: VidCodec,
    /// Standard VID only: run fusion quality verification even for long videos.
    pub force_ms_ssim_long: bool,
}

impl Default for ImgPolicy {
    fn default() -> Self {
        Self {
            allow_database: false,
            quality_heuristic: false,
            allow_size_tolerance: false,
            size_tolerance_bytes: crate::constants::DEFAULT_SIZE_TOLERANCE_BYTES,
            avif_quality: 100,
            avif_speed: 0,
            avif_metadata_policy: AvifMetadataPolicy::Clear,
            jpeg_effort: 11,
            jxl_distance: None,
            jxl_effort: None,
            jxl_effort_search: EffortSearch::Adaptive,
            jpeg_effort_search: JpegEffortSearch::Full,
            quality_search: QualitySearch::Adaptive,
            fallback_policy: FallbackPolicy::Strict,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PhotosPolicy {
    pub backend: PhotosBackend,
    pub import_root: Option<String>,
    pub album_name: Option<String>,
    pub preserve_folder_structure: bool,
    pub native_batch_size: usize,
    pub import_batch_size: usize,
    pub verification_batch_size: usize,
    pub adaptive_batching: bool,
    pub native_min_batch_size: usize,
    pub native_max_batch_size: usize,
    pub target_batch_seconds: u64,
}

impl Default for PhotosPolicy {
    fn default() -> Self {
        Self {
            backend: PhotosBackend::Native,
            import_root: None,
            album_name: None,
            preserve_folder_structure: true,
            native_batch_size: 100,
            import_batch_size: 50,
            verification_batch_size: 250,
            adaptive_batching: false,
            native_min_batch_size: 50,
            native_max_batch_size: 1000,
            target_batch_seconds: 10,
        }
    }
}

/// Production IMG encoder/decoder, metadata and content-identification tools.
/// This is the runtime-binding scope, not an allowlist for generic helper overrides.
pub const IMG_MEDIA_TOOL_NAMES: &[&str] = &[
    "ffmpeg",
    "ffprobe",
    "magick",
    "convert",
    "identify",
    "avifenc",
    "avifdec",
    "cjxl",
    "djxl",
    "jxlinfo",
    "exiftool",
    "cwebp",
    "dwebp",
    "heif-convert",
    "heif-enc",
    "heif-info",
    "webpmux",
    "gif2webp",
    "gifski",
    "osxphotos",
    "opj_decompress",
    "avifgainmaputil",
    "pngcheck",
    "sf",
    "sips",
];

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ToolsPolicy {
    pub policy: ToolPolicy,
    pub paths: BTreeMap<String, PathBuf>,
}

#[derive(Debug, Clone, Default, clap::Args)]
pub struct ToolsArgs {
    /// Override a media tool executable, e.g. --tool cjxl=/absolute/path; may be repeated.
    #[arg(long = "tool", global = true, value_parser = parse_tool_override)]
    pub tools: Vec<(String, PathBuf)>,
}

/// Parse an explicit tool path without reading or executing the selected file.
/// Additional '=' characters belong to the path.
///
/// # Errors
/// Rejects empty values, invalid tool names or non-absolute executable paths.
pub fn parse_tool_override(value: &str) -> std::result::Result<(String, PathBuf), String> {
    let (name, path) = value
        .split_once('=')
        .ok_or_else(|| "expected NAME=PATH".to_owned())?;
    if name.is_empty() || path.is_empty() {
        return Err("tool name and executable path must be nonempty".into());
    }
    let path = PathBuf::from(path);
    validate_tool_override(name, &path).map_err(|error| error.to_string())?;
    Ok((name.to_owned(), path))
}

fn validate_tool_override(name: &str, path: &Path) -> Result<()> {
    ensure!(
        !name.is_empty()
            && name
                .chars()
                .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-' || ch == '_'),
        "tools.paths has invalid tool name {name:?}"
    );
    ensure!(path.is_absolute(), "tools.paths.{name} must be absolute");
    ensure!(
        !path.as_os_str().as_encoded_bytes().contains(&0),
        "tools.paths.{name} cannot contain NUL"
    );
    Ok(())
}

impl ToolsArgs {
    /// Preserve the exact path at string-based launcher boundaries.
    ///
    /// # Errors
    /// Rejects invalid paths or paths that cannot be represented without UTF-8 loss.
    pub fn cli_arguments(&self) -> Result<Vec<String>> {
        let mut arguments = Vec::with_capacity(self.tools.len() * 2);
        for (name, path) in &self.tools {
            validate_tool_override(name, path)?;
            let path = path.to_str().with_context(|| {
                format!("tools.paths.{name} cannot be forwarded without UTF-8 loss")
            })?;
            arguments.extend(["--tool".to_owned(), format!("{name}={path}")]);
        }
        Ok(arguments)
    }

    /// Apply explicit overrides; omitted tools continue inheriting the loaded configuration.
    ///
    /// # Errors
    /// Returns invalid tool override or effective configuration errors.
    pub fn apply_to(&self, loaded: &mut LoadedConfig) -> Result<()> {
        for (name, path) in &self.tools {
            validate_tool_override(name, path)?;
        }
        for (name, path) in &self.tools {
            loaded.config.tools.paths.insert(name.clone(), path.clone());
            record_source(
                &mut loaded.sources,
                &mut loaded.source_chain,
                format!("tools.paths.{name}"),
                "CLI",
            );
        }
        loaded.config.validate()
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CachePolicy {
    /// Total serialized path-tree payload bytes, excluding SQLite overhead and durable state.
    pub path_tree_max_bytes: u64,
    /// Idle lifetime; successful cache reads refresh the timestamp.
    pub path_tree_ttl_seconds: u64,
}

impl Default for CachePolicy {
    fn default() -> Self {
        Self {
            path_tree_max_bytes: 256 * 1024 * 1024,
            path_tree_ttl_seconds: 30 * 24 * 60 * 60,
        }
    }
}

impl CachePolicy {
    /// Reject unusable limits before any cache mutation.
    ///
    /// # Errors
    /// Returns an error for zero or values outside SQLite's signed integer range.
    pub fn validate(&self) -> Result<()> {
        for (name, value) in [
            ("path_tree_max_bytes", self.path_tree_max_bytes),
            ("path_tree_ttl_seconds", self.path_tree_ttl_seconds),
        ] {
            ensure!(
                value > 0 && i64::try_from(value).is_ok(),
                "cache.{name} must be positive and fit a SQLite signed 64-bit integer"
            );
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RuntimeConfig {
    pub config_version: u32,
    pub img: ImgPolicy,
    pub vid: VidPolicy,
    pub photos: PhotosPolicy,
    pub tools: ToolsPolicy,
    pub performance: PerformancePolicy,
    pub cache: CachePolicy,
    pub execution: ExecutionPolicy,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            config_version: 1,
            img: ImgPolicy::default(),
            vid: VidPolicy::default(),
            photos: PhotosPolicy::default(),
            tools: ToolsPolicy::default(),
            performance: PerformancePolicy::default(),
            cache: CachePolicy::default(),
            execution: ExecutionPolicy::default(),
        }
    }
}

impl RuntimeConfig {
    pub fn validate(&self) -> Result<()> {
        self.cache.validate()?;
        ensure!(
            self.config_version == 1,
            "unsupported config_version {}",
            self.config_version
        );
        ensure!(
            (1..=11).contains(&self.img.jpeg_effort),
            "img.jpeg_effort must be 1..11"
        );
        ensure!(
            self.img
                .jxl_effort
                .is_none_or(|effort| (1..=11).contains(&effort)),
            "img.jxl_effort must be 1..11 when specified"
        );
        ensure!(
            self.img
                .jxl_distance
                .is_none_or(|distance| distance.is_finite() && (0.0..=25.0).contains(&distance)),
            "img.jxl_distance must be finite and within 0..25 when specified"
        );
        ensure!(
            (10..=100).contains(&self.img.avif_quality),
            "img.avif_quality must be 10..100 because progressive AVIF requires Q10 or higher"
        );
        ensure!(self.img.avif_speed <= 10, "img.avif_speed must be 0..10");
        ensure!(
            (1..=1000).contains(&self.photos.native_batch_size),
            "photos.native_batch_size must be 1..1000"
        );
        ensure!(
            (1..=50).contains(&self.photos.import_batch_size),
            "photos.import_batch_size must be 1..50"
        );
        ensure!(
            (1..=1000).contains(&self.photos.verification_batch_size),
            "photos.verification_batch_size must be 1..1000"
        );
        ensure!(
            (1..=self.photos.native_max_batch_size).contains(&self.photos.native_min_batch_size)
                && self.photos.native_max_batch_size <= 1000
                && (1..=600).contains(&self.photos.target_batch_seconds),
            "invalid Photos adaptive bounds or target_batch_seconds (1..600)"
        );
        ensure!(
            !self.photos.adaptive_batching
                || (self.photos.native_min_batch_size..=self.photos.native_max_batch_size)
                    .contains(&self.photos.native_batch_size),
            "photos.native_batch_size must lie within adaptive min/max bounds"
        );
        for (field, name) in [
            ("photos.import_root", self.photos.import_root.as_deref()),
            ("photos.album_name", self.photos.album_name.as_deref()),
        ] {
            if let Some(name) = name {
                ensure!(
                    !name.is_empty()
                        && name != "."
                        && name != ".."
                        && !name
                            .chars()
                            .any(|ch| ch == '/' || ch == '\\' || ch.is_control()),
                    "{field} must be one nonempty folder or album name"
                );
            }
        }
        for (name, path) in &self.tools.paths {
            validate_tool_override(name, path)?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct LoadedConfig {
    pub config: RuntimeConfig,
    pub sources: BTreeMap<String, String>,
    pub source_chain: BTreeMap<String, Vec<String>>,
}

pub fn record_source(
    sources: &mut BTreeMap<String, String>,
    source_chain: &mut BTreeMap<String, Vec<String>>,
    key: impl Into<String>,
    source: impl Into<String>,
) {
    let key = key.into();
    let source = source.into();
    sources.insert(key.clone(), source.clone());
    source_chain.entry(key).or_default().push(source);
}

impl LoadedConfig {
    pub fn to_json(&self, pretty: bool) -> Result<String> {
        if pretty {
            Ok(serde_json::to_string_pretty(self)?)
        } else {
            Ok(serde_json::to_string(self)?)
        }
    }

    fn write_history_marker(
        &self,
        pipeline: &str,
        session_id: Option<&OsStr>,
        output: &mut impl Write,
    ) -> Result<()> {
        if session_id.is_none_or(OsStr::is_empty) {
            return Ok(());
        }
        ensure!(
            matches!(pipeline, "img" | "vid"),
            "unknown processing pipeline"
        );
        let marker = json!({
            "schema_version": 1,
            "pipeline": pipeline,
            "package_version": env!("CARGO_PKG_VERSION"),
            "config": &self.config,
            "sources": &self.sources,
        });
        let line = format!("MFB_HISTORY_CONFIG={}\n", serde_json::to_string(&marker)?);
        output
            .write_all(line.as_bytes())
            .context("write effective runtime configuration history marker")?;
        output
            .flush()
            .context("flush effective runtime configuration history marker")
    }

    pub fn emit_history_marker_if_session(&self, pipeline: &str) -> Result<()> {
        let session_id = std::env::var_os("MFB_SESSION_ID");
        self.write_history_marker(
            pipeline,
            session_id.as_deref(),
            &mut std::io::stdout().lock(),
        )
    }
}

static ACTIVE: OnceLock<RuntimeConfig> = OnceLock::new();

pub fn install(config: RuntimeConfig) -> Result<()> {
    config.validate()?;
    ACTIVE
        .set(config)
        .map_err(|_| anyhow::anyhow!("runtime config already installed"))
}

#[must_use]
pub fn active() -> Option<&'static RuntimeConfig> {
    ACTIVE.get()
}

fn merge(
    into: &mut Value,
    overlay: Value,
    source: &str,
    prefix: &str,
    sources: &mut BTreeMap<String, String>,
    source_chain: &mut BTreeMap<String, Vec<String>>,
) {
    if let (Some(dst), Value::Object(src)) = (into.as_object_mut(), overlay) {
        for (key, value) in src {
            let path = if prefix.is_empty() {
                key.clone()
            } else {
                format!("{prefix}.{key}")
            };
            if value.is_object() && dst.get(&key).is_some_and(Value::is_object) {
                if let Some(child) = dst.get_mut(&key) {
                    merge(child, value, source, &path, sources, source_chain);
                }
            } else {
                dst.insert(key, value);
                record_source(sources, source_chain, path, source);
            }
        }
    }
}

fn apply_file(
    value: &mut Value,
    path: &Path,
    sources: &mut BTreeMap<String, String>,
    source_chain: &mut BTreeMap<String, Vec<String>>,
) -> Result<()> {
    let raw = std::fs::read(path).with_context(|| format!("read config {}", path.display()))?;
    let layer: Value =
        serde_json::from_slice(&raw).with_context(|| format!("parse config {}", path.display()))?;
    let object = layer
        .as_object()
        .ok_or_else(|| anyhow::anyhow!("{}: config must be an object", path.display()))?;
    ensure!(
        object.get("config_version") == Some(&json!(1)),
        "{}: config_version must be 1",
        path.display()
    );
    merge(
        value,
        layer,
        &path.display().to_string(),
        "",
        sources,
        source_chain,
    );
    let parsed: RuntimeConfig = serde_json::from_value(value.clone())
        .with_context(|| format!("invalid config {}", path.display()))?;
    parsed
        .validate()
        .with_context(|| format!("invalid config {}", path.display()))
}

fn legacy_env(name: &str) -> Result<Option<String>> {
    match std::env::var(name) {
        Ok(value) => Ok(Some(value)),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(error) => Err(error).with_context(|| format!("read {name}")),
    }
}

fn legacy_bool(name: &str) -> Result<Option<bool>> {
    match legacy_env(name)? {
        Some(value) => match value.trim().to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" | "on" => Ok(Some(true)),
            "0" | "false" | "no" | "off" => Ok(Some(false)),
            _ => bail!("{name} must be a boolean (true/false, yes/no, on/off, 1/0)"),
        },
        None => Ok(None),
    }
}

fn legacy_tool_name(name: &str) -> String {
    // The executable is named opj_decompress; the other known tool with a
    // separator (heif-convert) uses a hyphen.
    if name == "OPJ_DECOMPRESS" {
        "opj_decompress".to_owned()
    } else {
        name.to_ascii_lowercase().replace('_', "-")
    }
}

fn default_sources(
    value: &Value,
    prefix: &str,
    sources: &mut BTreeMap<String, String>,
    source_chain: &mut BTreeMap<String, Vec<String>>,
) {
    if let Some(object) = value.as_object() {
        for (key, child) in object {
            let path = if prefix.is_empty() {
                key.clone()
            } else {
                format!("{prefix}.{key}")
            };
            default_sources(child, &path, sources, source_chain);
        }
    } else {
        record_source(sources, source_chain, prefix, "default");
    }
}

pub mod cache_args;
pub mod photos_args;

#[must_use]
pub fn user_config_path() -> Option<PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
        .map(|dir| dir.join("modern-format-boost/config.json"))
}

pub fn project_config_path() -> Result<PathBuf> {
    crate::media_conversion_gate::delivery_join_relative_to_cwd_or_err(
        Path::new("mfb.json"),
        "runtime project configuration",
    )
    .map_err(anyhow::Error::msg)
}

pub fn load(explicit: Option<&Path>, no_config: bool) -> Result<LoadedConfig> {
    ensure!(
        !(explicit.is_some() && no_config),
        "--config and --no-config conflict"
    );
    let mut value = serde_json::to_value(RuntimeConfig::default())?;
    let mut sources = BTreeMap::new();
    let mut source_chain = BTreeMap::new();
    default_sources(&value, "", &mut sources, &mut source_chain);
    for (mode, name) in BatchErrorMode::environment_overrides()? {
        merge(
            &mut value,
            json!({"execution":{"error_mode":mode}}),
            &format!("env:{name}"),
            "",
            &mut sources,
            &mut source_chain,
        );
    }
    if let Some(mode) = legacy_env(crate::constants::ENV_MFB_PERF_TIER)? {
        let mode = match mode.trim().to_ascii_lowercase().as_str() {
            "adaptive" => PerformanceMode::Adaptive,
            "relaxed" | "wide" => PerformanceMode::Relaxed,
            "balanced" | "normal" | "default" => PerformanceMode::Balanced,
            "tight" | "strict" | "conservative" => PerformanceMode::Tight,
            _ => bail!("MFB_PERF_TIER must be adaptive, relaxed, balanced or tight"),
        };
        merge(
            &mut value,
            json!({"performance":{"mode":mode}}),
            "env:MFB_PERF_TIER",
            "",
            &mut sources,
            &mut source_chain,
        );
    }
    if let Some(backend) = legacy_env("MFB_PHOTOS_IMPORT_BACKEND")? {
        merge(
            &mut value,
            json!({"photos":{"backend":backend}}),
            "env:MFB_PHOTOS_IMPORT_BACKEND",
            "",
            &mut sources,
            &mut source_chain,
        );
    }
    if let Some(size) = legacy_env("MFB_PHOTOS_NATIVE_BATCH_SIZE")? {
        let size: usize = size
            .parse()
            .context("MFB_PHOTOS_NATIVE_BATCH_SIZE must be an integer")?;
        merge(
            &mut value,
            json!({"photos":{"native_batch_size":size}}),
            "env:MFB_PHOTOS_NATIVE_BATCH_SIZE",
            "",
            &mut sources,
            &mut source_chain,
        );
    }
    if let Some(size) = legacy_env("MFB_FAST_IMG_PHOTOS_IMPORT_BATCH_SIZE")? {
        let size: usize = size
            .parse()
            .context("MFB_FAST_IMG_PHOTOS_IMPORT_BATCH_SIZE must be an integer")?;
        merge(
            &mut value,
            json!({"photos":{"import_batch_size":size.min(50)}}),
            "env:MFB_FAST_IMG_PHOTOS_IMPORT_BATCH_SIZE",
            "",
            &mut sources,
            &mut source_chain,
        );
    }
    if let Some(size) = legacy_env("MFB_FAST_IMG_ICLOUD_VERIFY_BATCH_SIZE")? {
        let size: usize = size
            .parse()
            .context("MFB_FAST_IMG_ICLOUD_VERIFY_BATCH_SIZE must be an integer")?;
        merge(
            &mut value,
            json!({"photos":{"verification_batch_size":size.min(128)}}),
            "env:MFB_FAST_IMG_ICLOUD_VERIFY_BATCH_SIZE",
            "",
            &mut sources,
            &mut source_chain,
        );
    }
    let heuristic = legacy_bool(crate::constants::HEURISTIC_QUALITY_ENV_KEY)?;
    if let Some(heuristic) = heuristic {
        merge(
            &mut value,
            json!({"img":{"quality_heuristic":heuristic}}),
            "env:MODERN_FORMAT_ENABLE_IMAGE_QUALITY_HEURISTIC",
            "",
            &mut sources,
            &mut source_chain,
        );
    }
    let disable_feedback = legacy_bool(crate::constants::ENV_DISABLE_DB_FEEDBACK)?;
    let disable_image_db = legacy_bool(crate::constants::ENV_DISABLE_IMAGE_QUALITY_DB)?;
    if heuristic.is_some() || disable_feedback.is_some() || disable_image_db.is_some() {
        let allow_database = heuristic.unwrap_or(false)
            && !disable_feedback.unwrap_or(false)
            && !disable_image_db.unwrap_or(false);
        merge(
            &mut value,
            json!({"img":{"allow_database":allow_database}}),
            "legacy quality database gates",
            "",
            &mut sources,
            &mut source_chain,
        );
    }
    for (key, path) in std::env::vars_os() {
        let Some(name) = key.to_str().and_then(|key| key.strip_prefix("MFB_TOOL_")) else {
            continue;
        };
        let name = legacy_tool_name(name);
        let path = PathBuf::from(path);
        let mut paths = serde_json::Map::new();
        paths.insert(name, json!(path));
        merge(
            &mut value,
            json!({"tools":{"paths":paths}}),
            "legacy MFB_TOOL_ override",
            "",
            &mut sources,
            &mut source_chain,
        );
    }
    if !no_config {
        if let Some(path) = user_config_path()
            && path
                .try_exists()
                .with_context(|| format!("inspect config {}", path.display()))?
        {
            apply_file(&mut value, &path, &mut sources, &mut source_chain)?;
        }
        let project = project_config_path()?;
        if project
            .try_exists()
            .with_context(|| format!("inspect config {}", project.display()))?
        {
            apply_file(&mut value, &project, &mut sources, &mut source_chain)?;
        }
        if let Some(path) = explicit {
            apply_file(&mut value, path, &mut sources, &mut source_chain)?;
        }
    }
    let config: RuntimeConfig = serde_json::from_value(value)?;
    config.validate()?;
    Ok(LoadedConfig {
        config,
        sources,
        source_chain,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn direct_jxl_effort_is_bounded_and_independent_from_jpeg() -> Result<()> {
        let mut config = RuntimeConfig::default();
        assert_eq!(config.img.jxl_effort, None);
        assert_eq!(config.img.jxl_distance, None);
        assert_eq!(config.img.jxl_effort_search, EffortSearch::Adaptive);
        assert_eq!(config.img.jpeg_effort_search, JpegEffortSearch::Full);
        assert_eq!(config.img.quality_search, QualitySearch::Adaptive);
        assert_eq!(
            serde_json::to_value(&config)?["img"]["jxl_effort"],
            Value::Null
        );
        for effort in 1..=11 {
            config.img.jxl_effort = Some(effort);
            config.validate()?;
            assert_eq!(parse_direct_jxl_effort(&effort.to_string()), Ok(effort));
            assert_eq!(config.img.jpeg_effort, 11);
        }
        for effort in [0, 12, u8::MAX] {
            config.img.jxl_effort = Some(effort);
            assert!(config.validate().is_err());
            assert!(parse_direct_jxl_effort(&effort.to_string()).is_err());
        }
        let mut value = serde_json::to_value(RuntimeConfig::default())?;
        let mut sources = BTreeMap::new();
        let mut source_chain = BTreeMap::new();
        let temp = tempfile::tempdir()?;
        let saved = temp.path().join("direct.json");
        std::fs::write(
            &saved,
            br#"{"config_version":1,"img":{"jxl_effort":8,"jpeg_effort":9}}"#,
        )?;
        apply_file(&mut value, &saved, &mut sources, &mut source_chain)?;
        assert_eq!(value["img"]["jxl_effort"], 8);
        std::fs::write(&saved, br#"{"config_version":1,"img":{"jxl_effort":null}}"#)?;
        apply_file(&mut value, &saved, &mut sources, &mut source_chain)?;
        assert_eq!(value["img"]["jxl_effort"], Value::Null);
        assert_eq!(value["img"]["jpeg_effort"], 9);
        Ok(())
    }

    #[test]
    fn jxl_distance_rejects_non_finite_and_out_of_range_values() -> Result<()> {
        let mut config = RuntimeConfig::default();
        for distance in [0.0, 0.001, 0.4, 25.0] {
            config.img.jxl_distance = Some(distance);
            config.validate()?;
            assert_eq!(parse_jxl_distance(&distance.to_string()), Ok(distance));
        }
        for distance in [-0.1, 25.1, f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            config.img.jxl_distance = Some(distance);
            assert!(config.validate().is_err());
            assert!(parse_jxl_distance(&distance.to_string()).is_err());
        }
        config.img.jxl_distance = Some(0.4);
        config.img.quality_search = QualitySearch::Adaptive;
        config.validate()?;
        config.img.quality_search = QualitySearch::Fixed;
        config.validate()?;
        config.img.jxl_distance = None;
        config.validate()?;
        let restored: RuntimeConfig =
            serde_json::from_value(json!({"img":{"jpeg_effort_search":"adaptive"}}))?;
        assert_eq!(restored.img.jpeg_effort_search, JpegEffortSearch::Adaptive);
        Ok(())
    }

    #[test]
    fn jpeg_full_search_round_trips_independently_of_pixel_search() -> Result<()> {
        let restored: RuntimeConfig = serde_json::from_value(json!({
            "img": {
                "jpeg_effort": 1,
                "jpeg_effort_search": "full",
                "jxl_effort_search": "fixed"
            }
        }))?;
        restored.validate()?;
        assert_eq!(restored.img.jpeg_effort_search, JpegEffortSearch::Full);
        assert_eq!(restored.img.jpeg_effort, 1);
        assert_eq!(restored.img.jxl_effort_search, EffortSearch::Fixed);
        let serialized = serde_json::to_value(&restored)?;
        assert_eq!(serialized["img"]["jpeg_effort_search"], "full");
        assert_eq!(JpegEffortSearch::Full.as_str(), "full");
        assert!(
            serde_json::from_value::<RuntimeConfig>(json!({
                "img": {"jxl_effort_search": "full"}
            }))
            .is_err()
        );
        Ok(())
    }

    #[test]
    fn search_defaults_do_not_override_explicit_fixed_or_mode_default_choices() -> Result<()> {
        let empty: RuntimeConfig = serde_json::from_value(json!({"img": {}}))?;
        assert_eq!(empty.img.jxl_effort_search, EffortSearch::default());
        assert_eq!(empty.img.jpeg_effort_search, JpegEffortSearch::default());
        assert_eq!(empty.img.quality_search, QualitySearch::default());
        for quality in ["fixed", "mode-default"] {
            let saved = json!({
                "img": {
                    "jxl_effort_search": "fixed",
                    "jpeg_effort_search": "fixed",
                    "quality_search": quality
                }
            });
            let restored: RuntimeConfig = serde_json::from_value(saved.clone())?;
            restored.validate()?;
            let serialized = serde_json::to_value(restored)?;
            for key in ["jxl_effort_search", "jpeg_effort_search", "quality_search"] {
                assert_eq!(serialized["img"][key], saved["img"][key]);
            }
        }
        Ok(())
    }

    #[test]
    fn tool_overrides_preserve_inheritance_exact_paths_and_cli_sources() -> Result<()> {
        let mut loaded = LoadedConfig {
            config: RuntimeConfig::default(),
            sources: BTreeMap::new(),
            source_chain: BTreeMap::new(),
        };
        for name in ["cjxl", "djxl"] {
            loaded
                .config
                .tools
                .paths
                .insert(name.into(), PathBuf::from(format!("/saved/{name}")));
            record_source(
                &mut loaded.sources,
                &mut loaded.source_chain,
                format!("tools.paths.{name}"),
                "saved",
            );
        }
        ToolsArgs::default().apply_to(&mut loaded)?;
        assert_eq!(ToolsArgs::default().cli_arguments()?, Vec::<String>::new());
        let path = "/not-created/\u{5de5}\u{5177} nightly=1/cjxl";
        let tools = ToolsArgs {
            tools: vec![
                parse_tool_override("cjxl=/first/cjxl").map_err(anyhow::Error::msg)?,
                parse_tool_override(&format!("cjxl={path}")).map_err(anyhow::Error::msg)?,
            ],
        };
        assert_eq!(
            tools.cli_arguments()?,
            [
                "--tool",
                "cjxl=/first/cjxl",
                "--tool",
                &format!("cjxl={path}")
            ]
        );
        tools.apply_to(&mut loaded)?;
        assert_eq!(loaded.config.tools.paths["cjxl"], PathBuf::from(path));
        assert_eq!(
            loaded.config.tools.paths["djxl"],
            PathBuf::from("/saved/djxl")
        );
        assert_eq!(
            loaded.source_chain["tools.paths.cjxl"],
            ["saved", "CLI", "CLI"]
        );
        assert_eq!(loaded.sources["tools.paths.djxl"], "saved");
        let document: Value = serde_json::from_str(&loaded.to_json(false)?)?;
        assert_eq!(document["config"]["tools"]["paths"]["cjxl"], path);
        for invalid in [
            "missing-equals",
            "=empty",
            "cjxl=",
            "CJXL=/tool",
            "tool name=/tool",
            "cjxl=relative/tool",
            "cjxl=/bad\0path",
        ] {
            assert!(parse_tool_override(invalid).is_err());
        }
        let invalid = ToolsArgs {
            tools: vec![("cjxl".into(), PathBuf::from("relative/tool"))],
        };
        assert!(invalid.cli_arguments().is_err());
        assert!(invalid.apply_to(&mut loaded).is_err());
        assert_eq!(loaded.config.tools.paths["cjxl"], PathBuf::from(path));
        #[cfg(unix)]
        {
            use std::ffi::OsString;
            use std::os::unix::ffi::OsStringExt;
            let tools = ToolsArgs {
                tools: vec![(
                    "cjxl".into(),
                    PathBuf::from(OsString::from_vec(b"/tool-\xff".to_vec())),
                )],
            };
            assert!(tools.cli_arguments().is_err());
        }
        Ok(())
    }
    #[test]
    fn image_size_tolerance_budget_defaults_layers_and_integer_bounds() -> Result<()> {
        let mut value = serde_json::to_value(RuntimeConfig::default())?;
        let mut sources = BTreeMap::new();
        let mut source_chain = BTreeMap::new();
        default_sources(&value, "", &mut sources, &mut source_chain);
        assert_eq!(value["img"]["size_tolerance_bytes"], 524_288_u64);
        assert_eq!(value["img"]["allow_size_tolerance"], false);
        assert_eq!(sources["img.size_tolerance_bytes"], "default");
        let file = tempfile::NamedTempFile::new()?;
        std::fs::write(
            file.path(),
            br#"{"config_version":1,"img":{"size_tolerance_bytes":17}}"#,
        )?;
        apply_file(&mut value, file.path(), &mut sources, &mut source_chain)?;
        let parsed: RuntimeConfig = serde_json::from_value(value.clone())?;
        assert_eq!(parsed.img.size_tolerance_bytes, 17);
        assert!(
            !parsed.img.allow_size_tolerance,
            "a budget must not enable tolerance"
        );
        assert_eq!(
            source_chain["img.size_tolerance_bytes"],
            ["default", file.path().to_str().context("config path")?]
        );
        for bytes in [0, u64::MAX] {
            let parsed: RuntimeConfig =
                serde_json::from_value(json!({"img":{"size_tolerance_bytes":bytes}}))?;
            parsed.validate()?;
            assert_eq!(parsed.img.size_tolerance_bytes, bytes);
            assert!(!parsed.img.allow_size_tolerance);
        }
        for invalid in [json!(-1), json!(1.5), json!("17"), json!(true), Value::Null] {
            let mut candidate = value.clone();
            candidate["img"]["size_tolerance_bytes"] = invalid;
            assert!(serde_json::from_value::<RuntimeConfig>(candidate).is_err());
        }
        let old_config: RuntimeConfig = serde_json::from_value(
            json!({"config_version":1,"img":{"allow_size_tolerance":true}}),
        )?;
        assert_eq!(
            old_config.img.size_tolerance_bytes,
            crate::constants::DEFAULT_SIZE_TOLERANCE_BYTES
        );
        Ok(())
    }

    #[test]
    fn fast_img_avif_quality_and_speed_validate_encoder_bounds() -> Result<()> {
        let defaults = RuntimeConfig::default();
        assert_eq!(defaults.img.avif_quality, 100);
        assert_eq!(defaults.img.avif_speed, 0);
        assert_eq!(defaults.img.avif_metadata_policy, AvifMetadataPolicy::Clear);
        assert_eq!(
            defaults.img.avif_metadata_policy.output_policy(),
            crate::metadata::MetadataOutputPolicy::Clear
        );
        assert_eq!(defaults.img.avif_metadata_policy.version(), 3);
        defaults.validate()?;
        let empty_config: RuntimeConfig = serde_json::from_value(json!({}))?;
        assert_eq!(empty_config.img.avif_quality, 100);
        assert_eq!(empty_config.img.avif_speed, 0);
        assert_eq!(
            empty_config.img.avif_metadata_policy,
            AvifMetadataPolicy::Clear
        );
        let configured: RuntimeConfig = serde_json::from_value(json!({
            "img": {"avif_metadata_policy": "clear"}
        }))?;
        configured.validate()?;
        assert_eq!(configured.img.avif_metadata_policy, AvifMetadataPolicy::Clear);
        assert_eq!(
            serde_json::to_value(&configured)?["img"]["avif_metadata_policy"],
            "clear"
        );
        for invalid in [
            json!("preserve"),
            json!("unknown"),
            json!(""),
            json!(false),
            Value::Null,
        ] {
            assert!(
                serde_json::from_value::<RuntimeConfig>(json!({
                    "img": {"avif_metadata_policy": invalid}
                }))
                .is_err()
            );
        }

        for (quality, speed) in [(10, 0), (100, 10)] {
            let config: RuntimeConfig = serde_json::from_value(json!({
                "img": {"avif_quality": quality, "avif_speed": speed}
            }))?;
            config.validate()?;
            assert_eq!(config.img.avif_quality, quality);
            assert_eq!(config.img.avif_speed, speed);
        }

        for (field, value) in [
            ("avif_quality", 0),
            ("avif_quality", 9),
            ("avif_quality", 101),
            ("avif_speed", 11),
        ] {
            let config: RuntimeConfig = serde_json::from_value(json!({
                "img": {field: value}
            }))?;
            assert!(
                config.validate().is_err(),
                "{field}={value} must be rejected"
            );
        }
        Ok(())
    }

    #[test]
    fn execution_policy_defaults_file_layers_and_cli_are_transparent() -> Result<()> {
        let mut value = serde_json::to_value(RuntimeConfig::default())?;
        let mut sources = BTreeMap::new();
        let mut source_chain = BTreeMap::new();
        default_sources(&value, "", &mut sources, &mut source_chain);
        assert_eq!(value["execution"]["error_mode"], "log-and-continue");
        assert_eq!(sources["execution.error_mode"], "default");
        merge(
            &mut value,
            json!({"execution":{"error_mode":"fail-fast"}}),
            "env:MFB_ERROR_MODE",
            "",
            &mut sources,
            &mut source_chain,
        );
        let file = tempfile::NamedTempFile::new()?;
        std::fs::write(
            file.path(),
            br#"{"config_version":1,"execution":{"error_mode":"log-and-continue"}}"#,
        )?;
        apply_file(&mut value, file.path(), &mut sources, &mut source_chain)?;
        let mut loaded = LoadedConfig {
            config: serde_json::from_value(value)?,
            sources,
            source_chain,
        };
        assert_eq!(
            loaded.config.execution.error_mode,
            BatchErrorMode::LogAndContinue
        );
        ExecutionArgs::default().apply_to(&mut loaded);
        assert_eq!(
            loaded.sources["execution.error_mode"],
            file.path().display().to_string()
        );
        let args = ExecutionArgs {
            error_mode: Some(BatchErrorMode::FailFast),
        };
        args.apply_to(&mut loaded);
        assert_eq!(args.cli_arguments(), ["--error-mode", "fail-fast"]);
        assert_eq!(
            ExecutionArgs::default().cli_arguments(),
            Vec::<String>::new()
        );
        let shown: Value = serde_json::from_str(&loaded.to_json(false)?)?;
        assert_eq!(shown["config"]["execution"]["error_mode"], "fail-fast");
        assert_eq!(
            loaded.source_chain["execution.error_mode"],
            [
                "default",
                "env:MFB_ERROR_MODE",
                file.path().to_str().context("config path")?,
                "CLI"
            ]
        );
        for invalid in ["", "unknown", "debug", "log_and_continue"] {
            assert!(
                serde_json::from_value::<RuntimeConfig>(
                    json!({"execution":{"error_mode":invalid}})
                )
                .is_err()
            );
        }
        assert!(
            serde_json::from_value::<RuntimeConfig>(json!({"execution":{"unknown":1}})).is_err()
        );
        Ok(())
    }

    #[test]
    fn execution_environment_precedence_and_installed_policy_are_checked() -> Result<()> {
        const CHILD: &str = "MFB_TEST_EXECUTION_POLICY_CHILD";
        let case = match std::env::var(CHILD) {
            Ok(case) => Some(case),
            Err(std::env::VarError::NotPresent) => None,
            Err(error) => return Err(error).context("Invalid execution policy test case"),
        };
        if let Some(case) = case {
            let resolved = load(None, true);
            if case.starts_with("invalid") || case == "empty" {
                assert!(resolved.is_err());
                return Ok(());
            }
            let mut loaded = resolved?;
            if case == "default" {
                assert_eq!(
                    loaded.config.execution.error_mode,
                    BatchErrorMode::LogAndContinue
                );
                assert_eq!(loaded.source_chain["execution.error_mode"], ["default"]);
            } else {
                assert_eq!(
                    loaded.config.execution.error_mode,
                    if case == "cli-continue" {
                        BatchErrorMode::FailFast
                    } else {
                        BatchErrorMode::LogAndContinue
                    }
                );
                assert_eq!(
                    loaded.source_chain["execution.error_mode"],
                    [
                        "default",
                        "env:MFB_DRAG_DROP_ERROR_MODE",
                        "env:MFB_DRAG_DROP_FAIL_FAST",
                        "env:MFB_ERROR_MODE"
                    ]
                );
                let expected = if case == "cli-continue" {
                    BatchErrorMode::LogAndContinue
                } else {
                    BatchErrorMode::FailFast
                };
                ExecutionArgs {
                    error_mode: Some(expected),
                }
                .apply_to(&mut loaded);
                assert_eq!(loaded.sources["execution.error_mode"], "CLI");
                install(loaded.config)?;
                assert_eq!(BatchErrorMode::current(), expected);
            }
            return Ok(());
        }
        let name = format!(
            "{}::execution_environment_precedence_and_installed_policy_are_checked",
            module_path!()
        );
        let (_, name) = name.split_once("::").context("test module path")?;
        for case in [
            "default",
            "overrides",
            "cli-continue",
            "invalid",
            "invalid-legacy",
            "invalid-boolean",
            "empty",
        ] {
            let mut command = std::process::Command::new(std::env::current_exe()?);
            command.args(["--exact", name]).env(CHILD, case);
            for name in [
                crate::constants::ENV_MFB_DRAG_DROP_ERROR_MODE,
                crate::constants::ENV_MFB_DRAG_DROP_FAIL_FAST,
                crate::constants::ENV_MFB_ERROR_MODE,
            ] {
                command.env_remove(name);
            }
            if case != "default" {
                command
                    .env(
                        crate::constants::ENV_MFB_DRAG_DROP_ERROR_MODE,
                        if case == "invalid-legacy" {
                            "unknown"
                        } else {
                            "debug"
                        },
                    )
                    .env(
                        crate::constants::ENV_MFB_DRAG_DROP_FAIL_FAST,
                        if case == "invalid-boolean" {
                            "unknown"
                        } else {
                            "true"
                        },
                    )
                    .env(
                        crate::constants::ENV_MFB_ERROR_MODE,
                        match case {
                            "invalid" => "unknown",
                            "empty" => "",
                            "cli-continue" => "fail-fast",
                            _ => "log-and-continue",
                        },
                    );
            }
            let output = command.output()?;
            assert!(
                output.status.success(),
                "{case}: {}",
                String::from_utf8_lossy(&output.stdout)
            );
            assert!(String::from_utf8_lossy(&output.stdout).contains("running 1 test"));
        }
        Ok(())
    }

    #[test]
    fn cache_policy_defaults_layers_and_integer_bounds() -> Result<()> {
        let mut value = serde_json::to_value(RuntimeConfig::default())?;
        let mut sources = BTreeMap::new();
        let mut source_chain = BTreeMap::new();
        default_sources(&value, "", &mut sources, &mut source_chain);
        assert_eq!(value["cache"]["path_tree_max_bytes"], 268_435_456_u64);
        assert_eq!(value["cache"]["path_tree_ttl_seconds"], 2_592_000_u64);
        assert_eq!(sources["cache.path_tree_max_bytes"], "default");
        let dir = tempfile::tempdir()?;
        let file = dir.path().join("config.json");
        std::fs::write(
            &file,
            r#"{"config_version":1,"cache":{"path_tree_max_bytes":16}}"#,
        )?;
        apply_file(&mut value, &file, &mut sources, &mut source_chain)?;
        let config: RuntimeConfig = serde_json::from_value(value.clone())?;
        assert_eq!(config.cache.path_tree_max_bytes, 16);
        assert_eq!(config.cache.path_tree_ttl_seconds, 2_592_000);
        assert_eq!(
            source_chain["cache.path_tree_max_bytes"],
            ["default", file.to_str().unwrap()]
        );
        assert_eq!(source_chain["cache.path_tree_ttl_seconds"], ["default"]);
        let loaded = LoadedConfig {
            config,
            sources,
            source_chain,
        };
        let effective: Value = serde_json::from_str(&loaded.to_json(false)?)?;
        assert_eq!(effective["config"]["cache"]["path_tree_max_bytes"], 16);
        for field in ["path_tree_max_bytes", "path_tree_ttl_seconds"] {
            for invalid in [json!(0), json!(-1), json!(u64::MAX), json!(1.5)] {
                let mut candidate = value.clone();
                candidate["cache"][field] = invalid;
                assert!(
                    serde_json::from_value::<RuntimeConfig>(candidate)
                        .map_err(anyhow::Error::from)
                        .and_then(|config| config.validate())
                        .is_err()
                );
            }
            let mut boundary = value.clone();
            boundary["cache"][field] = json!(i64::MAX);
            serde_json::from_value::<RuntimeConfig>(boundary)?.validate()?;
        }
        Ok(())
    }

    #[test]
    fn defaults_and_explicit_performance_keep_one_policy() {
        let config = RuntimeConfig::default();
        assert_eq!(config.photos.backend, PhotosBackend::Native);
        assert_eq!(config.performance.mode, PerformanceMode::Adaptive);
        assert_eq!(config.vid.codec, VidCodec::Hevc);
        let mut loaded = LoadedConfig {
            config,
            sources: BTreeMap::new(),
            source_chain: BTreeMap::new(),
        };
        let args = PerformanceArgs {
            mode: Some(PerformanceMode::Tight),
        };
        args.apply_to(&mut loaded);
        assert_eq!(loaded.config.performance.mode, PerformanceMode::Tight);
        assert_eq!(loaded.sources["performance.mode"], "CLI");
        assert_eq!(loaded.source_chain["performance.mode"], ["CLI"]);
        assert_eq!(args.cli_arguments(), ["--performance", "tight"]);
        assert_eq!(
            PerformanceArgs::default().cli_arguments(),
            Vec::<String>::new()
        );
        assert!(
            serde_json::from_value::<RuntimeConfig>(json!({"performance":{"mode":"unknown"}}))
                .is_err()
        );
    }

    #[test]
    fn legacy_tool_names_match_resolver_names() {
        assert_eq!(legacy_tool_name("OPJ_DECOMPRESS"), "opj_decompress");
        assert_eq!(legacy_tool_name("HEIF_CONVERT"), "heif-convert");
    }

    #[test]
    fn history_marker_uses_resolved_values_only_for_launcher_sessions() -> Result<()> {
        let mut loaded = LoadedConfig {
            config: RuntimeConfig::default(),
            sources: BTreeMap::new(),
            source_chain: BTreeMap::new(),
        };
        loaded.config.performance.mode = PerformanceMode::Tight;
        record_source(
            &mut loaded.sources,
            &mut loaded.source_chain,
            "performance.mode",
            "CLI",
        );
        let mut output = Vec::new();
        loaded.write_history_marker("img", None, &mut output)?;
        loaded.write_history_marker("img", Some(OsStr::new("")), &mut output)?;
        assert_eq!(output, [] as [u8; 0]);
        loaded.write_history_marker("img", Some(OsStr::new("session")), &mut output)?;
        let line = std::str::from_utf8(&output)?;
        let marker: Value = serde_json::from_str(
            line.strip_prefix("MFB_HISTORY_CONFIG=")
                .context("missing history marker prefix")?
                .trim_end_matches('\n'),
        )?;
        assert_eq!(marker["schema_version"], 1);
        assert_eq!(marker["pipeline"], "img");
        assert_eq!(marker["package_version"], env!("CARGO_PKG_VERSION"));
        assert_eq!(marker["config"]["performance"]["mode"], "tight");
        assert_eq!(marker["sources"]["performance.mode"], "CLI");
        assert_eq!(line.lines().count(), 1);
        Ok(())
    }

    #[test]
    fn history_marker_propagates_stdout_write_failure() -> Result<()> {
        struct BrokenOutput;
        impl Write for BrokenOutput {
            fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
                Err(std::io::Error::other("broken output"))
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let loaded = LoadedConfig {
            config: RuntimeConfig::default(),
            sources: BTreeMap::new(),
            source_chain: BTreeMap::new(),
        };
        let error = loaded
            .write_history_marker("vid", Some(OsStr::new("session")), &mut BrokenOutput)
            .err()
            .context("stdout error was not propagated")?;
        assert!(
            error
                .to_string()
                .contains("write effective runtime configuration")
        );
        Ok(())
    }

    #[test]
    fn file_layers_inherit_and_reject_invalid_values() -> Result<()> {
        let file = tempfile::NamedTempFile::new()?;
        let mut value = serde_json::to_value(RuntimeConfig::default())?;
        let mut sources = BTreeMap::new();
        let mut source_chain = BTreeMap::new();
        default_sources(&value, "", &mut sources, &mut source_chain);
        merge(
            &mut value,
            json!({"photos":{"native_batch_size":200}}),
            "env:MFB_PHOTOS_NATIVE_BATCH_SIZE",
            "",
            &mut sources,
            &mut source_chain,
        );
        std::fs::write(file.path(), br#"{"config_version":1,"photos":{"native_batch_size":250},"img":{"fallback_policy":"same-semantics"}}"#)?;
        apply_file(&mut value, file.path(), &mut sources, &mut source_chain)?;
        let parsed: RuntimeConfig = serde_json::from_value(value.clone())?;
        assert_eq!(parsed.photos.native_batch_size, 250);
        assert_eq!(parsed.photos.import_batch_size, 50);
        assert_eq!(parsed.img.fallback_policy, FallbackPolicy::SameSemantics);
        assert_eq!(sources["photos.import_batch_size"], "default");
        assert_eq!(
            sources["photos.native_batch_size"],
            file.path().display().to_string()
        );
        assert_eq!(
            source_chain["photos.native_batch_size"],
            vec![
                "default".to_owned(),
                "env:MFB_PHOTOS_NATIVE_BATCH_SIZE".to_owned(),
                file.path().display().to_string()
            ]
        );
        assert_eq!(source_chain["photos.import_batch_size"], ["default"]);

        let second = tempfile::NamedTempFile::new()?;
        std::fs::write(
            second.path(),
            br#"{"config_version":1,"photos":{"import_batch_size":25},"tools":{"paths":{"cjxl":"/bin/echo"}}}"#,
        )?;
        apply_file(&mut value, second.path(), &mut sources, &mut source_chain)?;
        let parsed: RuntimeConfig = serde_json::from_value(value.clone())?;
        assert_eq!(parsed.photos.native_batch_size, 250);
        assert_eq!(parsed.photos.import_batch_size, 25);
        assert_eq!(
            sources["photos.native_batch_size"],
            file.path().display().to_string()
        );
        assert_eq!(
            sources["photos.import_batch_size"],
            second.path().display().to_string()
        );
        assert_eq!(
            source_chain["photos.import_batch_size"],
            vec!["default".to_owned(), second.path().display().to_string()]
        );

        let third = tempfile::NamedTempFile::new()?;
        std::fs::write(
            third.path(),
            br#"{"config_version":1,"photos":{"native_batch_size":250},"tools":{"paths":{"cjxl":"/bin/echo"}}}"#,
        )?;
        apply_file(&mut value, third.path(), &mut sources, &mut source_chain)?;
        assert_eq!(
            source_chain["tools.paths.cjxl"],
            vec![
                second.path().display().to_string(),
                third.path().display().to_string()
            ]
        );
        assert_eq!(
            sources["tools.paths.cjxl"],
            third.path().display().to_string()
        );
        apply_file(&mut value, third.path(), &mut sources, &mut source_chain)?;
        let mut loaded = LoadedConfig {
            config: serde_json::from_value(value.clone())?,
            sources: sources.clone(),
            source_chain: source_chain.clone(),
        };
        loaded.config.photos.native_batch_size = 250;
        record_source(
            &mut loaded.sources,
            &mut loaded.source_chain,
            "photos.native_batch_size",
            "CLI",
        );
        assert_eq!(
            loaded.source_chain["photos.native_batch_size"],
            vec![
                "default".to_owned(),
                "env:MFB_PHOTOS_NATIVE_BATCH_SIZE".to_owned(),
                file.path().display().to_string(),
                third.path().display().to_string(),
                third.path().display().to_string(),
                "CLI".to_owned(),
            ]
        );
        assert_eq!(loaded.config.photos.native_batch_size, 250);

        record_source(
            &mut sources,
            &mut source_chain,
            "photos.import_batch_size",
            "CLI",
        );
        assert_eq!(
            source_chain["photos.import_batch_size"],
            vec![
                "default".to_owned(),
                second.path().display().to_string(),
                "CLI".to_owned()
            ]
        );
        assert_eq!(
            sources["photos.import_batch_size"],
            *source_chain["photos.import_batch_size"].last().unwrap()
        );
        assert_eq!(sources.len(), source_chain.len());
        assert!(sources.iter().all(|(key, source)| {
            source_chain.get(key).and_then(|chain| chain.last()) == Some(source)
        }));

        for invalid in [
            r#"{"config_version":2}"#,
            r#"{"config_version":1,"photos":{"unknown":1}}"#,
            r#"{"config_version":1,"vid":{"codec":"av2"}}"#,
            r#"{"config_version":1,"photos":{"native_batch_size":"large"}}"#,
            r#"{"config_version":1,"photos":{"verification_batch_size":0}}"#,
            r#"{"config_version":1,"photos":{"verification_batch_size":1001}}"#,
            r#"{"config_version":1,"photos":{"native_min_batch_size":1001}}"#,
            r#"{"config_version":1,"photos":{"native_max_batch_size":0}}"#,
            r#"{"config_version":1,"photos":{"target_batch_seconds":0}}"#,
            r#"{"config_version":1,"photos":{"adaptive_batching":true,"native_batch_size":1}}"#,
            r#"{"config_version":1,"photos":{"album_name":"bad/name"}}"#,
            r#"{"config_version":1,"tools":{"paths":{"bad/name":"/bin/echo"}}}"#,
            r#"{"config_version":1,"tools":{"paths":{"CJXL":"/bin/echo"}}}"#,
        ] {
            std::fs::write(file.path(), invalid)?;
            assert!(
                apply_file(
                    &mut value.clone(),
                    file.path(),
                    &mut sources.clone(),
                    &mut source_chain.clone()
                )
                .is_err(),
                "accepted {invalid}"
            );
        }
        Ok(())
    }
}
