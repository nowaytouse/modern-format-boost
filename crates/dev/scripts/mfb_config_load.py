#!/usr/bin/env python3
"""Load dev JSON configs with mandatory ``_consumer`` validation (fail-closed)."""

from __future__ import annotations

import json
from collections.abc import Collection
from pathlib import Path
from typing import Final

JsonObject = dict[str, object]

CONFIG_CONSUMER_MANIFEST: Final = (
    Path(__file__).resolve().parents[3]
    / "docs"
    / "dev"
    / "config"
    / "CONFIG_CONSUMERS.md"
)


def load_consumer_json(path: Path, *, expected_consumer: str) -> JsonObject:
    """
    Load a JSON config that declares exactly one runtime owner via ``_consumer``.

    Rejects missing/wrong consumer, unknown top-level keys when ``allowed_root_keys`` given.
    """
    if not path.is_file():
        raise FileNotFoundError(f"config not found: {path}")
    with open(path, encoding="utf-8") as handle:
        root = json.load(handle)
    if not isinstance(root, dict):
        raise ValueError(f"{path.name}: root must be a JSON object")
    consumer = root.get("_consumer")
    if consumer != expected_consumer:
        raise ValueError(
            f"{path.name}: _consumer must be {expected_consumer!r}, got {consumer!r}. "
            f"See {CONFIG_CONSUMER_MANIFEST.name}"
        )
    return root


def ensure_allowed_keys(
    obj: JsonObject,
    allowed: Collection[str],
    context: str,
    *,
    optional: Collection[str] | None = None,
) -> None:
    allowed_set = set(allowed)
    optional_set = set(optional or ())
    extra = set(obj.keys()) - allowed_set - optional_set
    if extra:
        raise ValueError(f"{context}: unknown keys {sorted(extra)}")
