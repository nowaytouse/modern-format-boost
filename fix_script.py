import re
import os
import json
import subprocess

def run_cargo_check():
    result = subprocess.run(['cargo', 'check', '--message-format=json'], capture_output=True, text=True, cwd='crates/shared_utils')
    lines = result.stdout.split('\n')
    errors = []
    for line in lines:
        if not line:
            continue
        try:
            msg = json.loads(line)
            if msg.get('reason') == 'compiler-message' and msg['message']['level'] == 'error':
                errors.append(msg['message'])
        except json.JSONDecodeError:
            pass
    return errors

def fix_all():
    changed = True
    while changed:
        changed = False
        errors = run_cargo_check()
        if not errors:
            break
        
        file_changes = {}
        
        for error in errors:
            spans = error.get('spans', [])
            if not spans:
                continue
            
            primary_span = next((s for s in spans if s.get('is_primary')), None)
            if not primary_span:
                continue
                
            file_name = primary_span['file_name']
            if file_name not in file_changes:
                with open(os.path.join('crates/shared_utils', file_name), 'r') as f:
                    file_changes[file_name] = f.read()
                    
            content = file_changes[file_name]
            
            # Use line_start, line_end, column_start, column_end
            line_start = primary_span['line_start'] - 1
            line_end = primary_span['line_end'] - 1
            col_start = primary_span['column_start'] - 1
            col_end = primary_span['column_end'] - 1
            
            lines = content.split('\n')
            
            if error['code'] and error['code']['code'] == 'E0425':
                # cannot find function X
                if "i128_to_i64_sat" in error['message']:
                    # Replace with i128_to_i64_strict and .unwrap_or(0) maybe?
                    # Let's replace i128_to_i64_sat -> u128_to_i64_sat if the compiler suggests it.
                    content = content.replace("i128_to_i64_sat", "u128_to_i64_sat")
                    changed = True
                elif "parse_option_sat" in error['message']:
                    content = content.replace("parse_option_sat", "parse_option_strict")
                    changed = True
                elif "parse_sat" in error['message']:
                    content = content.replace("parse_sat", "parse_strict")
                    changed = True
                elif "option_f32_sat" in error['message']:
                    content = content.replace("option_f32_sat", "option_f32_strict")
                    changed = True
                elif "f64_to_rational_sat" in error['message']:
                    content = content.replace("f64_to_rational_sat", "f64_to_rational_strict")
                    changed = True
                elif "usize_to_u64_sat" in error['message']:
                    content = content.replace("usize_to_u64_sat", "usize_to_u64")
                    changed = True

            elif error['code'] and error['code']['code'] == 'E0061':
                # this function takes N arguments but M were supplied
                msg = error['message']
                if "1 argument was supplied" in msg and "_strict" in lines[line_start]:
                    # Missing name argument
                    # We need to insert `"name"` as second argument.
                    # This is tricky without syntax parsing. 
                    pass
                if "2 arguments were supplied" in msg and "_sat" in lines[line_start]:
                    # Too many arguments for _sat. We need to remove the second argument.
                    pass
            
            file_changes[file_name] = content
            
        for file_name, content in file_changes.items():
            with open(os.path.join('crates/shared_utils', file_name), 'w') as f:
                f.write(content)
                
        if changed:
            print("Fixed some basic name errors.")
            break # Let's not loop infinitely yet, just do basic name replacements.

fix_all()
