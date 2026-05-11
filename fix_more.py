import os
import re

def fix_ffprobe():
    path = 'crates/shared_utils/src/ffprobe.rs'
    with open(path, 'r') as f:
        content = f.read()

    # parse_rational_to_50k
    content = content.replace(
        'let val = crate::numeric_cast::f64_to_u64_strict(\n            (n / d) * crate::constants::HDR_COORD_SCALING_FACTOR,\n        );\n        Some(val)',
        'let val = crate::numeric_cast::f64_to_u64_strict(\n            (n / d) * crate::constants::HDR_COORD_SCALING_FACTOR,\n            "hdr_coord"\n        );\n        val'
    )
    content = content.replace(
        'let val =\n                crate::numeric_cast::f64_to_u64_strict(v * crate::constants::HDR_COORD_SCALING_FACTOR);\n            Some(val)',
        'let val =\n                crate::numeric_cast::f64_to_u64_strict(v * crate::constants::HDR_COORD_SCALING_FACTOR, "hdr_coord");\n            val'
    )
    content = content.replace(
        'let val = crate::numeric_cast::f64_to_u64_sat(v);\n            Some(val)',
        'let val = crate::numeric_cast::f64_to_u64_sat(v);\n            Some(val)' # This one is fine as it was
    )
    
    # parse_luminance_to_10k
    content = content.replace(
        'let n: f64 = crate::numeric_cast::parse_strict(num.trim())?;',
        'let n: f64 = crate::numeric_cast::parse_strict(num.trim(), "hdr_num")?;'
    )
    content = content.replace(
        'let val = crate::numeric_cast::f64_to_u64_strict(\n            (n / d) * crate::constants::HDR_LUMA_SCALING_FACTOR,\n        );\n        Some(val)',
        'let val = crate::numeric_cast::f64_to_u64_strict(\n            (n / d) * crate::constants::HDR_LUMA_SCALING_FACTOR,\n            "hdr_luma"\n        );\n        val'
    )
    content = content.replace(
        'let val =\n                crate::numeric_cast::f64_to_u64_strict(v * crate::constants::HDR_LUMA_SCALING_FACTOR);\n            Some(val)',
        'let val =\n                crate::numeric_cast::f64_to_u64_strict(v * crate::constants::HDR_LUMA_SCALING_FACTOR, "hdr_luma");\n            val'
    )

    with open(path, 'w') as f:
        f.write(content)

def fix_precheck():
    path = 'crates/shared_utils/src/video_explorer/precheck.rs'
    with open(path, 'r') as f:
        content = f.read()

    # get_video_info
    content = content.replace(
        'let frame_count = if frame_count_raw == 0 && duration > 0.0_f64 {\n        crate::numeric_cast::f64_to_u64_strict(duration * fps)\n    } else {\n        frame_count_raw.max(1)\n    };',
        'let frame_count = if frame_count_raw == 0 && duration > 0.0_f64 {\n        crate::numeric_cast::f64_to_u64_strict(duration * fps, "frame_count").unwrap_or(1)\n    } else {\n        frame_count_raw.max(1)\n    };'
    )
    content = content.replace(
        'crate::numeric_cast::f64_to_u64_strict(\n                    crate::numeric_cast::u64_to_f64(br) * duration / 8.0,\n                )',
        'crate::numeric_cast::f64_to_u64_strict(\n                    crate::numeric_cast::u64_to_f64(br) * duration / 8.0,\n                    "video_bytes"\n                ).unwrap_or(0)'
    )
    with open(path, 'w') as f:
        f.write(content)

def fix_video_explorer():
    path = 'crates/shared_utils/src/video_explorer.rs'
    with open(path, 'r') as f:
        content = f.read()

    # macro log_progress
    content = re.sub(
        r'let permille = crate::numeric_cast::u64_to_u32_sat\(\s*u64::try_from\(\s*\(u128::from\(\$size\) \* 10_000\) / u128::from\(self\.input_size\.max\(1\)\),\s*\)\s*\.unwrap_or\(u64::MAX\),\s*"size_percentage_permille",\s*\)\s*\.unwrap_or_else\(.*?\);',
        r'let permille = crate::numeric_cast::u64_to_u32_sat(\n                        u64::try_from(\n                            (u128::from($size) * 10_000) / u128::from(self.input_size.max(1)),\n                        )\n                        .unwrap_or(u64::MAX)\n                    );',
        content, flags=re.DOTALL
    )
    
    content = content.replace(
        'let pct_label = crate::numeric_cast::f64_to_u32_strict(segment_pct * 100.0);',
        'let pct_label = crate::numeric_cast::f64_to_u32_strict(segment_pct * 100.0, "pct").unwrap_or(0);'
    )

    with open(path, 'w') as f:
        f.write(content)

def fix_quality_detector():
    path = 'crates/shared_utils/src/video_quality_detector.rs'
    with open(path, 'r') as f:
        content = f.read()

    content = content.replace(
        'let Some(fps_r) = crate::numeric_cast::f64_to_rational_strict(fps_val) else {',
        'let Some(fps_r) = crate::numeric_cast::f64_to_rational_strict(fps_val, "fps") else {'
    )
    content = content.replace(
        'crate::numeric_cast::u32_to_u8_strict(crate::constants::FALLBACK_CRF_VIDEO, "fallback")',
        'crate::numeric_cast::f32_to_u8_strict(crate::constants::FALLBACK_CRF_VIDEO, "fallback").unwrap_or(0)'
    )
    # Wait, the error said f32_to_u8_strict was not found. Let's use f64_to_u8_strict.
    content = content.replace(
        'crate::numeric_cast::f32_to_u8_strict(crate::constants::FALLBACK_CRF_VIDEO, "fallback").unwrap_or(0)',
        'crate::numeric_cast::f64_to_u8_strict(f64::from(crate::constants::FALLBACK_CRF_VIDEO), "fallback").unwrap_or(0)'
    )
    
    with open(path, 'w') as f:
        f.write(content)

if __name__ == '__main__':
    fix_ffprobe()
    fix_precheck()
    fix_video_explorer()
    fix_quality_detector()
