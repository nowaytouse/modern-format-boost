#!/usr/bin/env python3
"""
Backfill `metadata.directory_loop_intent_score` inside `loop_samples`.

New-schema only:
- reads `loop_samples`
- writes JSONB metadata
- optionally refreshes loop statistics after the backfill
"""

from __future__ import annotations

import argparse
import os
import sys
import time
from collections.abc import Mapping, Sequence
from dataclasses import dataclass
from pathlib import Path
from types import TracebackType
from typing import Final, Protocol, cast

try:
    import psycopg2  # pyright: ignore[reportMissingModuleSource]
except ModuleNotFoundError:  # optional for `--help`
    psycopg2 = None

SCRIPT_DIR = Path(__file__).parent
if str(SCRIPT_DIR) not in sys.path:
    sys.path.insert(0, str(SCRIPT_DIR))

from typing import Self

from mfb_config_load import load_consumer_json
from mfb_entry_guard import guard_main, run_delegated

CONFIG_PATH = SCRIPT_DIR.parent / "src" / "config" / "directory_keywords.json"
CONFIG_CONSUMER = "backfill_directory_scores.py"
DEFAULT_CONNSTR: Final = "postgresql://localhost/modern_format_boost"
BACKFILL_PROGRESS_INTERVAL: Final = 1000
BACKFILL_HEARTBEAT_SECS: Final = 10.0

JsonObject = dict[str, object]
DbRow = tuple[object, ...]
DbRows = list[DbRow]


class DbCursor(Protocol):
    def __enter__(self) -> Self: ...
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
    def fetchone(self) -> DbRow | None: ...
    def fetchall(self) -> DbRows: ...
    def close(self) -> object: ...


class DbConnection(Protocol):
    def cursor(self) -> DbCursor: ...
    def set_session(self, *, autocommit: bool = False) -> object: ...
    def commit(self) -> object: ...
    def close(self) -> object: ...


@dataclass(frozen=True)
class DirectoryScoringConfig:
    base_score: float
    max_depth: int
    match_weight: float


@dataclass(frozen=True)
class KeywordsConfig:
    keywords: dict[str, tuple[str, ...]]
    scoring: DirectoryScoringConfig


DEFAULT_KEYWORDS_CONFIG: Final = KeywordsConfig(
    keywords={
        "meme_sticker": (
            "meme",
            "memes",
            "sticker",
            "stickers",
            "emoji",
            "emojis",
            "reaction",
            "reactions",
        )
    },
    scoring=DirectoryScoringConfig(base_score=0.5, max_depth=3, match_weight=0.5),
)


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


ROOT = discover_root(Path(__file__).resolve())
REFRESH_STATS_BIN = ROOT / "target" / "debug" / "refresh_stats"
SHARED_UTILS_SRC_DIR = ROOT / "crates" / "foundation" / "src"
REFRESH_STATS_SOURCES = [
    ROOT / "Cargo.toml",
    *(SHARED_UTILS_SRC_DIR.rglob("*.rs")),
]


def resolve_connstr(explicit: str | None) -> str:
    connstr = explicit or os.environ.get("MFB_PG_CONNSTR") or DEFAULT_CONNSTR
    return connstr.strip() or DEFAULT_CONNSTR


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


def as_object(value: object) -> JsonObject:
    if not isinstance(value, Mapping):
        return {}
    parsed: JsonObject = {}
    for key, item in value.items():
        if isinstance(key, str):
            parsed[key] = item
    return parsed


def as_object_list(value: object) -> list[object]:
    if not isinstance(value, list):
        return []
    parsed: list[object] = []
    for item in value:
        parsed.append(item)  # noqa: PERF402
    return parsed


def as_string_list(value: object) -> list[str]:
    return [item for item in as_object_list(value) if isinstance(item, str)]


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
        raise RuntimeError(f"{context} is not a float: {value!r}")  # noqa: TRY004
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


def as_optional_str(value: object, context: str) -> str | None:
    if value is None:
        return None
    if isinstance(value, str):
        return value
    raise RuntimeError(f"{context} is not a string or null: {value!r}")


def as_bytes(value: object, context: str) -> bytes:
    if isinstance(value, bytes):
        return value
    if isinstance(value, bytearray):
        return bytes(value)
    if isinstance(value, memoryview):
        return value.tobytes()
    raise RuntimeError(f"{context} is not bytes-like: {value!r}")


def format_elapsed_secs(seconds: float) -> str:
    if seconds >= 10.0:
        return f"{seconds:.1f}s"
    return f"{seconds:.2f}s"


def format_optional_score(value: float | None) -> str:
    if value is None:
        return "N/A"
    return f"{value:.4f}"


def require_row(row: DbRow | None, context: str) -> DbRow:
    if row is None:
        raise RuntimeError(f"{context} returned no rows")
    return row


def parse_scoring_config(value: object) -> DirectoryScoringConfig:
    scoring_obj = as_object(value)
    defaults = DEFAULT_KEYWORDS_CONFIG.scoring
    return DirectoryScoringConfig(
        base_score=as_float(
            scoring_obj.get("base_score", defaults.base_score), "base_score"
        ),
        max_depth=max(
            1, as_int(scoring_obj.get("max_depth", defaults.max_depth), "max_depth")
        ),
        match_weight=as_float(
            scoring_obj.get("match_weight", defaults.match_weight), "match_weight"
        ),
    )


def parse_keyword_groups(value: object) -> dict[str, tuple[str, ...]]:
    keywords_obj = as_object(value)
    parsed: dict[str, tuple[str, ...]] = {}
    for key, item in keywords_obj.items():
        parsed[key] = tuple(as_string_list(item))
    return parsed


def load_keywords_config() -> KeywordsConfig:
    if not CONFIG_PATH.exists():
        return DEFAULT_KEYWORDS_CONFIG

    raw_config = load_consumer_json(CONFIG_PATH, expected_consumer=CONFIG_CONSUMER)

    keywords = parse_keyword_groups(raw_config.get("keywords"))
    scoring = parse_scoring_config(raw_config.get("scoring"))
    return KeywordsConfig(
        keywords=keywords or DEFAULT_KEYWORDS_CONFIG.keywords,
        scoring=scoring,
    )


def compute_directory_score(
    source_path: str | None, keywords: Sequence[str], scoring: DirectoryScoringConfig
) -> float | None:
    if not source_path:
        return None

    try:
        path = Path(source_path)
        parents = list(path.parent.parts)
        max_depth = scoring.max_depth
        last_parts = parents[-max_depth:] if len(parents) >= max_depth else parents
        matches = 0
        for part in last_parts:
            lower = part.lower()
            if any(keyword in lower for keyword in keywords):
                matches += 1

        score = (
            scoring.base_score + (matches / max(max_depth, 1)) * scoring.match_weight
        )
        return max(0.0, min(1.0, score))
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
        print(
            f"directory score unavailable for {source_path!r}: {exc}",
            file=sys.stderr,
        )
        return None


def connect_pg(connstr: str) -> DbConnection:
    if psycopg2 is None:
        raise SystemExit("psycopg2 is required to backfill directory scores.")
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


def refresh_loop_stats(connstr: str) -> int:
    extra = {
        "MFB_PG_CONNSTR": connstr,
        "MFB_TRAINING_INVOKER": "backfill_directory_scores",
    }
    if artifact_is_stale(REFRESH_STATS_BIN, REFRESH_STATS_SOURCES):
        cmd = ["cargo", "run", "-p", "foundation", "--bin", "refresh_stats"]
    else:
        cmd = [str(REFRESH_STATS_BIN)]

    result = run_delegated(
        cmd, parent_script=CONFIG_CONSUMER, cwd=ROOT, env=extra, check=False
    )
    return result.returncode


def main() -> None:
    guard_main(CONFIG_CONSUMER, require_invoker=True)
    parser = argparse.ArgumentParser(
        description="Backfill loop_samples.metadata.directory_loop_intent_score"
    )
    parser.add_argument("--connstr", default=None, help="PostgreSQL connection string")
    parser.add_argument(
        "--no-refresh-stats",
        action="store_true",
        help="Skip the Rust loop-stat refresh after backfill",
    )
    args = parser.parse_args()

    connstr = resolve_connstr(args.connstr)
    config = load_keywords_config()

    all_keywords: list[str] = []
    for category_keywords in config.keywords.values():
        all_keywords.extend(category_keywords)
    scoring = config.scoring

    conn = connect_pg(connstr)
    try:
        with conn.cursor() as cur:
            cur.execute("SELECT blake3, source_path FROM loop_samples")
            rows = cur.fetchall()

            updates: list[tuple[float, bytes]] = []
            skipped_unknown = 0
            started = time.monotonic()
            last_progress_at = started
            last_progress_index = 0
            total_rows = len(rows)
            print(
                f"Scoring {total_rows} loop_samples rows "
                f"(heartbeat_every={BACKFILL_PROGRESS_INTERVAL} rows, "
                f"heartbeat_max_silence={format_elapsed_secs(BACKFILL_HEARTBEAT_SECS)})..."
            )
            for index, row in enumerate(rows, start=1):
                blake3 = as_bytes(row[0], "loop_samples.blake3")
                source_path = as_optional_str(row[1], "loop_samples.source_path")
                score = compute_directory_score(source_path, all_keywords, scoring)
                if score is None:
                    skipped_unknown += 1
                else:
                    updates.append((score, blake3))
                now = time.monotonic()
                if (
                    index == 1
                    or index == total_rows
                    or index - last_progress_index >= BACKFILL_PROGRESS_INTERVAL
                    or now - last_progress_at >= BACKFILL_HEARTBEAT_SECS
                ):
                    elapsed = now - started
                    rate = index / elapsed if elapsed > 0 else 0.0
                    print(
                        "  [BACKFILL] scoring… "
                        f"{index}/{total_rows} rows, "
                        f"rate={rate:.1f}/s, elapsed={format_elapsed_secs(elapsed)}, "
                        f"source_path={source_path or '<null>'}"
                    )
                    last_progress_at = now
                    last_progress_index = index

            print(f"Rows skipped for absent directory evidence: {skipped_unknown}")
            if updates:
                print(
                    f"Applying {len(updates)} metadata.directory_loop_intent_score updates via executemany..."
                )
                cur.executemany(
                    """
                    UPDATE loop_samples
                    SET metadata = jsonb_set(
                        COALESCE(metadata, '{}'::jsonb),
                        '{directory_loop_intent_score}',
                        to_jsonb(%s::double precision),
                        true
                    )
                    WHERE blake3 = %s
                    """,
                    updates,
                )
            else:
                print(
                    "No directory score updates had source-path evidence; no rows updated."
                )
            print("Committing directory score updates...")
            conn.commit()

            cur.execute(
                """
                SELECT COUNT(*),
                       COUNT(*) FILTER (WHERE metadata ? 'directory_loop_intent_score'),
                       AVG((metadata->>'directory_loop_intent_score')::double precision)
                           FILTER (WHERE metadata ? 'directory_loop_intent_score'),
                       MIN((metadata->>'directory_loop_intent_score')::double precision)
                           FILTER (WHERE metadata ? 'directory_loop_intent_score'),
                       MAX((metadata->>'directory_loop_intent_score')::double precision)
                           FILTER (WHERE metadata ? 'directory_loop_intent_score')
                FROM loop_samples
                """
            )
            summary_row = require_row(cur.fetchone(), "loop_samples backfill summary")

        count = as_int(summary_row[0], "loop_samples count")
        scored_count = as_int(summary_row[1], "loop_samples scored_count")
        avg_score = as_optional_float(summary_row[2], "loop_samples avg_score")
        min_score = as_optional_float(summary_row[3], "loop_samples min_score")
        max_score = as_optional_float(summary_row[4], "loop_samples max_score")

        print("Backfill complete.")
        print(f"  Total rows: {count}")
        print(f"  Rows with score evidence: {scored_count}")
        print(f"  Average score: {format_optional_score(avg_score)}")
        print(
            "  Score range: "
            f"{format_optional_score(min_score)} - {format_optional_score(max_score)}"
        )
    finally:
        conn.close()

    if not args.no_refresh_stats:
        raise SystemExit(refresh_loop_stats(connstr))


if __name__ == "__main__":
    main()
