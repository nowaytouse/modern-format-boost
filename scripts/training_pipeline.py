#!/usr/bin/env python3
"""
Modern Format Boost — KNN Training Pipeline

Complete, formalized Python training pipeline for the GIF value classification
KNN model. Connects to the PostgreSQL `modern_format_boost` database, trains
scikit-learn KNeighborsClassifier with cross-validated hyperparameter search,
and exports optimized feature statistics back to PostgreSQL.

Usage:
    python scripts/training_pipeline.py train
    python scripts/training_pipeline.py evaluate
    python scripts/training_pipeline.py export-stats
    python scripts/training_pipeline.py ingest /path/to/dataset
    python scripts/training_pipeline.py report
"""

from __future__ import annotations

import argparse
import json
import os
import sys
from dataclasses import dataclass, field
from typing import Any

import numpy as np
import pandas as pd
import psycopg2
from sklearn.metrics import (
    accuracy_score,
    classification_report,
    confusion_matrix,
    f1_score,
    precision_score,
    recall_score,
)
from sklearn.model_selection import GridSearchCV, StratifiedKFold, cross_val_score
from sklearn.neighbors import KNeighborsClassifier
from sklearn.preprocessing import StandardScaler
from tabulate import tabulate

# ── Constants ─────────────────────────────────────────────────────────────────

DEFAULT_CONNSTR = os.environ.get(
    "MFB_PG_CONNSTR", "host=localhost dbname=modern_format_boost"
)

FEATURE_COLUMNS = [
    "duration_secs",
    "frame_count",
    "fps",
    "temporal_bpp",
    "spatial_bpp",
    "total_pixels",
    "aspect_ratio",
    "loop_frequency",
    "cadence_score",
    "directory_meme_score",
    "palette_depth",
    "motion_gini",
    "block_skew",
    "temporal_flatness",
    "frame_payload_variation",
    "frame_delay_variation",
]

BOOL_COLUMNS = [
    "has_transparency",
    "has_embedded_icc",
    "has_complex_color_profile",
    "is_meme_platform",
    "is_human_semantic_name",
    "is_high_value_source",
    "is_native_gif",
]

LABEL_MAP = {"high": 1, "low": 0, "medium": 0}

STATS_KEY = "feature_stats_v1"


# ── Data Loading ──────────────────────────────────────────────────────────────


def connect_pg(connstr: str | None = None) -> psycopg2.extensions.connection:
    """Establish PostgreSQL connection."""
    cs = connstr or DEFAULT_CONNSTR
    try:
        conn = psycopg2.connect(cs)
        conn.set_session(autocommit=False)
        return conn
    except psycopg2.Error as e:
        print(f"❌ Failed to connect to PostgreSQL: {e}", file=sys.stderr)
        sys.exit(1)


def load_samples(conn: psycopg2.extensions.connection) -> pd.DataFrame:
    """Load all labeled samples from PostgreSQL."""
    query = """
        SELECT
            file_hash, file_name, loss_tolerance, labeled_by,
            width, height, duration_secs, frame_count, file_size_bytes, fps,
            has_embedded_icc, has_complex_color_profile, has_transparency,
            palette_size, frame_payload_variation, frame_delay_variation,
            temporal_bpp, spatial_bpp, aspect_ratio, total_pixels,
            loop_frequency, is_meme_platform, is_human_semantic_name,
            cadence_score, directory_meme_score, is_high_value_source,
            is_native_gif, palette_depth, motion_gini, block_skew,
            temporal_flatness
        FROM samples
        WHERE loss_tolerance IS NOT NULL
    """
    df = pd.read_sql_query(query, conn)
    print(f"📊 Loaded {len(df)} labeled samples")

    class_counts = df["loss_tolerance"].value_counts()
    for label, count in class_counts.items():
        print(f"   {label}: {count}")

    return df


def prepare_features(df: pd.DataFrame) -> tuple[np.ndarray, np.ndarray]:
    """Extract and normalize feature matrix and label vector."""
    # Compute derived features
    df = df.copy()
    df["total_pixels"] = df["width"].astype(float) * df["height"].astype(float)
    df["aspect_ratio"] = df.apply(
        lambda r: r["width"] / r["height"] if r["height"] > 0 else 1.0, axis=1
    )

    # Fill missing values with safe defaults
    for col in FEATURE_COLUMNS:
        if col in df.columns:
            df[col] = df[col].fillna(0.5 if col not in ("total_pixels",) else 0.0)
    for col in BOOL_COLUMNS:
        if col in df.columns:
            df[col] = df[col].fillna(False).astype(float)

    # Build feature matrix
    all_cols = FEATURE_COLUMNS + BOOL_COLUMNS
    existing = [c for c in all_cols if c in df.columns]
    X = df[existing].values.astype(np.float64)

    # Build labels
    y = df["loss_tolerance"].map(LABEL_MAP).fillna(0).values.astype(int)

    return X, y


# ── Training ──────────────────────────────────────────────────────────────────


@dataclass
class TrainingResult:
    best_k: int = 5
    best_metric: str = "euclidean"
    best_weights: str = "distance"
    best_score: float = 0.0
    cv_scores: list[float] = field(default_factory=list)
    feature_names: list[str] = field(default_factory=list)
    scaler: StandardScaler | None = None
    model: KNeighborsClassifier | None = None


def train_model(X: np.ndarray, y: np.ndarray) -> TrainingResult:
    """
    Train KNN classifier with cross-validated hyperparameter search.
    Returns the best model configuration and fitted model.
    """
    print("\n🧠 Training KNN classifier with hyperparameter grid search...")

    scaler = StandardScaler()
    X_scaled = scaler.fit_transform(X)

    param_grid = {
        "n_neighbors": [3, 5, 7, 9, 11, 15, 21],
        "metric": ["euclidean", "manhattan", "minkowski"],
        "weights": ["uniform", "distance"],
        "p": [1, 2, 3],  # Minkowski parameter
    }

    cv = StratifiedKFold(n_splits=5, shuffle=True, random_state=42)

    grid = GridSearchCV(
        KNeighborsClassifier(),
        param_grid,
        cv=cv,
        scoring="f1_weighted",
        n_jobs=-1,
        verbose=0,
    )

    grid.fit(X_scaled, y)

    best = grid.best_estimator_
    result = TrainingResult(
        best_k=best.n_neighbors,
        best_metric=best.metric,
        best_weights=best.weights,
        best_score=grid.best_score_,
        cv_scores=cross_val_score(
            best, X_scaled, y, cv=cv, scoring="f1_weighted"
        ).tolist(),
        feature_names=FEATURE_COLUMNS + BOOL_COLUMNS,
        scaler=scaler,
        model=best,
    )

    print(f"   ✅ Best K: {result.best_k}")
    print(f"   ✅ Best Metric: {result.best_metric}")
    print(f"   ✅ Best Weights: {result.best_weights}")
    print(f"   ✅ Best F1 (CV): {result.best_score:.4f}")
    print(
        f"   ✅ CV Scores: {[f'{s:.4f}' for s in result.cv_scores]} "
        f"(mean={np.mean(result.cv_scores):.4f} ± {np.std(result.cv_scores):.4f})"
    )

    return result


# ── Evaluation ────────────────────────────────────────────────────────────────


def evaluate_model(
    model: KNeighborsClassifier,
    scaler: StandardScaler,
    X: np.ndarray,
    y: np.ndarray,
) -> dict[str, Any]:
    """Full evaluation with per-class metrics."""
    X_scaled = scaler.transform(X)
    y_pred = model.predict(X_scaled)

    accuracy = accuracy_score(y, y_pred)
    precision = precision_score(y, y_pred, average="weighted", zero_division=0)
    recall = recall_score(y, y_pred, average="weighted", zero_division=0)
    f1 = f1_score(y, y_pred, average="weighted", zero_division=0)
    cm = confusion_matrix(y, y_pred)

    print("\n📊 Model Evaluation Results")
    print("=" * 50)
    print(f"   Accuracy:  {accuracy:.4f}")
    print(f"   Precision: {precision:.4f}")
    print(f"   Recall:    {recall:.4f}")
    print(f"   F1-Score:  {f1:.4f}")

    print("\n   Confusion Matrix:")
    labels = ["Low (Art)", "High (Meme)"]
    cm_table = []
    for i, row in enumerate(cm):
        cm_table.append([labels[i]] + row.tolist())
    print(tabulate(cm_table, headers=[""] + labels, tablefmt="simple_grid"))

    print("\n   Classification Report:")
    print(classification_report(y, y_pred, target_names=labels, zero_division=0))

    return {
        "accuracy": accuracy,
        "precision": precision,
        "recall": recall,
        "f1": f1,
        "confusion_matrix": cm.tolist(),
    }


def feature_importance_analysis(
    model: KNeighborsClassifier,
    scaler: StandardScaler,
    X: np.ndarray,
    y: np.ndarray,
    feature_names: list[str],
) -> None:
    """Permutation-based feature importance analysis."""
    print("\n🔬 Feature Importance Analysis (Permutation)")
    print("=" * 60)

    X_scaled = scaler.transform(X)
    baseline_score = f1_score(
        y, model.predict(X_scaled), average="weighted", zero_division=0
    )

    importances = []
    rng = np.random.RandomState(42)

    for i, name in enumerate(feature_names):
        if i >= X_scaled.shape[1]:
            continue
        scores = []
        for _ in range(10):  # 10 permutation rounds
            X_perm = X_scaled.copy()
            rng.shuffle(X_perm[:, i])
            perm_score = f1_score(
                y, model.predict(X_perm), average="weighted", zero_division=0
            )
            scores.append(baseline_score - perm_score)
        importances.append((name, np.mean(scores), np.std(scores)))

    importances.sort(key=lambda x: x[1], reverse=True)

    table = [(name, f"{imp:.4f}", f"±{std:.4f}") for name, imp, std in importances]
    print(
        tabulate(
            table, headers=["Feature", "Importance", "Std"], tablefmt="simple_grid"
        )
    )


# ── Stats Export ──────────────────────────────────────────────────────────────


def compute_and_export_stats(conn: psycopg2.extensions.connection) -> None:
    """Compute feature statistics and write to sample_metadata table."""
    print("\n📤 Computing and exporting feature statistics...")

    df = load_samples(conn)
    if df.empty:
        print("⚠️ No samples found. Cannot compute stats.")
        return

    df["total_pixels"] = df["width"].astype(float) * df["height"].astype(float)

    stat_features = {
        "pixels": df["total_pixels"],
        "duration": df["duration_secs"],
        "frame_count": df["frame_count"].astype(float),
        "density": df["fps"].fillna(0),
        "temporal_bpp": df["temporal_bpp"],
        "spatial_bpp": df["spatial_bpp"],
        "aspect": df["aspect_ratio"].fillna(1.0),
        "loop_freq": df["loop_frequency"].fillna(0.5),
        "cadence": df["cadence_score"].fillna(0.5),
        "payload_var": df["frame_payload_variation"].fillna(0.5),
        "delay_var": df["frame_delay_variation"].fillna(0.5),
        "p_depth": df["palette_depth"].fillna(0.5),
        "m_gini": df["motion_gini"].fillna(0.5),
        "b_skew": df["block_skew"].fillna(0.5),
        "t_flat": df["temporal_flatness"].fillna(0.5),
    }

    stats = {}
    for name, series in stat_features.items():
        values = series.astype(float)
        stats[name] = {
            "mean": float(values.mean()),
            "std_dev": float(values.std(ddof=0)),
        }

    stats_json = json.dumps({"stats": stats})

    cur = conn.cursor()
    cur.execute(
        "INSERT INTO sample_metadata (key, value) VALUES (%s, %s) "
        "ON CONFLICT (key) DO UPDATE SET value = %s",
        (STATS_KEY, stats_json, stats_json),
    )
    conn.commit()

    table = [
        (name, f"{s['mean']:.4f}", f"{s['std_dev']:.4f}") for name, s in stats.items()
    ]
    print(
        tabulate(table, headers=["Feature", "Mean", "Std Dev"], tablefmt="simple_grid")
    )
    print(f"\n   ✅ Exported {len(stats)} feature statistics to PostgreSQL")


# ── Report ────────────────────────────────────────────────────────────────────


def generate_report(conn: psycopg2.extensions.connection) -> None:
    """Generate a comprehensive dataset and model report."""
    print("\n📋 Dataset Report")
    print("=" * 60)

    df = load_samples(conn)
    if df.empty:
        print("⚠️ No samples found.")
        return

    # Dataset overview
    print(f"\n   Total samples: {len(df)}")
    print("   Labeled by:")
    for lb, count in df["labeled_by"].value_counts().items():
        print(f"     {lb}: {count}")

    # Class balance
    print("\n   Class distribution:")
    for tol, count in df["loss_tolerance"].value_counts().items():
        print(f"     {tol}: {count} ({100 * count / len(df):.1f}%)")

    # Feature summary
    df["total_pixels"] = df["width"].astype(float) * df["height"].astype(float)
    print("\n   Feature Ranges:")
    stats_table = []
    for col in FEATURE_COLUMNS:
        if col in df.columns:
            s = df[col].dropna()
            if len(s) > 0:
                stats_table.append(
                    [
                        col,
                        f"{s.min():.4f}",
                        f"{s.max():.4f}",
                        f"{s.mean():.4f}",
                        f"{s.std():.4f}",
                    ]
                )
    print(
        tabulate(
            stats_table,
            headers=["Feature", "Min", "Max", "Mean", "Std"],
            tablefmt="simple_grid",
        )
    )

    # Boolean feature distribution
    print("\n   Boolean Features:")
    bool_table = []
    for col in BOOL_COLUMNS:
        if col in df.columns:
            true_count = df[col].sum()
            bool_table.append([col, int(true_count), len(df) - int(true_count)])
    print(
        tabulate(
            bool_table, headers=["Feature", "True", "False"], tablefmt="simple_grid"
        )
    )


# ── CLI ───────────────────────────────────────────────────────────────────────


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Modern Format Boost — KNN Training Pipeline",
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )
    subparsers = parser.add_subparsers(dest="command", help="Available commands")

    # Train
    train_parser = subparsers.add_parser(
        "train", help="Train KNN model with grid search"
    )
    train_parser.add_argument(
        "--connstr", default=None, help="PostgreSQL connection string"
    )

    # Evaluate
    eval_parser = subparsers.add_parser("evaluate", help="Evaluate model performance")
    eval_parser.add_argument(
        "--connstr", default=None, help="PostgreSQL connection string"
    )

    # Export stats
    export_parser = subparsers.add_parser(
        "export-stats", help="Compute and export feature statistics"
    )
    export_parser.add_argument(
        "--connstr", default=None, help="PostgreSQL connection string"
    )

    # Report
    report_parser = subparsers.add_parser("report", help="Generate dataset report")
    report_parser.add_argument(
        "--connstr", default=None, help="PostgreSQL connection string"
    )

    # Ingest (placeholder — actual ingestion is done by the Rust binary)
    ingest_parser = subparsers.add_parser(
        "ingest", help="Ingest sample data (delegates to Rust binary)"
    )
    ingest_parser.add_argument("path", help="Path to dataset directory")

    args = parser.parse_args()

    if not args.command:
        parser.print_help()
        sys.exit(1)

    if args.command == "ingest":
        print(
            "ℹ️  Ingestion is handled by the Rust binary: "
            "`vid --ingest-samples /path/to/dataset`"
        )
        print(f"   Requested path: {args.path}")
        sys.exit(0)

    connstr = getattr(args, "connstr", None)
    conn = connect_pg(connstr)

    try:
        if args.command == "train":
            df = load_samples(conn)
            X, y = prepare_features(df)
            result = train_model(X, y)
            evaluate_model(result.model, result.scaler, X, y)
            feature_importance_analysis(
                result.model, result.scaler, X, y, result.feature_names
            )
            print("\n✅ Training complete.")

        elif args.command == "evaluate":
            df = load_samples(conn)
            X, y = prepare_features(df)
            result = train_model(X, y)
            evaluate_model(result.model, result.scaler, X, y)
            feature_importance_analysis(
                result.model, result.scaler, X, y, result.feature_names
            )

        elif args.command == "export-stats":
            compute_and_export_stats(conn)

        elif args.command == "report":
            generate_report(conn)

    finally:
        conn.close()


if __name__ == "__main__":
    main()
