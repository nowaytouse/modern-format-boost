import os
import re

def fix_file(filepath):
    with open(filepath, 'r') as f:
        content = f.read()

    # The corrupted pattern looks like:
    # crate::numeric_cast::FUNC_strict(EXPR.unwrap_or(0)
    # We want to restore it to:
    # crate::numeric_cast::FUNC_sat(EXPR
    
    # We must match non-greedy on the expression
    pattern = re.compile(r'crate::numeric_cast::([a-z0-9_]+)_strict\((.*?)\.unwrap_or\(0\)', re.DOTALL)
    new_content = pattern.sub(r'crate::numeric_cast::\1_sat(\2', content)
    
    if new_content != content:
        with open(filepath, 'w') as f:
            f.write(new_content)
        print(f"Reverted {filepath}")

for root, dirs, files in os.walk('crates/shared_utils/src'):
    for file in files:
        if file.endswith('.rs'):
            fix_file(os.path.join(root, file))
