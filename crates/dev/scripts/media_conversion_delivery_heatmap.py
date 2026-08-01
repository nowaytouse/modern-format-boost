#!/usr/bin/env python3
"""
Delivery-layer unwrap_or / numeric-forgery heatmap (M39).

--check   Run the Rust regression test (baseline + allowlist live in dev tests).
--report  Print per-file density of suspicious numeric-default patterns in production segments.
--deep    Regenerate media_conversion_deep_audit.json (allowlist vitality, blind spots, coverage gaps).
"""

from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys
from collections import defaultdict
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[3]

PATTERNS = (
    "unwrap_or(0)",
    "unwrap_or(0.0)",
    "unwrap_or(0usize)",
    "unwrap_or(0u32)",
    "unwrap_or(0u64)",
    "unwrap_or(1)",
    "unwrap_or(1.0)",
    "map_or(0,",
    "map_or(0.0,",
    "map_or(1,",
    "map_or(1.0,",
    "unwrap_or(2)",
    "map_or(4,",
)

SCAN_ROOTS = (
    REPO_ROOT / "crates/foundation/src",
    REPO_ROOT / "crates/img/src",
    REPO_ROOT / "crates/vid/src",
)

SKIP_NAMES = frozenset({"algorithm_audit.rs", "media_conversion_gate.rs"})

NUMERIC_FORGERY_PATTERNS = (
    "unwrap_or(0)",
    "unwrap_or(0.0)",
    "unwrap_or(&0.0",
    "unwrap_or(0usize)",
    "unwrap_or(0u32)",
    "unwrap_or(0u64)",
    "unwrap_or(1)",
    "unwrap_or(1.0)",
    "unwrap_or(0.5)",
    "unwrap_or(85)",
    "unwrap_or(35)",
    "unwrap_or(0x",
    "unwrap_or(u16::MAX",
    "unwrap_or(usize::MAX",
    "map_or(0,",
    "map_or(0.0,",
    "map_or(0.0_f64,",
    "map_or(1,",
    "map_or(1.0,",
    "unwrap_or(2)",
    "map_or(4,",
)

EXTENDED_PATTERNS: tuple[tuple[str, str], ...] = (
    ("map_or(100", "map_or_high_improvement"),
    ("map_or(100.0", "map_or_high_improvement"),
    ("unwrap_or_default()", "unwrap_or_default"),
    ("unwrap_or_else(|| estimate", "unwrap_or_else_estimate"),
    ("map_or(true", "map_or_bool_true_default"),
    ("unwrap_or(true)", "unwrap_or_bool_true"),
    ("unwrap_or(false)", "unwrap_or_bool_false"),
    (".lock().unwrap_or_else(|err|", "mutex_poison_inline"),
    (".lock().ok()", "mutex_lock_ok_silent"),
    ("std::env::temp_dir()", "temp_dir_inline"),
    ('map_or_else(|| "N/A"', "ui_na_inline"),
    ('|| "N/A".to_string()', "ui_na_closure"),
    ('map_or_else(|| "---"', "ui_dash_inline"),
    ('bail!("❌', "bail_raw_error_emoji"),
    ('"❌ PATH', "path_error_raw_emoji"),
    ('"🚨 DANGEROUS OPERATION BLOCKED', "safety_raw_crit_emoji"),
    ("map_or(std::ptr::null_mut()", "ffi_null_ptr_fallback"),
    ("map_or((0, 0)", "tuple_zero_fallback"),
    ('as_deref().unwrap_or("")', "option_str_empty_fallback"),
    (".map_or(fallback, |candidate|", "jxl_fallback_size"),
    (".map_or(luma_estimate.quality, |chroma|", "jpeg_chroma_quality_map_or"),
)


def production_segment(text: str) -> str:
    return text.split("#[cfg(test)]", 1)[0]


def iter_delivery_rs() -> list[Path]:
    out: list[Path] = []
    for root in SCAN_ROOTS:
        if not root.is_dir():
            continue
        for path in sorted(root.rglob("*.rs")):
            if path.name in SKIP_NAMES:
                continue
            out.append(path)
    return out


def report() -> int:
    by_file: dict[str, int] = defaultdict(int)
    total = 0
    for path in iter_delivery_rs():
        prod = production_segment(path.read_text(encoding="utf-8", errors="replace"))
        hits = sum(1 for line in prod.splitlines() if any(p in line for p in PATTERNS))
        if hits:
            rel = path.relative_to(REPO_ROOT).as_posix()
            by_file[rel] = hits
            total += hits
    print("Media conversion delivery heatmap (suspicious numeric-default patterns)")
    print(f"  files scanned: {len(iter_delivery_rs())}")
    print(f"  pattern hits (gate excluded; not allowlist-filtered): {total}")
    print()
    for rel, count in sorted(by_file.items(), key=lambda kv: (-kv[1], kv[0])):
        print(f"  {count:4d}  {rel}")
    if total == 0:
        print(
            "  (no raw pattern hits in production segments — see Rust test for allowlist)"
        )
    return 0


def load_allowlist() -> list[tuple[str, str]]:
    test_rs = (
        REPO_ROOT / "crates/dev/tests/contract/test_real_silent_fallbacks.rs"
    ).read_text(encoding="utf-8")
    block = test_rs.split("const ALLOWLIST")[1].split("];")[0]
    return re.findall(r'\(\s*\n\s*"([^"]+)",\s*\n\s*"([^"]+)"', block)


def deep_audit(write_json: bool = True) -> int:
    entries = load_allowlist()
    stale: list[dict[str, str]] = []
    live: list[dict[str, str]] = []
    for rel, snip in entries:
        path = REPO_ROOT / rel
        if not path.is_file():
            stale.append({"file": rel, "snippet": snip, "reason": "missing_file"})
            continue
        prod = production_segment(path.read_text(encoding="utf-8", errors="replace"))
        if snip in prod:
            live.append({"file": rel, "snippet": snip})
        else:
            stale.append({"file": rel, "snippet": snip, "reason": "snippet_gone"})

    forgery_hits: list[dict[str, object]] = []
    extended: dict[str, list[dict[str, object]]] = defaultdict(list)
    for path in iter_delivery_rs():
        rel = path.relative_to(REPO_ROOT).as_posix()
        lines = production_segment(
            path.read_text(encoding="utf-8", errors="replace")
        ).splitlines()
        for i, line in enumerate(lines, 1):
            if any(pat in line for pat in NUMERIC_FORGERY_PATTERNS):
                forgery_hits.append(
                    {
                        "file": rel,
                        "line": i,
                        "text": line.strip()[:140],
                        "allowlisted": any(
                            rel == f and sn in line for f, sn in entries
                        ),
                    }
                )
            for pat, tag in EXTENDED_PATTERNS:
                if pat in line:
                    extended[tag].append(
                        {"file": rel, "line": i, "text": line.strip()[:140]}
                    )

    unallowlisted = [h for h in forgery_hits if not h["allowlisted"]]
    report = {
        "allowlist": {"total": len(entries), "live": len(live), "stale": len(stale)},
        "numeric_forgery_scan": {
            "hits_total": len(forgery_hits),
            "unallowlisted": len(unallowlisted),
            "unallowlisted_samples": unallowlisted[:30],
        },
        "extended_scan": {k: len(v) for k, v in extended.items()},
        "extended_samples": {k: v[:12] for k, v in extended.items()},
        "stale_allowlist": stale,
        "live_allowlist": live,
    }
    if write_json:
        out = REPO_ROOT / "crates/dev/src/fixtures/media_conversion_deep_audit.json"
        out.write_text(
            json.dumps(report, indent=2, ensure_ascii=False) + "\n", encoding="utf-8"
        )
        print(f"Wrote {out.relative_to(REPO_ROOT)}", file=sys.stderr)

    print("Deep audit summary")
    print(f"  allowlist: {len(live)} live / {len(stale)} stale / {len(entries)} total")
    print(
        f"  M39 numeric hits: {len(forgery_hits)} ({len(unallowlisted)} unallowlisted)"
    )
    for tag, samples in extended.items():
        print(f"  extended [{tag}]: {len(samples)}")
        for s in samples[:3]:
            print(f"    {s['file']}:{s['line']}")
    if stale:
        print("  stale allowlist entries (first 5):")
        for s in stale[:5]:
            print(f"    {s['file']}")
    return 1 if unallowlisted else 0


def check() -> int:
    cmd = [
        "cargo",
        "test",
        "-p",
        "dev",
        "--test",
        "test_real_silent_fallbacks",
        "media_conversion_delivery_heatmap_no_regressions",
        "--",
        "--test-threads=1",
    ]
    print("Running:", " ".join(cmd), file=sys.stderr)
    proc = subprocess.run(cmd, cwd=REPO_ROOT, check=False)
    return proc.returncode


def main() -> int:
    _scripts = Path(__file__).resolve().parent
    if str(_scripts) not in sys.path:
        sys.path.insert(0, str(_scripts))
    from mfb_entry_guard import guard_main

    guard_main("media_conversion_delivery_heatmap.py")
    parser = argparse.ArgumentParser(description=__doc__)
    group = parser.add_mutually_exclusive_group(required=True)
    group.add_argument(
        "--check",
        action="store_true",
        help="Run media_conversion_delivery_heatmap_no_regressions (baseline enforced in Rust)",
    )
    group.add_argument(
        "--report",
        action="store_true",
        help="Print per-file suspicious pattern counts (informational; not CI-gating alone)",
    )
    group.add_argument(
        "--deep",
        action="store_true",
        help="Regenerate deep audit JSON and print summary",
    )
    args = parser.parse_args()
    if args.check:
        return check()
    if args.deep:
        return deep_audit(write_json=True)
    return report()


if __name__ == "__main__":
    raise SystemExit(main())
