#[cfg(test)]
mod parity_tests {
    use crate::ffmpeg_builder::{FfmpegBuilder, VideoCodec, EncoderPreset};
    use crate::jxl_builder::CjxlBuilder;
    use crate::image_builders::MagickBuilder;
    use std::path::Path;

    #[test]
    fn test_ffmpeg_flag_order_parity() {
        let cmd = FfmpegBuilder::new()
            .overwrite()
            .threads(4)
            .input(Path::new("in.mp4"))
            .vcodec(VideoCodec::Hevc)
            .crf(18.0)
            .preset(EncoderPreset::Slower)
            .build();
        
        let args: Vec<_> = cmd.get_args().map(|s| s.to_string_lossy()).collect();
        // Expect: -y -threads 4 -i in.mp4 -c:v libx265 -crf 18 -preset slower
        assert_eq!(args[0], "-y");
        assert_eq!(args[1], "-threads");
        assert_eq!(args[2], "4");
        assert_eq!(args[3], "-i");
        assert!(args[4].contains("in.mp4"));
        assert_eq!(args[5], "-c:v");
        assert_eq!(args[6], "libx265");
        assert_eq!(args[7], "-crf");
        assert_eq!(args[8], "18");
        assert_eq!(args[9], "-preset");
        assert_eq!(args[10], "slower");
    }

    #[test]
    fn test_cjxl_flag_order_parity() {
        let cmd = CjxlBuilder::new()
            .distance(0.5)
            .effort(7)
            .threads(8)
            .input(Path::new("in.png"))
            .output(Path::new("out.jxl"))
            .build();
        
        let args: Vec<_> = cmd.get_args().map(|s| s.to_string_lossy()).collect();
        // Expect (73edfa6): in.png out.jxl -d 0.50 -e 7 -j 8
        assert!(args[0].contains("in.png"));
        assert!(args[1].contains("out.jxl"));
        assert_eq!(args[2], "-d");
        assert_eq!(args[3], "0.50");
        assert_eq!(args[4], "-e");
        assert_eq!(args[5], "7");
        assert_eq!(args[6], "-j");
        assert_eq!(args[7], "8");
    }

    #[test]
    fn test_magick_identify_flag_order_parity() {
        let cmd = MagickBuilder::new()
            .arg("identify")
            .arg("-format")
            .arg("%T")
            .input(Path::new("in.gif"))
            .build();
        
        let args: Vec<_> = cmd.get_args().map(|s| s.to_string_lossy()).collect();
        // Expect (73edfa6): -- in.gif identify -format %T
        assert_eq!(args[0], "--");
        assert!(args[1].contains("in.gif"));
        assert_eq!(args[2], "identify");
        assert_eq!(args[3], "-format");
        assert_eq!(args[4], "%T");
    }

    #[test]
    fn test_x265_flag_order_parity() {
        let cmd = crate::tool_builders::X265Builder::new()
            .crf(18.0)
            .preset("slower")
            .input(Path::new("in.y4m"))
            .output(Path::new("out.hevc"))
            .build();
        
        let args: Vec<_> = cmd.get_args().map(|s| s.to_string_lossy()).collect();
        // Expect: --crf 18.0 --preset slower --input in.y4m --output out.hevc
        assert_eq!(args[0], "--crf");
        assert_eq!(args[1], "18.0");
        assert_eq!(args[2], "--preset");
        assert_eq!(args[3], "slower");
        assert_eq!(args[4], "--input");
        assert!(args[5].contains("in.y4m"));
        assert_eq!(args[6], "--output");
        assert!(args[7].contains("out.hevc"));
    }

    #[test]
    fn test_dovi_flag_order_parity() {
        let cmd = crate::tool_builders::DoviBuilder::new()
            .mode("demux")
            .input(Path::new("in.hevc"))
            .output(Path::new("out.rpu"))
            .build();
        
        let args: Vec<_> = cmd.get_args().map(|s| s.to_string_lossy()).collect();
        // Expect: demux -i in.hevc -o out.rpu
        assert_eq!(args[0], "demux");
        assert_eq!(args[1], "-i");
        assert!(args[2].contains("in.hevc"));
        assert_eq!(args[3], "-o");
        assert!(args[4].contains("out.rpu"));
    }

    #[test]
    fn test_exiftool_flag_order_parity() {
        let cmd = crate::image_builders::ExiftoolBuilder::new()
            .arg("-icc_profile")
            .arg("-b")
            .input(Path::new("in.jpg"))
            .build();
        
        let args: Vec<_> = cmd.get_args().map(|s| s.to_string_lossy()).collect();
        // Expect: -icc_profile -b in.jpg
        assert_eq!(args[0], "-icc_profile");
        assert_eq!(args[1], "-b");
        assert!(args[2].contains("in.jpg"));
    }

    #[test]
    fn test_rsync_flag_order_parity() {
        let cmd = crate::tool_builders::RsyncBuilder::new()
            .arg("-av")
            .add_source(Path::new("src"))
            .destination(Path::new("dest"))
            .build();
        
        let args: Vec<_> = cmd.get_args().map(|s| s.to_string_lossy()).collect();
        // Expect: -av src dest
        assert_eq!(args[0], "-av");
        assert!(args[1].contains("src"));
        assert!(args[2].contains("dest"));
    }
}
