import os
import re
import glob

def process_file(filepath):
    with open(filepath, 'r') as f:
        content = f.read()

    original = content
    
    # Simple name fixes
    content = content.replace('i128_to_i64_sat', 'u128_to_i64_sat')
    content = content.replace('parse_option_sat', 'parse_option_strict')
    content = content.replace('parse_sat', 'parse_strict')
    content = content.replace('option_f32_sat', 'option_f32_strict')
    content = content.replace('f64_to_rational_sat', 'f64_to_rational_strict')
    content = content.replace('usize_to_u64_sat', 'usize_to_u64')
    content = content.replace('f32_to_u8_strict(crate::constants::FALLBACK_CRF_VIDEO)', 'u32_to_u8_strict(crate::constants::FALLBACK_CRF_VIDEO, "fallback")')
    content = content.replace('i64_to_usize_strict(needed)', 'f64_to_usize_strict(needed as f64, "needed")')
    content = content.replace('f64_to_i64_sat(mse_sum.round(), "mse_sum_rounded")', 'f64_to_u64_sat(mse_sum.round())')
    
    # Fix _sat having a string argument.
    # regex for `_sat( <expr> , "some_string" )` -> `_sat( <expr> )`
    # Be careful with nested parentheses.
    # A simple regex for the ones in the codebase:
    # _sat(EXPR, "string")
    content = re.sub(r'(_sat\([^,]+?),\s*"[^"]+"\)', r'\1)', content)
    # the above might fail if EXPR has commas. So let's handle the specific ones:
    content = re.sub(r'(_sat\([^,]+,[^,]+),\s*"[^"]+"\)', r'\1)', content)
    
    # Removing `.unwrap_or(...)` after `_sat(...)`
    # e.g., `_sat(...).unwrap_or(...)`
    content = re.sub(r'(_sat\([^)]+\))\s*\.unwrap_or\([^)]+\)', r'\1', content)
    # with newlines:
    content = re.sub(r'(_sat\([^)]+\))\s*\n\s*\.unwrap_or\([^)]+\)', r'\1', content)
    content = re.sub(r'(_sat\([^)]+\))\s*\.expect\([^)]+\)', r'\1', content)

    # In batch.rs:
    content = content.replace('crate::numeric_cast::f64_to_u64_sat((value * 1000.0).round(), "float_ord_key")\n            .unwrap_or(u64::MAX)', 'crate::numeric_cast::f64_to_u64_sat((value * 1000.0).round())')

    if content != original:
        with open(filepath, 'w') as f:
            f.write(content)

def main():
    for root, dirs, files in os.walk('crates/shared_utils/src'):
        for file in files:
            if file.endswith('.rs'):
                process_file(os.path.join(root, file))

if __name__ == '__main__':
    main()
