#!/usr/bin/env python3
"""
Multi-scenario training database audit utility.

New-schema only:
- reads `loop_samples` / `image_quality_samples` / `animated_image_quality_samples` / `video_quality_samples`
- reads `multi_scenario_metadata`
- flags any legacy `gif_quality_*` animated-image schema objects
- never touches legacy `samples` or `sample_metadata`

Task-family split:
- `loop_intent` is loop-clustering: pgvector HNSW retrieval + HDBSCAN cluster priors (no LightGBM; finalize refreshes stats, directory scores, and HDBSCAN catalog)
- `image_quality` trains a real on-disk LightGBM model when the corpus is mature
- `animated_image_quality` / `video_quality` are quality-regression DB tables only (no GBM finalize yet)
- `finalize-runtime-assets` fills loop_intent runtime + image_quality LightGBM in one command

Before (re)training on a corpus that may predate M235, run
`cargo run --locked -p dev --bin normalize_stale_embed_measurement_slots -- --dry-run`
then without `--dry-run`
so `embedding_017` / `embedding_018` `0.0` sentinels become `NaN` in Postgres.
"""

from __future__ import annotations

import argparse
import os
import sys
from collections.abc import Callable, Sequence
from dataclasses import dataclass
from pathlib import Path
from types import TracebackType
from typing import Protocol, cast

from mfb_corpus_thresholds import (
    loop_corpus_is_mature,
    loop_corpus_samples_shortfall,
    min_loop_samples_per_class,
    min_loop_samples_total,
    min_quality_samples_per_class,
    min_quality_samples_total,
    quality_corpus_is_mature,
    quality_corpus_samples_shortfall,
)
from mfb_entry_guard import child_env_for_script, guard_main, run_delegated
from mfb_ui_tokens import pick_symbol

try:
    import psycopg2  # pyright: ignore[reportMissingModuleSource]
    from psycopg2 import sql as _pg_sql  # pyright: ignore[reportMissingModuleSource]
except ModuleNotFoundError:  # optional for `--help` and `ingest`
    psycopg2 = None
    _pg_sql = None

try:
    from tabulate import tabulate as _imported_tabulate_raw  # pyright: ignore[reportMissingModuleSource]
except ModuleNotFoundError:
    _imported_tabulate: Callable[..., str] | None = None
else:
    _imported_tabulate = cast(Callable[..., str], _imported_tabulate_raw)

TableHeaders = Sequence[str]
TableRows = Sequence[Sequence[object]]
DbRow = tuple[object, ...]
DbRows = list[DbRow]
NON_FINITE_PATTERN = r"(nan|-?infinity)"
REPLICA_SOURCE_PATTERN = "%mfb_training_replica_%"


class DbCursor(Protocol):
    def __enter__(self) -> DbCursor: ...
    def __exit__(
        self,
        exc_type: type[BaseException] | None,
        exc_val: BaseException | None,
        exc_tb: TracebackType | None,
    ) -> bool | None: ...
    def execute(
        self, query: object, params: Sequence[object] | None = None
    ) -> object: ...
    def fetchone(self) -> DbRow | None: ...
    def fetchall(self) -> DbRows: ...


class DbConnection(Protocol):
    def cursor(self) -> DbCursor: ...
    def set_session(self, *, autocommit: bool = False) -> object: ...
    def commit(self) -> object: ...
    def rollback(self) -> object: ...
    def close(self) -> object: ...


class SqlFragment(Protocol):
    def format(self, *args: object) -> object: ...


class SqlModule(Protocol):
    def SQL(self, text: str) -> SqlFragment: ...
    def Identifier(self, name: str) -> object: ...


PG_SQL: SqlModule | None = cast(SqlModule | None, _pg_sql)


@dataclass(frozen=True)
class ScenarioSpec:
    name: str
    table: str
    expected_dim: int
    score_col: str


@dataclass(frozen=True)
class QualityTableSummary:
    embedding_dim: int | None
    total: int
    null_embedding: int
    non_finite: int
    null_score: int | str
    avg_score: float | None
    replica_source_paths: int
    positive_count: int | None
    negative_count: int | None


@dataclass(frozen=True)
class ImageQualityModelStatus:
    ready_for_training: bool
    ready_for_runtime: bool
    readiness_issues: tuple[str, ...]
    model_path: Path
    metadata_path: Path
    model_exists: bool
    metadata_exists: bool


@dataclass(frozen=True)
class LoopIntentTableSummary:
    total: int
    null_embedding: int
    non_finite: int
    loop_positive_count: int
    video_negative_count: int
    non_neutral_directory_score: int
    replica_source_paths: int
    feature_stats_present: bool


@dataclass(frozen=True)
class LoopIntentRuntimeStatus:
    ready_for_knn: bool
    ready_for_runtime: bool
    readiness_issues: tuple[str, ...]
    predictor_family: str


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


def fallback_tabulate(
    rows: TableRows, headers: TableHeaders, tablefmt: str = "simple_grid"
) -> str:
    del tablefmt
    string_headers = [str(header) for header in headers]
    string_rows = [[str(cell) for cell in row] for row in rows]
    column_lengths = [len(string_headers), *(len(row) for row in string_rows)]
    column_count = max(column_lengths, default=0)
    if column_count == 0:
        return ""

    padded_rows = [
        [
            string_headers[idx] if idx < len(string_headers) else ""
            for idx in range(column_count)
        ]
    ]
    padded_rows.extend(
        [
            [row[idx] if idx < len(row) else "" for idx in range(column_count)]
            for row in string_rows
        ]
    )
    widths = [max(len(row[idx]) for row in padded_rows) for idx in range(column_count)]
    return "\n".join(
        " | ".join(cell.ljust(widths[idx]) for idx, cell in enumerate(row))
        for row in padded_rows
    )


def render_table(
    rows: TableRows, headers: TableHeaders, tablefmt: str = "simple_grid"
) -> str:
    if _imported_tabulate is None:
        return fallback_tabulate(rows, headers, tablefmt=tablefmt)
    return _imported_tabulate(rows, headers=headers, tablefmt=tablefmt)


def require_sql_module() -> SqlModule:
    if PG_SQL is None:
        raise SystemExit(
            "psycopg2.sql is required for database commands. Install psycopg2 or use `ingest`."
        )
    return PG_SQL


def sql_with_identifiers(template: str, *identifiers: str) -> object:
    sql_module = require_sql_module()
    return sql_module.SQL(template).format(
        *(sql_module.Identifier(identifier) for identifier in identifiers)
    )


def require_row(row: DbRow | None, context: str) -> DbRow:
    if row is None:
        raise RuntimeError(f"{context} returned no rows")
    return row


def as_int(value: object, context: str) -> int:
    if isinstance(value, bool):
        return int(value)
    if isinstance(value, int):
        return value
    try:
        return int(str(value))
    except (TypeError, ValueError) as exc:
        raise RuntimeError(f"{context} is not an integer: {value!r}") from exc


def as_float(value: object, context: str) -> float:
    if isinstance(value, bool):
        raise RuntimeError(f"{context} is not a float: {value!r}")
    if isinstance(value, (int, float)):
        return float(value)
    try:
        return float(str(value))
    except (TypeError, ValueError) as exc:
        raise RuntimeError(f"{context} is not a float: {value!r}") from exc


def as_optional_float(value: object, context: str) -> float | None:
    if value is None:
        return None
    return as_float(value, context)


DEFAULT_CONNSTR = "postgresql://localhost/modern_format_boost"
ROOT = discover_root(Path(__file__).resolve())
REFRESH_STATS_BIN = ROOT / "target" / "debug" / "refresh_stats"
REPAIR_LOOP_PROBE_BIN = ROOT / "target" / "debug" / "repair_loop_probe"
SHARED_UTILS_SRC_DIR = ROOT / "crates" / "foundation" / "src"
REPAIR_LOOP_PROBE_SOURCES = (
    ROOT / "crates" / "foundation" / "src" / "db" / "database.rs",
    ROOT / "crates" / "foundation" / "src" / "bin" / "repair_loop_probe.rs",
)
REFRESH_STATS_SOURCES = [
    ROOT / "Cargo.toml",
    *(SHARED_UTILS_SRC_DIR.rglob("*.rs")),
]
QUALITY_REGRESSION_MODEL_SCRIPT = (
    ROOT / "crates" / "dev" / "scripts" / "quality_regression_model.py"
)
NORMALIZE_STALE_EMBED_BIN = "normalize_stale_embed_measurement_slots"
MULTI_SCENARIO_MIGRATION_SQL = ROOT / "migrations" / "001_multi_scenario_embedding.sql"
WORKSPACE_VENV_PYTHON = ROOT / ".venv" / "bin" / "python"
QUALITY_MODEL_PYTHON_ENV = "MFB_QUALITY_MODEL_PYTHON"
IMAGE_QUALITY_MODEL_NAME = "lightgbm_model.txt"
IMAGE_QUALITY_METADATA_NAME = "lightgbm_model.metadata.json"
# Threshold accessors live in mfb_corpus_thresholds (Rust algorithm_runtime SSOT).
MIN_IMAGE_QUALITY_SAMPLES_TOTAL = min_quality_samples_total()
MIN_IMAGE_QUALITY_SAMPLES_PER_CLASS = min_quality_samples_per_class()
MIN_LOOP_SAMPLES_TOTAL = min_loop_samples_total()
MIN_LOOP_SAMPLES_PER_CLASS = min_loop_samples_per_class()
BACKFILL_DIRECTORY_SCORES_SCRIPT = None
LOOP_INTENT_CLUSTERING_SCRIPT = (
    ROOT / "crates" / "dev" / "scripts" / "loop_intent_clustering.py"
)


def append_workspace_venv_site_packages() -> None:
    site_packages = (
        ROOT
        / ".venv"
        / "lib"
        / f"python{sys.version_info.major}.{sys.version_info.minor}"
        / "site-packages"
    )
    if site_packages.exists():
        site_path = str(site_packages)
        if site_path not in sys.path:
            sys.path.append(site_path)


def preferred_training_python() -> str:
    explicit = os.environ.get(QUALITY_MODEL_PYTHON_ENV)
    if explicit and explicit.strip():
        return explicit.strip()
    if WORKSPACE_VENV_PYTHON.exists():
        return str(WORKSPACE_VENV_PYTHON)
    return sys.executable


def artifact_is_stale(artifact: Path, sources: Sequence[Path]) -> bool:
    if not artifact.exists():
        return True
    try:
        artifact_mtime = artifact.stat().st_mtime_ns
    except OSError:
        return True

    for source in sources:
        if not source.exists():
            continue
        try:
            if source.stat().st_mtime_ns > artifact_mtime:
                return True
        except OSError:
            return True
    return False


if psycopg2 is None or _imported_tabulate is None:
    append_workspace_venv_site_packages()
if psycopg2 is None:
    try:
        import psycopg2  # pyright: ignore[reportMissingModuleSource]
        from psycopg2 import sql as _pg_sql  # pyright: ignore[reportMissingModuleSource]
    except ModuleNotFoundError:
        pass
if _imported_tabulate is None:
    try:
        from tabulate import tabulate as _imported_tabulate_raw  # pyright: ignore[reportMissingModuleSource]
    except ModuleNotFoundError:
        pass
    else:
        _imported_tabulate = cast(Callable[..., str], _imported_tabulate_raw)

PG_SQL = cast(SqlModule | None, _pg_sql)

SCENARIOS: tuple[ScenarioSpec, ...] = (
    ScenarioSpec(
        name="loop_intent",
        table="loop_samples",
        expected_dim=261,
        score_col="label",
    ),
    ScenarioSpec(
        name="image_quality",
        table="image_quality_samples",
        expected_dim=256,
        score_col="quality_score",
    ),
    ScenarioSpec(
        name="animated_image_quality",
        table="animated_image_quality_samples",
        expected_dim=256,
        score_col="quality_score",
    ),
    ScenarioSpec(
        name="video_quality",
        table="video_quality_samples",
        expected_dim=256,
        score_col="quality_score",
    ),
)


def cache_base_dir() -> Path:
    base = (
        os.environ.get("MFB_HOME_ROOT")
        or os.environ.get("HOME")
        or os.environ.get("USERPROFILE")
        or str(Path.cwd())
    )
    path = Path(base)
    if path.name != ".modern_format_boost":
        path = path / ".modern_format_boost"
    return path / "cache"


def default_image_quality_model_path() -> Path:
    explicit = os.environ.get("MFB_IMAGE_QUALITY_MODEL_PATH")
    if explicit:
        return Path(explicit).expanduser()
    return cache_base_dir() / "models" / "image_quality" / IMAGE_QUALITY_MODEL_NAME


def default_image_quality_metadata_path() -> Path:
    explicit = os.environ.get("MFB_IMAGE_QUALITY_MODEL_METADATA_PATH")
    if explicit:
        return Path(explicit).expanduser()
    return cache_base_dir() / "models" / "image_quality" / IMAGE_QUALITY_METADATA_NAME


LOOP_CLUSTERING_SCENARIOS: tuple[ScenarioSpec, ...] = (SCENARIOS[0],)
QUALITY_REGRESSION_SCENARIOS: tuple[ScenarioSpec, ...] = SCENARIOS[1:]

LEGACY_TABLES: tuple[str, ...] = (
    "gif_quality_samples",
    "gif_quality_inference_log",
)
LEGACY_SEQUENCES: tuple[str, ...] = (
    "gif_quality_samples_id_seq",
    "gif_quality_inference_log_id_seq",
)
LEGACY_INDEXES: tuple[str, ...] = (
    "idx_gif_quality_blake3",
    "idx_gif_quality_hnsw",
)
LEGACY_CONSTRAINTS: tuple[str, ...] = ("gif_quality_samples_quality_score_check",)
LEGACY_TRIGGERS: tuple[str, ...] = (
    "trg_sync_gif_quality_samples_metadata",
    "trg_sync_gif_quality_samples_metadata_truncate",
)


def resolve_connstr(explicit: str | None) -> str:
    connstr = explicit or os.environ.get("MFB_PG_CONNSTR") or DEFAULT_CONNSTR
    return connstr.strip() or DEFAULT_CONNSTR


def connect_pg(connstr: str) -> DbConnection:
    if psycopg2 is None:
        raise SystemExit(
            "psycopg2 is required for database commands. Install it or use `ingest`."
        )
    try:
        conn = psycopg2.connect(connstr)
        conn.set_session(autocommit=False)
        return cast(DbConnection, conn)
    except (
        OSError,
        ValueError,
        RuntimeError,
        TypeError,
        KeyError,
        IndexError,
        AttributeError,
        UnicodeError,
    ) as exc:
        raise SystemExit(f"PostgreSQL connection failed: {exc}") from exc


def table_exists(conn: DbConnection, table: str) -> bool:
    with conn.cursor() as cur:
        cur.execute(
            """
            SELECT EXISTS (
                SELECT 1
                FROM information_schema.tables
                WHERE table_schema = 'public'
                  AND table_name = %s
            )
            """,
            (table,),
        )
        return bool(require_row(cur.fetchone(), f"table_exists({table})")[0])


def column_exists(conn: DbConnection, table: str, column: str) -> bool:
    with conn.cursor() as cur:
        cur.execute(
            """
            SELECT EXISTS (
                SELECT 1
                FROM information_schema.columns
                WHERE table_schema = 'public'
                  AND table_name = %s
                  AND column_name = %s
            )
            """,
            (table, column),
        )
        return bool(require_row(cur.fetchone(), f"column_exists({table}, {column})")[0])


def relation_exists(conn: DbConnection, relname: str, relkind: str) -> bool:
    with conn.cursor() as cur:
        cur.execute(
            """
            SELECT EXISTS (
                SELECT 1
                FROM pg_class
                WHERE relname = %s
                  AND relkind = %s
            )
            """,
            (relname, relkind),
        )
        return bool(require_row(cur.fetchone(), f"relation_exists({relname})")[0])


def constraint_exists(conn: DbConnection, constraint_name: str) -> bool:
    with conn.cursor() as cur:
        cur.execute(
            """
            SELECT EXISTS (
                SELECT 1
                FROM pg_constraint
                WHERE conname = %s
            )
            """,
            (constraint_name,),
        )
        return bool(
            require_row(cur.fetchone(), f"constraint_exists({constraint_name})")[0]
        )


def trigger_exists(conn: DbConnection, trigger_name: str) -> bool:
    with conn.cursor() as cur:
        cur.execute(
            """
            SELECT EXISTS (
                SELECT 1
                FROM pg_trigger
                WHERE tgname = %s
            )
            """,
            (trigger_name,),
        )
        return bool(require_row(cur.fetchone(), f"trigger_exists({trigger_name})")[0])


def detect_legacy_animated_image_schema(conn: DbConnection) -> list[str]:
    findings: list[str] = []

    for table in LEGACY_TABLES:
        if table_exists(conn, table):
            findings.append(f"legacy_table={table}")
    for sequence in LEGACY_SEQUENCES:
        if relation_exists(conn, sequence, "S"):
            findings.append(f"legacy_sequence={sequence}")
    for index in LEGACY_INDEXES:
        if relation_exists(conn, index, "i"):
            findings.append(f"legacy_index={index}")
    for constraint in LEGACY_CONSTRAINTS:
        if constraint_exists(conn, constraint):
            findings.append(f"legacy_constraint={constraint}")
    for trigger in LEGACY_TRIGGERS:
        if trigger_exists(conn, trigger):
            findings.append(f"legacy_trigger={trigger}")

    if table_exists(conn, "multi_scenario_metadata"):
        with conn.cursor() as cur:
            cur.execute(
                """
                SELECT EXISTS (
                    SELECT 1
                    FROM multi_scenario_metadata
                    WHERE scenario = 'gif_quality'
                )
                """
            )
            has_legacy_row = bool(
                require_row(cur.fetchone(), "legacy gif_quality metadata row check")[0]
            )
        if has_legacy_row:
            findings.append("legacy_metadata_row=gif_quality")

    return findings


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


def print_metadata(conn: DbConnection) -> None:
    print("\n=== multi_scenario_metadata ===")
    if not table_exists(conn, "multi_scenario_metadata"):
        print("table not found")
        return

    with conn.cursor() as cur:
        cur.execute(
            """
            SELECT scenario, table_name, embedding_dimension, sample_count,
                   COALESCE(last_updated::text, '')
            FROM multi_scenario_metadata
            ORDER BY scenario
            """
        )
        rows = cur.fetchall()

    print(
        render_table(
            rows,
            headers=["scenario", "table", "embed_dim", "sample_count", "last_updated"],
            tablefmt="simple_grid",
        )
    )


def print_legacy_schema_status(conn: DbConnection) -> None:
    print("\n=== legacy animated-image schema ===")
    findings = detect_legacy_animated_image_schema(conn)
    if findings:
        rows = [[item] for item in findings]
        print(render_table(rows, headers=["finding"], tablefmt="simple_grid"))
        return
    print("OK")


def print_loop_distribution(conn: DbConnection) -> None:
    print("\n=== loop_samples ===")
    if not table_exists(conn, "loop_samples"):
        print("table not found")
        return

    with conn.cursor() as cur:
        cur.execute(
            """
            SELECT
                CASE label
                    WHEN 1 THEN COALESCE(metadata->>'loss_tolerance', 'high')
                    WHEN 0 THEN 'video'
                    ELSE 'unknown'
                END AS training_label,
                COUNT(*)
            FROM loop_samples
            GROUP BY 1
            ORDER BY 1
            """
        )
        rows = cur.fetchall()

        cur.execute(
            """
            SELECT
                COUNT(*),
                COUNT(*) FILTER (WHERE embedding IS NULL),
                COUNT(*) FILTER (
                    WHERE embedding IS NOT NULL
                      AND embedding::text ~* %s
                ),
                COUNT(*) FILTER (
                    WHERE metadata ? 'directory_loop_intent_score'
                      AND (metadata->>'directory_loop_intent_score')::double precision <> 0.5
                )
            FROM loop_samples
            """,
            (NON_FINITE_PATTERN,),
        )
        totals_row = require_row(cur.fetchone(), "loop_samples summary")

        cur.execute(
            """
            SELECT COUNT(*)
            FROM loop_samples
            WHERE source_path LIKE %s
            """,
            (REPLICA_SOURCE_PATTERN,),
        )
        replica_row = require_row(cur.fetchone(), "loop_samples replica source summary")

    total = as_int(totals_row[0], "loop_samples total")
    null_embedding = as_int(totals_row[1], "loop_samples null_embedding")
    non_finite = as_int(totals_row[2], "loop_samples non_finite")
    non_neutral_directory_score = as_int(
        totals_row[3], "loop_samples non_neutral_directory_score"
    )
    replica_source_paths = as_int(replica_row[0], "loop_samples replica_source_paths")
    print(
        render_table(
            rows,
            headers=["training_label", "count"],
            tablefmt="simple_grid",
        )
    )
    print(
        "total={0} null_embedding={1} non_finite={2} non_neutral_directory_score={3} replica_source_paths={4}".format(
            total,
            null_embedding,
            non_finite,
            non_neutral_directory_score,
            replica_source_paths,
        )
    )


def read_quality_table_summary(conn: DbConnection, table: str) -> QualityTableSummary:
    embedding_dim = read_embedding_dimension(conn, table)
    has_quality_score = column_exists(conn, table, "quality_score")
    with conn.cursor() as cur:
        if has_quality_score:
            cur.execute(
                sql_with_identifiers(
                    """
                SELECT
                    COUNT(*),
                    COUNT(*) FILTER (WHERE embedding IS NULL),
                    COUNT(*) FILTER (
                        WHERE embedding IS NOT NULL
                          AND embedding::text ~* %s
                    ),
                    COUNT(*) FILTER (WHERE {} IS NULL),
                    AVG({}),
                    COUNT(*) FILTER (WHERE {} >= 0.5),
                    COUNT(*) FILTER (WHERE {} < 0.5)
                FROM {}
                """,
                    "quality_score",
                    "quality_score",
                    "quality_score",
                    "quality_score",
                    table,
                ),
                (NON_FINITE_PATTERN,),
            )
            row = require_row(cur.fetchone(), f"{table} quality summary")
            total = as_int(row[0], f"{table} total")
            null_embedding = as_int(row[1], f"{table} null_embedding")
            non_finite = as_int(row[2], f"{table} non_finite")
            null_score = as_int(row[3], f"{table} null_quality_score")
            avg_score: float | None = as_optional_float(
                row[4], f"{table} avg_quality_score"
            )
            positive_count: int | None = as_int(row[5], f"{table} positive_count")
            negative_count: int | None = as_int(row[6], f"{table} negative_count")
        else:
            cur.execute(
                sql_with_identifiers(
                    """
                SELECT
                    COUNT(*),
                    COUNT(*) FILTER (WHERE embedding IS NULL),
                    COUNT(*) FILTER (
                        WHERE embedding IS NOT NULL
                          AND embedding::text ~* %s
                    )
                FROM {}
                """,
                    table,
                ),
                (NON_FINITE_PATTERN,),
            )
            row = require_row(cur.fetchone(), f"{table} quality summary")
            total = as_int(row[0], f"{table} total")
            null_embedding = as_int(row[1], f"{table} null_embedding")
            non_finite = as_int(row[2], f"{table} non_finite")
            null_score = "missing-column"
            avg_score = None
            positive_count = None
            negative_count = None

        cur.execute(
            sql_with_identifiers(
                """
            SELECT COUNT(*)
            FROM {}
            WHERE source_path LIKE %s
            """,
                table,
            ),
            (REPLICA_SOURCE_PATTERN,),
        )
        replica_row = require_row(cur.fetchone(), f"{table} replica source summary")
        replica_source_paths = as_int(replica_row[0], f"{table} replica_source_paths")

    return QualityTableSummary(
        embedding_dim=embedding_dim,
        total=total,
        null_embedding=null_embedding,
        non_finite=non_finite,
        null_score=null_score,
        avg_score=avg_score,
        replica_source_paths=replica_source_paths,
        positive_count=positive_count,
        negative_count=negative_count,
    )


def evaluate_image_quality_model_status(
    summary: QualityTableSummary,
) -> ImageQualityModelStatus:
    readiness_issues: list[str] = []
    if summary.positive_count is None or summary.negative_count is None:
        readiness_issues.append("missing_quality_score")
    else:
        if not quality_corpus_is_mature(summary.positive_count, summary.negative_count):
            shortfall = quality_corpus_samples_shortfall(
                summary.positive_count, summary.negative_count
            )
            readiness_issues.append(
                f"corpus_shortfall={shortfall} "
                f"(need total>={MIN_IMAGE_QUALITY_SAMPLES_TOTAL}, "
                f"high/low>={MIN_IMAGE_QUALITY_SAMPLES_PER_CLASS}; "
                f"have total={summary.total} high={summary.positive_count} "
                f"low={summary.negative_count})"
            )

    model_path = default_image_quality_model_path()
    metadata_path = default_image_quality_metadata_path()
    model_exists = model_path.exists()
    metadata_exists = metadata_path.exists()
    ready_for_training = not readiness_issues
    ready_for_runtime = ready_for_training and model_exists and metadata_exists

    return ImageQualityModelStatus(
        ready_for_training=ready_for_training,
        ready_for_runtime=ready_for_runtime,
        readiness_issues=tuple(readiness_issues),
        model_path=model_path,
        metadata_path=metadata_path,
        model_exists=model_exists,
        metadata_exists=metadata_exists,
    )


def print_image_quality_model_status(summary: QualityTableSummary) -> None:
    status = evaluate_image_quality_model_status(summary)
    positive_count = (
        "n/a" if summary.positive_count is None else str(summary.positive_count)
    )
    negative_count = (
        "n/a" if summary.negative_count is None else str(summary.negative_count)
    )
    print(
        "training_readiness={0} thresholds=total>={1},high>={2},low>={2} high={3} low={4}".format(
            "ready" if status.ready_for_training else "pending",
            MIN_IMAGE_QUALITY_SAMPLES_TOTAL,
            MIN_IMAGE_QUALITY_SAMPLES_PER_CLASS,
            positive_count,
            negative_count,
        )
    )
    if status.readiness_issues:
        print(f"training_issues={'; '.join(status.readiness_issues)}")

    artifact_state = "ready"
    if not status.model_exists and not status.metadata_exists:
        artifact_state = "missing_model_and_metadata"
    elif not status.model_exists:
        artifact_state = "missing_model"
    elif not status.metadata_exists:
        artifact_state = "missing_metadata"

    print(
        f"model_artifacts={artifact_state} model={status.model_path} metadata={status.metadata_path}"
    )
    if status.ready_for_training and not status.ready_for_runtime:
        print(
            "next_step=python3 crates/dev/scripts/training_pipeline.py train-image-quality-model"
        )


def read_loop_intent_summary(conn: DbConnection) -> LoopIntentTableSummary | None:
    if not table_exists(conn, "loop_samples"):
        return None

    with conn.cursor() as cur:
        cur.execute(
            """
            SELECT
                COUNT(*),
                COUNT(*) FILTER (WHERE embedding IS NULL),
                COUNT(*) FILTER (
                    WHERE embedding IS NOT NULL
                      AND embedding::text ~* %s
                ),
                COUNT(*) FILTER (WHERE label = 1),
                COUNT(*) FILTER (WHERE label = 0),
                COUNT(*) FILTER (
                    WHERE metadata ? 'directory_loop_intent_score'
                      AND (metadata->>'directory_loop_intent_score')::double precision <> 0.5
                )
            FROM loop_samples
            """,
            (NON_FINITE_PATTERN,),
        )
        totals_row = require_row(cur.fetchone(), "loop_samples summary")

        cur.execute(
            """
            SELECT COUNT(*)
            FROM loop_samples
            WHERE source_path LIKE %s
            """,
            (REPLICA_SOURCE_PATTERN,),
        )
        replica_row = require_row(cur.fetchone(), "loop_samples replica source summary")

        feature_stats_present = False
        if table_exists(conn, "multi_scenario_metadata"):
            cur.execute(
                """
                SELECT COALESCE(jsonb_typeof(feature_stats), 'null') <> 'null'
                FROM multi_scenario_metadata
                WHERE scenario = 'loop_intent'
                """
            )
            meta_row = cur.fetchone()
            if meta_row is not None:
                feature_stats_present = bool(meta_row[0])

    return LoopIntentTableSummary(
        total=as_int(totals_row[0], "loop_samples total"),
        null_embedding=as_int(totals_row[1], "loop_samples null_embedding"),
        non_finite=as_int(totals_row[2], "loop_samples non_finite"),
        loop_positive_count=as_int(totals_row[3], "loop_samples loop_positive_count"),
        video_negative_count=as_int(totals_row[4], "loop_samples video_negative_count"),
        non_neutral_directory_score=as_int(
            totals_row[5], "loop_samples non_neutral_directory_score"
        ),
        replica_source_paths=as_int(
            replica_row[0], "loop_samples replica_source_paths"
        ),
        feature_stats_present=feature_stats_present,
    )


def evaluate_loop_intent_runtime_status(
    summary: LoopIntentTableSummary,
) -> LoopIntentRuntimeStatus:
    readiness_issues: list[str] = []
    loop_preservation_class = summary.loop_positive_count
    if not loop_corpus_is_mature(
        summary.total, loop_preservation_class, summary.video_negative_count
    ):
        shortfall = loop_corpus_samples_shortfall(
            summary.total, loop_preservation_class, summary.video_negative_count
        )
        readiness_issues.append(
            f"corpus_shortfall={shortfall} "
            f"(need total>={MIN_LOOP_SAMPLES_TOTAL}, "
            f"loop_high/video>={MIN_LOOP_SAMPLES_PER_CLASS}; "
            f"have total={summary.total} loop_high={loop_preservation_class} "
            f"video={summary.video_negative_count})"
        )
    if summary.null_embedding:
        readiness_issues.append(f"null_embedding={summary.null_embedding}")
    if summary.non_finite:
        readiness_issues.append(f"non_finite={summary.non_finite}")

    ready_for_knn = not readiness_issues
    directory_scores_ready = (
        summary.total > 0 and summary.non_neutral_directory_score >= summary.total
    )
    runtime_issues: list[str] = []
    if not ready_for_knn:
        runtime_issues.extend(readiness_issues)
    if not summary.feature_stats_present:
        runtime_issues.append("missing_loop_intent_feature_stats")
    if summary.total > 0 and not directory_scores_ready:
        runtime_issues.append(
            "directory_loop_intent_score_not_backfilled="
            f"{summary.non_neutral_directory_score}/{summary.total}"
        )

    return LoopIntentRuntimeStatus(
        ready_for_knn=ready_for_knn,
        ready_for_runtime=ready_for_knn
        and summary.feature_stats_present
        and directory_scores_ready,
        readiness_issues=tuple(runtime_issues),
        predictor_family="pgvector_hnsw+hdbscan",
    )


def print_loop_intent_runtime_status(summary: LoopIntentTableSummary) -> None:
    status = evaluate_loop_intent_runtime_status(summary)
    print(
        "knn_readiness={0} thresholds=total>={1},loop_high>={2},video>={2} "
        "loop_high={3} video={4} predictor={5}".format(
            "ready" if status.ready_for_knn else "pending",
            MIN_LOOP_SAMPLES_TOTAL,
            MIN_LOOP_SAMPLES_PER_CLASS,
            summary.loop_positive_count,
            summary.video_negative_count,
            status.predictor_family,
        )
    )
    if status.readiness_issues:
        print(f"knn_issues={'; '.join(status.readiness_issues)}")
    runtime_state = "ready" if status.ready_for_runtime else "pending"
    print(
        "loop_runtime={0} feature_stats={1} directory_scores_backfilled={2}/{3}".format(
            runtime_state,
            "yes" if summary.feature_stats_present else "no",
            summary.non_neutral_directory_score,
            summary.total,
        )
    )
    if status.ready_for_knn and not status.ready_for_runtime:
        print(
            "next_step=python3 crates/dev/scripts/training_pipeline.py finalize-loop-intent"
        )


def run_loop_hdbscan_clustering(connstr: str) -> int:
    python = preferred_training_python()
    cmd = [
        python,
        str(LOOP_INTENT_CLUSTERING_SCRIPT),
        "--connstr",
        connstr,
    ]
    print(
        f"  {pick_symbol('🌀', ('[CLUSTER]'))} Running loop_intent HDBSCAN clustering..."
    )
    result = run_delegated(
        cmd, parent_script="training_pipeline.py", cwd=ROOT, check=False
    )
    return result.returncode


def backfill_loop_directory_scores(connstr: str, *, skip_refresh: bool = True) -> int:
    cmd = [
        "cargo",
        "run",
        "--bin",
        "backfill_directory_scores",
        "--",
        "--connstr",
        connstr,
    ]
    if skip_refresh:
        cmd.append("--no-refresh-stats")
    result = run_delegated(
        cmd, parent_script="training_pipeline.py", cwd=ROOT, check=False
    )
    return result.returncode


def finalize_loop_intent_assets(connstr: str) -> int:
    conn = connect_pg(connstr)
    try:
        print("\n=== loop intent finalize ===")
        summary = read_loop_intent_summary(conn)
        if summary is None:
            print("finalize_blocked=missing_loop_samples_table")
            return 2
        print_loop_distribution(conn)
        print_loop_intent_runtime_status(summary)
        status = evaluate_loop_intent_runtime_status(summary)
    finally:
        conn.close()

    if summary.total == 0:
        print("finalize_blocked=no_loop_samples_rows")
        return 2

    refresh_exit = refresh_loop_stats(connstr)
    if refresh_exit != 0:
        print(f"finalize_failed=refresh_loop_stats exit={refresh_exit}")
        return refresh_exit

    backfill_exit = backfill_loop_directory_scores(connstr, skip_refresh=True)
    if backfill_exit != 0:
        print(f"finalize_failed=backfill_directory_scores exit={backfill_exit}")
        return backfill_exit

    cluster_exit = run_loop_hdbscan_clustering(connstr)
    if cluster_exit != 0:
        print(f"finalize_failed=loop_hdbscan_clustering exit={cluster_exit}")
        return cluster_exit

    conn = connect_pg(connstr)
    try:
        refreshed = read_loop_intent_summary(conn)
        if refreshed is None:
            status = evaluate_loop_intent_runtime_status(summary)
        else:
            print_loop_intent_runtime_status(refreshed)
            status = evaluate_loop_intent_runtime_status(refreshed)
    finally:
        conn.close()

    if status.ready_for_runtime:
        print("finalize_result=loop_intent_runtime_ready")
        return 0
    if status.ready_for_knn:
        print("finalize_result=loop_stats_refreshed_knn_pending_runtime_hygiene")
        return 2
    print("finalize_result=loop_stats_refreshed_corpus_immature")
    return 2


def combine_finalize_exit_codes(*exit_codes: int) -> int:
    """Prefer hard failures (1) over pending maturity (2) over success (0)."""
    if any(code == 1 for code in exit_codes):
        return 1
    if any(code == 2 for code in exit_codes):
        return 2
    return 0


def finalize_runtime_assets(
    connstr: str, *, install_missing_python_deps: bool = False
) -> int:
    print("\n=== runtime asset fill (multi-scenario) ===")
    print(
        "task_families=loop_intent(hnsw+hdbscan+stats+directory_scores) "
        "image_quality(lightgbm) animated_image_quality(db_only) video_quality(db_only)"
    )
    loop_exit = finalize_loop_intent_assets(connstr)
    image_exit = finalize_image_quality_model(
        connstr,
        install_missing_python_deps=install_missing_python_deps,
    )
    return combine_finalize_exit_codes(loop_exit, image_exit)


def print_quality_distribution(
    conn: DbConnection, scenario: str, table: str
) -> QualityTableSummary | None:
    print(f"\n=== {scenario} / {table} ===")
    if not table_exists(conn, table):
        print("table not found")
        return None

    summary = read_quality_table_summary(conn, table)
    print(
        f"embedding_dim={summary.embedding_dim} total={summary.total} "
        f"null_embedding={summary.null_embedding} non_finite={summary.non_finite} "
        f"null_score={summary.null_score} "
        f"avg_score={'n/a' if summary.avg_score is None else f'{summary.avg_score:.4f}'} "
        f"replica_source_paths={summary.replica_source_paths}"
    )
    return summary


def print_loop_clustering_report(conn: DbConnection) -> None:
    print("\n=== loop clustering ===")
    print_loop_distribution(conn)


def print_quality_regression_report(conn: DbConnection) -> None:
    print("\n=== quality regression ===")
    print_legacy_schema_status(conn)
    for scenario in QUALITY_REGRESSION_SCENARIOS:
        summary = print_quality_distribution(conn, scenario.name, scenario.table)
        if scenario.name == "image_quality" and summary is not None:
            print_image_quality_model_status(summary)


def print_full_report(conn: DbConnection) -> None:
    print_metadata(conn)
    print_loop_clustering_report(conn)
    print_quality_regression_report(conn)


def render_single_column_table(header: str, values: Sequence[str]) -> str:
    rows = [[value] for value in values]
    return render_table(rows, headers=[header], tablefmt="simple_grid")


def verify_embeddings(
    conn: DbConnection,
    scenarios: tuple[ScenarioSpec, ...] = SCENARIOS,
    heading: str = "embedding verification",
    *,
    include_legacy_schema: bool = True,
) -> int:
    failures = 0
    rows_out: list[list[object]] = []
    if include_legacy_schema:
        legacy_findings = detect_legacy_animated_image_schema(conn)
        if legacy_findings:
            failures += 1
            rows_out.append(["schema", "; ".join(legacy_findings)])
        else:
            rows_out.append(["schema", "OK"])
    metadata_table_present = table_exists(conn, "multi_scenario_metadata")
    with conn.cursor() as cur:
        for scenario in scenarios:
            name = scenario.name
            table = scenario.table
            expected_dim = scenario.expected_dim
            score_col = scenario.score_col
            if not table_exists(conn, table):
                rows_out.append([name, f"missing_table={table}"])
                failures += 1
                continue

            actual_dim = read_embedding_dimension(conn, table)
            has_score_col = column_exists(conn, table, score_col)
            issues: list[str] = []

            meta_dim: int | None = None
            meta_count: int | None = None
            if not metadata_table_present:
                issues.append("missing_multi_scenario_metadata")
            else:
                cur.execute(
                    """
                    SELECT embedding_dimension, sample_count
                    FROM multi_scenario_metadata
                    WHERE scenario = %s
                    """,
                    (name,),
                )
                meta_row = cur.fetchone()
                if meta_row is None:
                    issues.append("missing_metadata_row")
                else:
                    meta_dim = as_int(
                        meta_row[0], f"{name} metadata embedding_dimension"
                    )
                    meta_count = as_int(meta_row[1], f"{name} metadata sample_count")

            cur.execute(sql_with_identifiers("SELECT COUNT(*) FROM {}", table))
            live_count = as_int(
                require_row(cur.fetchone(), f"{table} live count")[0],
                f"{table} live count",
            )
            if has_score_col:
                cur.execute(
                    sql_with_identifiers(
                        """
                    SELECT
                        COUNT(*) FILTER (WHERE embedding IS NULL),
                        COUNT(*) FILTER (
                            WHERE embedding IS NOT NULL
                              AND embedding::text ~* %s
                        ),
                        COUNT(*) FILTER (WHERE {} IS NULL),
                        COUNT(*) FILTER (WHERE {} >= 0.5),
                        COUNT(*) FILTER (WHERE {} < 0.5)
                    FROM {}
                    """,
                        score_col,
                        score_col,
                        score_col,
                        table,
                    ),
                    (NON_FINITE_PATTERN,),
                )
                row = require_row(cur.fetchone(), f"{table} verification summary")
                null_embedding = as_int(row[0], f"{table} null_embedding")
                non_finite = as_int(row[1], f"{table} non_finite")
                null_score = as_int(row[2], f"{table} null_{score_col}")
                positive_count = as_int(row[3], f"{table} positive_count")
                negative_count = as_int(row[4], f"{table} negative_count")
            else:
                cur.execute(
                    sql_with_identifiers(
                        """
                    SELECT
                        COUNT(*) FILTER (WHERE embedding IS NULL),
                        COUNT(*) FILTER (
                            WHERE embedding IS NOT NULL
                              AND embedding::text ~* %s
                        )
                    FROM {}
                    """,
                        table,
                    ),
                    (NON_FINITE_PATTERN,),
                )
                row = require_row(cur.fetchone(), f"{table} verification summary")
                null_embedding = as_int(row[0], f"{table} null_embedding")
                non_finite = as_int(row[1], f"{table} non_finite")
                null_score = None
                positive_count = None
                negative_count = None

            cur.execute(
                sql_with_identifiers(
                    """
                    SELECT COUNT(*)
                    FROM {}
                    WHERE source_path LIKE %s
                    """,
                    table,
                ),
                (REPLICA_SOURCE_PATTERN,),
            )
            replica_row = require_row(cur.fetchone(), f"{table} replica source summary")
            replica_source_paths = as_int(
                replica_row[0], f"{table} replica_source_paths"
            )

            if meta_dim is not None and meta_dim != expected_dim:
                issues.append(f"metadata_dim={meta_dim} expected={expected_dim}")
            if actual_dim != expected_dim:
                issues.append(f"column_dim={actual_dim} expected={expected_dim}")
            if meta_count is not None and meta_count != live_count:
                issues.append(f"metadata_count={meta_count} live_count={live_count}")
            if null_embedding:
                issues.append(f"null_embedding={null_embedding}")
            if non_finite:
                issues.append(f"non_finite={non_finite}")
            if not has_score_col:
                issues.append(f"missing_{score_col}_column")
            elif null_score:
                issues.append(f"null_{score_col}={null_score}")
            if replica_source_paths:
                issues.append(f"replica_source_path={replica_source_paths}")
            if name == "image_quality" and has_score_col:
                summary = QualityTableSummary(
                    embedding_dim=actual_dim,
                    total=live_count,
                    null_embedding=null_embedding,
                    non_finite=non_finite,
                    null_score=0 if null_score is None else null_score,
                    avg_score=None,
                    replica_source_paths=replica_source_paths,
                    positive_count=positive_count,
                    negative_count=negative_count,
                )
                model_status = evaluate_image_quality_model_status(summary)
                if (
                    model_status.ready_for_training
                    and not model_status.ready_for_runtime
                ):
                    if not model_status.model_exists:
                        issues.append("missing_lightgbm_model")
                    if not model_status.metadata_exists:
                        issues.append("missing_lightgbm_metadata")

            if issues:
                failures += 1
                rows_out.append([name, "; ".join(issues)])
            else:
                rows_out.append([name, "OK"])

    print(f"\n=== {heading} ===")
    print(
        render_table(rows_out, headers=["scenario", "status"], tablefmt="simple_grid")
    )
    return failures


def apply_multi_scenario_migration(conn: DbConnection) -> None:
    if not MULTI_SCENARIO_MIGRATION_SQL.exists():
        raise RuntimeError(f"Migration file not found: {MULTI_SCENARIO_MIGRATION_SQL}")
    sql_text = MULTI_SCENARIO_MIGRATION_SQL.read_text(encoding="utf-8")
    with conn.cursor() as cur:
        cur.execute(sql_text)


def drop_legacy_animated_image_schema(conn: DbConnection) -> list[str]:
    actions = [
        "DROP TABLE IF EXISTS gif_quality_inference_log CASCADE",
        "DROP TABLE IF EXISTS gif_quality_samples CASCADE",
        "DROP SEQUENCE IF EXISTS gif_quality_inference_log_id_seq CASCADE",
        "DROP SEQUENCE IF EXISTS gif_quality_samples_id_seq CASCADE",
        "DROP INDEX IF EXISTS idx_gif_quality_blake3",
        "DROP INDEX IF EXISTS idx_gif_quality_hnsw",
    ]
    with conn.cursor() as cur:
        for statement in actions:
            cur.execute(statement)
        if table_exists(conn, "multi_scenario_metadata"):
            cur.execute(
                "DELETE FROM multi_scenario_metadata WHERE scenario = 'gif_quality'"
            )
            actions.append(
                "DELETE FROM multi_scenario_metadata WHERE scenario = 'gif_quality'"
            )
    return actions


def repair_multi_scenario_schema(connstr: str, *, drop_legacy_gif_schema: bool) -> int:
    conn = connect_pg(connstr)
    try:
        print("\n=== multi-scenario schema repair ===")
        legacy_findings = detect_legacy_animated_image_schema(conn)
        if legacy_findings:
            print(render_single_column_table("finding", legacy_findings))
            if not drop_legacy_gif_schema:
                print("repair_blocked=legacy_animated_image_schema")
                print(
                    "next_step=rerun with `python3 crates/dev/scripts/training_pipeline.py repair-multi-scenario-schema --drop-legacy-gif-schema`"
                )
                conn.rollback()
                return 2
            actions = drop_legacy_animated_image_schema(conn)
            print(render_single_column_table("action", actions))
        else:
            print("legacy_schema=clean")

        apply_multi_scenario_migration(conn)
        conn.commit()
        print(f"repair_result=applied_migration source={MULTI_SCENARIO_MIGRATION_SQL}")
    except (
        OSError,
        ValueError,
        RuntimeError,
        TypeError,
        KeyError,
        IndexError,
        AttributeError,
        UnicodeError,
    ) as exc:
        conn.rollback()
        print(f"repair_failed={exc}")
        return 1
    finally:
        conn.close()

    verify_conn = connect_pg(connstr)
    try:
        return (
            0
            if verify_embeddings(
                verify_conn,
                SCENARIOS,
                "post-repair verification",
            )
            == 0
            else 2
        )
    finally:
        verify_conn.close()


def verify_stack_readiness(conn: DbConnection) -> int:
    schema_failures = verify_embeddings(
        conn,
        SCENARIOS,
        "stack readiness / schema verification",
    )
    failures = schema_failures
    rows_out: list[list[object]] = []
    rows_out.append(
        [
            "schema_verification",
            "OK"
            if schema_failures == 0
            else f"{schema_failures} failure(s); see schema verification table above",
        ]
    )

    if table_exists(conn, "loop_samples"):
        loop_summary = read_loop_intent_summary(conn)
        if loop_summary is None:
            failures += 1
            rows_out.append(["loop_intent_runtime", "missing_loop_samples_summary"])
        else:
            loop_status = evaluate_loop_intent_runtime_status(loop_summary)
            if not loop_status.ready_for_knn:
                failures += 1
                rows_out.append(
                    [
                        "loop_intent_knn",
                        "; ".join(loop_status.readiness_issues),
                    ]
                )
            else:
                rows_out.append(["loop_intent_knn", "ready"])
            if not loop_status.ready_for_runtime:
                failures += 1
                rows_out.append(
                    [
                        "loop_intent_runtime",
                        "; ".join(loop_status.readiness_issues),
                    ]
                )
            else:
                rows_out.append(["loop_intent_runtime", "ready"])

    if table_exists(conn, "image_quality_samples"):
        summary = read_quality_table_summary(conn, "image_quality_samples")
        status = evaluate_image_quality_model_status(summary)
        if not status.ready_for_training:
            failures += 1
            rows_out.append(
                [
                    "image_quality_training",
                    "; ".join(status.readiness_issues),
                ]
            )
        else:
            rows_out.append(["image_quality_training", "ready"])

        if not status.ready_for_runtime:
            failures += 1
            artifact_issues: list[str] = []
            if not status.model_exists:
                artifact_issues.append("missing_model")
            if not status.metadata_exists:
                artifact_issues.append("missing_metadata")
            if not artifact_issues:
                artifact_issues.append("training_not_ready")
            rows_out.append(
                [
                    "image_quality_runtime",
                    "; ".join(artifact_issues),
                ]
            )
        else:
            rows_out.append(["image_quality_runtime", "ready"])

    print("\n=== stack readiness ===")
    print(render_table(rows_out, headers=["check", "status"], tablefmt="simple_grid"))
    return 0 if failures == 0 else 2


LOOP_PROBE_METADATA_REQUIRED_KEYS: tuple[str, ...] = (
    "frame_delay_variation",
    "frame_payload_variation",
    "aspect_ratio",
    "loop_frequency",
    "palette_depth",
    "block_skew",
    "temporal_flatness",
    "webp_compression_ratio",
    "directory_loop_intent_score",
)

LEGACY_DECISION_PREDICTOR_FAMILIES: frozenset[str] = frozenset(
    {"knn_only", "hybrid_bootstrap", "heuristic_only"}
)

LOOP_FEATURE_STATS_EMPIRICAL_KEYS: tuple[str, ...] = (
    "delay_var",
    "duration",
    "frame_count",
)


def verify_fabrication_stock(conn: DbConnection) -> int:
    """Audit DB stock for synthetic loop stats, missing probe fields, and legacy inference families."""
    failures = 0
    warnings = 0

    print("\n=== fabrication stock: loop feature_stats ===")
    loop_rows = 0
    stats_empty = True
    sample_count_meta = 0
    empirical_delay_var = False
    if table_exists(conn, "loop_samples"):
        with conn.cursor() as cur:
            cur.execute("SELECT COUNT(*) FROM loop_samples WHERE frame_count > 1")
            loop_row = require_row(cur.fetchone(), "loop_samples frame_count>1 count")
            loop_rows = as_int(loop_row[0], "loop_samples count")

    if table_exists(conn, "multi_scenario_metadata"):
        with conn.cursor() as cur:
            cur.execute(
                """
                SELECT sample_count,
                       COALESCE(feature_stats, '{}'::jsonb) AS feature_stats
                FROM multi_scenario_metadata
                WHERE scenario = 'loop_intent'
                """
            )
            meta_row = cur.fetchone()
            if meta_row is not None:
                sample_count_meta = as_int(meta_row[0], "loop_intent sample_count")
                feature_stats = meta_row[1]
                stats_empty = feature_stats in ({}, None) or feature_stats == {}
                if isinstance(feature_stats, dict):
                    stats_map = feature_stats.get("stats") or {}
                    delay_stats = stats_map.get("delay_var") or {}
                    empirical_delay_var = delay_stats.get("p50") is not None
                elif feature_stats is not None:
                    stats_empty = str(feature_stats).strip() in ("{}", "null", "")
    else:
        failures += 1
        print("loop_feature_stats=missing_multi_scenario_metadata_table")

    print(
        f"loop_samples_trainable={loop_rows} metadata_sample_count={sample_count_meta} "
        f"feature_stats_empty={stats_empty} delay_var_has_p50={empirical_delay_var}"
    )
    if loop_rows > 0 and stats_empty:
        failures += 1
        print(
            "fabrication_blocker=loop_feature_stats_empty_with_samples "
            "(run: training_pipeline.py refresh-loop-stats; fast SQL-only)"
        )
    if loop_rows > 0 and not stats_empty and not empirical_delay_var:
        failures += 1
        print(
            "fabrication_blocker=loop_feature_stats_missing_empirical_delay_var "
            "(likely bootstrap histogram; refresh-loop-stats after probe repair)"
        )

    print("\n=== fabrication stock: loop_samples probe fields ===")
    missing_delay_var = 0
    missing_any_probe = 0
    offenders: list[tuple[str, str, str]] = []
    if table_exists(conn, "loop_samples"):
        key_checks = " OR ".join(
            f"(metadata->>'{key}' IS NULL OR NOT (metadata ? '{key}'))"
            for key in LOOP_PROBE_METADATA_REQUIRED_KEYS
        )
        delay_check = (
            "(metadata->>'frame_delay_variation' IS NULL "
            "OR NOT (metadata ? 'frame_delay_variation'))"
        )
        with conn.cursor() as cur:
            cur.execute(
                f"""
                SELECT COUNT(*) FROM loop_samples
                WHERE frame_count > 1 AND ({delay_check})
                """
            )
            delay_row = require_row(
                cur.fetchone(), "missing frame_delay_variation count"
            )
            missing_delay_var = as_int(delay_row[0], "missing_delay_var")

            cur.execute(
                f"""
                SELECT COUNT(*) FROM loop_samples
                WHERE frame_count > 1 AND ({key_checks})
                """
            )
            any_row = require_row(cur.fetchone(), "missing any probe field count")
            missing_any_probe = as_int(any_row[0], "missing_any_probe")

            cur.execute(
                f"""
                SELECT encode(blake3, 'hex') AS blake3_hex,
                       COALESCE(source_path, file_name, '<no-path>') AS path_hint,
                       COALESCE(metadata->>'frame_delay_variation', '<null>') AS delay_var
                FROM loop_samples
                WHERE frame_count > 1 AND ({delay_check})
                ORDER BY blake3
                LIMIT 25
                """
            )
            for row in cur.fetchall():
                offenders.append(
                    (
                        str(row[0]),
                        str(row[1]),
                        str(row[2]),
                    )
                )

    print(
        f"missing_frame_delay_variation={missing_delay_var} "
        f"(maps_to loop_stats_delay_var) "
        f"missing_any_required_probe_key={missing_any_probe}"
    )
    if offenders:
        print(
            render_table(
                [[b, p, d] for b, p, d in offenders],
                headers=["blake3_hex", "path_hint", "frame_delay_variation"],
                tablefmt="simple_grid",
            )
        )
        if missing_delay_var > len(offenders):
            print(f"... and {missing_delay_var - len(offenders)} more row(s)")
    if loop_rows > 0 and missing_delay_var > 0:
        failures += 1
        print(
            "fabrication_blocker=loop_samples_missing_loop_stats_delay_var "
            "(run: training_pipeline.py repair-loop-probe-metadata — slow, re-reads files; "
            "then refresh-loop-stats)"
        )

    print("\n=== fabrication stock: inference history ===")
    inference_tables: tuple[tuple[str, str], ...] = (
        ("inference_log", "final_verdict"),
        ("loop_intent_inference_log", "final_verdict"),
        ("image_quality_inference_log", "predictor_family"),
        ("animated_image_quality_inference_log", "predictor_family"),
        ("video_quality_inference_log", "predictor_family"),
    )
    inference_rows: list[list[object]] = []
    for table, group_col in inference_tables:
        if not table_exists(conn, table):
            inference_rows.append([table, "missing_table", ""])
            continue
        if not column_exists(conn, table, group_col):
            inference_rows.append([table, f"missing_column={group_col}", ""])
            continue
        with conn.cursor() as cur:
            cur.execute(
                f"""
                SELECT {group_col}, COUNT(*)
                FROM {table}
                GROUP BY 1
                ORDER BY 2 DESC
                """
            )
            groups = cur.fetchall()
        total = sum(as_int(row[1], f"{table} count") for row in groups)
        summary = "; ".join(f"{row[0]}={row[1]}" for row in groups[:8]) or "empty"
        if len(groups) > 8:
            summary += f"; +{len(groups) - 8} more"
        inference_rows.append([table, total, summary])
        if table == "image_quality_inference_log":
            for row in groups:
                family = str(row[0])
                count = as_int(row[1], "predictor_family count")
                if family in LEGACY_DECISION_PREDICTOR_FAMILIES and count > 0:
                    warnings += 1
                    print(
                        f"fabrication_warning=historical_{family}_inference_log count={count} "
                        "(telemetry only; new runtime must not write these as decision scores)"
                    )

    print(
        render_table(
            inference_rows,
            headers=["table", "total", "breakdown"],
            tablefmt="simple_grid",
        )
    )

    print("\n=== fabrication stock: summary ===")
    if failures:
        print(f"fabrication_stock=FAIL failures={failures} warnings={warnings}")
        return 2
    if warnings:
        print(f"fabrication_stock=PASS_WITH_WARNINGS warnings={warnings}")
        return 0
    print("fabrication_stock=PASS")
    return 0


def refresh_loop_stats(connstr: str) -> int:
    """Fast path: recompute feature_stats + embeddings from existing DB rows (no full retrain)."""
    env = child_env_for_script("training_pipeline.py")
    env["MFB_PG_CONNSTR"] = connstr
    if artifact_is_stale(REFRESH_STATS_BIN, REFRESH_STATS_SOURCES):
        cmd = ["cargo", "run", "-p", "foundation", "--bin", "refresh_stats"]
    else:
        cmd = [str(REFRESH_STATS_BIN)]

    result = run_delegated(
        cmd, parent_script="training_pipeline.py", cwd=ROOT, env=env, check=False
    )
    return result.returncode


def repair_loop_probe_metadata(connstr: str) -> int:
    """Slow path: re-decode each broken loop sample from source_path to fill metadata."""
    env = child_env_for_script("training_pipeline.py")
    env["MFB_PG_CONNSTR"] = connstr
    if artifact_is_stale(REPAIR_LOOP_PROBE_BIN, REPAIR_LOOP_PROBE_SOURCES):
        cmd = ["cargo", "run", "-p", "foundation", "--bin", "repair_loop_probe"]
    else:
        cmd = [str(REPAIR_LOOP_PROBE_BIN)]
    print(
        f"{pick_symbol('⚠️', ('[WARN]'))} repair-loop-probe-metadata re-reads media files on disk; "
        "this is NOT run_training / full re-ingest. Cancel with Ctrl+C if unintended."
    )
    result = run_delegated(
        cmd, parent_script="training_pipeline.py", cwd=ROOT, env=env, check=False
    )
    return result.returncode


def run_training_batch(connstr: str) -> int:
    env = child_env_for_script("training_pipeline.py")
    env["MFB_PG_CONNSTR"] = connstr
    python = preferred_training_python()
    env.setdefault(QUALITY_MODEL_PYTHON_ENV, python)

    rust_bin = ROOT / "target" / "release" / "run_training"
    if rust_bin.is_file():
        cmd = [str(rust_bin)]
    else:
        cmd = [
            "cargo",
            "run",
            "--locked",
            "-p",
            "dev",
            "--bin",
            "run_training",
            "--",
        ]

    cmd.extend(
        [
            "--use-api",
            "--repair-schema",
            "--verify-after",
            "--install-missing-python-deps",
        ]
    )

    result = run_delegated(
        cmd, parent_script="training_pipeline.py", cwd=ROOT, env=env, check=False
    )
    return result.returncode


def train_image_quality_model(connstr: str) -> int:
    env = os.environ.copy()
    env["MFB_PG_CONNSTR"] = connstr
    python = preferred_training_python()
    env.setdefault(QUALITY_MODEL_PYTHON_ENV, python)
    cmd = [
        python,
        str(QUALITY_REGRESSION_MODEL_SCRIPT),
        "train-image-quality",
        "--connstr",
        connstr,
    ]
    result = run_delegated(
        cmd, parent_script="training_pipeline.py", cwd=ROOT, env=env, check=False
    )
    return result.returncode


def finalize_image_quality_model(
    connstr: str, *, install_missing_python_deps: bool = False
) -> int:
    """``install_missing_python_deps`` is accepted for API parity with ``finalize-runtime-assets``."""
    conn = connect_pg(connstr)
    try:
        print("\n=== image quality finalize ===")
        summary = print_quality_distribution(
            conn, "image_quality", "image_quality_samples"
        )
        if summary is None:
            print("finalize_blocked=missing_image_quality_table")
            return 2
        print_image_quality_model_status(summary)
        status = evaluate_image_quality_model_status(summary)
    finally:
        conn.close()

    if not status.ready_for_training:
        print(f"finalize_blocked={'; '.join(status.readiness_issues)}")
        return 2

    train_exit = train_image_quality_model(connstr)
    if train_exit != 0:
        return train_exit

    model_path = default_image_quality_model_path()
    metadata_path = default_image_quality_metadata_path()
    if not model_path.exists() or not metadata_path.exists():
        print(
            f"finalize_failed=missing_artifacts model_exists={model_path.exists()} metadata_exists={metadata_path.exists()}"
        )
        return 1

    artifact_state = "ready"
    if not status.model_exists and not status.metadata_exists:
        artifact_state = "created_model_and_metadata"
    elif not status.model_exists:
        artifact_state = "created_model"
    elif not status.metadata_exists:
        artifact_state = "created_metadata"

    print(f"finalize_result={artifact_state}")
    print(f"runtime_ready=model={model_path} metadata={metadata_path}")
    return 0


def print_ingest_guidance(dataset_path: str) -> None:
    print("Batch ingestion entrypoint:")
    print("  cargo run --locked -p dev --bin run_training -- --use-api")
    print(
        "  "
        f"{preferred_training_python()} {ROOT / 'crates/dev/scripts/run_training.py'} "
        "--execute --use-api  # compat reference"
    )
    print(f"Requested path hint: {dataset_path}")


def main() -> None:
    guard_main("training_pipeline.py")
    parser = argparse.ArgumentParser(
        description="New-schema-only training database audit utility"
    )
    parser.add_argument("--connstr", default=None, help="PostgreSQL connection string")
    subparsers = parser.add_subparsers(dest="command", required=True)

    subparsers.add_parser(
        "train",
        help="Compatibility alias: run the batch ingestion entrypoint for all task families",
    )
    subparsers.add_parser(
        "evaluate",
        help="Compatibility alias: run the combined loop-clustering + quality-regression audit",
    )
    subparsers.add_parser(
        "export-stats",
        help="Compatibility alias: refresh loop-clustering stats in the new schema",
    )
    subparsers.add_parser("report", help="Print the combined task-family report")
    subparsers.add_parser(
        "report-quality-regression",
        help="Print counts and score hygiene for the quality-regression tables",
    )
    subparsers.add_parser(
        "report-loop-clustering",
        help="Print counts and label distribution for the loop-clustering table",
    )
    subparsers.add_parser(
        "verify-embeddings",
        help="Check all task families for metadata counts, null embeddings, and non-finite vectors",
    )
    subparsers.add_parser(
        "verify-quality-regression",
        help="Check quality-regression embeddings, quality_score coverage, and metadata counts",
    )
    subparsers.add_parser(
        "verify-stack-readiness",
        help="Return success only when schema is clean, all scenario tables exist, and image_quality runtime artifacts are trained",
    )
    subparsers.add_parser(
        "verify-fabrication-stock",
        help=(
            "Audit loop feature_stats, loop_samples probe metadata, and inference_log "
            "predictor_family history for fabrication blockers"
        ),
    )
    subparsers.add_parser(
        "verify-loop-clustering",
        help="Check loop-clustering embeddings, labels, and metadata counts",
    )
    subparsers.add_parser(
        "refresh-loop-stats",
        help=(
            "Fast: recompute loop_intent feature_stats + embeddings from existing loop_samples "
            "(SQL/vector only; does not re-ingest or retrain LightGBM)"
        ),
    )
    subparsers.add_parser(
        "repair-loop-probe-metadata",
        help=(
            "Slow: re-read source_path files to backfill missing loop probe metadata "
            "(run before refresh-loop-stats when verify-fabrication-stock reports missing delay_var)"
        ),
    )
    subparsers.add_parser(
        "train-image-quality-model",
        help="Train the real LightGBM model for the image_quality regression scenario",
    )
    subparsers.add_parser(
        "finalize-image-quality-model",
        help="Check readiness and train the real LightGBM model when image_quality is mature",
    )
    subparsers.add_parser(
        "finalize-loop-intent",
        help=(
            "Refresh loop_intent feature stats, directory_loop_intent_score, and HDBSCAN catalog "
            "(pgvector HNSW + cluster priors; not LightGBM)"
        ),
    )
    finalize_all_parser = subparsers.add_parser(
        "finalize-runtime-assets",
        help=(
            "Fill all runtime assets after ingestion: loop_intent KNN stats + directory scores "
            "and image_quality LightGBM when mature"
        ),
    )
    finalize_all_parser.add_argument(
        "--install-missing-python-deps",
        action="store_true",
        help="Install missing LightGBM Python deps into the workspace venv when needed",
    )
    repair_parser = subparsers.add_parser(
        "repair-multi-scenario-schema",
        help="Apply the idempotent multi-scenario migration and optionally drop legacy gif_quality blockers first",
    )
    repair_parser.add_argument(
        "--drop-legacy-gif-schema",
        action="store_true",
        help="Drop legacy gif_quality schema objects and metadata before applying the strict animated_image_quality schema",
    )
    subparsers.add_parser(
        "show-image-quality-model-paths",
        help="Print the default LightGBM model artifact paths for image_quality",
    )
    ingest_parser = subparsers.add_parser(
        "ingest",
        help="Show the supported batch-ingestion entrypoint",
    )
    ingest_parser.add_argument("path", help="Dataset path hint")

    args = parser.parse_args()

    if args.command == "ingest":
        print_ingest_guidance(args.path)
        return

    connstr = resolve_connstr(args.connstr)
    if args.command == "train":
        print(
            "Legacy `train` is deprecated; running `run_training.py --execute --use-api`."
        )
        raise SystemExit(run_training_batch(connstr))
    if args.command == "export-stats":
        print("Legacy `export-stats` is deprecated; running `refresh-loop-stats`.")
        raise SystemExit(refresh_loop_stats(connstr))
    if args.command == "refresh-loop-stats":
        raise SystemExit(refresh_loop_stats(connstr))
    if args.command == "repair-loop-probe-metadata":
        raise SystemExit(repair_loop_probe_metadata(connstr))
    if args.command == "train-image-quality-model":
        raise SystemExit(train_image_quality_model(connstr))
    if args.command == "finalize-image-quality-model":
        raise SystemExit(finalize_image_quality_model(connstr))
    if args.command == "finalize-loop-intent":
        raise SystemExit(finalize_loop_intent_assets(connstr))
    if args.command == "finalize-runtime-assets":
        install_deps = bool(getattr(args, "install_missing_python_deps", False))
        raise SystemExit(
            finalize_runtime_assets(connstr, install_missing_python_deps=install_deps)
        )
    if args.command == "repair-multi-scenario-schema":
        raise SystemExit(
            repair_multi_scenario_schema(
                connstr,
                drop_legacy_gif_schema=args.drop_legacy_gif_schema,
            )
        )
    if args.command == "show-image-quality-model-paths":
        python = preferred_training_python()
        env = os.environ.copy()
        env.setdefault(QUALITY_MODEL_PYTHON_ENV, python)
        raise SystemExit(
            run_delegated(
                [python, str(QUALITY_REGRESSION_MODEL_SCRIPT), "show-paths"],
                parent_script="training_pipeline.py",
                cwd=ROOT,
                env=env,
                check=False,
            ).returncode
        )

    conn = connect_pg(connstr)
    try:
        if args.command == "report":
            print_full_report(conn)
        elif args.command == "report-quality-regression":
            print_quality_regression_report(conn)
        elif args.command == "report-loop-clustering":
            print_loop_clustering_report(conn)
        elif args.command == "evaluate":
            print(
                "Legacy `evaluate` is deprecated; running the combined loop-clustering + quality-regression audit."
            )
            print_full_report(conn)
            raise SystemExit(verify_embeddings(conn))
        elif args.command == "verify-embeddings":
            raise SystemExit(verify_embeddings(conn))
        elif args.command == "verify-quality-regression":
            raise SystemExit(
                verify_embeddings(
                    conn,
                    QUALITY_REGRESSION_SCENARIOS,
                    "quality regression verification",
                )
            )
        elif args.command == "verify-stack-readiness":
            raise SystemExit(verify_stack_readiness(conn))
        elif args.command == "verify-fabrication-stock":
            raise SystemExit(verify_fabrication_stock(conn))
        elif args.command == "verify-loop-clustering":
            raise SystemExit(
                verify_embeddings(
                    conn,
                    LOOP_CLUSTERING_SCENARIOS,
                    "loop clustering verification",
                    include_legacy_schema=False,
                )
            )
    finally:
        conn.close()


if __name__ == "__main__":
    main()
