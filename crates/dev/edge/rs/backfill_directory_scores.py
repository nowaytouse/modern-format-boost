#!/usr/bin/env python3
"""
[DEPRECATED] Backfill `directory_loop_intent_score` by recomputing it from `source_path`.

⚠️ This script is deprecated and kept for reference only.

The logic has been moved to:
- Configuration: crates/dev/config/directory_keywords.json
- Implementation: Should be in Rust code (database.rs or similar)

This approach is better because:
1. Keywords are in a JSON config file (easy to update)
2. Scoring logic runs automatically when inserting/updating samples
3. No need for manual backfill scripts
4. Consistent scoring across all operations

If you need to backfill existing data, consider implementing this in Rust
as a one-time migration or database maintenance function.
"""

from __future__ import annotations

import os
from pathlib import Path

import psycopg2

DEFAULT_CONN = os.getenv("MFB_PG_CONNSTR", "host=localhost dbname=modern_format_boost")

MEME_KEYWORDS = [
    "meme",
    "memes",
    "sticker",
    "stickers",
    "emoji",
    "emojis",
    "reaction",
    "reactions",
    "sticker_pack",
    "sticker_pkg",
    "sticker_collection",
    "meme_collection",
    "funny",
    "humor",
]


def compute_directory_score(source_path: str | None) -> float:
    if not source_path:
        return 0.5
    try:
        p = Path(source_path)
        parents = list(p.parent.parts)
        max_depth = 3
        last = parents[-max_depth:] if len(parents) >= max_depth else parents
        matches = 0
        for part in last:
            lower = part.lower()
            if any(kw in lower for kw in MEME_KEYWORDS):
                matches += 1
        score = 0.5 + (matches / max_depth) * 0.5
        return max(0.0, min(1.0, score))
    except Exception:
        return 0.5


def main() -> None:
    conn = psycopg2.connect(DEFAULT_CONN)
    cur = conn.cursor()
    # Ensure schema has numeric score + conservative hint to tolerate older DBs
    cur.execute(
        "ALTER TABLE samples ADD COLUMN IF NOT EXISTS directory_loop_intent_score DOUBLE PRECISION DEFAULT 0.5"
    )
    cur.execute(
        "ALTER TABLE samples ADD COLUMN IF NOT EXISTS directory_meme_hint BOOLEAN DEFAULT false"
    )
    conn.commit()
    cur.execute("SELECT file_hash, source_path FROM samples")
    rows = cur.fetchall()
    updates = []
    for file_hash, source_path in rows:
        score = compute_directory_score(source_path)
        updates.append((score, file_hash))

    print(f"Updating {len(updates)} rows with computed directory_loop_intent_score...")
    cur.executemany(
        "UPDATE samples SET directory_loop_intent_score = %s WHERE file_hash = %s",
        updates,
    )
    conn.commit()
    cur.close()
    conn.close()
    print("Backfill complete.")


if __name__ == "__main__":
    main()
