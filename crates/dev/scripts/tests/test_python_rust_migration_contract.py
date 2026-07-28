"""Protect the Rust-first operational entry-point contract."""

import re
import subprocess
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[4]


def test_readmes_use_rust_training_entry_point() -> None:
    english = (REPO_ROOT / "README.md").read_text(encoding="utf-8")
    chinese = (REPO_ROOT / "docs" / "README_ZH.md").read_text(encoding="utf-8")
    rust_command = "cargo run --locked -p dev --bin run_training --"

    assert rust_command in english
    assert rust_command in chinese
    assert "**Entry**: `python3 crates/dev/scripts/run_training.py`" not in english
    assert (
        "**唯一推荐入口**：`python3 crates/dev/scripts/run_training.py`" not in chinese
    )


def test_migration_contract_documents_retained_python_categories() -> None:
    migration = (REPO_ROOT / "docs" / "PYTHON_RUST_MIGRATION.md").read_text(
        encoding="utf-8"
    )

    assert "Production and CI orchestration are Rust-first" in migration
    assert "Operational migration is complete for production and CI" in migration
    for category in (
        "ML implementation",
        "Tests and fixtures",
        "Fuzzing",
        "Compatibility bridges",
    ):
        assert category in migration
    for binary in ("run_training", "check_all", "install_deps", "icloud_import"):
        assert f"`{binary}`" in migration
    for boundary in (
        "CI media dependency bootstrap is a standalone Rust binary",
        "`kondo` cache cleanup belongs to",
    ):
        assert boundary in migration


def test_github_workflows_do_not_invoke_script_files() -> None:
    script_file = re.compile(r"\.(?:py|sh|bash)\b")
    violations = []
    for workflow in sorted((REPO_ROOT / ".github" / "workflows").glob("*.y*ml")):
        for line_number, line in enumerate(
            workflow.read_text(encoding="utf-8").splitlines(), start=1
        ):
            if not line.lstrip().startswith("#") and script_file.search(line):
                violations.append(f"{workflow.name}:{line_number}: {line.strip()}")

    assert violations == []


def test_media_dependency_installer_compiles_without_cargo(tmp_path: Path) -> None:
    source = (
        REPO_ROOT / "crates" / "dev" / "src" / "bin" / "install_media_dependencies.rs"
    )
    subprocess.run(
        ["rustc", "--edition", "2024", str(source), "-o", str(tmp_path / "installer")],
        cwd=REPO_ROOT,
        check=True,
        capture_output=True,
        text=True,
    )


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
    orchestrate = (
        REPO_ROOT / "crates" / "dev" / "src" / "training_pipeline" / "orchestrate.rs"
    ).read_text(encoding="utf-8")
    database = (
        REPO_ROOT / "crates" / "foundation" / "src" / "db" / "database.rs"
    ).read_text(encoding="utf-8")
    training_rules = (
        REPO_ROOT / "crates" / "dev" / "src" / "config" / "training_rules.json"
    ).read_text(encoding="utf-8")

    production_text = (
        f"{entry_guard}\n{training_guard}\n{train_quality}\n{orchestrate}\n{database}"
    )
    assert (
        "cargo run --locked -p dev --bin run_training -- --execute" in production_text
    )
    assert "cargo run --locked -p dev --bin training_pipeline --" in production_text
    assert (
        "cargo run --locked -p dev --bin training_pipeline -- finalize-loop-intent"
        in production_text
    )

    assert (
        "Runtime JSON loader: run_training.py load_rules() ONLY" not in training_rules
    )
    assert "canonical Rust run_training binary" in training_rules
    for legacy_hint in (
        "Use: python3 crates/dev/scripts/run_training.py",
        "Production: python3 crates/dev/scripts/run_training.py",
        "python3 crates/dev/scripts/run_training.py --execute --use-api",
        "Next: run `python3 crates/dev/scripts/training_pipeline.py",
        "invoke via training_pipeline.py / run_training.py",
        "Run loop_intent_clustering.py after stats refresh",
    ):
        assert legacy_hint not in production_text
