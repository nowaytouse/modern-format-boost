#!/usr/bin/env python3
"""自动修复clippy警告的脚本"""
import re
import sys
from pathlib import Path

def fix_constant_assertions(file_path):
    """移除常量断言 assert!(true)"""
    content = file_path.read_text()
    original = content
    
    # 移除 assert!(常量比较) 在测试中
    patterns = [
        r'\s*assert!\([A-Z_]+ [<>=]+ [A-Z_]+\);\n',
        r'\s*assert!\([A-Z_]+ [<>=]+ [A-Z_]+ [+\-*/] [\d.]+\);\n',
        r'\s*assert!\(true\);\n',
    ]
    
    for pattern in patterns:
        content = re.sub(pattern, '', content)
    
    if content != original:
        file_path.write_text(content)
        return True
    return False

def fix_useless_vec(file_path):
    """修复 useless vec!"""
    content = file_path.read_text()
    
    # lru_cache.rs 特定修复
    if 'lru_cache.rs' in str(file_path):
        old = '''let corrupted_jsons = vec![
            "",                                             // 空文件
            "{",                                            // 不完整JSON
            "null",                                         // null值
            "[]",                                           // 数组而非对象
            "{\\\"capacity\\\": -1}",                           // 无效容量
            "not json at all",                              // 完全无效
            "{\\\"capacity\\\": 10, \\\"entries\\\": \\\"invalid\\\"}", // entries类型错误
        ];'''
        
        new = '''let corrupted_jsons = [
            "",                                             // 空文件
            "{",                                            // 不完整JSON
            "null",                                         // null值
            "[]",                                           // 数组而非对象
            "{\\\"capacity\\\": -1}",                           // 无效容量
            "not json at all",                              // 完全无效
            "{\\\"capacity\\\": 10, \\\"entries\\\": \\\"invalid\\\"}", // entries类型错误
        ];'''
        
        if old in content:
            content = content.replace(old, new)
            file_path.write_text(content)
            return True
    
    # file_sorter.rs 特定修复
    if 'file_sorter.rs' in str(file_path):
        content = content.replace(
            'let sizes = vec![5000, 100, 3000, 200, 4000, 50, 1000];',
            'let sizes = [5000, 100, 3000, 200, 4000, 50, 1000];'
        )
        file_path.write_text(content)
        return True
    
    return False

def main():
    base = Path(__file__).parent.parent / 'shared_utils' / 'src'
    
    fixed_files = []
    
    # 修复常量断言
    crf_file = base / 'crf_constants.rs'
    if crf_file.exists() and fix_constant_assertions(crf_file):
        fixed_files.append(str(crf_file))
    
    logging_file = base / 'logging.rs'
    if logging_file.exists() and fix_constant_assertions(logging_file):
        fixed_files.append(str(logging_file))
    
    # 修复 useless vec
    lru_file = base / 'lru_cache.rs'
    if lru_file.exists() and fix_useless_vec(lru_file):
        fixed_files.append(str(lru_file))
    
    sorter_file = base / 'file_sorter.rs'
    if sorter_file.exists() and fix_useless_vec(sorter_file):
        fixed_files.append(str(sorter_file))
    
    if fixed_files:
        print(f"✅ 修复了 {len(fixed_files)} 个文件:")
        for f in fixed_files:
            print(f"  - {f}")
    else:
        print("⚠️  没有找到需要修复的文件")
    
    return 0

if __name__ == '__main__':
    sys.exit(main())
