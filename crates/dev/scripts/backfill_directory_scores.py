#!/usr/bin/env python3
"""
Backfill `directory_loop_intent_score` by recomputing it from `source_path`.

This script reads keywords from crates/dev/config/directory_keywords.json
and updates the database with computed scores based on directory names.

Usage:
    python3 crates/dev/scripts/backfill_directory_scores.py
"""

from __future__ import annotations

import json
import os
from pathlib import Path

import psycopg2

DEFAULT_CONN = os.getenv("MFB_PG_CONNSTR", "host=localhost dbname=modern_format_boost")

# Path to keywords configuration
SCRIPT_DIR = Path(__file__).parent
CONFIG_PATH = SCRIPT_DIR.parent / "config" / "directory_keywords.json"


def load_keywords_config() -> dict:
    """Load keywords and scoring configuration from JSON file."""
    if not CONFIG_PATH.exists():
        print(f"⚠️  Warning: Config file not found at {CONFIG_PATH}")
        print("   Using default configuration.")
        return {
            "keywords": {
                "meme_sticker": [
                    "meme",
                    "memes",
                    "sticker",
                    "stickers",
                    "emoji",
                    "emojis",
                    "reaction",
                    "reactions",
                ]
            },
            "scoring": {"base_score": 0.5, "max_depth": 3, "match_weight": 0.5},
        }

    with open(CONFIG_PATH, encoding="utf-8") as f:
        config = json.load(f)
        print(f"✅ Loaded configuration from {CONFIG_PATH}")
        return config


def compute_directory_score(
    source_path: str | None, keywords: list[str], scoring: dict
) -> float:
    """Compute directory loop intent score based on path and keywords.

    Args:
        source_path: File path to analyze
        keywords: List of keywords to search for in directory names
        scoring: Scoring parameters (base_score, max_depth, match_weight)

    Returns:
        Score between 0.0 and 1.0
    """
    if not source_path:
        return scoring["base_score"]

    try:
        p = Path(source_path)
        parents = list(p.parent.parts)
        max_depth = scoring["max_depth"]
        last = parents[-max_depth:] if len(parents) >= max_depth else parents
        matches = 0

        for part in last:
            lower = part.lower()
            if any(kw in lower for kw in keywords):
                matches += 1

        score = scoring["base_score"] + (matches / max_depth) * scoring["match_weight"]
        return max(0.0, min(1.0, score))
    except Exception:
        return scoring["base_score"]


def main() -> None:
    # Load configuration
    config = load_keywords_config()

    # Extract all keywords from all categories
    all_keywords = []
    for category_keywords in config["keywords"].values():
        all_keywords.extend(category_keywords)

    scoring = config["scoring"]

    print("\n📊 Configuration:")
    print(f"   Keywords: {len(all_keywords)} total")
    print(f"   Base score: {scoring['base_score']}")
    print(f"   Max depth: {scoring['max_depth']}")
    print(f"   Match weight: {scoring['match_weight']}")

    # Connect to database
    conn = psycopg2.connect(DEFAULT_CONN)
    cur = conn.cursor()

    # Ensure schema has numeric score column
    cur.execute(
        "ALTER TABLE samples ADD COLUMN IF NOT EXISTS directory_loop_intent_score DOUBLE PRECISION DEFAULT 0.5"
    )
    cur.execute(
        "ALTER TABLE samples ADD COLUMN IF NOT EXISTS directory_meme_hint BOOLEAN DEFAULT false"
    )
    conn.commit()

    # Fetch all samples
    cur.execute("SELECT file_hash, source_path FROM samples")
    rows = cur.fetchall()

    # Compute scores
    updates = []
    for file_hash, source_path in rows:
        score = compute_directory_score(source_path, all_keywords, scoring)
        updates.append((score, file_hash))

    print(
        f"\n🔄 Updating {len(updates)} rows with computed directory_loop_intent_score..."
    )
    cur.executemany(
        "UPDATE samples SET directory_loop_intent_score = %s WHERE file_hash = %s",
        updates,
    )
    conn.commit()

    # Show statistics
    cur.execute(
        "SELECT COUNT(*), AVG(directory_loop_intent_score), "
        "MIN(directory_loop_intent_score), MAX(directory_loop_intent_score) "
        "FROM samples"
    )
    count, avg, min_score, max_score = cur.fetchone()

    print("\n✅ Backfill complete!")
    print(f"   Total samples: {count}")
    print(f"   Average score: {avg:.3f}")
    print(f"   Score range: {min_score:.3f} - {max_score:.3f}")

    cur.close()
    conn.close()


if __name__ == "__main__":
    main()
