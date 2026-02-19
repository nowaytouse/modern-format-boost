#!/usr/bin/env python3
"""
JXL Container to Codestream Converter
Extracts bare codestream from ISOBMFF container for iCloud Photos compatibility

Part of Modern Format Boost - Premium Media Optimizer
"""

import sys
import struct
import os
from pathlib import Path
from typing import Optional, Tuple, List

def read_box(f) -> Tuple[Optional[bytes], Optional[int], Optional[int], Optional[int]]:
    """Read ISOBMFF box header"""
    size_data = f.read(4)
    if len(size_data) < 4:
        return None, None, None, None
    
    size = struct.unpack('>I', size_data)[0]
    box_type = f.read(4)
    
    # 64-bit size extension
    if size == 1:
        size = struct.unpack('>Q', f.read(8))[0]
        header_size = 16
    else:
        header_size = 8
    
    return box_type, size, header_size, f.tell() - header_size

def is_jxl_container(filepath: str) -> bool:
    """Check if file is JXL container format (not bare codestream)"""
    try:
        with open(filepath, 'rb') as f:
            sig = f.read(12)
            # Container: 0x00 0x00 0x00 0x0C 'JXL '
            if sig[:4] == b'\x00\x00\x00\x0c':
                return True
            # Bare codestream: 0xFF 0x0A
            if sig[:2] == b'\xff\x0a':
                return False
    except:
        pass
    return False

def extract_codestream(input_path: str, output_path: str, verbose: bool = False) -> bool:
    """Extract bare codestream from JXL container"""
    
    try:
        with open(input_path, 'rb') as f:
            # Check signature
            sig = f.read(12)
            if sig[:4] != b'\x00\x00\x00\x0c':
                # Already bare codestream
                if sig[:2] == b'\xff\x0a':
                    if verbose:
                        print(f"   ⊘ Already bare codestream, copying...")
                    f.seek(0)
                    with open(output_path, 'wb') as out:
                        out.write(f.read())
                    return True
                else:
                    if verbose:
                        print(f"   ✗ Not a valid JXL file")
                    return False
            
            if verbose:
                print(f"   ✓ JXL container detected")
            
            # Collect codestream parts from jxlc/jxlp boxes
            codestream_parts: List[Tuple[int, bytes]] = []
            
            while True:
                box_type, size, header_size, box_start = read_box(f)
                
                if box_type is None:
                    break
                
                if box_type == b'jxlc':
                    # Complete codestream box
                    if verbose:
                        print(f"   ✓ Found jxlc box (complete codestream)")
                    codestream_size = size - header_size
                    codestream_data = f.read(codestream_size)
                    codestream_parts.append((0, codestream_data))
                    break
                    
                elif box_type == b'jxlp':
                    # Partial codestream box
                    if verbose:
                        print(f"   ✓ Found jxlp box (partial codestream)")
                    
                    # Read index (4 bytes)
                    index = struct.unpack('>I', f.read(4))[0]
                    
                    # Read codestream data
                    data_size = size - header_size - 4
                    data = f.read(data_size)
                    codestream_parts.append((index, data))
                    continue
                else:
                    # Skip this box
                    f.seek(box_start + size)
            
            if not codestream_parts:
                if verbose:
                    print(f"   ✗ No jxlc/jxlp box found")
                return False
            
            # Merge codestream data
            if len(codestream_parts) > 1:
                if verbose:
                    print(f"   Merging {len(codestream_parts)} jxlp boxes...")
                codestream_parts.sort(key=lambda x: x[0])
            
            codestream_data = b''.join([data for _, data in codestream_parts])
            
            # Write output
            with open(output_path, 'wb') as out:
                out.write(codestream_data)
            
            if verbose:
                print(f"   ✓ Extracted {len(codestream_data):,} bytes")
            
            # Verify header
            if codestream_data[:2] != b'\xff\x0a':
                if verbose:
                    print(f"   ⚠️  Warning: Unexpected header {codestream_data[:2].hex()}")
                return False
            
            return True
            
    except Exception as e:
        if verbose:
            print(f"   ✗ Error: {e}")
        return False

def main():
    if len(sys.argv) < 3:
        print("Usage: jxl_container_fixer.py <input.jxl> <output.jxl> [--verbose]")
        sys.exit(1)
    
    input_file = sys.argv[1]
    output_file = sys.argv[2]
    verbose = '--verbose' in sys.argv
    
    if not os.path.exists(input_file):
        print(f"✗ Input file not found: {input_file}")
        sys.exit(1)
    
    if verbose:
        print(f"Processing: {Path(input_file).name}")
    
    if extract_codestream(input_file, output_file, verbose):
        if verbose:
            input_size = os.path.getsize(input_file)
            output_size = os.path.getsize(output_file)
            overhead = input_size - output_size
            print(f"\n✓ Success")
            print(f"  Container: {input_size:,} bytes")
            print(f"  Codestream: {output_size:,} bytes")
            print(f"  Overhead removed: {overhead:,} bytes")
        sys.exit(0)
    else:
        if verbose:
            print(f"\n✗ Failed to extract codestream")
        sys.exit(1)

if __name__ == '__main__':
    main()
