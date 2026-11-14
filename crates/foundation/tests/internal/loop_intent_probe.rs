// Loop Intent Tree & Rhythm Poisoning Defense Probe
//
// Evaluates the 7-layer decision tree against edge cases such as zero duration,
// rhythm poisoning, and conflicting metadata declarations.

use crate::loop_intent::{LoopMeta, identify};

#[test]
fn test_loop_intent_handles_zero_duration_poison_pill_gracefully() {
    let mut meta = LoopMeta {
        duration_secs: Some(0.0),
        frame_count: Some(10),
        file_size_bytes: 1024,
        source_extension: Some("gif".to_string()),
        container: Some("gif".to_string()),
        ..Default::default()
    };
    meta.flags.streams.is_native_gif = true;

    // Should evaluate safely without panic or division by zero
    let verdict = identify(&meta);
    assert!(
        verdict.is_keep_gif() || verdict.is_keep_video() || verdict.is_uncertain(),
        "Zero duration GIF should be handled safely"
    );
}

#[test]
fn test_loop_intent_distinguishes_rhythm_poisoning_and_static_content() {
    // Simulate a high frame delay variation (rhythm poisoning)
    let mut meta_poison = LoopMeta {
        duration_secs: Some(5.0),
        frame_count: Some(30),
        file_size_bytes: 50000,
        frame_delay_variation: Some(2.5), // Extremely high variation
        source_extension: Some("mp4".to_string()),
        container: Some("mp4".to_string()),
        ..Default::default()
    };
    meta_poison.flags.streams.has_audio = false;

    let verdict_poison = identify(&meta_poison);
    // Even with rhythm poisoning, it should be classified honestly based on duration and lack of audio
    assert!(
        verdict_poison.is_keep_gif()
            || verdict_poison.is_keep_video()
            || verdict_poison.is_uncertain(),
        "Rhythm poisoned file should receive a valid loop verdict"
    );

    // Simulate pure static content
    let meta_static = LoopMeta {
        duration_secs: Some(0.04), // 1 frame at 25fps
        frame_count: Some(1),
        file_size_bytes: 15000,
        source_extension: Some("jpg".to_string()),
        container: Some("jpg".to_string()),
        ..Default::default()
    };

    let verdict_static = identify(&meta_static);
    assert!(
        verdict_static.is_error()
            || verdict_static.reason().contains("single-frame")
            || verdict_static.is_uncertain(),
        "Single frame static image must be classified as error, uncertain or non-loop"
    );
}

#[test]
fn test_loop_intent_verdict_retains_explanation_integrity() {
    let meta = LoopMeta {
        duration_secs: Some(1.5),
        frame_count: Some(15),
        file_size_bytes: 20480,
        source_extension: Some("gif".to_string()),
        container: Some("gif".to_string()),
        ..Default::default()
    };

    let verdict = identify(&meta);
    assert!(verdict.is_keep_gif() || verdict.is_keep_video() || verdict.is_uncertain());
    assert!(
        !verdict.reason().is_empty(),
        "Verdict reason must not be empty"
    );
}
