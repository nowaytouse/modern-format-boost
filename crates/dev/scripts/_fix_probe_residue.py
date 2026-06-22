#!/usr/bin/env python3
"""One-shot codemod for contract probe-residue patterns in dev Rust sources."""

from __future__ import annotations

import re
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
DEV = ROOT / "crates" / "dev"

REPLACEMENTS = [
    (
        r'std::env::var\("([^"]+)"\)\.ok\(\)',
        r'dev::infra::hardening::optional_env("\1")',
    ),
    (
        r"\.is_ok_and\(\|([^|]+)\| ([^)]+)\)",
        r'match \1 { Ok(v) => \2, Err(err) => { eprintln!("[HARDENING] probe failed: {err}"); false } }',
    ),
]

SKIP = {"hardening.rs"}


def main() -> None:
    changed = 0
    for path in sorted(DEV.rglob("*.rs")):
        if path.name in SKIP:
            continue
        text = path.read_text()
        orig = text
        for pattern, repl in REPLACEMENTS:
            text = re.sub(pattern, repl, text)
        if text != orig:
            path.write_text(text)
            changed += 1
            print(f"updated {path.relative_to(ROOT)}")
    print(f"done: {changed} files")


if __name__ == "__main__":
    main()
