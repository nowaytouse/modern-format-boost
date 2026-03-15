//! Audio & subtitle passthrough strategy helpers for container muxing.
//!
//! These functions determine whether audio/subtitle streams can be copied
//! directly (`-c:a copy`, `-c:s copy`) or must be transcoded for the target
//! container format (MP4/MOV vs MKV).

/// Determine FFmpeg audio arguments for the target container.
///
/// - MKV: always `-c:a copy` (supports every codec).
/// - MP4/MOV: `-c:a copy` unless the codec is incompatible (opus, vorbis).
///   Incompatible codecs are transcoded to AAC 256 kbps.
/// - No audio (`None` codec): returns `-an`.
pub fn audio_args_for_container(audio_codec: Option<&str>, container: &str) -> Vec<String> {
    let codec = match audio_codec {
        Some(c) if !c.is_empty() => c.to_lowercase(),
        _ => return vec!["-an".to_string()],
    };

    let is_mkv = container.eq_ignore_ascii_case("mkv");
    if is_mkv {
        // MKV accepts every audio codec — always copy.
        return vec!["-c:a".to_string(), "copy".to_string()];
    }

    // MP4/MOV: check for incompatible codecs
    let incompatible = codec.contains("opus") || codec.contains("vorbis");
    if incompatible {
        vec![
            "-c:a".to_string(),
            "aac".to_string(),
            "-b:a".to_string(),
            "256k".to_string(),
        ]
    } else {
        vec!["-c:a".to_string(), "copy".to_string()]
    }
}

/// Determine FFmpeg subtitle arguments for the target container.
///
/// - No subtitles: returns empty vec (nothing to map).
/// - MKV: `-c:s copy` (supports all subtitle formats).
/// - MP4/MOV: text-based subs → `-c:s mov_text`; image-based subs → skip
///   (MP4 doesn't support bitmap subtitle tracks like dvd_subtitle / hdmv_pgs_subtitle).
pub fn subtitle_args_for_container(
    has_subtitles: bool,
    subtitle_codec: Option<&str>,
    container: &str,
) -> Vec<String> {
    if !has_subtitles {
        return Vec::new();
    }

    let is_mkv = container.eq_ignore_ascii_case("mkv");
    if is_mkv {
        return vec!["-c:s".to_string(), "copy".to_string()];
    }

    // MP4/MOV: only text-based subtitles are supported (as mov_text).
    let codec_lower = subtitle_codec
        .map(|s| s.to_lowercase())
        .unwrap_or_default();
    let is_text_based = matches!(
        codec_lower.as_str(),
        "srt" | "subrip" | "ass" | "ssa" | "mov_text" | "webvtt" | "text"
    );

    if is_text_based {
        vec!["-c:s".to_string(), "mov_text".to_string()]
    } else {
        // Image-based subtitles (dvd_subtitle, hdmv_pgs_subtitle, etc.) cannot go into MP4.
        // Drop them silently rather than failing the encode.
        vec!["-sn".to_string()]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_audio_mkv_always_copy() {
        assert_eq!(
            audio_args_for_container(Some("opus"), "mkv"),
            vec!["-c:a", "copy"]
        );
        assert_eq!(
            audio_args_for_container(Some("vorbis"), "mkv"),
            vec!["-c:a", "copy"]
        );
        assert_eq!(
            audio_args_for_container(Some("aac"), "mkv"),
            vec!["-c:a", "copy"]
        );
    }

    #[test]
    fn test_audio_mp4_copy_compatible() {
        assert_eq!(
            audio_args_for_container(Some("aac"), "mp4"),
            vec!["-c:a", "copy"]
        );
        assert_eq!(
            audio_args_for_container(Some("ac3"), "mp4"),
            vec!["-c:a", "copy"]
        );
        assert_eq!(
            audio_args_for_container(Some("eac3"), "mp4"),
            vec!["-c:a", "copy"]
        );
        assert_eq!(
            audio_args_for_container(Some("alac"), "mp4"),
            vec!["-c:a", "copy"]
        );
    }

    #[test]
    fn test_audio_mp4_transcode_incompatible() {
        assert_eq!(
            audio_args_for_container(Some("opus"), "mp4"),
            vec!["-c:a", "aac", "-b:a", "256k"]
        );
        assert_eq!(
            audio_args_for_container(Some("vorbis"), "mp4"),
            vec!["-c:a", "aac", "-b:a", "256k"]
        );
    }

    #[test]
    fn test_audio_no_audio() {
        assert_eq!(audio_args_for_container(None, "mp4"), vec!["-an"]);
        assert_eq!(audio_args_for_container(None, "mkv"), vec!["-an"]);
    }

    #[test]
    fn test_subtitle_no_subs() {
        let result = subtitle_args_for_container(false, None, "mp4");
        assert!(result.is_empty());
    }

    #[test]
    fn test_subtitle_mkv_always_copy() {
        assert_eq!(
            subtitle_args_for_container(true, Some("ass"), "mkv"),
            vec!["-c:s", "copy"]
        );
        assert_eq!(
            subtitle_args_for_container(true, Some("hdmv_pgs_subtitle"), "mkv"),
            vec!["-c:s", "copy"]
        );
    }

    #[test]
    fn test_subtitle_mp4_text_based() {
        assert_eq!(
            subtitle_args_for_container(true, Some("srt"), "mp4"),
            vec!["-c:s", "mov_text"]
        );
        assert_eq!(
            subtitle_args_for_container(true, Some("subrip"), "mp4"),
            vec!["-c:s", "mov_text"]
        );
        assert_eq!(
            subtitle_args_for_container(true, Some("ass"), "mp4"),
            vec!["-c:s", "mov_text"]
        );
    }

    #[test]
    fn test_subtitle_mp4_image_based_dropped() {
        assert_eq!(
            subtitle_args_for_container(true, Some("hdmv_pgs_subtitle"), "mp4"),
            vec!["-sn"]
        );
        assert_eq!(
            subtitle_args_for_container(true, Some("dvd_subtitle"), "mp4"),
            vec!["-sn"]
        );
    }
}
