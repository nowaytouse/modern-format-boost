import os
import shutil
from pathlib import Path

# Configuration
SOURCE_DIR = Path("/Users/nyamiiko/Downloads/all/1")
DEST_DIR = Path("/Users/nyamiiko/Downloads/all/1_optimized")
RESUME_DIR = Path("/Users/nyamiiko/Downloads/all/2")

def get_stem_map(directory):
    """
    Creates a map of {relative_path_stem: full_path} for files in a directory.
    Ignores extensions to match source files to converted files (e.g. image.png -> image.jxl).
    Handles relative paths to support recursive structures.
    """
    stem_map = set()
    if not directory.exists():
        return stem_map
        
    for root, _, files in os.walk(directory):
        for file in files:
            if file.startswith('.'):
                continue
            # Calculate relative path from the base directory
            rel_path = Path(root).relative_to(directory)
            # Use the stem of the filename (without extension)
            # Combine relative path + stem as the key
            # Example: "sub/folder/image.jpg" -> "sub/folder/image"
            key = str(rel_path / Path(file).stem)
            stem_map.add(key)
    return stem_map

def main():
    print(f"🔍 Analyzing...")
    print(f"   Source: {SOURCE_DIR}")
    print(f"   Dest:   {DEST_DIR}")
    
    if not SOURCE_DIR.exists():
        print("❌ Source directory not found!")
        return

    # Get set of processed file stems (relative to root)
    processed_stems = get_stem_map(DEST_DIR)
    print(f"   Found {len(processed_stems)} processed items in destination.")

    to_copy = []
    
    # Scan source directory
    for root, _, files in os.walk(SOURCE_DIR):
        for file in files:
            if file.startswith('.'):
                continue
                
            src_file = Path(root) / file
            rel_path = src_file.relative_to(SOURCE_DIR)
            stem_key = str(rel_path.parent / src_file.stem)
            
            # Check if this file's stem exists in the processed set
            if stem_key not in processed_stems:
                to_copy.append(src_file)

    print(f"📋 Found {len(to_copy)} files remaining to process.")
    
    if len(to_copy) == 0:
        print("✅ All files appear to be processed.")
        return

    print(f"🚀 Copying {len(to_copy)} files to Resume Directory: {RESUME_DIR}")
    
    if RESUME_DIR.exists():
        print(f"⚠️  Warning: Resume directory {RESUME_DIR} already exists.")
        # We don't delete it, we merge/overwrite
    else:
        RESUME_DIR.mkdir(parents=True, exist_ok=True)

    count = 0
    for src_file in to_copy:
        rel_path = src_file.relative_to(SOURCE_DIR)
        dest_file = RESUME_DIR / rel_path
        
        # Ensure parent dir exists
        dest_file.parent.mkdir(parents=True, exist_ok=True)
        
        # Copy
        shutil.copy2(src_file, dest_file)
        count += 1
        if count % 100 == 0:
            print(f"   Copied {count}/{len(to_copy)}...")

    print(f"✅ Copy complete. {count} files ready in {RESUME_DIR}")
    print(f"   You can now run the processor on: {RESUME_DIR}")

if __name__ == "__main__":
    main()
