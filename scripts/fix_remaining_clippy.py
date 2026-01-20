#!/usr/bin/env python3
"""修复剩余的clippy警告"""
from pathlib import Path

def fix_flatten(file_path):
    """修复flatten问题"""
    content = file_path.read_text()
    original = content
    
    # 替换所有 .flatten() 为 .map_while(Result::ok)
    content = content.replace('.flatten()', '.map_while(Result::ok)')
    
    if content != original:
        file_path.write_text(content)
        return True
    return False

def fix_clamp(file_path):
    """修复clamp问题"""
    content = file_path.read_text()
    original = content
    
    # 查找并替换 .max().min() 模式
    import re
    pattern = r'(\w+)\.max\(([A-Z_]+)\)\.min\(([A-Z_]+)\)'
    replacement = r'\1.clamp(\2, \3)'
    content = re.sub(pattern, replacement, content)
    
    if content != original:
        file_path.write_text(content)
        return True
    return False

def main():
    base = Path(__file__).parent.parent / 'shared_utils' / 'src'
    
    files_to_check = [
        base / 'video_explorer.rs',
        base / 'stream_size.rs',
    ]
    
    fixed = []
    for file_path in files_to_check:
        if file_path.exists():
            changed = False
            if fix_flatten(file_path):
                changed = True
            if fix_clamp(file_path):
                changed = True
            if changed:
                fixed.append(str(file_path))
    
    if fixed:
        print(f"✅ 修复了 {len(fixed)} 个文件")
        for f in fixed:
            print(f"  - {f}")
    else:
        print("⚠️  没有需要修复的文件")
    
    return 0

if __name__ == '__main__':
    import sys
    sys.exit(main())
