//! Type-safe builder for constructing `ffmpeg` and `ffprobe` commands.

use crate::constants;
use crate::ffmpeg_process::FfmpegProcess;
pub use crate::types::EncoderPreset;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::str::FromStr;

/// Common video codecs supported by the system.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoCodec {
    H264,
    Hevc,
    Av1,
    Vp9,
    Png,
    Apng,
    Copy,
    None,
}

#[derive(Debug, Default, Clone)]
struct FfmpegParams {
    x265: Option<String>,
    tag_video: Option<String>,
}

#[derive(Debug, Clone)]
enum OutputTarget {
    Path(PathBuf),
    Pipe,
}

impl VideoCodec {
    #[must_use]
    pub const fn ffmpeg_name(&self, is_gpu: bool) -> &'static str {
        match self {
            Self::H264 => {
                if is_gpu {
                    "h264_videotoolbox"
                } else {
                    constants::FFMPEG_ENCODER_X264
                }
            }
            Self::Hevc => {
                if is_gpu {
                    "hevc_videotoolbox"
                } else {
                    constants::FFMPEG_ENCODER_X265
                }
            }
            Self::Av1 => constants::FFMPEG_ENCODER_SVTAV1,
            Self::Vp9 => "libvpx-vp9",
            Self::Png => "png",
            Self::Apng => "apng",
            Self::Copy => "copy",
            Self::None => "none",
        }
    }
}

impl FromStr for VideoCodec {
    type Err = ();

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "libx264" | "h264" => Ok(Self::H264),
            "libx265" | "x265" | "hevc" => Ok(Self::Hevc),
            "libsvtav1" | "av1" => Ok(Self::Av1),
            "libvpx-vp9" | "vp9" => Ok(Self::Vp9),
            "png" => Ok(Self::Png),
            "apng" => Ok(Self::Apng),
            "copy" => Ok(Self::Copy),
            "" => Ok(Self::None),
            _ => Err(()),
        }
    }
}

/// `FFmpeg` video profiles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoProfile {
    Main,
    Main10,
    High,
    High10,
}

impl VideoProfile {
    #[must_use]
    pub const fn ffmpeg_name(&self) -> &'static str {
        match self {
            Self::Main => "main",
            Self::Main10 => "main10",
            Self::High => "high",
            Self::High10 => "high10",
        }
    }
}

impl FromStr for VideoProfile {
    type Err = ();

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "main" => Ok(Self::Main),
            "main10" => Ok(Self::Main10),
            "high" => Ok(Self::High),
            "high10" => Ok(Self::High10),
            _ => Err(()),
        }
    }
}

/// Common pixel formats.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PixFmt {
    Yuv420p,
    Yuv420p10le,
    Yuv444p,
    Yuv444p10le,
    Rgb24,
    Rgba,
    Rgb48le,
    Gray,
}

impl PixFmt {
    #[must_use]
    pub const fn ffmpeg_name(&self) -> &'static str {
        match self {
            Self::Yuv420p => "yuv420p",
            Self::Yuv420p10le => "yuv420p10le",
            Self::Yuv444p => "yuv444p",
            Self::Yuv444p10le => "yuv444p10le",
            Self::Rgb24 => "rgb24",
            Self::Rgba => "rgba",
            Self::Rgb48le => "rgb48le",
            Self::Gray => "gray",
        }
    }
}

impl FromStr for PixFmt {
    type Err = ();

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "yuv420p" => Ok(Self::Yuv420p),
            "yuv420p10le" => Ok(Self::Yuv420p10le),
            "yuv444p" => Ok(Self::Yuv444p),
            "yuv444p10le" => Ok(Self::Yuv444p10le),
            "rgb24" => Ok(Self::Rgb24),
            "rgba" => Ok(Self::Rgba),
            "rgb48le" => Ok(Self::Rgb48le),
            "gray" => Ok(Self::Gray),
            _ => Err(()),
        }
    }
}

/// Types of media streams for selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamType {
    Video,
    Audio,
    Subtitle,
    Data,
    Attachments,
}

impl StreamType {
    #[must_use]
    pub const fn ffmpeg_name(&self) -> &'static str {
        match self {
            Self::Video => "v",
            Self::Audio => "a",
            Self::Subtitle => "s",
            Self::Data => "d",
            Self::Attachments => "t",
        }
    }
}

/// Builder for constructing `ffmpeg` commands.
#[derive(Debug, Default, Clone)]
pub struct FfmpegBuilder {
    inputs: Vec<PathBuf>,
    output_target: Option<OutputTarget>,
    vcodec: Option<VideoCodec>,
    input_format: Option<String>,
    format: Option<String>,
    frames_v: Option<u32>,
    crf: Option<f32>,
    preset: Option<EncoderPreset>,
    profile: Option<VideoProfile>,
    pix_fmt: Option<PixFmt>,
    threads: Option<usize>,
    overwrite: bool,
    hide_banner: bool,
    loglevel: Option<String>,
    map: Vec<String>,
    filter_complex: Option<String>,
    input_args: Vec<String>,
    extra_args: Vec<String>,
    is_gpu: bool,
    params: FfmpegParams,
    odd_dim_correction: bool,
}

impl FfmpegBuilder {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn input<P: AsRef<Path>>(&mut self, path: P) -> &mut Self {
        self.inputs.push(path.as_ref().to_path_buf());
        self
    }

    pub fn output<P: AsRef<Path>>(&mut self, path: P) -> &mut Self {
        self.output_target = Some(OutputTarget::Path(path.as_ref().to_path_buf()));
        self
    }

    /// Emit the `FFmpeg` output to `-` (stdout/pipe target).
    pub fn output_pipe(&mut self) -> &mut Self {
        self.output_target = Some(OutputTarget::Pipe);
        self
    }

    pub const fn vcodec(&mut self, codec: VideoCodec) -> &mut Self {
        self.vcodec = Some(codec);
        self
    }

    pub fn vcodec_str<S: AsRef<str>>(&mut self, codec: S) -> &mut Self {
        if let Ok(c) = VideoCodec::from_str(codec.as_ref()) {
            self.vcodec = Some(c);
        } else {
            self.extra_args.push("-c:v".to_string());
            self.extra_args.push(codec.as_ref().to_string());
        }
        self
    }

    pub fn codec_video<S: AsRef<str>>(&mut self, codec: S) -> &mut Self {
        self.vcodec_str(codec)
    }

    pub fn codec_v<S: AsRef<str>>(&mut self, codec: S) -> &mut Self {
        self.vcodec_str(codec)
    }

    pub fn codec_audio<S: AsRef<str>>(&mut self, codec: S) -> &mut Self {
        self.extra_args.push("-c:a".to_string());
        self.extra_args.push(codec.as_ref().to_string());
        self
    }

    pub fn input_format<S: AsRef<str>>(&mut self, format: S) -> &mut Self {
        self.input_format = Some(format.as_ref().to_string());
        self
    }

    pub fn format<S: AsRef<str>>(&mut self, format: S) -> &mut Self {
        self.format = Some(format.as_ref().to_string());
        self
    }

    pub const fn frames_v(&mut self, count: u32) -> &mut Self {
        self.frames_v = Some(count);
        self
    }

    pub const fn crf(&mut self, crf: f32) -> &mut Self {
        self.crf = Some(crf);
        self
    }

    pub const fn preset(&mut self, preset: EncoderPreset) -> &mut Self {
        self.preset = Some(preset);
        self
    }

    pub const fn profile(&mut self, profile: VideoProfile) -> &mut Self {
        self.profile = Some(profile);
        self
    }

    pub const fn pix_fmt(&mut self, pix_fmt: PixFmt) -> &mut Self {
        self.pix_fmt = Some(pix_fmt);
        self
    }

    pub fn pix_fmt_str<S: AsRef<str>>(&mut self, fmt: S) -> &mut Self {
        self.extra_args.push("-pix_fmt".to_string());
        self.extra_args.push(fmt.as_ref().to_string());
        self
    }

    pub const fn threads(&mut self, threads: usize) -> &mut Self {
        self.threads = Some(threads);
        self
    }

    pub const fn overwrite(&mut self) -> &mut Self {
        self.overwrite = true;
        self
    }

    pub const fn overwrite_bool(&mut self, overwrite: bool) -> &mut Self {
        self.overwrite = overwrite;
        self
    }

    pub fn map<S: AsRef<str>>(&mut self, map: S) -> &mut Self {
        self.map.push(map.as_ref().to_string());
        self
    }

    pub fn map_stream(&mut self, stream_type: StreamType) -> &mut Self {
        self.map.push(stream_type.ffmpeg_name().to_string());
        self
    }

    pub fn map_stream_index(&mut self, stream_type: StreamType, index: usize) -> &mut Self {
        self.map
            .push(format!("{}:{}", stream_type.ffmpeg_name(), index));
        self
    }

    pub const fn use_gpu(&mut self, use_gpu: bool) -> &mut Self {
        self.is_gpu = use_gpu;
        self
    }

    pub fn filter_complex<S: AsRef<str>>(&mut self, filter: S) -> &mut Self {
        self.filter_complex = Some(filter.as_ref().to_string());
        self
    }

    pub fn filter_lavfi<S: AsRef<str>>(&mut self, filter: S) -> &mut Self {
        self.filter_complex = Some(filter.as_ref().to_string());
        self
    }

    pub const fn hide_banner(&mut self) -> &mut Self {
        self.hide_banner = true;
        self
    }

    pub fn loglevel<S: AsRef<str>>(&mut self, level: S) -> &mut Self {
        self.loglevel = Some(level.as_ref().to_string());
        self
    }

    pub fn arg<S: AsRef<str>>(&mut self, arg: S) -> &mut Self {
        self.extra_args.push(arg.as_ref().to_string());
        self
    }

    pub fn input_arg<S: AsRef<str>>(&mut self, arg: S) -> &mut Self {
        self.input_args.push(arg.as_ref().to_string());
        self
    }

    pub fn x265_params<S: AsRef<str>>(&mut self, params: S) -> &mut Self {
        self.params.x265 = Some(params.as_ref().to_string());
        self
    }

    pub fn tag_video<S: AsRef<str>>(&mut self, tag: S) -> &mut Self {
        self.params.tag_video = Some(tag.as_ref().to_string());
        self
    }

    /// Automatically corrects odd dimensions by scaling to floor(w/2)*2.
    /// Prevents filtergraph crashes with SSIM/VMAF on odd-sized inputs.
    pub const fn with_odd_dim_correction(&mut self) -> &mut Self {
        self.odd_dim_correction = true;
        self
    }

    pub fn args<I, S>(&mut self, args: I) -> &mut Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        for arg in args {
            self.extra_args.push(arg.as_ref().to_string());
        }
        self
    }

    fn assert_output_target(&self) {
        assert!(
            self.output_target.is_some(),
            "FFmpeg output target is required; use output(...) or output_pipe() before build()"
        );
    }

    /// Construct the `std::process::Command`.
    #[must_use]
    pub fn build(&self) -> Command {
        self.assert_output_target();

        let mut cmd = Command::new(constants::TOOL_FFMPEG);

        if self.overwrite {
            cmd.arg(constants::FFMPEG_ARG_OVERWRITE);
        }

        if self.hide_banner {
            cmd.arg(constants::FFMPEG_ARG_HIDE_BANNER);
        }

        if let Some(level) = &self.loglevel {
            cmd.arg(constants::FFMPEG_ARG_LOG_LEVEL).arg(level);
        }

        if let Some(threads) = self.threads {
            cmd.arg(constants::FFMPEG_ARG_THREADS);
            cmd.arg(threads.to_string());
        }

        for i_arg in &self.input_args {
            cmd.arg(i_arg);
        }

        if let Some(fmt) = &self.input_format {
            cmd.arg("-f").arg(fmt);
        }

        for input in &self.inputs {
            cmd.arg(constants::FFMPEG_ARG_INPUT);
            cmd.arg(crate::safe_path_arg(input).as_ref());
        }

        if let Some(mut filter) = self.filter_complex.clone() {
            if self.odd_dim_correction {
                // Prepend scaling to align dimensions - standard fix for filter compatibility
                filter = format!("scale=trunc(iw/2)*2:trunc(ih/2)*2,{filter}");
            }
            cmd.arg(constants::FFMPEG_ARG_FILTER_COMPLEX).arg(filter);
        } else if self.odd_dim_correction {
            cmd.arg(constants::FFMPEG_ARG_FILTER_COMPLEX)
                .arg("scale=trunc(iw/2)*2:trunc(ih/2)*2");
        }

        if let Some(vcodec) = self.vcodec {
            cmd.arg(constants::FFMPEG_ARG_CODEC_VIDEO);
            cmd.arg(vcodec.ffmpeg_name(self.is_gpu));
        }

        if let Some(crf) = self.crf {
            cmd.arg(constants::FFMPEG_ARG_CRF);
            cmd.arg(crf.to_string());
        }

        if let Some(preset) = self.preset {
            cmd.arg(constants::FFMPEG_ARG_PRESET);
            if matches!(self.vcodec, Some(VideoCodec::Av1)) {
                cmd.arg(preset.svtav1_preset().to_string());
            } else if matches!(self.vcodec, Some(VideoCodec::Hevc)) {
                cmd.arg(preset.hevc_name());
            } else {
                cmd.arg(preset.x26x_name());
            }
        }

        if let Some(profile) = self.profile {
            cmd.arg("-profile:v");
            cmd.arg(profile.ffmpeg_name());
        }

        if let Some(pix_fmt) = self.pix_fmt {
            cmd.arg(constants::FFMPEG_ARG_PIX_FMT);
            cmd.arg(pix_fmt.ffmpeg_name());
        }

        if let Some(fmt) = &self.format {
            cmd.arg("-f").arg(fmt);
        }

        if let Some(frames) = self.frames_v {
            cmd.arg("-frames:v").arg(frames.to_string());
        }

        for m in &self.map {
            cmd.arg("-map").arg(m);
        }

        if let Some(p) = &self.params.x265 {
            cmd.arg(constants::FFMPEG_ARG_X265_PARAMS).arg(p);
        }

        if let Some(t) = &self.params.tag_video {
            cmd.arg(constants::FFMPEG_ARG_TAG_VIDEO).arg(t);
        }

        for arg in &self.extra_args {
            cmd.arg(arg);
        }

        match &self.output_target {
            Some(OutputTarget::Pipe) => {
                cmd.arg("-");
            }
            Some(OutputTarget::Path(output)) => {
                cmd.arg(crate::safe_path_arg(output).as_ref());
            }
            None => unreachable!("output target is asserted above"),
        }

        cmd
    }

    /// Spawn the command and return an `FfmpegProcess` for monitoring.
    /// Spawn the `FFmpeg` process.
    ///
    /// # Errors
    /// Returns an error if the process fails to start.
    pub fn spawn(&self) -> anyhow::Result<FfmpegProcess> {
        let mut cmd = self.build();
        FfmpegProcess::spawn(&mut cmd)
    }
}

/// Builder for constructing `ffprobe` commands.
#[derive(Debug, Default)]
pub struct FfprobeBuilder {
    input: Option<PathBuf>,
    show_streams: bool,
    show_format: bool,
    show_frames: bool,
    show_entries: Option<String>,
    select_streams: Option<String>,
    print_format: Option<String>,
    read_intervals: Option<String>,
    count_frames: bool,
    loglevel: Option<String>,
    pattern_type: Option<String>,
    extra_args: Vec<String>,
}

impl FfprobeBuilder {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn input<P: AsRef<Path>>(&mut self, path: P) -> &mut Self {
        self.input = Some(path.as_ref().to_path_buf());
        self
    }

    pub const fn show_streams(&mut self) -> &mut Self {
        self.show_streams = true;
        self
    }

    pub const fn show_format(&mut self) -> &mut Self {
        self.show_format = true;
        self
    }

    pub fn show_entries<S: AsRef<str>>(&mut self, entries: S) -> &mut Self {
        self.show_entries = Some(entries.as_ref().to_string());
        self
    }

    pub fn select_streams(&mut self, stream_type: StreamType) -> &mut Self {
        self.select_streams = Some(stream_type.ffmpeg_name().to_string());
        self
    }

    pub fn select_stream(&mut self, stream_type: StreamType, index: usize) -> &mut Self {
        self.select_streams = Some(format!("{}:{}", stream_type.ffmpeg_name(), index));
        self
    }

    pub fn select_streams_custom<S: AsRef<str>>(&mut self, select: S) -> &mut Self {
        self.select_streams = Some(select.as_ref().to_string());
        self
    }

    pub fn print_format<S: AsRef<str>>(&mut self, format: S) -> &mut Self {
        self.print_format = Some(format.as_ref().to_string());
        self
    }

    pub fn read_intervals<S: AsRef<str>>(&mut self, intervals: S) -> &mut Self {
        self.read_intervals = Some(intervals.as_ref().to_string());
        self
    }

    pub const fn show_frames(&mut self) -> &mut Self {
        self.show_frames = true;
        self
    }

    pub const fn count_frames(&mut self) -> &mut Self {
        self.count_frames = true;
        self
    }

    pub fn loglevel<S: AsRef<str>>(&mut self, level: S) -> &mut Self {
        self.loglevel = Some(level.as_ref().to_string());
        self
    }

    pub fn pattern_type<S: AsRef<str>>(&mut self, pt: S) -> &mut Self {
        self.pattern_type = Some(pt.as_ref().to_string());
        self
    }

    pub fn arg<S: AsRef<str>>(&mut self, arg: S) -> &mut Self {
        self.extra_args.push(arg.as_ref().to_string());
        self
    }

    /// Construct the `std::process::Command`.
    #[must_use]
    pub fn build(&self) -> Command {
        let mut cmd = Command::new(constants::TOOL_FFPROBE);

        if self.show_streams {
            cmd.arg("-show_streams");
        }

        if self.show_format {
            cmd.arg("-show_format");
        }

        if self.show_frames {
            cmd.arg("-show_frames");
        }

        if self.count_frames {
            cmd.arg("-count_frames");
        }

        if let Some(entries) = &self.show_entries {
            cmd.arg("-show_entries").arg(entries);
        }

        if let Some(select) = &self.select_streams {
            cmd.arg("-select_streams").arg(select);
        }

        if let Some(fmt) = &self.print_format {
            cmd.arg("-print_format").arg(fmt);
        }

        if let Some(intervals) = &self.read_intervals {
            cmd.arg("-read_intervals").arg(intervals);
        }

        if let Some(level) = &self.loglevel {
            cmd.arg("-v").arg(level);
        }

        if let Some(pt) = &self.pattern_type {
            cmd.arg("-pattern_type").arg(pt);
        } else if let Some(input) = &self.input {
            // Auto-fallback: if filename contains [ or ], we MUST use -pattern_type none
            // to avoid demuxer sequence errors.
            let s = input.to_string_lossy();
            if s.contains('[') || s.contains(']') {
                cmd.arg("-pattern_type").arg("none");
            }
        }

        for arg in &self.extra_args {
            cmd.arg(arg);
        }

        if let Some(input) = &self.input {
            cmd.arg("--").arg(crate::safe_path_arg(input).as_ref());
        }

        cmd
    }

    #[must_use]
    pub fn check_available() -> bool {
        Command::new(constants::TOOL_FFPROBE)
            .arg("-version")
            .output()
            .is_ok_and(|o| o.status.success())
    }
}

impl FfmpegBuilder {
    /// List available `FFmpeg` encoders.
    ///
    /// # Errors
    /// Returns an error if the command fails.
    pub fn list_encoders() -> anyhow::Result<String> {
        let output = Command::new(constants::TOOL_FFMPEG)
            .arg("-hide_banner")
            .arg("-encoders")
            .output()?;
        if !output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            let summary = stderr
                .lines()
                .chain(stdout.lines())
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .take(3)
                .collect::<Vec<_>>()
                .join(" | ");

            anyhow::bail!(
                "ffmpeg -encoders failed{}",
                if summary.is_empty() {
                    format!(" with status {}", output.status)
                } else {
                    format!(": {summary}")
                }
            );
        }
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::FfmpegBuilder;
    use std::path::Path;

    #[test]
    fn input_format_is_emitted_before_input_paths() {
        let mut builder = FfmpegBuilder::new();
        builder
            .hide_banner()
            .input_format("lavfi")
            .input("nullsrc=s=128x128:d=0.1")
            .frames_v(1)
            .format("null")
            .output_pipe();

        let args: Vec<String> = builder
            .build()
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();

        assert_eq!(args[0], "-hide_banner");
        assert_eq!(args[1], "-f");
        assert_eq!(args[2], "lavfi");
        assert_eq!(args[3], "-i");
        assert_eq!(args[4], "nullsrc=s=128x128:d=0.1");

        let output_format_pos = args
            .iter()
            .rposition(|arg| arg == "-f")
            .expect("output format flag should be present");
        assert!(
            output_format_pos > 3,
            "output format should stay after the input"
        );
        assert_eq!(args[output_format_pos + 1], "null");
    }

    #[test]
    fn output_pipe_emits_dash_target() {
        let mut builder = FfmpegBuilder::new();
        builder
            .hide_banner()
            .input("input.mov")
            .format("yuv4mpegpipe")
            .pix_fmt_str("yuv420p")
            .output_pipe();

        let args: Vec<String> = builder
            .build()
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();

        assert_eq!(args.last().map(String::as_str), Some("-"));
        assert!(args.windows(2).any(|pair| pair == ["-f", "yuv4mpegpipe"]));
    }

    #[test]
    #[should_panic(expected = "FFmpeg output target is required")]
    fn build_panics_without_output_target() {
        let _ = FfmpegBuilder::new()
            .hide_banner()
            .input(Path::new("input.mov"))
            .build();
    }
}
