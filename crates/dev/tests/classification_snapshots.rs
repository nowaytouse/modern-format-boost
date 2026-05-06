use insta::assert_debug_snapshot;
use shared_utils::loop_intent::{assess_loop_intent_from_meta, LoopMeta};

fn main() {
    println!("运行分类快照测试...");

    test_prores_debug_verdict_snapshot();
    test_definitively_long_verdict_snapshot();
    test_meme_verdict_snapshot();
    test_sticker_verdict_snapshot();
    test_long_clip_verdict_snapshot();
    test_silent_technical_verdict_snapshot();

    println!("✅ 所有分类快照测试通过！");
}

fn test_prores_debug_verdict_snapshot() {
    fn mock_sticker_profile() -> LoopMeta {
        LoopMeta {
            duration_secs: 1.5,
            duration_tier: Some(shared_utils::loop_intent::DurationTier::from_secs(1.5)),
            width: 400,
            height: 400,
            fps: 12.0,
            frame_count: Some(18),
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

    let meta = mock_sticker_profile();
    let verdict = assess_loop_intent_from_meta(&meta, None);
    assert_debug_snapshot!(verdict.reason());
}

fn test_definitively_long_verdict_snapshot() {
    fn mock_definitively_long_profile() -> LoopMeta {
        LoopMeta {
            duration_secs: 45.0,
            duration_tier: Some(shared_utils::loop_intent::DurationTier::from_secs(45.0)),
            width: 1920,
            height: 1080,
            fps: 30.0,
            frame_count: Some(1350),
            file_size_bytes: 800_000_000,
            has_audio: false,
            is_native_gif: false,
            source_extension: Some("mov".to_string()),
            container: Some("prores".to_string()),
            ..LoopMeta::default()
        }
    }

    let meta = mock_definitively_long_profile();
    let verdict = assess_loop_intent_from_meta(&meta, None);
    assert_debug_snapshot!(verdict.reason());
}

fn test_meme_verdict_snapshot() {
    fn mock_meme_profile() -> LoopMeta {
        LoopMeta {
            duration_secs: 3.5,
            duration_tier: Some(shared_utils::loop_intent::DurationTier::from_secs(3.5)),
            width: 640,
            height: 640,
            fps: 30.0,
            frame_count: Some(105),
            file_size_bytes: 2_000_000,
            has_audio: false,
            is_native_gif: true,
            source_extension: Some("gif".to_string()),
            container: Some("gif".to_string()),
            ..LoopMeta::default()
        }
    }

    let meta = mock_meme_profile();
    let verdict = assess_loop_intent_from_meta(&meta, None);
    assert_debug_snapshot!(verdict.reason());
}

fn test_sticker_verdict_snapshot() {
    fn mock_sticker_profile() -> LoopMeta {
        LoopMeta {
            duration_secs: 1.5,
            duration_tier: Some(shared_utils::loop_intent::DurationTier::from_secs(1.5)),
            width: 400,
            height: 400,
            fps: 12.0,
            frame_count: Some(18),
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

    let meta = mock_sticker_profile();
    let verdict = assess_loop_intent_from_meta(&meta, None);
    assert_debug_snapshot!(verdict.reason());
}

fn test_long_clip_verdict_snapshot() {
    fn mock_long_clip_profile() -> LoopMeta {
        LoopMeta {
            duration_secs: 18.5,
            duration_tier: Some(shared_utils::loop_intent::DurationTier::from_secs(18.5)),
            width: 1920,
            height: 1080,
            fps: 30.0,
            frame_count: Some(555),
            file_size_bytes: 50_000_000,
            has_audio: false,
            is_native_gif: false,
            source_extension: Some("mov".to_string()),
            container: Some("prores".to_string()),
            ..LoopMeta::default()
        }
    }

    let meta = mock_long_clip_profile();
    let verdict = assess_loop_intent_from_meta(&meta, None);
    assert_debug_snapshot!(verdict.reason());
}

fn test_silent_technical_verdict_snapshot() {
    fn mock_silent_technical_profile() -> LoopMeta {
        LoopMeta {
            duration_secs: 18.20,
            duration_tier: Some(shared_utils::loop_intent::DurationTier::from_secs(18.20)),
            width: 1920,
            height: 1080,
            fps: 30.0,
            frame_count: Some(1092),
            file_size_bytes: 800_000_000,
            has_audio: false,
            is_native_gif: false,
            source_extension: Some("mov".to_string()),
            container: Some("prores".to_string()),
            ..LoopMeta::default()
        }
    }

    let meta = mock_silent_technical_profile();
    let verdict = assess_loop_intent_from_meta(&meta, None);
    assert_debug_snapshot!(verdict.reason());
}
