import json
import os
import sys

def apply_fixes(json_file):
    suggestions = []
    with open(json_file, 'r') as f:
        for line in f:
            try:
                data = json.loads(line)
            except json.JSONDecodeError:
                continue
            
            if data.get('reason') != 'compiler-message':
                continue
            
            message = data.get('message', {})
            code = message.get('code')
            if not code or code.get('code') != 'clippy::default_numeric_fallback':
                continue
            
            # Find the suggestion
            for child in message.get('children', []):
                if child.get('message') == 'consider adding suffix':
                    for span in child.get('spans', []):
                        if span.get('suggested_replacement'):
                            suggestions.append({
                                'file': span['file_name'],
                                'byte_start': span['byte_start'],
                                'byte_end': span['byte_end'],
                                'replacement': span['suggested_replacement']
                            })
    
    print(f"Found {len(suggestions)} suggestions.")
    
    # Group by file
    files = {}
    for s in suggestions:
        files.setdefault(s['file'], []).append(s)
    
    for file_path, file_suggestions in files.items():
        if not os.path.exists(file_path):
            print(f"File not found: {file_path}")
            continue
            
        # Sort suggestions by byte_start in reverse order
        file_suggestions.sort(key=lambda x: x['byte_start'], reverse=True)
        
        with open(file_path, 'rb') as f:
            content = f.read()
        
        new_content = bytearray(content)
        for s in file_suggestions:
            start = s['byte_start']
            end = s['byte_end']
            replacement = s['replacement'].encode('utf-8')
            
            # Basic sanity check: ensure the original text matches what we expect
            # Actually, clippy gives us the exact bytes to replace.
            new_content[start:end] = replacement
            
        with open(file_path, 'wb') as f:
            f.write(new_content)
        print(f"Applied {len(file_suggestions)} fixes to {file_path}")

if __name__ == "__main__":
    apply_fixes('clippy_suggestions.json')
