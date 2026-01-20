#!/usr/bin/env python3
"""批量修复剩余的clippy警告"""
import re
from pathlib import Path

def fix_xmp_merger_field_assignment(file_path):
    """修复字段赋值问题"""
    content = file_path.read_text()
    
    # 修复 from_results 方法
    old = '''    pub fn from_results(results: &[MergeResult]) -> Self {
        let mut summary = Self::default();
        summary.total = results.len();'''
    
    new = '''    pub fn from_results(results: &[MergeResult]) -> Self {
        let mut summary = Self {
            total: results.len(),
            ..Default::default()
        };'''
    
    if old in content:
        content = content.replace(old, new)
        file_path.write_text(content)
        return True
    return False

def fix_stream_size_assertions(file_path):
    """修复stream_size.rs中的常量断言"""
    content = file_path.read_text()
    original = content
    
    # 移除常量断言
    patterns = [
        (r'\s*assert!\(STAGE_B1_MAX_ITERATIONS > 0\);\n', ''),
        (r'\s*assert!\(STAGE_B2_MAX_ITERATIONS > 0\);\n', ''),
        (r'\s*assert!\(GLOBAL_MAX_ITERATIONS >= \d+\);\n', ''),
        (r'\s*assert!\(GLOBAL_MAX_ITERATIONS <= \d+\);.*\n', ''),
        (r'\s*assert!\(BINARY_SEARCH_MAX_ITERATIONS >= \d+\);\n', ''),
        (r'\s*assert!\(BINARY_SEARCH_MAX_ITERATIONS <= \d+\);\n', ''),
    ]
    
    for pattern, replacement in patterns:
        content = re.sub(pattern, replacement, content)
    
    if content != original:
        file_path.write_text(content)
        return True
    return False

def main():
    base = Path(__file__).parent.parent / 'shared_utils' / 'src'
    fixed = []
    
    # 修复 xmp_merger.rs
    xmp_file = base / 'xmp_merger.rs'
    if xmp_file.exists() and fix_xmp_merger_field_assignment(xmp_file):
        fixed.append(str(xmp_file))
    
    # 修复 stream_size.rs
    stream_file = base / 'stream_size.rs'
    if stream_file.exists() and fix_stream_size_assertions(stream_file):
        fixed.append(str(stream_file))
    
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
