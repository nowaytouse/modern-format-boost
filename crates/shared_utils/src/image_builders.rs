//! Type-safe builders for imaging tools (ImageMagick, webpmux, gifski, avifenc, sips, exiftool).

use crate::builder_base::ToolBuilder;
use crate::constants;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Builder for constructing `magick` (`ImageMagick`) commands.
#[derive(Debug, Default)]
pub struct MagickBuilder {
    input: Option<PathBuf>,
    output: Option<PathBuf>,
    strip: bool,
    depth: Option<u8>,
    colorspace: Option<String>,
    defines: Vec<(String, String)>,
    sets: Vec<(String, String)>,
    extra_args: Vec<String>,
    use_stdout: bool,
    format: Option<String>,
}

impl MagickBuilder {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn check_available() -> bool {
        Self::default().check_available()
    }

    pub fn input<P: AsRef<Path>>(&mut self, path: P) -> &mut Self {
        self.input = Some(path.as_ref().to_path_buf());
        self
    }

    pub fn output<P: AsRef<Path>>(&mut self, path: P) -> &mut Self {
        self.output = Some(path.as_ref().to_path_buf());
        self
    }

    pub const fn strip(&mut self, enabled: bool) -> &mut Self {
        self.strip = enabled;
        self
    }

    pub const fn depth(&mut self, depth: u8) -> &mut Self {
        self.depth = Some(depth);
        self
    }

    pub fn colorspace<S: AsRef<str>>(&mut self, colorspace: S) -> &mut Self {
        self.colorspace = Some(colorspace.as_ref().to_string());
        self
    }

    pub fn define<K, V>(&mut self, key: K, value: V) -> &mut Self
    where
        K: AsRef<str>,
        V: AsRef<str>,
    {
        self.defines
            .push((key.as_ref().to_string(), value.as_ref().to_string()));
        self
    }

    pub fn set<K, V>(&mut self, key: K, value: V) -> &mut Self
    where
        K: AsRef<str>,
        V: AsRef<str>,
    {
        self.sets
            .push((key.as_ref().to_string(), value.as_ref().to_string()));
        self
    }

    pub const fn use_stdout(&mut self, enabled: bool) -> &mut Self {
        self.use_stdout = enabled;
        self
    }

    pub fn format<S: AsRef<str>>(&mut self, format: S) -> &mut Self {
        self.format = Some(format.as_ref().to_string());
        self
    }

    pub fn arg<S: AsRef<str>>(&mut self, arg: S) -> &mut Self {
        self.extra_args.push(arg.as_ref().to_string());
        self
    }
}

impl ToolBuilder for MagickBuilder {
    fn get_command_name(&self) -> &str {
        constants::TOOL_MAGICK
    }

    /// Construct the `std::process::Command`.
    fn build(&self) -> Command {
        let mut cmd = self.get_resolved_command();

        if let Some(input) = &self.input {
            cmd.arg("--")
                .arg(crate::path_safety::magick_safe_path(input).as_ref());
        }

        for arg in &self.extra_args {
            cmd.arg(arg);
        }

        if self.strip {
            cmd.arg(constants::MAGICK_ARG_STRIP);
        }

        for (k, v) in &self.defines {
            cmd.arg(constants::MAGICK_ARG_DEFINE)
                .arg(format!("{k}={v}"));
        }

        for (k, v) in &self.sets {
            cmd.arg(constants::MAGICK_ARG_SET).arg(k).arg(v);
        }

        if let Some(cs) = &self.colorspace {
            cmd.arg(constants::MAGICK_ARG_COLORSPACE).arg(cs);
        }

        if let Some(d) = self.depth {
            cmd.arg(constants::MAGICK_ARG_DEPTH).arg(d.to_string());
        }

        if let Some(fmt) = &self.format {
            cmd.arg(constants::MAGICK_ARG_FORMAT).arg(fmt);
        }

        if self.use_stdout {
            if !self.extra_args.iter().any(|a| a.ends_with(":-")) {
                cmd.arg("png:-");
            }
            cmd.stdin(Stdio::null()); // Default for IM piped
            cmd.stdout(Stdio::piped());
        } else if let Some(output) = &self.output {
            cmd.arg(crate::safe_path_arg(output).as_ref());
        }

        cmd
    }
}

/// Builder for constructing `identify` (`ImageMagick`) commands.
#[derive(Debug, Default)]
pub struct IdentifyBuilder {
    input: Option<PathBuf>,
    format: Option<String>,
    extra_args: Vec<String>,
    use_magick: bool, // toggle between 'magick identify' and 'identify'
    verbose: bool,
}

impl IdentifyBuilder {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn check_available() -> bool {
        Self::default().check_available()
    }

    pub fn input<P: AsRef<Path>>(&mut self, path: P) -> &mut Self {
        self.input = Some(path.as_ref().to_path_buf());
        self
    }

    pub fn format<S: AsRef<str>>(&mut self, format: S) -> &mut Self {
        self.format = Some(format.as_ref().to_string());
        self
    }

    pub const fn use_magick(&mut self, enabled: bool) -> &mut Self {
        self.use_magick = enabled;
        self
    }

    /// Enables verbose output.
    pub const fn verbose(&mut self, enabled: bool) -> &mut Self {
        self.verbose = enabled;
        self
    }

    pub fn arg<S: AsRef<str>>(&mut self, arg: S) -> &mut Self {
        self.extra_args.push(arg.as_ref().to_string());
        self
    }
}

impl ToolBuilder for IdentifyBuilder {
    fn get_command_name(&self) -> &str {
        if self.use_magick {
            constants::TOOL_MAGICK
        } else {
            constants::TOOL_IDENTIFY
        }
    }

    fn get_check_args(&self) -> &[&str] {
        if self.use_magick {
            &[constants::MAGICK_ARG_IDENTIFY, constants::ARG_VERSION]
        } else {
            &[constants::ARG_VERSION]
        }
    }

    fn build(&self) -> Command {
        let mut cmd = self.get_resolved_command();
        if self.use_magick {
            cmd.arg(constants::MAGICK_ARG_IDENTIFY);
        }

        if let Some(fmt) = &self.format {
            cmd.arg(constants::MAGICK_ARG_FORMAT).arg(fmt);
        }

        if self.verbose {
            cmd.arg(constants::FFMPEG_ARG_VERBOSE);
        }

        for arg in &self.extra_args {
            cmd.arg(arg);
        }

        if let Some(input) = &self.input {
            cmd.arg(crate::safe_path_arg(input).as_ref());
        }

        cmd
    }

    fn check_available(&self) -> bool {
        let mut cmd = self.get_resolved_command();
        if self.use_magick {
            cmd.arg(constants::MAGICK_ARG_IDENTIFY);
        }
        cmd.arg(constants::ARG_VERSION);

        cmd.output().map_or_else(
            |_| {
                if self.use_magick {
                    false
                } else {
                    let path = crate::common_utils::resolve_tool_path(constants::TOOL_MAGICK)
                        .unwrap_or_else(|| std::path::PathBuf::from(constants::TOOL_MAGICK));
                    let mut fallback = Command::new(path);
                    fallback
                        .arg(constants::MAGICK_ARG_IDENTIFY)
                        .arg(constants::ARG_VERSION);
                    fallback.output().is_ok_and(|o| o.status.success())
                }
            },
            |o| o.status.success(),
        )
    }
}

/// Builder for constructing `webpmux` commands.
#[derive(Debug, Default)]
pub struct WebpmuxBuilder {
    output: Option<PathBuf>,
    frames: Vec<(PathBuf, u32, i32, i32, bool)>, // (path, duration, x, y, blend)
    loop_count: Option<u32>,
    bgcolor: Option<String>,
    info: bool,
    get_frame: Option<u32>,
    input: Option<PathBuf>,
    extra_args: Vec<String>,
}

impl WebpmuxBuilder {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn check_available() -> bool {
        Self::default().check_available()
    }

    pub fn input<P: AsRef<Path>>(&mut self, path: P) -> &mut Self {
        self.input = Some(path.as_ref().to_path_buf());
        self
    }

    pub fn output<P: AsRef<Path>>(&mut self, path: P) -> &mut Self {
        self.output = Some(path.as_ref().to_path_buf());
        self
    }

    pub const fn info(&mut self, enabled: bool) -> &mut Self {
        self.info = enabled;
        self
    }

    pub const fn get_frame(&mut self, index: u32) -> &mut Self {
        self.get_frame = Some(index);
        self
    }

    pub fn add_frame<P: AsRef<Path>>(
        &mut self,
        path: P,
        duration: u32,
        x: i32,
        y: i32,
        blend: bool,
    ) -> &mut Self {
        self.frames
            .push((path.as_ref().to_path_buf(), duration, x, y, blend));
        self
    }

    pub const fn loop_count(&mut self, count: u32) -> &mut Self {
        self.loop_count = Some(count);
        self
    }

    pub fn bgcolor<S: AsRef<str>>(&mut self, color: S) -> &mut Self {
        self.bgcolor = Some(color.as_ref().to_string());
        self
    }

    pub fn arg<S: AsRef<str>>(&mut self, arg: S) -> &mut Self {
        self.extra_args.push(arg.as_ref().to_string());
        self
    }
}

impl ToolBuilder for WebpmuxBuilder {
    fn get_command_name(&self) -> &str {
        constants::TOOL_WEBPMUX
    }

    /// Construct the `std::process::Command`.
    fn build(&self) -> Command {
        let mut cmd = self.get_resolved_command();

        if self.info {
            cmd.arg(constants::WEBPMUX_ARG_INFO);
        }

        if let Some(i) = self.get_frame {
            cmd.arg(constants::WEBPMUX_ARG_GET)
                .arg(constants::WEBPMUX_ARG_FRAME)
                .arg(i.to_string());
        }

        for arg in &self.extra_args {
            cmd.arg(arg);
        }

        if let Some(input) = &self.input {
            cmd.arg(crate::safe_path_arg(input).as_ref());
        }

        for (path, dur, x, y, blend) in &self.frames {
            let blend_str = if *blend { "+b" } else { "-b" };
            cmd.arg("-frame")
                .arg(crate::safe_path_arg(path).as_ref())
                .arg(format!("+{dur}+{x}+{y}+{blend_str}"));
        }

        if let Some(count) = self.loop_count {
            cmd.arg(constants::WEBPMUX_ARG_LOOP).arg(count.to_string());
        }

        if let Some(bg) = &self.bgcolor {
            cmd.arg(constants::WEBPMUX_ARG_BGCOLOR).arg(bg);
        }

        if let Some(output) = &self.output {
            cmd.arg(constants::WEBPMUX_ARG_OUTPUT)
                .arg(crate::safe_path_arg(output).as_ref());
        }

        cmd
    }
}

/// Builder for constructing `gifski` commands.
#[derive(Debug, Default)]
pub struct GifskiBuilder {
    output: Option<PathBuf>,
    inputs: Vec<PathBuf>,
    fps: Option<f32>,
    quality: Option<u8>,
    motion_quality: Option<u8>,
    lossy_quality: Option<u8>,
    width: Option<u32>,
    height: Option<u32>,
    repeat: Option<u32>,
    fast: bool,
    extra_args: Vec<String>,
}

impl GifskiBuilder {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn check_available() -> bool {
        Self::default().check_available()
    }

    pub fn output<P: AsRef<Path>>(&mut self, path: P) -> &mut Self {
        self.output = Some(path.as_ref().to_path_buf());
        self
    }

    pub fn add_input<P: AsRef<Path>>(&mut self, path: P) -> &mut Self {
        self.inputs.push(path.as_ref().to_path_buf());
        self
    }

    pub const fn fps(&mut self, fps: f32) -> &mut Self {
        self.fps = Some(fps);
        self
    }

    pub const fn quality(&mut self, quality: u8) -> &mut Self {
        self.quality = Some(quality);
        self
    }

    pub const fn motion_quality(&mut self, quality: u8) -> &mut Self {
        self.motion_quality = Some(quality);
        self
    }

    pub const fn lossy_quality(&mut self, quality: u8) -> &mut Self {
        self.lossy_quality = Some(quality);
        self
    }

    pub const fn dimensions(&mut self, width: u32, height: u32) -> &mut Self {
        self.width = Some(width);
        self.height = Some(height);
        self
    }

    pub const fn repeat(&mut self, repeat: u32) -> &mut Self {
        self.repeat = Some(repeat);
        self
    }

    pub const fn fast(&mut self, enabled: bool) -> &mut Self {
        self.fast = enabled;
        self
    }

    pub fn arg<S: AsRef<str>>(&mut self, arg: S) -> &mut Self {
        self.extra_args.push(arg.as_ref().to_string());
        self
    }
}

impl ToolBuilder for GifskiBuilder {
    fn get_command_name(&self) -> &str {
        constants::TOOL_GIFSKI
    }

    /// Construct the `std::process::Command`.
    fn build(&self) -> Command {
        let mut cmd = self.get_resolved_command();

        if let Some(fps) = self.fps {
            cmd.arg(constants::GIFSKI_ARG_FPS).arg(format!("{fps:.3}"));
        }

        if let Some(q) = self.quality {
            cmd.arg(constants::GIFSKI_ARG_QUALITY).arg(q.to_string());
        }

        if let Some(q) = self.motion_quality {
            cmd.arg(constants::GIFSKI_ARG_MOTION_QUALITY)
                .arg(q.to_string());
        }

        if let Some(q) = self.lossy_quality {
            cmd.arg(constants::GIFSKI_ARG_LOSSY_QUALITY)
                .arg(q.to_string());
        }

        if let (Some(w), Some(h)) = (self.width, self.height) {
            cmd.arg(constants::GIFSKI_ARG_WIDTH)
                .arg(w.to_string())
                .arg(constants::GIFSKI_ARG_HEIGHT)
                .arg(h.to_string());
        }

        if let Some(r) = self.repeat {
            cmd.arg(constants::GIFSKI_ARG_REPEAT).arg(r.to_string());
        }

        if self.fast {
            cmd.arg(constants::GIFSKI_ARG_FAST);
        }

        for arg in &self.extra_args {
            cmd.arg(arg);
        }

        if let Some(output) = &self.output {
            cmd.arg(constants::WEBPMUX_ARG_OUTPUT)
                .arg(crate::safe_path_arg(output).as_ref());
        }

        for input in &self.inputs {
            cmd.arg(crate::safe_path_arg(input).as_ref());
        }

        cmd
    }
}

/// Builder for constructing `avifenc` commands.
#[derive(Debug, Default)]
pub struct AvifencBuilder {
    input: Option<PathBuf>,
    output: Option<PathBuf>,
    min_quality: Option<u8>,
    max_quality: Option<u8>,
    lossless: bool,
    speed: Option<u8>,
    threads: Option<String>,
    depth: Option<u8>,
    yuv: Option<String>,
    extra_args: Vec<String>,
}

impl AvifencBuilder {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn check_available() -> bool {
        Self::default().check_available()
    }

    pub fn input<P: AsRef<Path>>(&mut self, path: P) -> &mut Self {
        self.input = Some(path.as_ref().to_path_buf());
        self
    }

    pub fn output<P: AsRef<Path>>(&mut self, path: P) -> &mut Self {
        self.output = Some(path.as_ref().to_path_buf());
        self
    }

    pub const fn speed(&mut self, speed: u8) -> &mut Self {
        self.speed = Some(speed);
        self
    }

    pub const fn quality(&mut self, min: u8, max: u8) -> &mut Self {
        self.min_quality = Some(min);
        self.max_quality = Some(max);
        self
    }

    pub const fn lossless(&mut self, enabled: bool) -> &mut Self {
        self.lossless = enabled;
        self
    }

    pub fn jobs(&mut self, jobs: &str) -> &mut Self {
        self.threads = Some(jobs.to_string());
        self
    }

    pub const fn depth(&mut self, depth: u8) -> &mut Self {
        self.depth = Some(depth);
        self
    }

    pub fn yuv<S: AsRef<str>>(&mut self, yuv: S) -> &mut Self {
        self.yuv = Some(yuv.as_ref().to_string());
        self
    }

    pub fn arg<S: AsRef<str>>(&mut self, arg: S) -> &mut Self {
        self.extra_args.push(arg.as_ref().to_string());
        self
    }
}

impl ToolBuilder for AvifencBuilder {
    fn get_command_name(&self) -> &str {
        constants::TOOL_AVIFENC
    }

    fn build(&self) -> Command {
        let mut cmd = self.get_resolved_command();

        if self.lossless {
            cmd.arg(constants::AVIFENC_ARG_LOSSLESS);
        }

        if let Some(s) = self.speed {
            cmd.arg(constants::AVIFENC_ARG_SPEED).arg(s.to_string());
        }

        if let Some(j) = &self.threads {
            cmd.arg(constants::AVIFENC_ARG_JOBS).arg(j);
        }

        if let Some(q) = self.max_quality {
            cmd.arg(constants::AVIFENC_ARG_MAX).arg(q.to_string());
        }

        if let Some(q) = self.min_quality {
            cmd.arg(constants::AVIFENC_ARG_MIN).arg(q.to_string());
        }

        if let Some(d) = self.depth {
            cmd.arg(constants::AVIFENC_ARG_DEPTH).arg(d.to_string());
        }

        if let Some(yuv) = &self.yuv {
            cmd.arg(constants::AVIFENC_ARG_YUV).arg(yuv);
        }

        for arg in &self.extra_args {
            cmd.arg(arg);
        }

        if let Some(input) = &self.input {
            cmd.arg(crate::safe_path_arg(input).as_ref());
        }

        if let Some(output) = &self.output {
            cmd.arg(crate::safe_path_arg(output).as_ref());
        }

        cmd
    }
}

/// Builder for constructing `sips` commands (macOS only).
#[derive(Debug, Default)]
pub struct SipsBuilder {
    input: Option<PathBuf>,
    output: Option<PathBuf>,
    format: Option<String>,
    quality: Option<u32>,
    extra_args: Vec<String>,
}

impl SipsBuilder {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn check_available() -> bool {
        Self::default().check_available()
    }

    pub fn input<P: AsRef<Path>>(&mut self, path: P) -> &mut Self {
        self.input = Some(path.as_ref().to_path_buf());
        self
    }

    pub fn output<P: AsRef<Path>>(&mut self, path: P) -> &mut Self {
        self.output = Some(path.as_ref().to_path_buf());
        self
    }

    pub fn format<S: AsRef<str>>(&mut self, format: S) -> &mut Self {
        self.format = Some(format.as_ref().to_string());
        self
    }
    /// Sets the output image quality (1-100).
    /// Values outside this range will be clamped to prevent sips errors.
    pub fn quality(&mut self, q: u32) -> &mut Self {
        self.quality = Some(q.clamp(1, 100));
        self
    }

    pub fn arg<S: AsRef<str>>(&mut self, arg: S) -> &mut Self {
        self.extra_args.push(arg.as_ref().to_string());
        self
    }
}

impl ToolBuilder for SipsBuilder {
    fn get_command_name(&self) -> &str {
        constants::TOOL_SIPS
    }

    fn get_check_args(&self) -> &[&str] {
        &[constants::ARG_V]
    }

    fn build(&self) -> Command {
        let mut cmd = self.get_resolved_command();

        if let Some(fmt) = &self.format {
            cmd.arg(constants::SIPS_ARG_S)
                .arg(constants::SIPS_ARG_FORMAT)
                .arg(fmt);
        }

        if let Some(q) = self.quality {
            cmd.arg(constants::SIPS_ARG_S)
                .arg(constants::SIPS_ARG_FORMAT_OPTIONS)
                .arg(q.to_string());
        }

        for arg in &self.extra_args {
            cmd.arg(arg);
        }

        if let Some(input) = &self.input {
            cmd.arg(crate::safe_path_arg(input).as_ref());
        }

        if let Some(output) = &self.output {
            cmd.arg(constants::SIPS_ARG_OUT)
                .arg(crate::safe_path_arg(output).as_ref());
        }

        cmd
    }

    fn check_available(&self) -> bool {
        #[cfg(target_os = "macos")]
        {
            self.get_resolved_command()
                .arg(constants::ARG_V)
                .output()
                .is_ok_and(|o| o.status.success())
        }
        #[cfg(not(target_os = "macos"))]
        {
            false
        }
    }
}

/// Builder for constructing `exiftool` commands.
#[derive(Debug, Default)]
pub struct ExiftoolBuilder {
    input: Option<PathBuf>,
    args: Vec<String>,
    overwrite_original: bool,
    use_stdin: bool,
}

impl ExiftoolBuilder {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn check_available() -> bool {
        Self::default().check_available()
    }

    pub fn input<P: AsRef<Path>>(&mut self, path: P) -> &mut Self {
        self.input = Some(path.as_ref().to_path_buf());
        self
    }

    pub fn tags_from_file<P: AsRef<Path>>(&mut self, path: P) -> &mut Self {
        self.arg(constants::EXIFTOOL_ARG_TAGS_FROM_FILE);
        // ExifTool format-interprets % in tagsfromfile argument, so we must double them.
        self.arg(crate::path_safety::property_safe_path(path.as_ref()));
        self
    }

    pub fn arg<S: AsRef<str>>(&mut self, arg: S) -> &mut Self {
        self.args.push(arg.as_ref().to_string());
        self
    }

    pub fn args<I, S>(&mut self, args: I) -> &mut Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        for arg in args {
            self.args.push(arg.as_ref().to_string());
        }
        self
    }

    pub const fn overwrite_original(&mut self) -> &mut Self {
        self.overwrite_original = true;
        self
    }

    pub fn extract_icc_profile(&mut self) -> &mut Self {
        self.arg(constants::EXIFTOOL_ARG_ICC_PROFILE)
            .arg(constants::EXIFTOOL_ARG_B);
        self
    }

    pub const fn use_stdin(&mut self) -> &mut Self {
        self.use_stdin = true;
        self
    }

    /// Strips all metadata tags from the file.
    /// Equivalent to `exiftool -all=`.
    pub fn strip_all(&mut self) -> &mut Self {
        self.arg(constants::EXIFTOOL_ARG_ALL);
        self
    }

    /// Ignores minor errors and warnings.
    /// Equivalent to `exiftool -m`.
    pub fn ignore_minor(&mut self) -> &mut Self {
        self.arg(constants::EXIFTOOL_ARG_M);
        self
    }

    /// Suppresses warnings and logs.
    /// Equivalent to `exiftool -q`. Call twice for absolute silence.
    pub fn quiet(&mut self) -> &mut Self {
        self.arg(constants::EXIFTOOL_ARG_Q);
        self
    }

    /// Preserves file modification date/time.
    /// Equivalent to `exiftool -P`.
    pub fn preserve_date(&mut self) -> &mut Self {
        self.arg(constants::EXIFTOOL_ARG_P);
        self
    }

    /// Allows processing of 'unsafe' tags.
    /// Equivalent to `exiftool -unsafe`.
    pub fn unsafe_tags(&mut self) -> &mut Self {
        self.arg(constants::EXIFTOOL_ARG_UNSAFE);
        self
    }
}

impl ToolBuilder for ExiftoolBuilder {
    fn get_command_name(&self) -> &str {
        constants::TOOL_EXIFTOOL
    }

    fn get_check_args(&self) -> &[&str] {
        &[constants::ARG_VER]
    }

    fn build(&self) -> Command {
        let mut cmd = self.get_resolved_command();
        if self.overwrite_original {
            cmd.arg(constants::EXIFTOOL_ARG_OVERWRITE_ORIGINAL);
        }

        for arg in &self.args {
            cmd.arg(arg);
        }

        if let Some(input) = &self.input {
            cmd.arg(crate::safe_path_arg(input).as_ref());
        }

        if self.use_stdin {
            cmd.stdin(std::process::Stdio::piped());
        }

        cmd
    }
}

/// Builder for constructing `dwebp` commands.
#[derive(Debug, Default)]
pub struct DwebpBuilder {
    input: Option<PathBuf>,
    output: Option<PathBuf>,
    extra_args: Vec<String>,
}

impl DwebpBuilder {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn check_available() -> bool {
        Self::default().check_available()
    }

    pub fn input<P: AsRef<Path>>(&mut self, path: P) -> &mut Self {
        self.input = Some(path.as_ref().to_path_buf());
        self
    }

    pub fn output<P: AsRef<Path>>(&mut self, path: P) -> &mut Self {
        self.output = Some(path.as_ref().to_path_buf());
        self
    }

    pub fn arg<S: AsRef<str>>(&mut self, arg: S) -> &mut Self {
        self.extra_args.push(arg.as_ref().to_string());
        self
    }
}

impl ToolBuilder for DwebpBuilder {
    fn get_command_name(&self) -> &str {
        constants::TOOL_DWEBP
    }

    fn build(&self) -> Command {
        let mut cmd = self.get_resolved_command();

        if let Some(input) = &self.input {
            cmd.arg(crate::safe_path_arg(input).as_ref());
        }

        for arg in &self.extra_args {
            cmd.arg(arg);
        }

        if let Some(output) = &self.output {
            cmd.arg(constants::WEBPMUX_ARG_OUTPUT)
                .arg(crate::safe_path_arg(output).as_ref());
        }

        cmd
    }
}
