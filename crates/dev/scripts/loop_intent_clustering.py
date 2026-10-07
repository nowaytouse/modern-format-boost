#!/usr/bin/env python3
"""
Offline HDBSCAN clustering for loop_intent (loop_samples embeddings).

Pipeline role:
- Retrieval at inference already uses pgvector + HNSW (approximate neighbor search).
- This script discovers density clusters (no fixed K) and writes:
  - per-row `metadata.hdbscan_cluster_id` / `hdbscan_cluster_loop_prior`
  - `multi_scenario_metadata.feature_stats.hdbscan_catalog` (centroids + priors)

Rust inference fuses the nearest cluster loop-prior with the HNSW neighbor vote.
"""

from __future__ import annotations

import argparse
import json
import math
import os
import sys as _sys
import time
from collections import defaultdict
from collections.abc import Sequence
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path
from types import TracebackType
from typing import Final, Protocol

_SCRIPT_DIR = Path(__file__).resolve().parent
if str(_SCRIPT_DIR) not in _sys.path:
    _sys.path.insert(0, str(_SCRIPT_DIR))

try:
    import numpy as np  # pyright: ignore[reportMissingImports]
except ModuleNotFoundError:
    np = None  # type: ignore[assignment]

try:
    import hdbscan  # pyright: ignore[reportMissingImports]
except ModuleNotFoundError:
    hdbscan = None  # type: ignore[assignment]

try:
    import psycopg2  # pyright: ignore[reportMissingModuleSource]
except ModuleNotFoundError:
    psycopg2 = None

DEFAULT_CONNSTR: Final = "postgresql://localhost/modern_format_boost"
LOOP_INTENT_SCENARIO: Final = "loop_intent"
LOOP_TABLE: Final = "loop_samples"
MIN_SAMPLES_FOR_CLUSTERING: Final = 40
CLUSTER_LOAD_PROGRESS_INTERVAL: Final = 1000
CLUSTER_LOAD_HEARTBEAT_SECS: Final = 10.0


class DbCursor(Protocol):
    def __enter__(self) -> DbCursor: ...  # noqa: PYI034
    def __exit__(
        self,
        exc_type: type[BaseException] | None,
        exc_val: BaseException | None,
        exc_tb: TracebackType | None,
    ) -> bool | None: ...
    def execute(
        self, query: object, params: Sequence[object] | None = None
    ) -> object: ...
    def executemany(
        self, query: object, params_seq: Sequence[Sequence[object]]
    ) -> object: ...
    def fetchone(self) -> tuple[object, ...] | None: ...
    def fetchall(self) -> list[tuple[object, ...]]: ...


class DbConnection(Protocol):
    def cursor(self) -> DbCursor: ...
    def commit(self) -> object: ...
    def close(self) -> object: ...


@dataclass(frozen=True)
class LoopEmbeddingRow:
    blake3: bytes
    label: int
    embedding: list[float]


@dataclass(frozen=True)
class LoopEmbeddingLoadResult:
    rows: list[LoopEmbeddingRow]
    total_rows: int
    expected_dim: int
    rejected_bad_blake3: int
    rejected_unexpected_label: int
    rejected_unexpected_dim: int
    rejected_non_finite: int


def format_elapsed_secs(seconds: float) -> str:
    if seconds >= 10.0:
        return f"{seconds:.1f}s"
    return f"{seconds:.2f}s"


def discover_root(script_path: Path) -> Path:
    expected_relative = Path("crates") / "dev" / "scripts" / script_path.name
    for candidate in script_path.parents:
        if (candidate / "Cargo.toml").exists() and (
            candidate / expected_relative
        ).exists():
            return candidate
    raise SystemExit(
        f"Could not locate repository root from {script_path}; "
        f"expected Cargo.toml and {expected_relative.as_posix()}"
    )


def parse_pgvector_text(raw: object) -> list[float]:
    if raw is None:
        return []
    text = str(raw).strip()
    if not text:
        return []
    if text.startswith("["):
        parsed = json.loads(text)
    else:
        parsed = json.loads(f"[{text}]")
    return [float(value) for value in parsed]


def as_bytes(value: object) -> bytes | None:
    if isinstance(value, bytes):
        return value
    if isinstance(value, bytearray):
        return bytes(value)
    if isinstance(value, memoryview):
        return value.tobytes()
    return None


def read_embedding_dimension(conn: DbConnection, table: str) -> int | None:
    with conn.cursor() as cur:
        cur.execute(
            """
            SELECT format_type(a.atttypid, a.atttypmod)
            FROM pg_attribute a
            JOIN pg_class c ON c.oid = a.attrelid
            JOIN pg_namespace n ON n.oid = c.relnamespace
            WHERE n.nspname = 'public'
              AND c.relname = %s
              AND a.attname = 'embedding'
              AND NOT a.attisdropped
            """,
            (table,),
        )
        row = cur.fetchone()

    if row is None or row[0] is None:
        return None
    type_name = str(row[0]).strip()
    if not type_name.startswith("vector(") or not type_name.endswith(")"):
        return None
    try:
        return int(type_name[len("vector(") : -1])
    except ValueError:
        return None


def read_metadata_embedding_dimension(conn: DbConnection, scenario: str) -> int | None:
    with conn.cursor() as cur:
        cur.execute(
            """
            SELECT embedding_dimension
            FROM multi_scenario_metadata
            WHERE scenario = %s
            """,
            (scenario,),
        )
        row = cur.fetchone()

    if row is None or row[0] is None:
        return None
    try:
        return int(row[0])  # type: ignore[arg-type]
    except (TypeError, ValueError):
        return None


def resolve_loop_embedding_dimension(conn: DbConnection) -> int:
    table_dim = read_embedding_dimension(conn, LOOP_TABLE)
    metadata_dim = read_metadata_embedding_dimension(conn, LOOP_INTENT_SCENARIO)
    if table_dim is not None and metadata_dim is not None and table_dim != metadata_dim:
        raise SystemExit(
            "clustering_blocked=embedding_dimension_mismatch "
            f"table_dim={table_dim} metadata_dim={metadata_dim}"
        )
    resolved = metadata_dim if metadata_dim is not None else table_dim
    if resolved is None:
        raise SystemExit(
            "clustering_blocked=missing_embedding_dimension "
            f"table={LOOP_TABLE} scenario={LOOP_INTENT_SCENARIO}"
        )
    return resolved


def label_to_loop_prior(label: int) -> float | None:
    if label == 1:
        return 1.0
    if label == 0:
        return 0.0
    return None


def label_is_valid_training_class(label: int) -> bool:
    return label_to_loop_prior(label) is not None


def connect_pg(connstr: str) -> DbConnection:
    if psycopg2 is None:
        raise SystemExit(
            "psycopg2 is required; install crates/dev/scripts/requirements.txt"
        )
    return psycopg2.connect(connstr)  # type: ignore[return-value]


def load_loop_embeddings(conn: DbConnection) -> LoopEmbeddingLoadResult:
    expected_dim = resolve_loop_embedding_dimension(conn)
    with conn.cursor() as cur:
        cur.execute(
            """
            SELECT blake3, label, embedding::text
            FROM loop_samples
            WHERE embedding IS NOT NULL
              AND frame_count > 1
            """
        )
        rows = cur.fetchall()

    loaded: list[LoopEmbeddingRow] = []
    rejected_bad_blake3 = 0
    rejected_unexpected_label = 0
    rejected_unexpected_dim = 0
    rejected_non_finite = 0
    started = time.monotonic()
    last_progress_at = started
    last_progress_index = 0
    total_rows = len(rows)
    print(
        "clustering_load_start "
        f"rows={total_rows} expected_dim={expected_dim} "
        f"heartbeat_every={CLUSTER_LOAD_PROGRESS_INTERVAL} "
        f"heartbeat_max_silence={format_elapsed_secs(CLUSTER_LOAD_HEARTBEAT_SECS)}"
    )
    for index, (blake3_raw, label_raw, embedding_raw) in enumerate(rows, start=1):
        blake3 = as_bytes(blake3_raw)
        if blake3 is None:
            rejected_bad_blake3 += 1
        else:
            try:
                label = int(label_raw)  # type: ignore[arg-type]
            except (TypeError, ValueError):
                rejected_unexpected_label += 1
                continue
            if not label_is_valid_training_class(label):
                rejected_unexpected_label += 1
                continue
            embedding = parse_pgvector_text(embedding_raw)
            if len(embedding) != expected_dim:
                rejected_unexpected_dim += 1
            elif not all(
                not math.isnan(value) and abs(value) != float("inf")
                for value in embedding
            ):
                rejected_non_finite += 1
            else:
                loaded.append(
                    LoopEmbeddingRow(
                        blake3=blake3,
                        label=label,
                        embedding=embedding,
                    )
                )
        now = time.monotonic()
        if (
            index == 1
            or index == total_rows
            or index - last_progress_index >= CLUSTER_LOAD_PROGRESS_INTERVAL
            or now - last_progress_at >= CLUSTER_LOAD_HEARTBEAT_SECS
        ):
            elapsed = now - started
            rate = index / elapsed if elapsed > 0 else 0.0
            print(
                "clustering_load_progress "
                f"{index}/{total_rows} usable={len(loaded)} "
                f"reject_bad_blake3={rejected_bad_blake3} "
                f"reject_label={rejected_unexpected_label} "
                f"reject_dim={rejected_unexpected_dim} "
                f"reject_non_finite={rejected_non_finite} "
                f"rate={rate:.1f}/s elapsed={format_elapsed_secs(elapsed)}"
            )
            last_progress_at = now
            last_progress_index = index
    return LoopEmbeddingLoadResult(
        rows=loaded,
        total_rows=len(rows),
        expected_dim=expected_dim,
        rejected_bad_blake3=rejected_bad_blake3,
        rejected_unexpected_label=rejected_unexpected_label,
        rejected_unexpected_dim=rejected_unexpected_dim,
        rejected_non_finite=rejected_non_finite,
    )


def choose_min_cluster_size(sample_count: int) -> int:
    return max(5, min(50, sample_count // 30))


def run_hdbscan(matrix: np.ndarray, *, min_cluster_size: int) -> np.ndarray:
    if hdbscan is None:
        raise SystemExit(
            "hdbscan is required; pip install hdbscan (see crates/dev/scripts/requirements.txt)"
        )
    clusterer = hdbscan.HDBSCAN(
        min_cluster_size=min_cluster_size,
        min_samples=min_cluster_size,
        metric="euclidean",
        cluster_selection_method="eom",
    )
    return clusterer.fit_predict(matrix)


def build_catalog(
    rows: list[LoopEmbeddingRow],
    labels: np.ndarray,
    *,
    min_cluster_size: int,
) -> tuple[dict[str, object], list[tuple[bytes, int, float]]]:
    cluster_members: dict[int, list[LoopEmbeddingRow]] = defaultdict(list)
    per_row_updates: list[tuple[bytes, int, float]] = []
    noise_count = 0

    for row, cluster_id in zip(rows, labels.tolist(), strict=True):
        cluster_int = int(cluster_id)
        if cluster_int < 0:
            noise_count += 1
            continue
        cluster_members[cluster_int].append(row)

    clusters: list[dict[str, object]] = []
    unlabeled_cluster_count = 0
    for cluster_id in sorted(cluster_members):
        members = cluster_members[cluster_id]
        priors = [
            prior
            for member in members
            if (prior := label_to_loop_prior(member.label)) is not None
        ]
        if not priors:
            unlabeled_cluster_count += 1
            continue
        loop_prior = float(np.clip(sum(priors) / len(priors), 0.0, 1.0))
        matrix = np.array([member.embedding for member in members], dtype=np.float64)
        centroid = matrix.mean(axis=0).tolist()
        clusters.append(
            {
                "cluster_id": int(cluster_id),
                "loop_prior": float(loop_prior),
                "member_count": len(members),
                "centroid": centroid,
            }
        )
        for member in members:
            per_row_updates.append((member.blake3, int(cluster_id), float(loop_prior)))

    catalog: dict[str, object] = {
        "version": 1,
        "trained_at": datetime.now(timezone.utc).isoformat(),
        "min_cluster_size": min_cluster_size,
        "noise_count": noise_count,
        "unlabeled_cluster_count": unlabeled_cluster_count,
        "clusters": clusters,
    }
    return catalog, per_row_updates


def merge_catalog_into_feature_stats(
    conn: DbConnection, catalog: dict[str, object]
) -> None:
    with conn.cursor() as cur:
        cur.execute(
            """
            SELECT feature_stats::text
            FROM multi_scenario_metadata
            WHERE scenario = %s
            """,
            (LOOP_INTENT_SCENARIO,),
        )
        row = cur.fetchone()
        if row is None or row[0] is None:
            feature_stats: dict[str, object] = {}
        else:
            feature_stats = json.loads(str(row[0]))
        feature_stats["hdbscan_catalog"] = catalog
        cur.execute(
            """
            UPDATE multi_scenario_metadata
            SET feature_stats = %s::jsonb,
                last_updated = CURRENT_TIMESTAMP
            WHERE scenario = %s
            """,
            (json.dumps(feature_stats), LOOP_INTENT_SCENARIO),
        )


def backfill_row_metadata(
    conn: DbConnection, updates: list[tuple[bytes, int, float]]
) -> None:
    if not updates:
        return
    sql = """
        UPDATE loop_samples
        SET metadata = jsonb_set(
            jsonb_set(
                COALESCE(metadata, '{}'::jsonb),
                '{hdbscan_cluster_id}',
                to_jsonb(%s::int),
                true
            ),
            '{hdbscan_cluster_loop_prior}',
            to_jsonb(%s::double precision),
            true
        )
        WHERE blake3 = %s
    """
    params = [
        (cluster_id, loop_prior, blake3) for blake3, cluster_id, loop_prior in updates
    ]
    with conn.cursor() as cur:
        cur.executemany(sql, params)


def run_clustering(connstr: str, *, min_cluster_size: int | None) -> int:
    if np is None:
        print("clustering_blocked=missing_numpy")
        return 1

    conn = connect_pg(connstr)
    try:
        load_result = load_loop_embeddings(conn)
        rows = load_result.rows
        if (
            load_result.rejected_bad_blake3
            or load_result.rejected_unexpected_label
            or load_result.rejected_unexpected_dim
            or load_result.rejected_non_finite
        ):
            print(
                "clustering_input_audit "
                f"total={load_result.total_rows} usable={len(rows)} "
                f"expected_dim={load_result.expected_dim} "
                f"rejected_bad_blake3={load_result.rejected_bad_blake3} "
                f"rejected_label={load_result.rejected_unexpected_label} "
                f"rejected_dim={load_result.rejected_unexpected_dim} "
                f"rejected_non_finite={load_result.rejected_non_finite}"
            )
        if (
            len(rows) == 0
            and load_result.total_rows > 0
            and (
                load_result.rejected_bad_blake3
                or load_result.rejected_unexpected_label
                or load_result.rejected_unexpected_dim
                or load_result.rejected_non_finite
            )
        ):
            print(
                "clustering_blocked=no_usable_embeddings "
                f"total={load_result.total_rows} expected_dim={load_result.expected_dim}"
            )
            return 2
        if len(rows) < MIN_SAMPLES_FOR_CLUSTERING:
            print(
                f"clustering_skipped=insufficient_samples "
                f"have={len(rows)} need>={MIN_SAMPLES_FOR_CLUSTERING}"
            )
            return 0

        mcs = min_cluster_size or choose_min_cluster_size(len(rows))
        print(
            "clustering_hdbscan_start "
            f"samples={len(rows)} min_cluster_size={mcs} "
            f"embedding_dim={load_result.expected_dim}"
        )
        matrix = np.array([row.embedding for row in rows], dtype=np.float64)
        labels = run_hdbscan(matrix, min_cluster_size=mcs)
        print("clustering_hdbscan_done labels_ready=yes")
        catalog, updates = build_catalog(rows, labels, min_cluster_size=mcs)
        if not catalog.get("clusters"):
            print("clustering_skipped=no_valid_clusters_after_hdbscan")
            return 0
        print(
            "clustering_writeback_start "
            f"clusters={len(catalog.get('clusters', []))} row_updates={len(updates)}"
        )
        merge_catalog_into_feature_stats(conn, catalog)
        backfill_row_metadata(conn, updates)
        print("clustering_writeback_commit")
        conn.commit()

        cluster_count = len(catalog.get("clusters", []))
        noise_count = int(catalog.get("noise_count", 0))
        print(
            f"clustering_result=ok samples={len(rows)} "
            f"clusters={cluster_count} noise={noise_count} min_cluster_size={mcs}"
        )
        return 0
    finally:
        conn.close()


def main() -> None:
    from mfb_entry_guard import guard_main

    guard_main("loop_intent_clustering.py", require_invoker=True)
    parser = argparse.ArgumentParser(
        description="HDBSCAN clustering for loop_intent embeddings (offline finalize step)"
    )
    parser.add_argument(
        "--connstr",
        default=os.environ.get("MFB_PG_CONNSTR", DEFAULT_CONNSTR),
        help="PostgreSQL connection string",
    )
    parser.add_argument(
        "--min-cluster-size",
        type=int,
        default=None,
        help="HDBSCAN min_cluster_size (default: adaptive from corpus size)",
    )
    args = parser.parse_args()
    raise SystemExit(
        run_clustering(args.connstr, min_cluster_size=args.min_cluster_size)
    )


if __name__ == "__main__":
    main()
