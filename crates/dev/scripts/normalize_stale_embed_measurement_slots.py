#!/usr/bin/env python3
"""Backfill stale measurement sentinels to pgvector-safe missing values.

Training and Rust inference treat optional measurement slots as absent when they
carry the DB sentinel below. pgvector rejects NaN at insert/query time, so this
script rewrites historical `0.0` sentinels to a finite out-of-band value instead.
"""

from __future__ import annotations

import argparse
import sys
from pathlib import Path

_SCRIPT_DIR = Path(__file__).resolve().parent
if str(_SCRIPT_DIR) not in sys.path:
    sys.path.insert(0, str(_SCRIPT_DIR))

from quality_regression_model import DEFAULT_CONNSTR, NULLABLE_EMBED_FEATURES  # noqa: E402

try:
    import psycopg2
    from psycopg2.extras import RealDictCursor
except ImportError:
    psycopg2 = None  # type: ignore[assignment, misc]

# 256D quality vectors in DB; optional measurement slots are 0-based.
EMBED_SLOT_INDICES = (12, 17, 18, 19, 20)
PGVECTOR_MISSING_MEASUREMENT = -1.0

QUALITY_TABLES = (
    "image_quality_samples",
    "animated_image_quality_samples",
    "video_quality_samples",
)


def parse_pgvector(text: str) -> list[float]:
    inner = text.strip()
    if inner.startswith("[") and inner.endswith("]"):
        inner = inner[1:-1]
    if not inner.strip():
        return []
    return [float(part) for part in inner.split(",")]


def format_pgvector(values: list[float]) -> str:
    return "[" + ",".join(repr(v) for v in values) + "]"


def normalize_vector(embedding: list[float]) -> tuple[list[float], int]:
    changed = 0
    out = list(embedding)
    for index in EMBED_SLOT_INDICES:
        if index >= len(out):
            continue
        if out[index] == 0.0:
            out[index] = PGVECTOR_MISSING_MEASUREMENT
            changed += 1
    return out, changed


def run(connstr: str, dry_run: bool) -> int:
    if psycopg2 is None:
        raise SystemExit("psycopg2 is required: pip install psycopg2-binary")

    total_rows = 0
    total_slots = 0
    conn = psycopg2.connect(connstr)
    try:
        with conn.cursor(cursor_factory=RealDictCursor) as cur:
            for table in QUALITY_TABLES:
                cur.execute(
                    f"""
                    SELECT id, embedding::text AS embedding_text
                    FROM {table}
                    WHERE embedding IS NOT NULL
                    """
                )
                rows = cur.fetchall()
                for row in rows:
                    vec = parse_pgvector(row["embedding_text"])
                    if len(vec) < max(EMBED_SLOT_INDICES) + 1:
                        continue
                    normalized, changed = normalize_vector(vec)
                    if changed == 0:
                        continue
                    total_rows += 1
                    total_slots += changed
                    if dry_run:
                        print(
                            f"[dry-run] {table} id={row['id']}: "
                            f"rewrite slots {EMBED_SLOT_INDICES} ({', '.join(sorted(NULLABLE_EMBED_FEATURES))})"
                        )
                        continue
                    cur.execute(
                        f"UPDATE {table} SET embedding = %s::vector WHERE id = %s",
                        (format_pgvector(normalized), row["id"]),
                    )
        if not dry_run:
            conn.commit()
    finally:
        conn.close()

    mode = "would update" if dry_run else "updated"
    print(
        f"{mode} {total_rows} row(s); {total_slots} embed slot(s) "
        f"({', '.join(sorted(NULLABLE_EMBED_FEATURES))})"
    )
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Normalize stale 0.0 PSNR/SSIM embed components to NaN in quality tables."
    )
    parser.add_argument(
        "--connstr",
        default=DEFAULT_CONNSTR,
        help=f"PostgreSQL connection string (default: {DEFAULT_CONNSTR})",
    )
    parser.add_argument(
        "--dry-run",
        action="store_true",
        help="Report rows that would change without writing",
    )
    args = parser.parse_args()
    return run(args.connstr, args.dry_run)


if __name__ == "__main__":
    raise SystemExit(main())
