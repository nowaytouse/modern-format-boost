//! Type-safe builders for imaging tools (ImageMagick, webpmux, gifski, avifenc, sips, exiftool).

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use crate::constants;

/// Builder for constructing `magick` (ImageMagick) commands.
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

    pub fn input<P: AsRef<Path>>(&mut self, path: P) -> &mut Self {
        self.input = Some(path.as_ref().to_path_buf());
        self
    }

    pub fn output<P: AsRef<Path>>(&mut self, path: P) -> &mut Self {
        self.output = Some(path.as_ref().to_path_buf());
        self
    }

    pub fn strip(&mut self, enabled: bool) -> &mut Self {
        self.strip = enabled;
        self
    }

    pub fn depth(&mut self, depth: u8) -> &mut Self {
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
        V: AsRef<str>
    {
        self.defines.push((key.as_ref().to_string(), value.as_ref().to_string()));
        self
    }

    pub fn set<K, V>(&mut self, key: K, value: V) -> &mut Self 
    where 
        K: AsRef<str>,
        V: AsRef<str>
    {
        self.sets.push((key.as_ref().to_string(), value.as_ref().to_string()));
        self
    }

    pub fn use_stdout(&mut self, enabled: bool) -> &mut Self {
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

    /// Construct the `std::process::Command`.
    #[must_use]
    pub fn build(&self) -> Command {
        let mut cmd = Command::new(constants::TOOL_MAGICK);

        if let Some(input) = &self.input {
            cmd.arg("--").arg(crate::safe_path_arg(input).as_ref());
        }

        for arg in &self.extra_args {
            cmd.arg(arg);
        }

        if self.strip {
            cmd.arg(constants::MAGICK_ARG_STRIP);
        }

        for (k, v) in &self.defines {
            cmd.arg(constants::MAGICK_ARG_DEFINE).arg(format!("{}={}", k, v));
        }

        for (k, v) in &self.sets {
            cmd.arg(constants::MAGICK_ARG_SET).arg(k).arg(v);
        }

        if let Some(cs) = &self.colorspace {
            cmd.arg("-colorspace").arg(cs);
        }

        if let Some(d) = self.depth {
            cmd.arg(constants::MAGICK_ARG_DEPTH).arg(d.to_string());
        }

        if let Some(fmt) = &self.format {
            cmd.arg("-format").arg(fmt);
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

    #[must_use]
    pub fn check_available() -> bool {
        Command::new(constants::TOOL_MAGICK).arg("-version").output().map(|o| o.status.success()).unwrap_or(false)
    }
}

/// Builder for constructing `identify` (ImageMagick) commands.
#[derive(Debug, Default)]
pub struct IdentifyBuilder {
    input: Option<PathBuf>,
    format: Option<String>,
    extra_args: Vec<String>,
    use_magick: bool, // toggle between 'magick identify' and 'identify'
}

impl IdentifyBuilder {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn input<P: AsRef<Path>>(&mut self, path: P) -> &mut Self {
        self.input = Some(path.as_ref().to_path_buf());
        self
    }

    pub fn format<S: AsRef<str>>(&mut self, format: S) -> &mut Self {
        self.format = Some(format.as_ref().to_string());
        self
    }

    pub fn use_magick(&mut self, enabled: bool) -> &mut Self {
        self.use_magick = enabled;
        self
    }

    pub fn arg<S: AsRef<str>>(&mut self, arg: S) -> &mut Self {
        self.extra_args.push(arg.as_ref().to_string());
        self
    }

    #[must_use]
    pub fn build(&self) -> Command {
        let mut cmd = if self.use_magick {
            let mut c = Command::new(constants::TOOL_MAGICK);
            c.arg("identify");
            c
        } else {
            Command::new(constants::TOOL_IDENTIFY)
        };

        if let Some(fmt) = &self.format {
            cmd.arg("-format").arg(fmt);
        }

        for arg in &self.extra_args {
            cmd.arg(arg);
        }

        if let Some(input) = &self.input {
            cmd.arg(crate::safe_path_arg(input).as_ref());
        }

        cmd
    }

    #[must_use]
    pub fn check_available() -> bool {
        Command::new(constants::TOOL_IDENTIFY).arg("-version").output().map(|o| o.status.success()).unwrap_or_else(|_| {
            Command::new(constants::TOOL_MAGICK).arg("identify").arg("-version").output().map(|o| o.status.success()).unwrap_or(false)
        })
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

    pub fn input<P: AsRef<Path>>(&mut self, path: P) -> &mut Self {
        self.input = Some(path.as_ref().to_path_buf());
        self
    }

    pub fn output<P: AsRef<Path>>(&mut self, path: P) -> &mut Self {
        self.output = Some(path.as_ref().to_path_buf());
        self
    }

    pub fn info(&mut self, enabled: bool) -> &mut Self {
        self.info = enabled;
        self
    }

    pub fn get_frame(&mut self, index: u32) -> &mut Self {
        self.get_frame = Some(index);
        self
    }

    pub fn add_frame<P: AsRef<Path>>(&mut self, path: P, duration: u32, x: i32, y: i32, blend: bool) -> &mut Self {
        self.frames.push((path.as_ref().to_path_buf(), duration, x, y, blend));
        self
    }

    pub fn loop_count(&mut self, count: u32) -> &mut Self {
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

    /// Construct the `std::process::Command`.
    #[must_use]
    pub fn build(&self) -> Command {
        let mut cmd = Command::new(constants::TOOL_WEBPMUX);

        if self.info {
            cmd.arg("-info");
        }

        if let Some(i) = self.get_frame {
            cmd.arg("-get").arg("frame").arg(i.to_string());
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
            cmd.arg("-loop").arg(count.to_string());
        }

        if let Some(bg) = &self.bgcolor {
            cmd.arg("-bgcolor").arg(bg);
        }

        for arg in &self.extra_args {
            cmd.arg(arg);
        }

        if let Some(output) = &self.output {
            cmd.arg("-o").arg(crate::safe_path_arg(output).as_ref());
        }

        cmd
    }

    #[must_use]
    pub fn check_available() -> bool {
        Command::new(constants::TOOL_WEBPMUX).arg("-version").output().map(|o| o.status.success()).unwrap_or(false)
    }
}

/// Builder for constructing `gifski` commands.
#[derive(Debug, Default)]
pub struct GifskiBuilder {
    output: Option<PathBuf>,
    inputs: Vec<PathBuf>,
    input_pattern: Option<String>,
    fps: Option<f32>,
    quality: Option<u8>,
    motion_quality: Option<u8>,
    lossy_quality: Option<u8>,
    width: Option<u32>,
    height: Option<u32>,
    repeat: Option<u32>,
    extra_args: Vec<String>,
}

impl GifskiBuilder {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn output<P: AsRef<Path>>(&mut self, path: P) -> &mut Self {
        self.output = Some(path.as_ref().to_path_buf());
        self
    }

    pub fn add_input<P: AsRef<Path>>(&mut self, path: P) -> &mut Self {
        self.inputs.push(path.as_ref().to_path_buf());
        self
    }

    pub fn input_pattern<S: AsRef<str>>(&mut self, pattern: S) -> &mut Self {
        self.input_pattern = Some(pattern.as_ref().to_string());
        self
    }

    pub fn fps(&mut self, fps: f32) -> &mut Self {
        self.fps = Some(fps);
        self
    }

    pub fn quality(&mut self, quality: u8) -> &mut Self {
        self.quality = Some(quality);
        self
    }

    pub fn motion_quality(&mut self, quality: u8) -> &mut Self {
        self.motion_quality = Some(quality);
        self
    }

    pub fn lossy_quality(&mut self, quality: u8) -> &mut Self {
        self.lossy_quality = Some(quality);
        self
    }

    pub fn dimensions(&mut self, width: u32, height: u32) -> &mut Self {
        self.width = Some(width);
        self.height = Some(height);
        self
    }

    pub fn repeat(&mut self, repeat: u32) -> &mut Self {
        self.repeat = Some(repeat);
        self
    }

    pub fn arg<S: AsRef<str>>(&mut self, arg: S) -> &mut Self {
        self.extra_args.push(arg.as_ref().to_string());
        self
    }

    /// Construct the `std::process::Command`.
    #[must_use]
    pub fn build(&self) -> Command {
        let mut cmd = Command::new(constants::TOOL_GIFSKI);

        if let Some(output) = &self.output {
            cmd.arg("-o").arg(crate::safe_path_arg(output).as_ref());
        }

        if let Some(fps) = self.fps {
            cmd.arg("--fps").arg(format!("{fps:.3}"));
        }

        if let Some(q) = self.quality {
            cmd.arg("--quality").arg(q.to_string());
        }

        if let Some(q) = self.motion_quality {
            cmd.arg("--motion-quality").arg(q.to_string());
        }

        if let Some(q) = self.lossy_quality {
            cmd.arg("--lossy-quality").arg(q.to_string());
        }

        if let (Some(w), Some(h)) = (self.width, self.height) {
            cmd.arg("--width").arg(w.to_string()).arg("--height").arg(h.to_string());
        }

        if let Some(r) = self.repeat {
            cmd.arg("--repeat").arg(r.to_string());
        }

        for arg in &self.extra_args {
            cmd.arg(arg);
        }

        if let Some(pattern) = &self.input_pattern {
            cmd.arg(pattern);
        }

        for input in &self.inputs {
            cmd.arg(crate::safe_path_arg(input).as_ref());
        }

        cmd
    }

    #[must_use]
    pub fn check_available() -> bool {
        Command::new(constants::TOOL_GIFSKI).arg("--version").output().map(|o| o.status.success()).unwrap_or(false)
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

    pub fn input<P: AsRef<Path>>(&mut self, path: P) -> &mut Self {
        self.input = Some(path.as_ref().to_path_buf());
        self
    }

    pub fn output<P: AsRef<Path>>(&mut self, path: P) -> &mut Self {
        self.output = Some(path.as_ref().to_path_buf());
        self
    }

    pub fn speed(&mut self, speed: u8) -> &mut Self {
        self.speed = Some(speed);
        self
    }

    pub fn quality(&mut self, min: u8, max: u8) -> &mut Self {
        self.min_quality = Some(min);
        self.max_quality = Some(max);
        self
    }

    pub fn lossless(&mut self, enabled: bool) -> &mut Self {
        self.lossless = enabled;
        self
    }

    pub fn jobs(&mut self, jobs: &str) -> &mut Self {
        self.threads = Some(jobs.to_string());
        self
    }

    pub fn depth(&mut self, depth: u8) -> &mut Self {
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

    #[must_use]
    pub fn build(&self) -> Command {
        let mut cmd = Command::new("avifenc");

        if self.lossless {
            cmd.arg("--lossless");
        }

        if let Some(s) = self.speed {
            cmd.arg("--speed").arg(s.to_string());
        }

        if let Some(j) = &self.threads {
            cmd.arg("-j").arg(j);
        }

        if let Some(q) = self.min_quality {
            cmd.arg("--min").arg(q.to_string());
        }

        if let Some(q) = self.max_quality {
            cmd.arg("--max").arg(q.to_string());
        }

        if let Some(d) = self.depth {
            cmd.arg("--depth").arg(d.to_string());
        }

        if let Some(yuv) = &self.yuv {
            cmd.arg("--yuv").arg(yuv);
        }

        for arg in &self.extra_args {
            cmd.arg(arg);
        }

        if self.input.is_some() || self.output.is_some() {
            cmd.arg("--");
        }

        if let Some(input) = &self.input {
            cmd.arg(crate::safe_path_arg(input).as_ref());
        }

        if let Some(output) = &self.output {
            cmd.arg(crate::safe_path_arg(output).as_ref());
        }

        cmd
    }

    #[must_use]
    pub fn check_available() -> bool {
        Command::new("avifenc").arg("--version").output().map(|o| o.status.success()).unwrap_or(false)
    }
}

/// Builder for constructing `sips` commands (macOS only).
#[derive(Debug, Default)]
pub struct SipsBuilder {
    input: Option<PathBuf>,
    output: Option<PathBuf>,
    format: Option<String>,
    extra_args: Vec<String>,
}

impl SipsBuilder {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
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

    pub fn arg<S: AsRef<str>>(&mut self, arg: S) -> &mut Self {
        self.extra_args.push(arg.as_ref().to_string());
        self
    }

    #[must_use]
    pub fn build(&self) -> Command {
        let mut cmd = Command::new(constants::TOOL_SIPS);

        if let Some(fmt) = &self.format {
            cmd.arg("-s").arg("format").arg(fmt);
        }

        for arg in &self.extra_args {
            cmd.arg(arg);
        }

        if let Some(input) = &self.input {
            cmd.arg(crate::safe_path_arg(input).as_ref());
        }

        if let Some(output) = &self.output {
            cmd.arg("--out").arg(crate::safe_path_arg(output).as_ref());
        }

        cmd
    }

    #[must_use]
    pub fn check_available() -> bool {
        #[cfg(target_os = "macos")]
        {
            Command::new(constants::TOOL_SIPS).arg("-v").output().map(|o| o.status.success()).unwrap_or(false)
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

    pub fn input<P: AsRef<Path>>(&mut self, path: P) -> &mut Self {
        self.input = Some(path.as_ref().to_path_buf());
        self
    }

    pub fn tags_from_file<P: AsRef<Path>>(&mut self, path: P) -> &mut Self {
        self.arg("-tagsfromfile");
        self.arg(crate::path_safety::property_safe_path(path.as_ref()).to_string());
        self
    }

    pub fn arg<S: AsRef<str>>(&mut self, arg: S) -> &mut Self {
        self.args.push(arg.as_ref().to_string());
        self
    }

    pub fn args<I, S>(&mut self, args: I) -> &mut Self 
    where 
        I: IntoIterator<Item = S>,
        S: AsRef<str>
    {
        for arg in args {
            self.args.push(arg.as_ref().to_string());
        }
        self
    }

    pub fn overwrite_original(&mut self) -> &mut Self {
        self.overwrite_original = true;
        self
    }

    pub fn extract_icc_profile(&mut self) -> &mut Self {
        self.arg("-icc_profile").arg("-b");
        self
    }

    pub fn use_stdin(&mut self) -> &mut Self {
        self.use_stdin = true;
        self
    }

    #[must_use]
    pub fn build(&self) -> Command {
        let mut cmd = Command::new("exiftool");

        if self.overwrite_original {
            cmd.arg("-overwrite_original");
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

    #[must_use]
    pub fn check_available() -> bool {
        Command::new("exiftool").arg("-ver").output().map(|o| o.status.success()).unwrap_or(false)
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

    #[must_use]
    pub fn build(&self) -> Command {
        let mut cmd = Command::new(crate::constants::TOOL_DWEBP);

        if let Some(input) = &self.input {
            cmd.arg(crate::safe_path_arg(input).as_ref());
        }

        for arg in &self.extra_args {
            cmd.arg(arg);
        }

        if let Some(output) = &self.output {
            cmd.arg("-o").arg(crate::safe_path_arg(output).as_ref());
        }

        cmd
    }

    #[must_use]
    pub fn check_available() -> bool {
        Command::new(crate::constants::TOOL_DWEBP).arg("-version").output().map(|o| o.status.success()).unwrap_or(false)
    }
}
