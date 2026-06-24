//! Delivery strategy SSOT — **`img` static stills** vs **`vid`
//! video/animated**.
//!
//! ## Product boundary (no cross-pipeline relay)
//!
//! | Product | Scope | `--codec hevc\|av1` meaning |
//! |---------|-------|------------------------------|
//! | **`img`** | **Static stills only** | **`hevc`** (default) → **JXL** batch; **`av1`** → **AVIF** still strategy. Animated assets are **ignored** (never forwarded to `vid`). |
//! | **`vid`** | **Video + animated raster** | **HEVC** / **AV1** video delivery (`SelectedCodec`). |
//!
//! Same flag names, **different semantics per binary**. `img` does **not**
//! invoke `vid` and does **not** pass work to `vid`.
//!
//! ## Layers (do not conflate)
//!
//! | Layer | Module | Responsibility |
//! |-------|--------|----------------|
//! | **Img static delivery** | [`crate::delivery_codec_strategy::ImgStaticDelivery`] | JXL vs AVIF (`--codec hevc` / `av1` on `img`) |
//! | **Video delivery codec** | [`crate::conversion_types::SelectedCodec`] | HEVC / AV1 GPU explore, containers (`vid` only) |
//! | **CRF search mode** | [`crate::explore_strategy`] | `ExploreMode` — not HEVC vs AV1 |
//! | **Static API strategy** | `img::determine_strategy` | JXL / AVIF for `smart_convert()` |
//! | **Video container path** | `vid::determine_strategy_with_apple_compat` | Skip/GIF/loop + `delivery_target` |
//!
//! ## HEVC vs AV1 policy (tightened)
//!
//! | Policy | HEVC (default) | AV1 |
//! |--------|----------------|-----|
//! | CLI default | yes | `--codec av1` |
//! | Container | MP4 (`hev1`) or MOV (`hvc1` + `--apple-compat`) | MP4 (`av01`) only |
//! | Apple compat | supported | **rejected** (fail-closed) |
//! | Encoder | libx265 / GPU HEVC; x265-params + HDR merge | libsvtav1 / GPU AV1; no x265 |
//! | Ultimate explore | search preset → slower final HEVC encode | same preset search + final |
//! | Animated lossless preset | `slower` when ultimate else `medium` | same SVT numeric window (`2` / `6`) |
//! | CRF mapping | `calculate_hevc_crf` / `CrfMapping::hevc` | `calculate_av1_crf` / `CrfMapping::av1` |
//! | Warm-start hint | `get_global_last_hit_crf_hevc` | `get_global_last_hit_crf_av1` |
//! | Lossless archival MKV | yes (lossless sources) | **fail-closed** (skip + message; use `--codec hevc`) |

use anyhow::{Result, bail};
use std::path::PathBuf;

use crate::conversion::{ConvertFlags, ConvertOptions};
use crate::conversion_types::{ConversionConfig, SelectedCodec, TargetVideoFormat};
use crate::quality_matcher::{EncoderType, MatchedQuality, QualityAnalysis};
use crate::video_explorer::gpu_coarse_search::{
    GpuSearchFeatures, GpuSearchFlags, GpuSearchRequest, GpuSearchValidation, explore_av1_with_gpu,
    explore_hevc_with_gpu,
};
use crate::video_explorer::{ExploreResult, VideoEncoder};

/// Committed default video delivery codec (HEVC).
pub const DEFAULT_DELIVERY_CODEC: SelectedCodec = SelectedCodec::Hevc;

/// Default static still delivery on `img run` (JXL).
pub const DEFAULT_IMG_STATIC_DELIVERY: ImgStaticDelivery = ImgStaticDelivery::Jxl;

/// `img run --codec` semantics (static still only — **not** video HEVC/AV1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImgStaticDelivery {
    /// CLI `hevc` → primary **JXL** batch path (default).
    Jxl,
    /// CLI `av1` → **AVIF** still strategy for applicable lossy assets (not
    /// video AV1).
    Avif,
}

impl ImgStaticDelivery {
    /// Parse `img run --codec` (`hevc` → JXL, `av1` → AVIF).
    ///
    /// # Errors
    /// Returns an error when the label is not `hevc`/`av1` (or accepted
    /// aliases).
    pub fn parse_cli_label(label: &str) -> Result<Self, String> {
        match label.trim().to_ascii_lowercase().as_str() {
            "hevc" | "h265" | "x265" | "jxl" => Ok(Self::Jxl),
            "av1" | "av01" | "avif" => Ok(Self::Avif),
            other => Err(format!(
                "unsupported img --codec '{other}'; use hevc (JXL, default) or av1 (AVIF stills)"
            )),
        }
    }

    /// Fail-closed validation for `img run` flags.
    ///
    /// # Errors
    /// Returns an error when AVIF static strategy is combined with
    /// `--apple-compat`.
    pub fn validate_img_flags(self, apple_compat: bool) -> Result<()> {
        if self == Self::Avif && apple_compat {
            bail!(
                "AVIF static strategy (--codec av1) does not use --apple-compat; that flag is for \
                 vid video delivery"
            );
        }
        Ok(())
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Jxl => "hevc",
            Self::Avif => "av1",
        }
    }

    #[must_use]
    pub const fn static_format_label(self) -> &'static str {
        match self {
            Self::Jxl => "JXL",
            Self::Avif => "AVIF",
        }
    }
}

/// Parse and validate `img run --codec`.
///
/// # Errors
/// Returns an error when the label is unsupported or flags are incompatible.
pub fn resolve_cli_img_static_delivery(
    label: &str,
    apple_compat: bool,
) -> Result<ImgStaticDelivery> {
    let delivery = ImgStaticDelivery::parse_cli_label(label).map_err(anyhow::Error::msg)?;
    delivery.validate_img_flags(apple_compat)?;
    Ok(delivery)
}

/// Startup log line for `img run`.
#[must_use]
pub fn img_run_routing_summary(delivery: ImgStaticDelivery) -> String {
    format!(
        "img static-only: confirmed stills→{}; animated/unverified animatable→ignore",
        delivery.static_format_label()
    )
}

/// CLI surface for **`vid run` only** (video HEVC/AV1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliveryProduct {
    Vid,
}

/// One-line routing hint for CLI startup (`vid`).
#[must_use]
pub fn vid_run_routing_summary(codec: SelectedCodec) -> String {
    format!(
        "per-file: loop_intent→GIF? → skip rules → {} delivery (lossless archival→{} MKV today)",
        codec.delivery_label_prefix(),
        SelectedCodec::Hevc.delivery_label_prefix()
    )
}

/// File IO toggles for a `vid run` invocation.
#[derive(Debug, Clone, Copy, Default)]
pub struct RunDeliveryIoFlags {
    pub force: bool,
    pub delete_original: bool,
    pub in_place: bool,
}

/// Quality / explore toggles for a `vid run` invocation.
#[derive(Debug, Clone, Copy, Default)]
pub struct RunDeliveryQualityFlags {
    pub explore: bool,
    pub match_quality: bool,
    pub compress: bool,
}

// Encoder quality-mode bits for a `vid run` invocation.
bitflags::bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
    pub struct EncoderModeFlags: u8 {
        const ULTIMATE = 1 << 0;
        const ARCHIVE = 1 << 1;
    }
}

#[derive(Debug, Clone, Copy, Default)]
/// Encoder / GPU toggles for a `vid run` invocation.
pub struct RunDeliveryEncoderFlags {
    pub apple_compat: bool,
    pub use_gpu: bool,
    pub modes: EncoderModeFlags,
}

/// Run flags for `vid` when building [`ConvertOptions`].
#[derive(Debug, Clone)]
pub struct RunDeliveryFlags {
    pub io: RunDeliveryIoFlags,
    pub quality: RunDeliveryQualityFlags,
    pub encoder: RunDeliveryEncoderFlags,
    pub allow_size_tolerance: bool,
    pub verbose: bool,
    pub output_dir: Option<PathBuf>,
    pub base_dir: Option<PathBuf>,
    pub codec: SelectedCodec,
    pub child_threads: usize,
    pub input_format: Option<String>,
    pub quality_label: Option<String>,
}

impl RunDeliveryFlags {
    #[must_use]
    pub fn from_conversion_config(config: &ConversionConfig) -> Self {
        Self {
            io: RunDeliveryIoFlags {
                force: config.force(),
                delete_original: config.delete_original(),
                in_place: config.in_place(),
            },
            quality: RunDeliveryQualityFlags {
                explore: config.explore_smaller(),
                match_quality: config.match_quality(),
                compress: config.require_compression(),
            },
            encoder: RunDeliveryEncoderFlags {
                apple_compat: config.apple_compat(),
                use_gpu: config.use_gpu(),
                modes: EncoderModeFlags::empty()
                    | if config.ultimate_mode() {
                        EncoderModeFlags::ULTIMATE
                    } else {
                        EncoderModeFlags::empty()
                    }
                    | if config.archive_mode() {
                        EncoderModeFlags::ARCHIVE
                    } else {
                        EncoderModeFlags::empty()
                    },
            },
            allow_size_tolerance: config.allow_size_tolerance(),
            verbose: crate::progress_mode::is_verbose_mode(),
            output_dir: config.output_dir.clone(),
            base_dir: config.base_dir.clone(),
            codec: config.codec,
            child_threads: config.child_threads,
            input_format: None,
            quality_label: None,
        }
    }
}

/// Map run flags to shared [`ConvertOptions`] (vid video / animated raster
/// paths only).
#[must_use]
pub fn build_video_convert_options(flags: &RunDeliveryFlags) -> ConvertOptions {
    let convert_flags = ConvertFlags::empty()
        | if flags.io.force {
            ConvertFlags::FORCE
        } else {
            ConvertFlags::empty()
        }
        | if flags.io.delete_original {
            ConvertFlags::DELETE_ORIGINAL
        } else {
            ConvertFlags::empty()
        }
        | if flags.io.in_place {
            ConvertFlags::IN_PLACE
        } else {
            ConvertFlags::empty()
        }
        | if flags.quality.explore {
            ConvertFlags::EXPLORE
        } else {
            ConvertFlags::empty()
        }
        | if flags.quality.match_quality {
            ConvertFlags::MATCH_QUALITY
        } else {
            ConvertFlags::empty()
        }
        | if flags.encoder.apple_compat {
            ConvertFlags::APPLE_COMPAT
        } else {
            ConvertFlags::empty()
        }
        | if flags.quality.compress {
            ConvertFlags::COMPRESS
        } else {
            ConvertFlags::empty()
        }
        | if flags.encoder.use_gpu {
            ConvertFlags::USE_GPU
        } else {
            ConvertFlags::empty()
        }
        | if flags.encoder.modes.contains(EncoderModeFlags::ULTIMATE) {
            ConvertFlags::ULTIMATE
        } else {
            ConvertFlags::empty()
        }
        | if flags.encoder.modes.contains(EncoderModeFlags::ARCHIVE) {
            ConvertFlags::ARCHIVE
        } else {
            ConvertFlags::empty()
        }
        | if flags.allow_size_tolerance {
            ConvertFlags::ALLOW_SIZE_TOLERANCE
        } else {
            ConvertFlags::empty()
        }
        | if flags.verbose {
            ConvertFlags::VERBOSE
        } else {
            ConvertFlags::empty()
        };

    ConvertOptions {
        flags: convert_flags,
        output_dir: flags.output_dir.clone(),
        base_dir: flags.base_dir.clone(),
        codec: flags.codec,
        child_threads: flags.child_threads,
        input_format: flags.input_format.clone(),
        quality_label: flags.quality_label.clone(),
    }
}

/// `FFmpeg` video encoder tuple for animated lossless intermediate (CRF 0)
/// encodes.
#[derive(Debug, Clone)]
pub struct AnimatedFfmpegVideoSpec {
    pub v_codec: &'static str,
    pub v_tag: &'static str,
    pub params_flag: &'static str,
    pub params: String,
    pub preset: &'static str,
}

impl SelectedCodec {
    /// Parse `--codec` label (`hevc` default, `av1` optional).
    ///
    /// # Errors
    /// Returns an error when the label is not `hevc` or `av1` (or accepted
    /// aliases).
    pub fn parse_cli_label(label: &str) -> Result<Self, String> {
        match label.trim().to_ascii_lowercase().as_str() {
            "hevc" | "h265" | "x265" => Ok(Self::Hevc),
            "av1" | "av01" => Ok(Self::Av1),
            other => Err(format!(
                "unsupported delivery codec '{other}'; use hevc (default) or av1"
            )),
        }
    }

    /// Parse `--codec` for **`vid run`** (video HEVC/AV1). Use
    /// [`resolve_cli_img_static_delivery`] on `img`.
    ///
    /// # Errors
    /// Returns an error when the label is unsupported or delivery flags are
    /// incompatible.
    pub fn resolve_cli_delivery_codec(
        _product: DeliveryProduct,
        label: &str,
        apple_compat: bool,
    ) -> Result<Self> {
        let codec = Self::parse_cli_label(label).map_err(anyhow::Error::msg)?;
        codec.validate_delivery_flags(apple_compat)?;
        Ok(codec)
    }

    /// One-line policy summary for run logs.
    #[must_use]
    pub const fn delivery_policy_summary(self) -> &'static str {
        match self {
            Self::Hevc => {
                "HEVC: MP4/MOV (hvc1 apple-compat), libx265/GPU, ultimate search→slower final, HDR \
                 x265 merge"
            }
            Self::Av1 => {
                "AV1: MP4 av01 only, libsvtav1/GPU, no apple-compat, SVT single-preset explore"
            }
            Self::Av2 | Self::Vvc => "experimental codec (not implemented)",
        }
    }

    /// Map delivery codec to GPU coarse-search [`VideoEncoder`].
    #[must_use]
    pub const fn video_encoder(self) -> Option<VideoEncoder> {
        match self {
            Self::Hevc => Some(VideoEncoder::Hevc),
            Self::Av1 => Some(VideoEncoder::Av1),
            Self::Av2 | Self::Vvc => None,
        }
    }

    /// Fail-closed validation before strategy or explore (AV1 + Apple compat is
    /// illegal).
    ///
    /// # Errors
    /// Returns an error when AV1 is combined with `--apple-compat` or the codec
    /// is experimental.
    pub fn validate_delivery_flags(self, apple_compat: bool) -> Result<()> {
        if self == Self::Av1 && apple_compat {
            bail!(
                "AV1 strategy does not support Apple compatibility; remove --apple-compat or use \
                 --codec hevc"
            );
        }
        if self.is_experimental() {
            bail!(
                "{} delivery is not implemented; use --codec hevc or av1",
                self.as_str().to_uppercase()
            );
        }
        Ok(())
    }

    /// Fail-closed guard before routing lossless sources to archival MKV.
    ///
    /// # Errors
    /// Returns an error when `lossless` is requested for a codec that cannot
    /// archival-deliver.
    pub fn validate_lossless_archival_delivery(self, lossless: bool) -> Result<()> {
        if lossless && !self.supports_lossless_archival_mkv() {
            bail!(
                "lossless archival MKV requires --codec hevc; {} does not support lossless \
                 archival delivery",
                self.delivery_label_prefix()
            );
        }
        Ok(())
    }

    /// Whether this codec may use lossless archival MKV for lossless sources.
    #[must_use]
    pub const fn supports_lossless_archival_mkv(self) -> bool {
        matches!(self, Self::Hevc)
    }

    /// Output container target for lossy delivery.
    ///
    /// For `lossless == true`, only [`SelectedCodec::Hevc`] is valid; callers
    /// must use `supports_lossless_archival_mkv` or
    /// `validate_lossless_archival_delivery` first.
    #[must_use]
    pub const fn delivery_target(self, apple_compat: bool, lossless: bool) -> TargetVideoFormat {
        if lossless {
            debug_assert!(
                self.supports_lossless_archival_mkv(),
                "lossless archival MKV requires HEVC; non-HEVC must fail-closed before \
                 delivery_target"
            );
            return TargetVideoFormat::HevcLosslessMkv;
        }
        match self {
            Self::Hevc => {
                if apple_compat {
                    TargetVideoFormat::HevcMov
                } else {
                    TargetVideoFormat::HevcMp4
                }
            }
            Self::Av1 => TargetVideoFormat::Av1Mp4,
            Self::Av2 => TargetVideoFormat::Av2Mp4,
            Self::Vvc => TargetVideoFormat::VvcMp4,
        }
    }

    #[must_use]
    pub const fn delivery_label_prefix(self) -> &'static str {
        match self {
            Self::Hevc => "HEVC",
            Self::Av1 => "AV1",
            Self::Av2 => "AV2",
            Self::Vvc => "VVC",
        }
    }

    #[must_use]
    pub fn warm_start_crf_hint(self) -> Option<f64> {
        match self {
            Self::Hevc => crate::crf_constants::get_global_last_hit_crf_hevc(),
            Self::Av1 => crate::crf_constants::get_global_last_hit_crf_av1(),
            Self::Av2 | Self::Vvc => None,
        }
    }

    #[must_use]
    pub const fn cpu_encoder_name(self) -> &'static str {
        match self {
            Self::Hevc => crate::constants::LIB_X265,
            Self::Av1 => crate::constants::LIB_SVTAV1,
            Self::Av2 => crate::constants::LIB_AV2,
            Self::Vvc => crate::constants::LIB_VVENC,
        }
    }

    #[must_use]
    pub const fn quality_encoder_type(self) -> Option<EncoderType> {
        match self {
            Self::Hevc => Some(EncoderType::Hevc),
            Self::Av1 => Some(EncoderType::Av1),
            Self::Av2 | Self::Vvc => None,
        }
    }

    /// Predicted CRF from a built [`QualityAnalysis`] for this delivery codec.
    ///
    /// # Errors
    /// Returns an error when delivery flags are invalid or CRF calculation
    /// fails.
    pub fn calculate_crf_from_quality_analysis(
        self,
        analysis: &QualityAnalysis,
    ) -> Result<MatchedQuality> {
        self.validate_delivery_flags(false)?;
        match self {
            Self::Hevc => crate::calculate_hevc_crf(analysis).map_err(anyhow::Error::msg),
            Self::Av1 => crate::calculate_av1_crf(analysis).map_err(anyhow::Error::msg),
            Self::Av2 | Self::Vvc => {
                bail!(
                    "{} CRF calculation not implemented",
                    self.as_str().to_uppercase()
                );
            }
        }
    }

    /// Persist a successful explore hit for warm-start (codec-specific global
    /// hint).
    pub fn record_global_crf_hit(self, crf: f32) {
        if crf <= 0.0 {
            return;
        }
        match self {
            Self::Hevc => {
                crate::crf_constants::update_global_last_hit_crf_hevc(f64::from(crf));
            }
            Self::Av1 => {
                crate::crf_constants::update_global_last_hit_crf_av1(f64::from(crf));
            }
            Self::Av2 | Self::Vvc => {}
        }
    }

    /// Animated raster lossless intermediate encode (CRF 0) `FFmpeg` knobs.
    ///
    /// # Errors
    /// Returns an error when delivery flags are invalid or the codec is not
    /// implemented.
    pub fn animated_lossless_ffmpeg_video_spec(
        self,
        apple_compat: bool,
        ultimate: bool,
        archive: bool,
        max_threads: usize,
    ) -> Result<AnimatedFfmpegVideoSpec> {
        self.validate_delivery_flags(apple_compat)?;
        match self {
            Self::Hevc => Ok(AnimatedFfmpegVideoSpec {
                v_codec: crate::constants::FFMPEG_ENCODER_X265,
                v_tag: if apple_compat {
                    crate::constants::FFMPEG_TAG_HVC1
                } else {
                    crate::constants::FFMPEG_TAG_HEV1
                },
                params_flag: "-x265-params",
                params: format!("log-level=error:pools={max_threads}"),
                preset: if archive {
                    crate::constants::FFMPEG_PRESET_VERYSLOW
                } else if ultimate {
                    crate::constants::FFMPEG_PRESET_SLOWER
                } else {
                    crate::constants::FFMPEG_PRESET_MEDIUM
                },
            }),
            Self::Av1 => Ok(AnimatedFfmpegVideoSpec {
                v_codec: "libsvtav1",
                v_tag: "av01",
                params_flag: "-svtav1-params",
                params: format!("tune=0:film-grain=0:lp={max_threads}"),
                preset: if archive {
                    crate::constants::FFMPEG_SVTAV1_SLOWEST_PRESET
                } else if ultimate {
                    crate::constants::FFMPEG_SVTAV1_SLOWER_PRESET
                } else {
                    crate::constants::FFMPEG_SVTAV1_DEFAULT_PRESET
                },
            }),
            Self::Av2 | Self::Vvc => {
                bail!(
                    "{} encoding not implemented for animated raster",
                    self.as_str().to_uppercase()
                );
            }
        }
    }

    /// Dispatch GPU coarse-search explore (HEVC dual-preset ultimate vs AV1
    /// SVT-AV1).
    ///
    /// # Errors
    /// Returns an error when flags are invalid or exploration fails.
    pub fn explore_with_gpu(self, req: &GpuSearchRequest) -> Result<ExploreResult> {
        self.validate_delivery_flags(req.flags.features.apple_compat)?;
        match self {
            Self::Hevc => explore_hevc_with_gpu(req),
            Self::Av1 => explore_av1_with_gpu(req),
            Self::Av2 | Self::Vvc => {
                bail!("{} encoding not implemented", self.as_str().to_uppercase());
            }
        }
    }
}

/// Build [`GpuSearchFlags`] with AV1 never inheriting `apple_compat` (HEVC-only
/// tag path).
#[must_use]
pub const fn gpu_search_flags_for_codec(
    codec: SelectedCodec,
    features: GpuSearchFeatures,
    validation: GpuSearchValidation,
) -> GpuSearchFlags {
    GpuSearchFlags {
        features: GpuSearchFeatures {
            ultimate_mode: features.ultimate_mode,
            apple_compat: features.apple_compat && matches!(codec, SelectedCodec::Hevc),
            archive_mode: features.archive_mode,
        },
        validation,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_cli_defaults_to_hevc_family() {
        assert_eq!(
            SelectedCodec::parse_cli_label("HEVC").unwrap(),
            SelectedCodec::Hevc
        );
        assert_eq!(
            SelectedCodec::parse_cli_label("av1").unwrap(),
            SelectedCodec::Av1
        );
        assert!(SelectedCodec::parse_cli_label("vp9").is_err());
    }

    #[test]
    fn av1_rejects_apple_compat() {
        assert!(SelectedCodec::Av1.validate_delivery_flags(true).is_err());
        assert!(SelectedCodec::Av1.validate_delivery_flags(false).is_ok());
    }

    #[test]
    fn av1_rejects_lossless_archival_mkv() {
        assert!(
            SelectedCodec::Av1
                .validate_lossless_archival_delivery(true)
                .is_err()
        );
        assert!(
            SelectedCodec::Hevc
                .validate_lossless_archival_delivery(true)
                .is_ok()
        );
    }

    #[test]
    fn img_codec_labels_map_to_static_delivery_not_video() {
        assert_eq!(
            resolve_cli_img_static_delivery("hevc", false).unwrap(),
            ImgStaticDelivery::Jxl
        );
        assert_eq!(
            resolve_cli_img_static_delivery("av1", false).unwrap(),
            ImgStaticDelivery::Avif
        );
        assert!(resolve_cli_img_static_delivery("av1", true).is_err());
    }

    #[test]
    fn vid_codec_is_video_delivery() {
        assert_eq!(
            SelectedCodec::resolve_cli_delivery_codec(DeliveryProduct::Vid, "av1", false).unwrap(),
            SelectedCodec::Av1
        );
    }

    #[test]
    fn vid_honors_av1() {
        let c =
            SelectedCodec::resolve_cli_delivery_codec(DeliveryProduct::Vid, "av1", false).unwrap();
        assert_eq!(c, SelectedCodec::Av1);
        let c =
            SelectedCodec::resolve_cli_delivery_codec(DeliveryProduct::Vid, "hevc", false).unwrap();
        assert_eq!(c, SelectedCodec::Hevc);
    }

    #[test]
    fn gpu_flags_strip_apple_compat_for_av1() {
        let features = GpuSearchFeatures {
            ultimate_mode: false,
            apple_compat: true,
            archive_mode: false,
        };
        let validation = GpuSearchValidation {
            force_ms_ssim_long: false,
            allow_size_tolerance: false,
        };
        let flags = gpu_search_flags_for_codec(SelectedCodec::Av1, features, validation);
        assert!(!flags.features.apple_compat);
        let flags = gpu_search_flags_for_codec(SelectedCodec::Hevc, features, validation);
        assert!(flags.features.apple_compat);
    }

    #[test]
    fn animated_lossless_av1_preset_tracks_ultimate_like_hevc() {
        let hevc_std = SelectedCodec::Hevc
            .animated_lossless_ffmpeg_video_spec(false, false, false, 4)
            .unwrap();
        let hevc_ult = SelectedCodec::Hevc
            .animated_lossless_ffmpeg_video_spec(false, true, false, 4)
            .unwrap();
        let av1_std = SelectedCodec::Av1
            .animated_lossless_ffmpeg_video_spec(false, false, false, 4)
            .unwrap();
        let av1_ult = SelectedCodec::Av1
            .animated_lossless_ffmpeg_video_spec(false, true, false, 4)
            .unwrap();

        assert_eq!(hevc_std.preset, crate::constants::FFMPEG_PRESET_MEDIUM);
        assert_eq!(hevc_ult.preset, crate::constants::FFMPEG_PRESET_SLOWER);
        assert_eq!(
            av1_std.preset,
            crate::constants::FFMPEG_SVTAV1_DEFAULT_PRESET
        );
        assert_eq!(
            av1_ult.preset,
            crate::constants::FFMPEG_SVTAV1_SLOWER_PRESET
        );
    }

    #[test]
    fn animated_lossless_archive_mode_hard_overrides_video_presets() {
        let hevc_archive = SelectedCodec::Hevc
            .animated_lossless_ffmpeg_video_spec(false, false, true, 4)
            .unwrap();
        let av1_archive = SelectedCodec::Av1
            .animated_lossless_ffmpeg_video_spec(false, false, true, 4)
            .unwrap();

        assert_eq!(
            hevc_archive.preset,
            crate::constants::FFMPEG_PRESET_VERYSLOW
        );
        assert_eq!(
            av1_archive.preset,
            crate::constants::FFMPEG_SVTAV1_SLOWEST_PRESET
        );
    }
}
