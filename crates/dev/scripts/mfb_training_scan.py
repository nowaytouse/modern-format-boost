"""Training corpus directory scan planning (segmented vs one-shot).

Large material trees use **segmented** scans (top-level subfolders, one segment at a
time) so the UI stays responsive and progress is visible per chunk. Small trees use a
**one-shot** full-tree walk (single segment).

Env:
  ``MFB_TRAINING_SEGMENT_FILE_THRESHOLD`` — file-count cap for one-shot (default 20000)
  ``MFB_TRAINING_SEGMENT_SUBDIR_BATCH`` — subdirs per segment when segmented (default 1)
"""

from __future__ import annotations

import os
from dataclasses import dataclass
from enum import Enum
from pathlib import Path

ENV_SEGMENT_FILE_THRESHOLD = "MFB_TRAINING_SEGMENT_FILE_THRESHOLD"
ENV_SEGMENT_SUBDIR_BATCH = "MFB_TRAINING_SEGMENT_SUBDIR_BATCH"

DEFAULT_SEGMENT_FILE_THRESHOLD = 20_000
DEFAULT_SEGMENT_SUBDIR_BATCH = 1


class ScanPlanningError(RuntimeError):
    """Raised when the training source tree cannot be scanned honestly."""


class ScanMode(str, Enum):
    ONESHOT = "oneshot"
    SEGMENTED = "segmented"


@dataclass(frozen=True)
class ScanSegment:
    """One bounded scan unit under a configured local_dir root."""

    mode: ScanMode
    roots: tuple[Path, ...]
    label: str
    index: int
    total: int

    @property
    def display_root(self) -> Path:
        return self.roots[0]


def _positive_int_env(name: str, default: int) -> int:
    raw = (os.environ.get(name) or "").strip()
    if not raw:
        return default
    try:
        value = int(raw)
    except ValueError:
        return default
    return value if value >= 1 else default


def segment_file_threshold() -> int:
    return _positive_int_env(ENV_SEGMENT_FILE_THRESHOLD, DEFAULT_SEGMENT_FILE_THRESHOLD)


def segment_subdir_batch() -> int:
    return _positive_int_env(ENV_SEGMENT_SUBDIR_BATCH, DEFAULT_SEGMENT_SUBDIR_BATCH)


def estimate_media_file_count(
    root: Path,
    *,
    is_media_file: object,
    cap: int,
) -> int | None:
    """Count media files up to ``cap``; return ``None`` when count exceeds ``cap``."""
    count = 0
    stack: list[Path] = [root]
    while stack:
        current = stack.pop()
        try:
            with os.scandir(current) as entries:
                subdirs: list[Path] = []
                for entry in entries:
                    try:
                        if entry.is_dir(follow_symlinks=False):
                            subdirs.append(Path(entry.path))
                            continue
                        if not entry.is_file(follow_symlinks=False):
                            continue
                        item = Path(entry.path)
                    except OSError as exc:
                        raise ScanPlanningError(
                            f"media count entry probe failed under {current}: {exc}"
                        ) from exc
                    if is_media_file(item):
                        count += 1
                        if count > cap:
                            return None
                stack.extend(reversed(subdirs))
        except OSError as exc:
            raise ScanPlanningError(
                f"media count scan failed for {current}: {exc}"
            ) from exc
    return count


def _top_level_subdirs(root: Path) -> list[Path]:
    children: list[Path] = []
    try:
        with os.scandir(root) as entries:
            for entry in entries:
                try:
                    if entry.is_dir(follow_symlinks=False):
                        children.append(Path(entry.path))
                except OSError as exc:
                    raise ScanPlanningError(
                        f"top-level subdir entry probe failed under {root}: {exc}"
                    ) from exc
    except OSError as exc:
        raise ScanPlanningError(
            f"top-level subdir scan failed for {root}: {exc}"
        ) from exc
    return sorted(children, key=lambda p: p.name.lower())


def _segment_label_for_paths(paths: list[Path]) -> str:
    if len(paths) == 1:
        return paths[0].name
    names = "+".join(p.name for p in paths[:3])
    if len(paths) > 3:
        names += f"+{len(paths) - 3}_more"
    return names


def plan_scan_segments(root: Path, *, is_media_file: object) -> list[ScanSegment]:
    """Choose one-shot vs segmented plan for a single ``local_dirs`` root."""
    threshold = segment_file_threshold()
    estimate = estimate_media_file_count(
        root, is_media_file=is_media_file, cap=threshold
    )
    if estimate is not None:
        return [
            ScanSegment(
                mode=ScanMode.ONESHOT,
                roots=(root,),
                label="full",
                index=1,
                total=1,
            )
        ]

    subdirs = _top_level_subdirs(root)
    if not subdirs:
        return [
            ScanSegment(
                mode=ScanMode.ONESHOT,
                roots=(root,),
                label="full",
                index=1,
                total=1,
            )
        ]

    batch = segment_subdir_batch()
    raw_segments: list[tuple[str, tuple[Path, ...]]] = []
    for batch_index in range(0, len(subdirs), batch):
        chunk = subdirs[batch_index : batch_index + batch]
        raw_segments.append((_segment_label_for_paths(chunk), tuple(chunk)))

    total = len(raw_segments)
    return [
        ScanSegment(
            mode=ScanMode.SEGMENTED,
            roots=roots,
            label=label,
            index=index,
            total=total,
        )
        for index, (label, roots) in enumerate(raw_segments, start=1)
    ]


def format_scan_plan_summary(root: Path, segments: list[ScanSegment]) -> str:
    if not segments:
        return f"path={root} segments=0"
    mode = segments[0].mode.value
    if len(segments) == 1 and segments[0].mode == ScanMode.ONESHOT:
        return f"path={root} mode={mode} files≤{segment_file_threshold()}"
    return (
        f"path={root} mode={mode} segments={len(segments)} "
        f"threshold>{segment_file_threshold()} files"
    )
