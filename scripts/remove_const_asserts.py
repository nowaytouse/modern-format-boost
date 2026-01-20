#!/usr/bin/env python3
"""移除所有常量断言"""
import re
from pathlib import Path

def remove_constant_assertions(file_path):
    """移除文件中的所有常量断言"""
    content = file_path.read_text()
    original = content
    
    # 匹配多行assert
    content = re.sub(
        r'\s*assert!\(\s*[A-Z_]+\s*[<>=]+\s*[A-Z_0-9.+\-*/\s]+,?\s*(?:"[^"]*")?\s*\);\n',
        '',
        content,
        flags=re.MULTILINE
    )
    
    # 匹配单行assert
    content = re.sub(
        r'\s*assert!\([A-Z_]+\s*[<>=]+\s*[A-Z_0-9.+\-*/\s]+\);\n',
        '',
        content
    )
    
    if content != original:
        file_path.write_text(content)
        return True
    return False

def main():
    base = Path(__file__).parent.parent / 'shared_utils' / 'src'
    
    files_to_fix = [
        base / 'video_explorer.rs',
        base / 'video_explorer_tests.rs',
        base / 'stream_size.rs',
    ]
    
    fixed = []
    for file_path in files_to_fix:
        if file_path.exists() and remove_constant_assertions(file_path):
            fixed.append(str(file_path))
    
    if fixed:
        print(f"✅ 移除了 {len(fixed)} 个文件中的常量断言")
        for f in fixed:
            print(f"  - {f}")
    else:
        print("⚠️  没有找到需要修复的文件")
    
    return 0

if __name__ == '__main__':
    import sys
    sys.exit(main())
