"""Protect the Rust-first operational entry-point contract."""

from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[4]


def test_readmes_use_rust_training_entry_point() -> None:
    english = (REPO_ROOT / "README.md").read_text(encoding="utf-8")
    chinese = (REPO_ROOT / "docs" / "README_ZH.md").read_text(encoding="utf-8")
    rust_command = "cargo run --locked -p dev --bin run_training --"

    assert rust_command in english
    assert rust_command in chinese
    assert "**Entry**: `python3 crates/dev/scripts/run_training.py`" not in english
    assert "**唯一推荐入口**：`python3 crates/dev/scripts/run_training.py`" not in chinese


def test_migration_contract_documents_retained_python_categories() -> None:
    migration = (
        REPO_ROOT / "docs" / "PYTHON_RUST_MIGRATION.md"
    ).read_text(encoding="utf-8")

    assert "Production and CI orchestration are Rust-first" in migration
    for category in ("ML implementation", "Tests and fixtures", "Fuzzing", "Compatibility bridges"):
        assert category in migration
    for binary in ("run_training", "check_all", "install_deps", "icloud_import"):
        assert f"`{binary}`" in migration


def test_production_hints_use_rust_entry_points() -> None:
    entry_guard = (
        REPO_ROOT / "crates" / "foundation" / "src" / "infra" / "entry_guard.rs"
    ).read_text(encoding="utf-8")
    training_guard = (
        REPO_ROOT
        / "crates"
        / "foundation"
        / "src"
        / "train"
        / "training_entry_guard.rs"
    ).read_text(encoding="utf-8")
    train_quality = (
        REPO_ROOT / "crates" / "foundation" / "src" / "bin" / "train_quality.rs"
    ).read_text(encoding="utf-8")

    production_text = "\n".join((entry_guard, training_guard, train_quality))
    assert "cargo run --locked -p dev --bin run_training -- --execute" in production_text
    assert "cargo run --locked -p dev --bin training_pipeline --" in production_text

    for legacy_hint in (
        "Use: python3 crates/dev/scripts/run_training.py",
        "Production: python3 crates/dev/scripts/run_training.py",
        "Next: run `python3 crates/dev/scripts/training_pipeline.py",
        "invoke via training_pipeline.py / run_training.py",
    ):
        assert legacy_hint not in production_text
