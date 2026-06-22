//! Shared CI contract helpers for integration tests (metadata, probe, video).

/// True when running under GitHub Actions / generic `CI=true`.
#[must_use]
pub fn is_ci() -> bool {
    std::env::var_os("CI").is_some()
}

/// Panics in CI when `tool` is not on `PATH`; no-op locally.
///
/// # Panics
/// Panics when `CI` is set and `tool` is not on `PATH`.
pub fn require_tool_on_path(tool: &str, contract_label: &str) {
    if !is_ci() {
        return;
    }
    assert!(
        which::which(tool).is_ok(),
        "CONTRACT: CI requires {tool} ({contract_label})"
    );
}

/// Returns false when `exiftool` is missing; panics in CI instead of silent skip.
///
/// # Panics
/// Panics when `CI` is set and `exiftool` is not on `PATH`.
#[must_use]
pub fn exiftool_available_or_ci_panic() -> bool {
    let available = which::which(crate::constants::TOOL_EXIFTOOL).is_ok();
    if is_ci() {
        assert!(
            available,
            "CONTRACT: CI requires exiftool for metadata delivery tests"
        );
    }
    available
}

/// Panics in CI when `ImageMagick` is not available.
///
/// # Panics
/// Panics when `CI` is set and neither `magick` nor `convert` is on `PATH`.
pub fn require_imagemagick_in_ci(contract_label: &str) {
    if !is_ci() {
        return;
    }
    assert!(
        crate::MagickBuilder::check_available(),
        "CONTRACT: CI requires ImageMagick (magick or convert) ({contract_label})"
    );
}

/// Panics in CI when `ffmpeg` / `ffprobe` are missing.
///
/// # Panics
/// Panics when `CI` is set and `ffmpeg` or `ffprobe` is not on `PATH`.
pub fn require_ffmpeg_toolchain_in_ci(contract_label: &str) {
    if !is_ci() {
        return;
    }
    require_tool_on_path(crate::constants::TOOL_FFMPEG, contract_label);
    require_tool_on_path(crate::constants::TOOL_FFPROBE, contract_label);
}

/// Panics in CI when `libx265` is not exposed by the installed `ffmpeg`.
///
/// # Panics
/// Panics when `CI` is set and `ffmpeg` cannot be executed, or when `libx265` is unavailable.
pub fn require_libx265_encoder_in_ci(contract_label: &str) {
    if !is_ci() {
        return;
    }
    let output = std::process::Command::new(crate::constants::TOOL_FFMPEG)
        .args(["-hide_banner", "-h", "encoder=libx265"])
        .output()
        .unwrap_or_else(|err| panic!("CONTRACT: cannot inspect libx265 ({contract_label}): {err}"));
    assert!(
        output.status.success(),
        "CONTRACT: CI requires ffmpeg libx265 encoder ({contract_label})"
    );
}
