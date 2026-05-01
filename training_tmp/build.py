import os
import shutil
import subprocess
import uuid
import json
import re
from pathlib import Path

SOURCE_DIRS = [
    "/Users/nyamiiko/Downloads/优化/1一批",
    "/Users/nyamiiko/Downloads/优化/2二批",
    "/Users/nyamiiko/Downloads/优化/3 三批",
    "/Users/nyamiiko/Downloads/优化/4 四批/闷茶子",
    "/Users/nyamiiko/Downloads/优化/4 四批/蜜汁工坊",
    "/Users/nyamiiko/Downloads/优化/4 四批/待查询",
    "/Users/nyamiiko/Downloads/优化/4 四批/鬼针草",
    "/Users/nyamiiko/Downloads/FINAL DONE/⭕️短"
]

TMP_DIR = Path("/Users/nyamiiko/Downloads/GitHub/modern_format_boost/training_tmp")
STAGING_DIR = TMP_DIR / "staging"
CATEGORIES = {
    "hq_static": TMP_DIR / "hq_static",
    "lq_static": TMP_DIR / "lq_static",
    "meme_anim": TMP_DIR / "meme_anim",
    "long_video": TMP_DIR / "long_video",
    "art_loop": TMP_DIR / "art_loop",
    "meme_square": TMP_DIR / "meme_square"
}

for d in list(CATEGORIES.values()) + [STAGING_DIR]:
    if d.exists():
        shutil.rmtree(d, ignore_errors=True)
    d.mkdir(parents=True, exist_ok=True)

ANIM_EXTS = {'.gif', '.webp', '.mp4', '.mov', '.mkv', '.webm', '.apng', '.avif'}
STATIC_EXTS = {'.jpg', '.jpeg', '.png', '.heic', '.heif', '.jxl', '.tiff', '.bmp'}
MODERN_ANIM_EXTS = {'.webp', '.apng', '.avif'}

def has_icc_profile(path):
    try:
        res = subprocess.run(["identify", "-format", "%[profiles]", str(path)], capture_output=True, text=True, timeout=5)
        return "icc" in res.stdout.lower() or "icm" in res.stdout.lower()
    except: return False

def get_dpi(path):
    try:
        res = subprocess.run(["identify", "-format", "%x", str(path)], capture_output=True, text=True, timeout=5)
        match = re.search(r"(\d+)", res.stdout)
        return int(match.group(1)) if match else 72
    except: return 72

def check_real_audio(path):
    try:
        res = subprocess.run(["ffmpeg", "-t", "5", "-i", str(path), "-af", "volumedetect", "-f", "null", "-"], capture_output=True, text=True, timeout=10)
        match = re.search(r"max_volume: ([\-\d\.]+) dB", res.stderr)
        return float(match.group(1)) > -60.0 if match else False
    except: return False

def get_media_info_safe(path):
    try:
        res = subprocess.run(["ffprobe", "-v", "error", "-show_entries", "format=duration:stream=width,height,codec_type", "-of", "json", str(path)], capture_output=True, text=True, timeout=5)
        data = json.loads(res.stdout)
        duration = float(data.get("format", {}).get("duration", 0))
        streams = data.get("streams", [])
        has_audio = any(s.get("codec_type") == "audio" for s in streams)
        v_stream = next((s for s in streams if s.get("codec_type") == "video"), None)
        width = int(v_stream.get("width", 0)) if v_stream else 0
        height = int(v_stream.get("height", 0)) if v_stream else 0
        has_real_sound = check_real_audio(path) if has_audio else False
        return duration, width, height, has_real_sound
    except: return 0.0, 0, 0, False

print("Scanning with RELAXED categories (1080p Art, Sound-based Long Video)...")
counts = {k: 0 for k in CATEGORIES}

for sdir in SOURCE_DIRS:
    if not os.path.exists(sdir): continue
    for root, _, files in os.walk(sdir):
        for f in files:
            ext = os.path.splitext(f)[1].lower()
            path = Path(root) / f
            try: size = path.stat().st_size
            except: continue
            if ext not in STATIC_EXTS and ext not in ANIM_EXTS: continue
            
            staging_path = STAGING_DIR / f"temp_{uuid.uuid4().hex}{ext}"
            shutil.copy2(path, staging_path)
            dur, w, h, has_real_sound = get_media_info_safe(staging_path)
            pixels = w * h
            is_2k = (w >= 2560 or h >= 1440 or pixels >= 3686400)
            is_1080p = (w >= 1920 or h >= 1080 or pixels >= 2000000)
            is_480p = (w > 0 and h > 0 and pixels <= 400000)
            
            matched = False
            if ext in STATIC_EXTS:
                dpi = get_dpi(staging_path)
                if (is_2k or dpi >= 300) and size > 1 * 1024 * 1024 and counts["hq_static"] < 100:
                    shutil.move(staging_path, CATEGORIES["hq_static"] / f"{uuid.uuid4().hex}{ext}")
                    counts["hq_static"] += 1
                    matched = True
                elif is_480p and dpi <= 72 and counts["lq_static"] < 100:
                    shutil.move(staging_path, CATEGORIES["lq_static"] / f"{uuid.uuid4().hex}{ext}")
                    counts["lq_static"] += 1
                    matched = True
            elif ext in ANIM_EXTS:
                # Long Video: Duration > 15s AND HAS SOUND
                if dur > 15 and has_real_sound and counts["long_video"] < 100:
                    shutil.move(staging_path, CATEGORIES["long_video"] / f"{uuid.uuid4().hex}{ext}")
                    counts["long_video"] += 1
                    matched = True
                # Art Loop: (1080p+ OR ICC OR Modern Format) AND < 10s AND SILENT
                elif 0 < dur < 10 and not has_real_sound:
                    has_icc = has_icc_profile(staging_path)
                    is_modern = ext in MODERN_ANIM_EXTS
                    if (is_1080p or has_icc or is_modern) and counts["art_loop"] < 100:
                        shutil.move(staging_path, CATEGORIES["art_loop"] / f"{uuid.uuid4().hex}{ext}")
                        counts["art_loop"] += 1
                        matched = True
                
                # Meme Anim: <= 480p, short
                if not matched and is_480p and 0 < dur < 10 and counts["meme_anim"] < 100:
                    shutil.move(staging_path, CATEGORIES["meme_anim"] / f"{uuid.uuid4().hex}{ext}")
                    counts["meme_anim"] += 1
                    matched = True
            
            if not matched and w > 0 and h > 0 and 0.9 <= w/h <= 1.1 and w < 400 and counts["meme_square"] < 100:
                shutil.move(staging_path, CATEGORIES["meme_square"] / f"{uuid.uuid4().hex}{ext}")
                counts["meme_square"] += 1
                matched = True
            if not matched: staging_path.unlink()

print("Extraction complete.")
for k, v in counts.items(): print(f"  {k}: {v}")
