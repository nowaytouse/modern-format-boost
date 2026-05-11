import json
import subprocess
import os

def run_cargo_check():
    result = subprocess.run(['cargo', 'check', '--message-format=json'], capture_output=True, text=True)
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

def main():
    errors = run_cargo_check()
    file_changes = {}
    
    # Read files
    for error in errors:
        for span in error.get('spans', []):
            if span.get('is_primary'):
                filename = span['file_name']
                if filename.startswith('crates/shared_utils/src') and filename not in file_changes:
                    with open(filename, 'r') as f:
                        file_changes[filename] = f.read()

    # Apply simple name fixes
    for filename, content in file_changes.items():
        content = content.replace('i128_to_i64_sat', 'u128_to_i64_sat')
        content = content.replace('parse_option_sat', 'parse_option_strict')
        content = content.replace('parse_sat', 'parse_strict')
        content = content.replace('option_f32_sat', 'option_f32_strict')
        content = content.replace('f64_to_rational_sat', 'f64_to_rational_strict')
        content = content.replace('usize_to_u64_sat', 'usize_to_u64')
        file_changes[filename] = content

    for filename, content in file_changes.items():
        with open(filename, 'w') as f:
            f.write(content)
            
if __name__ == '__main__':
    main()
