#[cfg(test)]
mod tests {
    use insta::assert_debug_snapshot;
    use shared_utils::loop_intent::{assess_loop_intent_from_meta, LoopMeta};

    fn mock_sticker_profile() -> LoopMeta {
        LoopMeta {
            duration_secs: 1.5,
            duration_tier: Some(shared_utils::loop_intent::DurationTier::from_secs(1.5)),
            width: 400,
            height: 400,
            fps: 12.0,
            frame_count: 18,
            file_size_bytes: 500_000,
            has_audio: false,
            has_transparency: true,
            is_native_gif: true,
            source_extension: Some("gif".to_string()),
            loop_count: Some(0),
            container: Some("gif".to_string()),
            ..LoopMeta::default()
        }
    }

    fn mock_meme_profile() -> LoopMeta {
        LoopMeta {
            duration_secs: 3.5,
            duration_tier: Some(shared_utils::loop_intent::DurationTier::from_secs(3.5)),
            width: 640,
            height: 360,
            fps: 24.0,
            frame_count: 84,
            file_size_bytes: 2_000_000,
            has_audio: false,
            is_native_gif: false,
            source_extension: Some("mp4".to_string()),
            container: Some("mp4".to_string()),
            loop_closure_score: Some(0.95), // High closure
            motion_gini: Some(0.15),        // Smooth constant motion
            ..LoopMeta::default()
        }
    }

    fn mock_long_clip_profile() -> LoopMeta {
        LoopMeta {
            duration_secs: 45.0,
            duration_tier: Some(shared_utils::loop_intent::DurationTier::from_secs(45.0)),
            width: 1920,
            height: 1080,
            fps: 30.0,
            frame_count: 1350,
            file_size_bytes: 50_000_000,
            has_audio: true,
            is_native_gif: false,
            source_extension: Some("mp4".to_string()),
            container: Some("mp4".to_string()),
            ..LoopMeta::default()
        }
    }

    fn mock_silent_technical_profile() -> LoopMeta {
        LoopMeta {
            duration_secs: 8.5,
            duration_tier: Some(shared_utils::loop_intent::DurationTier::from_secs(8.5)),
            width: 1280,
            height: 720,
            fps: 30.0,
            frame_count: 255,
            file_size_bytes: 8_000_000,
            has_audio: false,
            is_native_gif: false,
            source_extension: Some("mp4".to_string()),
            container: Some("mp4".to_string()),
            motion_gini: Some(0.85), // Highly irregular motion (not a simple loop)
            ..LoopMeta::default()
        }
    }

    fn mock_definitively_long_profile() -> LoopMeta {
        LoopMeta {
            duration_secs: 18.5,
            duration_tier: Some(shared_utils::loop_intent::DurationTier::from_secs(18.5)),
            width: 1920,
            height: 1080,
            fps: 30.0,
            frame_count: 555,
            file_size_bytes: 12_000_000,
            has_audio: false,
            is_native_gif: false,
            source_extension: Some("mp4".to_string()),
            container: Some("mp4".to_string()),
            ..LoopMeta::default()
        }
    }

    #[test]
    fn test_sticker_verdict_snapshot() {
        let meta = mock_sticker_profile();
        let verdict = assess_loop_intent_from_meta(&meta, None);
        assert_debug_snapshot!(verdict.reason());
    }

    #[test]
    fn test_meme_verdict_snapshot() {
        let meta = mock_meme_profile();
        let verdict = assess_loop_intent_from_meta(&meta, None);
        assert_debug_snapshot!(verdict.reason());
    }

    #[test]
    fn test_long_clip_verdict_snapshot() {
        let meta = mock_long_clip_profile();
        let verdict = assess_loop_intent_from_meta(&meta, None);
        assert_debug_snapshot!(verdict.reason());
    }

    #[test]
    fn test_silent_technical_verdict_snapshot() {
        let meta = mock_silent_technical_profile();
        let verdict = assess_loop_intent_from_meta(&meta, None);
        assert_debug_snapshot!(verdict.reason());
    }

    #[test]
    fn test_definitively_long_verdict_snapshot() {
        let meta = mock_definitively_long_profile();
        let verdict = assess_loop_intent_from_meta(&meta, None);
        assert_debug_snapshot!(verdict.reason());
    }
}
