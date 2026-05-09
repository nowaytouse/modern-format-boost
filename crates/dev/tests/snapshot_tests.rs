fn main() {
    println!("Running test...");
    println!("✅ Test completed!");
}

#[cfg(test)]
mod tests {
    use insta::assert_debug_snapshot;
    use shared_utils::ToolBuilder;
    use shared_utils::ffmpeg_builder::{EncoderPreset, FfmpegBuilder, VideoCodec};
    use shared_utils::jxl_builder::CjxlBuilder;
    use std::path::Path;

    #[test]
    fn test_ffmpeg_builder_snapshot() {
        let cmd = FfmpegBuilder::new()
            .overwrite()
            .threads(4)
            .input(Path::new("input.mp4"))
            .vcodec(VideoCodec::Hevc)
            .crf(18.0)
            .preset(EncoderPreset::Slower)
            .output(Path::new("output.mp4"))
            .build();

        let args: Vec<String> = cmd
            .get_args()
            .map(|s| s.to_string_lossy().to_string())
            .collect();

        assert_debug_snapshot!(args);
    }

    #[test]
    fn test_cjxl_builder_snapshot() {
        let cmd = CjxlBuilder::new()
            .input(Path::new("in.png"))
            .output(Path::new("out.jxl"))
            .distance(0.5)
            .effort(7)
            .build();

        let args: Vec<String> = cmd
            .get_args()
            .map(|s| s.to_string_lossy().to_string())
            .collect();

        assert_debug_snapshot!(args);
    }
}
