#!/usr/bin/env python3
import re
import argparse
import os
import sys
from datetime import datetime
from pathlib import Path

def parse_logs(log_paths, output_report):
    # Regex for standard MFB conversion success/fail lines:
    # Example: input.webp → output.GIF (GIF (Apple Compat): size reduced 33.7%) ✅
    # Note: Modern path names may contain spaces and special characters. We match the central → arrow.
    # The (✅|❌) indicates the final status.
    result_pattern = re.compile(r'([\S\s]+?)\s*→\s*([\S\s]+?)\s*\(([^)]+)\)\s*([✅❌])')

    # Regex for activity markers (often seen in stderr/console logs):
    # Example: 🔄 Animated→HEVC MP4 (SMART QUALITY, 2.0s): /path/to/file.webp
    activity_pattern = re.compile(r'🔄\s*Animated→([A-Z0-9\s]+)\s*\(([^)]+)\):\s*(.+)')

    modern_exts = {'.webp', '.avif', '.jxl', '.heic', '.heif'}
    target_formats = {'GIF', 'MOV', 'MP4', 'HEVC', 'AV1'}

    results = []

    for path in log_paths:
        p = Path(path)
        files = []
        if p.is_dir():
            files = list(p.glob('**/*.log')) + list(p.glob('**/error'))
        else:
            files = [p]

        for log_file in files:
            try:
                with open(log_file, 'r', encoding='utf-8', errors='ignore') as f:
                    for line in f:
                        line = line.strip()
                        if not line:
                            continue

                        # Method 1: Result Pattern (cli_runner output)
                        match = result_pattern.search(line)
                        if match:
                            source = match.group(1).split('>')[-1].strip() # Strip logger prefix if any
                            target = match.group(2).strip()
                            message = match.group(3).strip()
                            status = "SUCCESS" if match.group(4) == "✅" else "FAILED"
                            
                            src_ext = Path(source).suffix.lower()
                            tgt_ext = Path(target).suffix.upper()
                            
                            # We search for "modern to legacy" specifically
                            if src_ext in modern_exts and (tgt_ext.strip('.') in target_formats or "GIF" in message or "MOV" in message):
                                results.append({
                                    "log": log_file.name,
                                    "source": source,
                                    "target": target,
                                    "status": status,
                                    "details": message
                                })
                                continue

                        # Method 2: Activity Pattern (animated header)
                        match = activity_pattern.search(line)
                        if match:
                            target_fmt = match.group(1).strip()
                            details = match.group(2).strip()
                            source = match.group(3).strip()
                            
                            src_ext = Path(source).suffix.lower()
                            if src_ext in modern_exts and (target_fmt in target_formats or "GIF" in target_fmt or "MOV" in target_fmt):
                                results.append({
                                    "log": log_file.name,
                                    "source": source,
                                    "target": f"CONVERTED TO {target_fmt}",
                                    "status": "PROCESSING/UNKNOWN", # Activity markers don't always show final status in the same line
                                    "details": details
                                })

            except Exception as e:
                print(f"⚠️ Error reading {log_file}: {e}", file=sys.stderr)

    # Dedup and sort
    seen = set()
    unique_results = []
    for r in results:
        key = (r['source'], r['target'])
        if key not in seen:
            unique_results.append(r)
            seen.add(key)

    # Write report
    with open(output_report, 'w', encoding='utf-8') as f:
        f.write("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n")
        f.write("      MODERN FORMAT BOOST - CONVERSION ANALYSIS REPORT\n")
        f.write(f"      Generated at: {datetime.now().strftime('%Y-%m-%d %H:%M:%S')}\n")
        f.write("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n\n")
        
        if not unique_results:
            f.write("No 'Modern → Legacy' conversions (WebP/AVIF/JXL to GIF/MOV) found in logs.\n")
        else:
            f.write(f"Found {len(unique_results)} modern-to-legacy conversion events:\n\n")
            for i, r in enumerate(unique_results, 1):
                f.write(f"[{i}] SOURCE: {r['source']}\n")
                f.write(f"    TARGET: {r['target']}\n")
                f.write(f"    STATUS: {r['status']}\n")
                f.write(f"    INFO:   {r['details']}\n")
                f.write(f"    LOG:    {r['log']}\n")
                f.write("-" * 40 + "\n")

    print(f"📊 Report generated: {output_report}")
    print(f"📈 Total events found: {len(unique_results)}")

if __name__ == "__main__":
    parser = argparse.ArgumentParser(description="Analyze MFB logs for modern format conversions to GIF/MOV.")
    parser.add_argument("logs", nargs="+", help="One or more log files or directories to scan.")
    parser.add_argument("-o", "--output", help="Custom output report path.")
    
    args = parser.parse_args()
    
    timestamp = datetime.now().strftime("%Y%m%d_%H%M%S")
    output_report = args.output if args.output else f"logs/conversion_summary_{timestamp}.txt"
    
    # Ensure logs dir exists
    os.makedirs(os.path.dirname(output_report), exist_ok=True)
    
    parse_logs(args.logs, output_report)
