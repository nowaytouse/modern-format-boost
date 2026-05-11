import os
import re

def fix_file(filepath):
    with open(filepath, 'r') as f:
        content = f.read()

    # The corrupted pattern looks like:
    # crate::numeric_cast::FUNC_strict(EXPR, "EXPR").unwrap_or(0))
    # where EXPR has an unclosed parenthesis, like `val.ceil(`
    # So the full corrupted string is:
    # crate::numeric_cast::FUNC_strict(val.ceil(, "val.ceil(").unwrap_or(0))
    
    # We want to replace `(, ".*"\)\.unwrap_or\(0\)` with `().unwrap_or(0)`
    pattern1 = re.compile(r'\(, "[^"]+"\)\.unwrap_or\(0\)')
    new_content = pattern1.sub('().unwrap_or(0)', content)
    
    # What about `u32::from_be_bytes([..., "u32::from_be_bytes([").unwrap_or(0)`?
    # Error: `], "u32::from_be_bytes([...").unwrap_or(0)`
    pattern2 = re.compile(r'\], "[^"]+"\)\.unwrap_or\(0\)')
    new_content = pattern2.sub(']).unwrap_or(0)', new_content)

    # What about `read_u64(ifd_pos, "read_u64(ifd_pos").unwrap_or(0)`?
    # Error: `read_u64(ifd_pos, "read_u64(ifd_pos").unwrap_or(0)`
    # This is trickier because it might match valid code.
    # Let's target exactly the broken ones. We know they are always `.unwrap_or(0)`.
    # And they always have a `"string"` right before the `)`.
    # Let's just find `, "[^"]+"\)\.unwrap_or\(0\)` and replace with `).unwrap_or(0)`
    pattern3 = re.compile(r', "[^"]+"\)\.unwrap_or\(0\)')
    new_content = pattern3.sub(').unwrap_or(0)', new_content)
    
    if new_content != content:
        with open(filepath, 'w') as f:
            f.write(new_content)
        print(f"Fixed {filepath}")

for root, dirs, files in os.walk('crates/shared_utils/src'):
    for file in files:
        if file.endswith('.rs'):
            fix_file(os.path.join(root, file))
