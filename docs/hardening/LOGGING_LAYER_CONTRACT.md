# SOURCE: LOGGING_LAYER_CONTRACT.md

# Logging layer contract

Session and run-log presentation for Modern Format Boost: `logging.rs`, `static_logs.rs`,
`tracing` targets (`mfb::report`, `mfb::audit`, `static_log`), and quality-intel report blocks.
Delivery-layer mutex/path rules remain in [`MEDIA_CONVERSION_LAYER_CONTRACT.md`](MEDIA_CONVERSION_LAYER_CONTRACT.md) (M27, M44–M46).

## Core invariants

| ID  | Invariant                                                                                                                                      | Enforcement                                                                                                       |
| --- | ---------------------------------------------------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------- |
| L1  | Log directory / rotation / mutex recovery use delivery gate helpers (no inline poison `into_inner` on session paths)                           | `media_conversion_logging_strict_defaults`, `media_conversion_session_mutex_hardening_m44`                        |
| L2  | Quality-intel `log_summary_header!` titles use gate icon helpers (no raw emoji in format literals at call sites)                               | `ui_log_summary_title_with_icon`, `ui_visual_artifact_audit_title`; dev test `quality_report_headers_use_gate_l2` |
| L3  | Quality-layer user error strings (`video_quality_detector` BPP/bitrate paths) use `ui_quality_user_error` (no inline `"❌ …".to_string()`)     | `media_conversion_quality_user_errors_m51`                                                                        |
| L4  | `static_logs` detail / hint macros route through `plain_aware_detail` + label symbols                                                          | `stderr_adjacent_paths_use_ui_stderr_or_symbol_pick` (U8 cross-ref)                                               |
| L5  | `mfb::audit` lines stay plain `key=value` (no decorative emoji); algorithm truth in JSON snapshots                                             | `format_mfb_audit_line`, algorithm contract I4/I10                                                                |
| L6  | CLI batch errors (`cli_runner` disk / verification) use `ui_user_facing_error` + `symbols::pick` hints (no raw `bail!("❌`)                    | `media_conversion_infra_user_errors_m52`                                                                          |
| L7  | Runtime safety multi-line blocks + explore CRF `log_detail` marks use gate icon helpers (no raw emoji / inline `symbols::pick` in `safety.rs`) | `ui_safety_*_blocked`, `ui_explore_crf_*_mark`; dev test `media_conversion_safety_and_explore_icons_m54`          |
| L8  | `ErrorSeverity::label_colored` banners route through gate (no inline `symbols::pick` in severity match arms)                                   | `ui_error_severity_colored_label`; dev test `media_conversion_static_log_severity_icons_m55`                      |
| L9  | All `static_logs` icon prefixes (macros + `log_success`/`log_skip`/`log_ignore`/`log_enhanced_*`) use `ui_icon_pick`                           | `media_conversion_static_logs_icon_pick_m56`; cross-ref U7/U8                                                     |
| L10 | `video_explorer` explore progress / audit `log_detail` icons use `ui_icon_pick` (no direct `symbols::pick` in module)                          | `media_conversion_video_explorer_icon_pick_m57`; cross-ref M47                                                    |
| L11 | `explore_strategy` + `progress_mode` + delivery tooling (`ffmpeg_process`, `msssim_*`, `jxl_utils`) stderr icons use `ui_icon_pick`            | M59–M61 dev tests; cross-ref U7                                                                                   |
| L12 | Coarse progress + tracing subscriber + batch report icons use `ui_icon_pick` (`progress.rs`, `logging.rs`, `report.rs`)                        | M62–M63 dev tests                                                                                                 |
| L13 | Delivery I/O + GPU search stderr icons use `ui_icon_pick` (`file_copier`, `image_analyzer`, `video_detection`, `gpu_accel`, etc.)              | M65–M66 dev tests                                                                                                 |

## Verification

```bash
cargo test -p dev --test test_real_silent_fallbacks logging_layer -- --test-threads=1
cargo test -p dev --test test_real_silent_fallbacks media_conversion_quality_user -- --test-threads=1
```

---

