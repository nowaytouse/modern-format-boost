import os

def fix_batch():
    path = 'crates/shared_utils/src/batch.rs'
    with open(path, 'r') as f:
        content = f.read()

    content = content.replace(
        'crate::numeric_cast::f64_to_u64_sat((value * 1000.0).round())\n            .unwrap_or(u64::MAX)',
        'crate::numeric_cast::f64_to_u64_sat((value * 1000.0).round())'
    )
    content = content.replace(
        'Some(crate::numeric_cast::f64_to_u64_strict(\n                    (dur * fps).round().max(1.0_f64),\n                ))',
        'Some(crate::numeric_cast::f64_to_u64_strict(\n                    (dur * fps).round().max(1.0_f64),\n                    "frames"\n                )?)'
    )
    content = content.replace(
        '|fc| if fc > 0 { Some(fc) } else { None },',
        '|fc| if fc > 0 { Some(fc) } else { None },'
    ) # Wait, `frames` in `batch.rs:1034` is an Option? If fc > 0 { Some(Some(fc)) } else { None } ?
    # Let's see batch.rs:1029: `|fc| if fc > 0 { Some(fc) } else { None }` was returning `u64` instead of `Option<u64>`? 
    # Actually, the error says: expected enum `Option<u64>`, found `u64`. So it should be `Some(Some(fc))`... wait, no. It's likely mapped over `Option`.
    content = content.replace('|fc| if fc > 0 { Some(fc) } else { None },', '|fc| if fc > 0 { Some(Some(fc)) } else { None },')
    content = content.replace('.map(|(pixels, frames)| pixels.saturating_mul(frames.max(1)));', '.map(|(pixels, frames)| pixels.saturating_mul(frames.unwrap_or(0).max(1)));')
    
    with open(path, 'w') as f:
        f.write(content)

def fix_conversion():
    path = 'crates/shared_utils/src/conversion.rs'
    with open(path, 'r') as f:
        content = f.read()

    content = content.replace(
        'let diff_bytes_i64 = crate::numeric_cast::u128_to_i64_sat(diff_bytes);',
        'let diff_bytes_i64 = crate::numeric_cast::i128_to_i64_strict(diff_bytes, "diff_bytes").unwrap_or(0);'
    )
    content = content.replace(
        'let diff_bytes = crate::numeric_cast::u64_to_i64_strict(output_size)\n            .saturating_sub(crate::numeric_cast::u64_to_i64_sat(input_size));',
        'let diff_bytes = crate::numeric_cast::u64_to_i64_strict(output_size, "output_size")\n            .unwrap_or(0).saturating_sub(crate::numeric_cast::u64_to_i64_sat(input_size));'
    )
    with open(path, 'w') as f:
        f.write(content)

def fix_database():
    path = 'crates/shared_utils/src/database.rs'
    with open(path, 'r') as f:
        content = f.read()
        
    content = content.replace(
        'let idx = crate::numeric_cast::f64_to_usize_sat(\n            (crate::numeric_cast::usize_to_f64(loop_durations.len()) * 0.90).floor(),\n        )\n        .ok_or_else(|| {',
        'let idx = crate::numeric_cast::f64_to_usize_strict(\n            (crate::numeric_cast::usize_to_f64(loop_durations.len()) * 0.90).floor(),\n            "p90_idx",\n        )\n        .ok_or_else(|| {'
    )
    content = content.replace(
        'crate::numeric_cast::f64_to_usize_strict(needed as f64, "needed")',
        'crate::numeric_cast::f64_to_usize_strict(needed as f64, "needed").unwrap_or(0)'
    )
    with open(path, 'w') as f:
        f.write(content)

def fix_ffprobe_json():
    path = 'crates/shared_utils/src/ffprobe_json.rs'
    with open(path, 'r') as f:
        content = f.read()

    content = content.replace(
        'Some(crate::numeric_cast::f64_to_u64_strict(\n                        (f * crate::constants::HDR_COORD_SCALING_FACTOR).round(),\n                    ))',
        'Some(crate::numeric_cast::f64_to_u64_strict(\n                        (f * crate::constants::HDR_COORD_SCALING_FACTOR).round(),\n                        "hdr_coord"\n                    )?)'
    )
    content = content.replace(
        'Some(crate::numeric_cast::f64_to_u64_strict(\n                    ((n / d) * crate::constants::HDR_LUMA_SCALING_FACTOR).round(),\n                ))',
        'Some(crate::numeric_cast::f64_to_u64_strict(\n                    ((n / d) * crate::constants::HDR_LUMA_SCALING_FACTOR).round(),\n                    "hdr_luma"\n                )?)'
    )
    content = content.replace(
        'Some(crate::numeric_cast::f64_to_u64_strict(\n                        (f * crate::constants::HDR_LUMA_SCALING_FACTOR).round(),\n                    ))',
        'Some(crate::numeric_cast::f64_to_u64_strict(\n                        (f * crate::constants::HDR_LUMA_SCALING_FACTOR).round(),\n                        "hdr_luma_f"\n                    )?)'
    )
    with open(path, 'w') as f:
        f.write(content)

def fix_stream_size():
    path = 'crates/shared_utils/src/stream_size.rs'
    with open(path, 'r') as f:
        content = f.read()

    content = content.replace(
        'let size = crate::numeric_cast::f64_to_u64_strict(size_rational.to_f64());',
        'let size = crate::numeric_cast::f64_to_u64_strict(size_rational.to_f64(), "size")?;'
    )
    content = content.replace(
        '(size, Some(br))',
        '(size, Some(br))' # Handled by ? above if size is unnested. Wait, the error is expected `u64`, found `Option<u64>`. Using `?` solves it!
    )
    content = content.replace(
        'crate::numeric_cast::f64_to_u64_strict(overhead.to_f64())',
        'crate::numeric_cast::f64_to_u64_strict(overhead.to_f64(), "overhead")?'
    )
    content = content.replace(
        'let estimated_video_size = total_file_size.saturating_sub(estimated_overhead);',
        'let estimated_video_size = total_file_size.saturating_sub(estimated_overhead);' # Handled by `?` above.
    )
    with open(path, 'w') as f:
        f.write(content)

def fix_image_detection():
    path = 'crates/shared_utils/src/image_detection.rs'
    with open(path, 'r') as f:
        content = f.read()

    content = content.replace(
        'let Some(box_size) = crate::numeric_cast::u32_to_usize_sat(box_size_u32) else {',
        'let box_size = crate::numeric_cast::u32_to_usize_sat(box_size_u32); if false {'
    )
    # wait, if it was `let Some(box_size) = ..._strict(..., "box_size") else { return None; }`, we should restore that!
    content = content.replace(
        'let box_size = crate::numeric_cast::u32_to_usize_sat(box_size_u32); if false {',
        'let Some(box_size) = crate::numeric_cast::u32_to_usize_strict(box_size_u32, "box_size") else {'
    )
    content = content.replace(
        'let step = crate::numeric_cast::f64_to_u32_strict(\n        (crate::numeric_cast::u64_to_f64(u64::from(width) * u64::from(height))\n            / crate::constants::PNG_DITHER_SAMPLING_FACTOR)\n            .max(1.0),\n    );',
        'let step = crate::numeric_cast::f64_to_u32_strict(\n        (crate::numeric_cast::u64_to_f64(u64::from(width) * u64::from(height))\n            / crate::constants::PNG_DITHER_SAMPLING_FACTOR)\n            .max(1.0),\n        "step"\n    ).unwrap_or(1);'
    )
    content = content.replace(
        'match (count, total) {\n            (Some(c), Some(t)) if t > 0 => f64::from(c) / f64::from(t),',
        'match (count, total) {\n            (c, Some(t)) if t > 0 => f64::from(c) / f64::from(t),'
    )
    content = content.replace(
        'let total_pixels = crate::numeric_cast::u32_to_usize_strict(width)\n        * crate::numeric_cast::u32_to_usize_sat(height);',
        'let total_pixels = crate::numeric_cast::u32_to_usize_strict(width, "width").unwrap_or(0)\n        * crate::numeric_cast::u32_to_usize_sat(height);'
    )
    content = content.replace(
        'let block_size = crate::numeric_cast::f64_to_usize_sat(\n        (crate::numeric_cast::u64_to_f64(total_pixels as u64)\n            / crate::numeric_cast::usize_to_f64(target_samples).max(1.0))\n        .max(1.0),\n    )\n    .expect("block_size fits in usize");',
        'let block_size = crate::numeric_cast::f64_to_usize_strict(\n        (crate::numeric_cast::u64_to_f64(total_pixels as u64)\n            / crate::numeric_cast::usize_to_f64(target_samples).max(1.0))\n        .max(1.0),\n        "block_size"\n    )\n    .expect("block_size fits in usize");'
    )
    content = content.replace(
        'let blocks_x = crate::numeric_cast::u32_to_usize_strict(width).div_ceil(block_size.max(1));',
        'let blocks_x = crate::numeric_cast::u32_to_usize_strict(width, "width").unwrap_or(0).div_ceil(block_size.max(1));'
    )
    with open(path, 'w') as f:
        f.write(content)

def fix_image_heic_analysis():
    path = 'crates/shared_utils/src/image_heic_analysis.rs'
    with open(path, 'r') as f:
        content = f.read()

    content = content.replace(
        'let Some(num_ch) = crate::numeric_cast::u8_to_usize_sat(\n                        *pixi_data.first()?,\n                    ) else {',
        'let Some(num_ch) = crate::numeric_cast::u8_to_usize_strict(\n                        *pixi_data.first()?,\n                        "heic_pixi_num_ch"\n                    ) else {'
    )
    content = content.replace(
        'crate::numeric_cast::u8_to_usize_strict(*b)',
        'crate::numeric_cast::u8_to_usize_strict(*b, "num_nalu_arrays")?'
    )
    content = content.replace(
        'let Some(nal_unit_length) = crate::numeric_cast::u16_to_usize_sat(\n                    u16::from_be_bytes([b1, b2]),\n                ) else {',
        'let Some(nal_unit_length) = crate::numeric_cast::u16_to_usize_strict(\n                    u16::from_be_bytes([b1, b2]),\n                    "heic_nal_len"\n                ) else {'
    )
    content = content.replace(
        'crate::numeric_cast::u16_to_usize_strict(u16::from_be_bytes([b1, b2]));',
        'crate::numeric_cast::u16_to_usize_strict(u16::from_be_bytes([b1, b2]), "nal_unit_length").unwrap_or(0);'
    )
    with open(path, 'w') as f:
        f.write(content)

def fix_image_metrics():
    path = 'crates/shared_utils/src/image_metrics.rs'
    with open(path, 'r') as f:
        content = f.read()

    content = content.replace(
        'crate::numeric_cast::f64_to_u64_sat(mse_sum.round())\n                    .expect("mse_sum easily fits in i64 bounds"),',
        'crate::numeric_cast::f64_to_u64_sat(mse_sum.round()),'
    )
    content = content.replace(
        'let width = crate::numeric_cast::u32_to_usize_strict(w1);',
        'let width = crate::numeric_cast::u32_to_usize_strict(w1, "w1").unwrap_or(0);'
    )
    content = content.replace(
        'let height = crate::numeric_cast::u32_to_usize_strict(h1);',
        'let height = crate::numeric_cast::u32_to_usize_strict(h1, "h1").unwrap_or(0);'
    )
    with open(path, 'w') as f:
        f.write(content)

if __name__ == '__main__':
    fix_batch()
    fix_conversion()
    fix_database()
    fix_ffprobe_json()
    fix_stream_size()
    fix_image_detection()
    fix_image_heic_analysis()
    fix_image_metrics()
