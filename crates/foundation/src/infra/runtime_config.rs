//! Layered runtime preferences. Optional quality DB access is separate from
//! mandatory Photos custody verification.
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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ImgPolicy {
    pub allow_database: bool,
    pub quality_heuristic: bool,
    pub jpeg_effort: u8,
    pub fallback_policy: FallbackPolicy,
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
}

impl Default for ImgPolicy {
    fn default() -> Self {
        Self {
            allow_database: false,
            quality_heuristic: false,
            jpeg_effort: 11,
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

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ToolsPolicy {
    pub policy: ToolPolicy,
    pub paths: BTreeMap<String, PathBuf>,
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
        }
    }
}

impl RuntimeConfig {
    pub fn validate(&self) -> Result<()> {
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
            ensure!(
                !name.is_empty()
                    && name.chars().all(|ch| ch.is_ascii_lowercase()
                        || ch.is_ascii_digit()
                        || ch == '-'
                        || ch == '_'),
                "tools.paths has invalid tool name {name:?}"
            );
            ensure!(path.is_absolute(), "tools.paths.{name} must be absolute");
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
