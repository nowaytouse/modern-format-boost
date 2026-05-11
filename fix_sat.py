import os
import re
import glob

def process_file(filepath):
    with open(filepath, 'r') as f:
        content = f.read()

    # Regex to match numeric_cast::f64_to_u64_sat(val)
    # We want to replace it with numeric_cast::f64_to_u64_strict(val, "val").unwrap_or_else(|| { log_anomaly!(...); 0 })
    # But some might be chained or inside macros.
    
    # A safer approach is to replace numeric_cast::xxx_sat(expr) with
    # numeric_cast::xxx_strict(expr, "expr").unwrap_or(0 /* TODO */)
    
    # Let's just find and replace simple calls.
    pattern = re.compile(r'crate::numeric_cast::([a-z0-9_]+)_sat\(([^)]+)\)')
    
    def replacer(match):
        func = match.group(1)
        expr = match.group(2)
        # return f'crate::numeric_cast::{func}_strict({expr}, "{expr}").unwrap_or_else(|| {{ crate::log_anomaly!(crate::static_logs::messages::LABEL_NUMERIC, "Fallback 0 used for {expr}"); 0 }})'
        return f'crate::numeric_cast::{func}_strict({expr}, "{expr}").unwrap_or(0)'

    new_content = pattern.sub(replacer, content)
    
    if new_content != content:
        with open(filepath, 'w') as f:
            f.write(new_content)
        print(f"Updated {filepath}")

for root, dirs, files in os.walk('crates'):
    if 'tests' in root or 'fuzz' in root:
        continue
    for file in files:
        if file.endswith('.rs'):
            process_file(os.path.join(root, file))
