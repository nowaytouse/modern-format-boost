#!/usr/bin/env python3
"""Real LightGBM training and inference for `image_quality`."""

from __future__ import annotations

import argparse
import json
import math
import os
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Any

import lightgbm as lgb
import numpy as np
from sklearn.metrics import log_loss, roc_auc_score
from sklearn.model_selection import train_test_split
from sklearn.neighbors import NearestNeighbors

DEFAULT_CONNSTR = "postgresql://localhost/modern_format_boost"
SCENARIO = "image_quality"
FEATURE_SCHEMA = "image_quality_lgbm_v1"
MODEL_NAME = "lightgbm_model.txt"
METADATA_NAME = "lightgbm_model.metadata.json"
EMBEDDING_DIM = 264
KNN_K = 5
KNN_THRESHOLD = 2.0
MIN_SAMPLES_TOTAL = 30
MIN_SAMPLES_PER_CLASS = 10

SCALAR_FEATURE_NAMES = [
    "width",
    "height",
    "file_size_bytes",
    "total_pixels",
    "entropy",
    "compression_ratio",
    "spatial_bpp",
    "log_total_pixels",
    "log_file_size_bytes",
    "log_spatial_bpp",
    "aspect_ratio",
    "is_lossless",
    "bpp_heuristic_score",
]
FORMAT_FEATURE_NAMES = [
    "fmt_png",
    "fmt_jpeg",
    "fmt_webp",
    "fmt_tiff",
    "fmt_avif",
    "fmt_heic",
    "fmt_jxl",
    "fmt_other",
]
KNN_FEATURE_NAMES = [
    "knn_score_mean_k5",
    "knn_score_std_k5",
    "knn_score_min_k5",
    "dist_to_nearest",
    "dist_weighted_score",
    "knn_confidence",
    "knn_neighbor_count",
    "knn_neighbor_coverage",
    "knn_available",
]
EMBEDDING_FEATURE_NAMES = [f"embedding_{index:03d}" for index in range(EMBEDDING_DIM)]
FEATURE_NAMES = (
    SCALAR_FEATURE_NAMES
    + FORMAT_FEATURE_NAMES
    + KNN_FEATURE_NAMES
    + EMBEDDING_FEATURE_NAMES
)
# Rust inference may send JSON null for absent measurement embed slots (M223/M225).
NULLABLE_EMBED_FEATURES = frozenset(
    {
        "embedding_012",
        "embedding_017",
        "embedding_018",
        "embedding_019",
        "embedding_020",
    }
)
OPTIONAL_MEASUREMENT_FLAGS = frozenset({"psnr_measured", "ssim_measured"})
LIGHTGBM_MISSING_MEASUREMENT = float("nan")
PGVECTOR_MISSING_MEASUREMENT = -1.0


@dataclass(frozen=True)
class SampleRow:
    width: int
    height: int
    file_size_bytes: int
    format: str
    total_pixels: int
    entropy: float
    compression_ratio: float
    spatial_bpp: float
    is_lossless: bool
    embedding: list[float]
    quality_score: float


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


try:
    import psycopg2  # pyright: ignore[reportMissingModuleSource]
except ModuleNotFoundError:
    append_workspace_venv_site_packages()
    try:
        import psycopg2  # pyright: ignore[reportMissingModuleSource]
    except ModuleNotFoundError:
        psycopg2 = None


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


def default_model_dir() -> Path:
    return cache_base_dir() / "models" / "image_quality"


def default_model_path() -> Path:
    explicit = os.environ.get("MFB_IMAGE_QUALITY_MODEL_PATH")
    if explicit:
        return Path(explicit).expanduser()
    return default_model_dir() / MODEL_NAME


def default_metadata_path() -> Path:
    explicit = os.environ.get("MFB_IMAGE_QUALITY_MODEL_METADATA_PATH")
    if explicit:
        return Path(explicit).expanduser()
    return default_model_dir() / METADATA_NAME


def resolve_connstr(explicit: str | None) -> str:
    value = explicit or os.environ.get("MFB_PG_CONNSTR") or DEFAULT_CONNSTR
    return value.strip() or DEFAULT_CONNSTR


def parse_embedding_text(text: str) -> list[float]:
    stripped = text.strip()
    if not stripped.startswith("[") or not stripped.endswith("]"):
        raise ValueError(f"Unexpected pgvector text payload: {text[:40]!r}")
    body = stripped[1:-1].strip()
    if not body:
        return []
    values = [float(part.strip()) for part in body.split(",")]
    if len(values) != EMBEDDING_DIM:
        raise ValueError(
            f"Embedding dimension mismatch: expected {EMBEDDING_DIM}, got {len(values)}"
        )
    return values


def bpp_heuristic_score(entropy: float, spatial_bpp: float, is_lossless: bool) -> float:
    entropy_score = max(0.0, min(1.0, entropy / 8.0))
    bpp_score = max(0.0, 1.0 - min(1.0, spatial_bpp / 20.0))
    lossless_bonus = 0.1 if is_lossless else 0.0
    return max(0.0, min(1.0, entropy_score * 0.5 + bpp_score * 0.5 + lossless_bonus))


def append_format_flags(row: dict[str, float], format_name: str) -> None:
    format_lower = format_name.strip().lower()
    is_png = "png" in format_lower
    is_jpeg = "jpeg" in format_lower or "jpg" in format_lower
    is_webp = "webp" in format_lower
    is_tiff = "tiff" in format_lower or "tif" in format_lower
    is_avif = "avif" in format_lower
    is_heic = "heic" in format_lower or "heif" in format_lower
    is_jxl = "jxl" in format_lower or "jpeg-xl" in format_lower
    is_other = not any([is_png, is_jpeg, is_webp, is_tiff, is_avif, is_heic, is_jxl])
    row.update(
        {
            "fmt_png": 1.0 if is_png else 0.0,
            "fmt_jpeg": 1.0 if is_jpeg else 0.0,
            "fmt_webp": 1.0 if is_webp else 0.0,
            "fmt_tiff": 1.0 if is_tiff else 0.0,
            "fmt_avif": 1.0 if is_avif else 0.0,
            "fmt_heic": 1.0 if is_heic else 0.0,
            "fmt_jxl": 1.0 if is_jxl else 0.0,
            "fmt_other": 1.0 if is_other else 0.0,
        }
    )


def build_scalar_features(sample: SampleRow) -> dict[str, float]:
    total_pixels = float(max(sample.total_pixels, 1))
    file_size_bytes = float(max(sample.file_size_bytes, 1))
    spatial_bpp = float(sample.spatial_bpp)
    row: dict[str, float] = {
        "width": float(sample.width),
        "height": float(sample.height),
        "file_size_bytes": file_size_bytes,
        "total_pixels": total_pixels,
        "entropy": float(sample.entropy),
        "compression_ratio": float(sample.compression_ratio),
        "spatial_bpp": spatial_bpp,
        "log_total_pixels": math.log10(total_pixels),
        "log_file_size_bytes": math.log10(file_size_bytes),
        "log_spatial_bpp": math.log1p(max(0.0, spatial_bpp)),
        "aspect_ratio": float(sample.width) / float(max(sample.height, 1)),
        "is_lossless": 1.0 if sample.is_lossless else 0.0,
        "bpp_heuristic_score": bpp_heuristic_score(
            float(sample.entropy), spatial_bpp, sample.is_lossless
        ),
    }
    append_format_flags(row, sample.format)
    return row


def normalize_nullable_embed_slots(row: dict[str, float]) -> None:
    """Training parity with Rust predict: DB sentinel = absent measurement."""
    for name in NULLABLE_EMBED_FEATURES:
        if row.get(name) in (0.0, PGVECTOR_MISSING_MEASUREMENT):
            row[name] = LIGHTGBM_MISSING_MEASUREMENT


def zero_knn_features() -> dict[str, float]:
    return {
        "knn_score_mean_k5": 0.0,
        "knn_score_std_k5": 0.0,
        "knn_score_min_k5": 0.0,
        "dist_to_nearest": 0.0,
        "dist_weighted_score": 0.0,
        "knn_confidence": 0.0,
        "knn_neighbor_count": 0.0,
        "knn_neighbor_coverage": 0.0,
        "knn_available": 0.0,
    }


def compute_knn_features(
    embeddings: np.ndarray, labels: np.ndarray
) -> list[dict[str, float]]:
    if embeddings.shape[0] <= 1:
        return [zero_knn_features() for _ in range(embeddings.shape[0])]

    neighbor_count = min(embeddings.shape[0], KNN_K + 1)
    nn = NearestNeighbors(n_neighbors=neighbor_count, metric="euclidean")
    nn.fit(embeddings)
    distances, indices = nn.kneighbors(embeddings)

    features: list[dict[str, float]] = []
    for sample_index, (dist_row, index_row) in enumerate(
        zip(distances, indices, strict=True)
    ):
        usable: list[tuple[float, int]] = []
        for distance, neighbor_index in zip(
            dist_row.tolist(), index_row.tolist(), strict=True
        ):
            if neighbor_index == sample_index:
                continue
            if distance <= KNN_THRESHOLD:
                usable.append((float(distance), int(neighbor_index)))
            if len(usable) >= KNN_K:
                break

        if not usable:
            features.append(zero_knn_features())
            continue

        neighbor_scores = np.asarray(
            [labels[idx] for _, idx in usable], dtype=np.float64
        )
        neighbor_distances = np.asarray(
            [distance for distance, _ in usable], dtype=np.float64
        )
        mean = float(np.mean(neighbor_scores))
        std = float(np.std(neighbor_scores))
        score_min = float(np.min(neighbor_scores))
        dist_to_nearest = float(np.min(neighbor_distances))
        weights = 1.0 / (neighbor_distances + 0.01)
        dist_weighted_score = float(np.sum(neighbor_scores * weights) / np.sum(weights))
        confidence = float(1.0 / (1.0 + std * dist_to_nearest))
        count = float(len(usable))
        coverage = min(1.0, count / float(KNN_K))
        features.append(
            {
                "knn_score_mean_k5": mean,
                "knn_score_std_k5": std,
                "knn_score_min_k5": score_min,
                "dist_to_nearest": dist_to_nearest,
                "dist_weighted_score": dist_weighted_score,
                "knn_confidence": confidence,
                "knn_neighbor_count": count,
                "knn_neighbor_coverage": coverage,
                "knn_available": 1.0,
            }
        )

    return features


def build_feature_row(sample: SampleRow, knn: dict[str, float]) -> dict[str, float]:
    row = build_scalar_features(sample)
    row.update(knn)
    for index, value in enumerate(sample.embedding):
        row[f"embedding_{index:03d}"] = float(value)
    normalize_nullable_embed_slots(row)
    return row


def fetch_training_rows(connstr: str) -> list[SampleRow]:
    if psycopg2 is None:
        raise SystemExit("psycopg2 is required to train the image quality model")

    conn = psycopg2.connect(connstr)
    try:
        with conn.cursor() as cur:
            cur.execute(
                """
                SELECT
                    width,
                    height,
                    file_size_bytes,
                    format,
                    COALESCE(total_pixels, width::bigint * height::bigint),
                    entropy,
                    compression_ratio,
                    spatial_bpp,
                    is_lossless,
                    embedding::text,
                    quality_score::double precision
                FROM image_quality_samples
                WHERE embedding IS NOT NULL
                  AND quality_score IS NOT NULL
                ORDER BY id
                """
            )
            rows = cur.fetchall()
    finally:
        conn.close()

    samples: list[SampleRow] = []
    for row in rows:
        try:
            samples.append(
                SampleRow(
                    width=int(row[0]),
                    height=int(row[1]),
                    file_size_bytes=int(row[2]),
                    format=str(row[3]),
                    total_pixels=int(row[4]),
                    entropy=float(row[5]),
                    compression_ratio=float(row[6]),
                    spatial_bpp=float(row[7]),
                    is_lossless=bool(row[8]),
                    embedding=parse_embedding_text(str(row[9])),
                    quality_score=float(row[10]),
                )
            )
        except (TypeError, ValueError) as exc:
            print(f"[WARN] Skipping malformed training row: {exc}", file=sys.stderr)
    return samples


def train_image_quality_model(
    connstr: str,
    model_path: Path,
    metadata_path: Path,
    min_samples: int,
    min_samples_per_class: int,
    seed: int,
) -> int:
    samples = fetch_training_rows(connstr)
    embeddings = np.asarray([sample.embedding for sample in samples], dtype=np.float64)
    labels = np.asarray([sample.quality_score for sample in samples], dtype=np.float64)
    positive_count = int(np.sum(labels >= 0.5))
    negative_count = int(labels.shape[0] - positive_count)
    readiness_issues: list[str] = []
    if len(samples) < min_samples:
        readiness_issues.append(f"total<{min_samples} ({len(samples)})")
    if positive_count < min_samples_per_class:
        readiness_issues.append(f"high<{min_samples_per_class} ({positive_count})")
    if negative_count < min_samples_per_class:
        readiness_issues.append(f"low<{min_samples_per_class} ({negative_count})")
    if readiness_issues:
        print(
            "Image-quality model training blocked: "
            + "; ".join(readiness_issues)
            + ".",
            file=sys.stderr,
        )
        return 2

    knn_features = compute_knn_features(embeddings, labels)
    feature_rows = [
        build_feature_row(sample, knn)
        for sample, knn in zip(samples, knn_features, strict=True)
    ]
    matrix = np.asarray(
        [[row[name] for name in FEATURE_NAMES] for row in feature_rows],
        dtype=np.float64,
    )

    stratify: np.ndarray | None = None
    if positive_count >= 2 and negative_count >= 2 and labels.shape[0] >= 10:
        stratify = (labels >= 0.5).astype(np.int32)

    if stratify is not None:
        (
            x_train,
            x_valid,
            y_train,
            y_valid,
        ) = train_test_split(
            matrix,
            labels,
            test_size=0.2,
            random_state=seed,
            stratify=stratify,
        )
    else:
        x_train = matrix
        y_train = labels
        x_valid = None
        y_valid = None

    train_set = lgb.Dataset(
        x_train, label=y_train, feature_name=FEATURE_NAMES, free_raw_data=False
    )
    params: dict[str, Any] = {
        "objective": "binary",
        "metric": ["binary_logloss", "auc"],
        "learning_rate": 0.05,
        "num_leaves": 31,
        "feature_fraction": 0.9,
        "bagging_fraction": 0.9,
        "bagging_freq": 1,
        "min_data_in_leaf": max(5, int(len(samples) * 0.05)),
        "verbosity": -1,
        "seed": seed,
    }

    callbacks: list[Any] = [lgb.log_evaluation(period=0)]
    valid_sets: list[Any] | None = None
    valid_names: list[str] | None = None
    if x_valid is not None and y_valid is not None:
        valid_set = lgb.Dataset(
            x_valid,
            label=y_valid,
            feature_name=FEATURE_NAMES,
            reference=train_set,
            free_raw_data=False,
        )
        valid_sets = [valid_set]
        valid_names = ["valid"]
        callbacks.append(lgb.early_stopping(stopping_rounds=25, verbose=False))

    booster = lgb.train(
        params,
        train_set,
        num_boost_round=300,
        valid_sets=valid_sets,
        valid_names=valid_names,
        callbacks=callbacks,
    )

    best_iteration = booster.best_iteration or booster.current_iteration()
    train_predictions = booster.predict(x_train, num_iteration=best_iteration)
    train_logloss = float(log_loss(y_train, train_predictions, labels=[0.0, 1.0]))
    train_auc: float | None
    try:
        train_auc = float(roc_auc_score(y_train, train_predictions))
    except ValueError:
        train_auc = None

    validation_metrics: dict[str, float | None] = {}
    if x_valid is not None and y_valid is not None:
        valid_predictions = booster.predict(x_valid, num_iteration=best_iteration)
        validation_metrics["logloss"] = float(
            log_loss(y_valid, valid_predictions, labels=[0.0, 1.0])
        )
        try:
            validation_metrics["auc"] = float(roc_auc_score(y_valid, valid_predictions))
        except ValueError:
            validation_metrics["auc"] = None

    model_path.parent.mkdir(parents=True, exist_ok=True)
    metadata_path.parent.mkdir(parents=True, exist_ok=True)
    booster.save_model(str(model_path), num_iteration=best_iteration)

    metadata = {
        "scenario": SCENARIO,
        "feature_schema": FEATURE_SCHEMA,
        "predictor_family": "lightgbm_binary",
        "feature_names": FEATURE_NAMES,
        "embedding_dimension": EMBEDDING_DIM,
        "knn_k": KNN_K,
        "knn_threshold": KNN_THRESHOLD,
        "training_rows": int(labels.shape[0]),
        "positive_rows": positive_count,
        "negative_rows": negative_count,
        "min_samples_total": min_samples,
        "min_samples_per_class": min_samples_per_class,
        "train_logloss": train_logloss,
        "train_auc": train_auc,
        "validation_metrics": validation_metrics,
        "best_iteration": int(best_iteration),
    }
    metadata_path.write_text(
        json.dumps(metadata, indent=2, sort_keys=True), encoding="utf-8"
    )

    print(f"Trained {SCENARIO} LightGBM model")
    print(f"  model:    {model_path}")
    print(f"  metadata: {metadata_path}")
    print(f"  rows:     {labels.shape[0]}")
    print(f"  train logloss: {train_logloss:.6f}")
    if train_auc is not None:
        print(f"  train auc:     {train_auc:.6f}")
    if validation_metrics:
        print(f"  valid logloss: {validation_metrics.get('logloss', float('nan')):.6f}")
        valid_auc = validation_metrics.get("auc")
        if valid_auc is not None:
            print(f"  valid auc:     {valid_auc:.6f}")
    return 0


def as_float(value: Any, name: str) -> float:
    try:
        result = float(value)
    except (TypeError, ValueError) as exc:
        raise SystemExit(f"Feature {name} is not numeric: {value!r}") from exc
    if not math.isfinite(result):
        raise SystemExit(f"Feature {name} is not finite: {value!r}")
    return result


def predict_feature_scalar(features: dict[str, Any], name: str) -> float:
    """Map Rust JSON features to a LightGBM row (NaN = absent measurement, not forged 0)."""
    if name not in features:
        if name in NULLABLE_EMBED_FEATURES:
            return LIGHTGBM_MISSING_MEASUREMENT
        if name in OPTIONAL_MEASUREMENT_FLAGS:
            return 0.0
        raise SystemExit(f"Feature {name} is missing from request")
    raw = features[name]
    if raw is None:
        if name in NULLABLE_EMBED_FEATURES:
            return LIGHTGBM_MISSING_MEASUREMENT
        if name in OPTIONAL_MEASUREMENT_FLAGS:
            return 0.0
        raise SystemExit(f"Feature {name} is null but required")
    if name in OPTIONAL_MEASUREMENT_FLAGS and isinstance(raw, bool):
        return 1.0 if raw else 0.0
    return as_float(raw, name)


def predict_image_quality(
    model_path: Path,
    metadata_path: Path,
    request: dict[str, Any],
) -> int:
    metadata = json.loads(metadata_path.read_text(encoding="utf-8"))
    if metadata.get("scenario") != SCENARIO:
        raise SystemExit(
            f"Metadata scenario mismatch: expected {SCENARIO}, got {metadata.get('scenario')!r}"
        )
    if metadata.get("feature_schema") != FEATURE_SCHEMA:
        raise SystemExit(
            "Feature schema mismatch: "
            f"expected {FEATURE_SCHEMA}, got {metadata.get('feature_schema')!r}"
        )
    feature_names = metadata.get("feature_names")
    if feature_names != FEATURE_NAMES:
        raise SystemExit(
            "Feature name mismatch between model metadata and runtime contract"
        )

    features = request.get("features")
    if not isinstance(features, dict):
        raise SystemExit("Prediction request must contain a 'features' object")
    vector = np.asarray(
        [[predict_feature_scalar(features, name) for name in feature_names]],
        dtype=np.float64,
    )

    booster = lgb.Booster(model_file=str(model_path))
    best_iteration = metadata.get("best_iteration")
    prediction = float(booster.predict(vector, num_iteration=best_iteration)[0])
    raw_score = float(
        booster.predict(vector, raw_score=True, num_iteration=best_iteration)[0]
    )
    confidence = max(0.0, min(1.0, abs(prediction - 0.5) * 2.0))
    response = {
        "scenario": SCENARIO,
        "feature_schema": FEATURE_SCHEMA,
        "predictor_family": "lightgbm_binary",
        "score": max(0.0, min(1.0, prediction)),
        "confidence": confidence,
        "raw_score": raw_score,
    }
    print(json.dumps(response))
    return 0


def read_request_json(path: str | None) -> dict[str, Any]:
    if path:
        return json.loads(Path(path).read_text(encoding="utf-8"))
    return json.load(sys.stdin)


def show_paths() -> int:
    print(
        json.dumps(
            {
                "model": str(default_model_path()),
                "metadata": str(default_metadata_path()),
                "scenario": SCENARIO,
                "feature_schema": FEATURE_SCHEMA,
                "min_samples_total": MIN_SAMPLES_TOTAL,
                "min_samples_per_class": MIN_SAMPLES_PER_CLASS,
            },
            indent=2,
            sort_keys=True,
        )
    )
    return 0


def main() -> int:
    _scripts = Path(__file__).resolve().parent
    if str(_scripts) not in sys.path:
        sys.path.insert(0, str(_scripts))
    from mfb_entry_guard import guard_main  # noqa: E402

    guard_main("quality_regression_model.py", require_invoker=True)
    parser = argparse.ArgumentParser(
        description="Real LightGBM training and inference for image_quality"
    )
    subparsers = parser.add_subparsers(dest="command", required=True)

    train_parser = subparsers.add_parser(
        "train-image-quality", help="Train the real image_quality LightGBM model"
    )
    train_parser.add_argument("--connstr", default=None)
    train_parser.add_argument("--model", default=str(default_model_path()))
    train_parser.add_argument("--metadata", default=str(default_metadata_path()))
    train_parser.add_argument("--min-samples", type=int, default=MIN_SAMPLES_TOTAL)
    train_parser.add_argument(
        "--min-samples-per-class", type=int, default=MIN_SAMPLES_PER_CLASS
    )
    train_parser.add_argument("--seed", type=int, default=42)

    predict_parser = subparsers.add_parser(
        "predict-image-quality",
        help="Run a saved LightGBM model on a JSON feature payload",
    )
    predict_parser.add_argument("--model", default=str(default_model_path()))
    predict_parser.add_argument("--metadata", default=str(default_metadata_path()))
    predict_parser.add_argument("--input-json", default=None)

    subparsers.add_parser("show-paths", help="Print default model artifact paths")

    args = parser.parse_args()
    if args.command == "show-paths":
        return show_paths()
    if args.command == "train-image-quality":
        return train_image_quality_model(
            resolve_connstr(args.connstr),
            Path(args.model),
            Path(args.metadata),
            args.min_samples,
            args.min_samples_per_class,
            args.seed,
        )
    if args.command == "predict-image-quality":
        return predict_image_quality(
            Path(args.model),
            Path(args.metadata),
            read_request_json(args.input_json),
        )
    raise SystemExit(f"Unknown command: {args.command}")


if __name__ == "__main__":
    raise SystemExit(main())
