"""Performance governor for long training scans (mirrors Rust ``performance_schedule``).

Wider progress intervals when RAM is plentiful; faster tightening under pressure,
``MFB_LOW_MEMORY``, ``MFB_MULTI_INSTANCE``, or concurrent ``run_training.py`` jobs.
"""

from __future__ import annotations

import os
import re
import subprocess
import sys
import time
from dataclasses import dataclass
from enum import Enum

ENV_MFB_PERF_TIER = "MFB_PERF_TIER"
ENV_MFB_PERF_REPROBE_SECS = "MFB_PERF_REPROBE_SECS"
ENV_MFB_LOW_MEMORY = "MFB_LOW_MEMORY"
ENV_MFB_MULTI_INSTANCE = "MFB_MULTI_INSTANCE"
ENV_MFB_STATIC_TIER_SCAN_INTERVAL = "MFB_STATIC_TIER_SCAN_INTERVAL"
ENV_MFB_TRAINING_SCAN_HEARTBEAT_SECS = "MFB_TRAINING_SCAN_HEARTBEAT_SECS"

DEFAULT_REPROBE_SECS = 6.0
DEFAULT_SCAN_INTERVAL = 200
DEFAULT_HEARTBEAT_SECS = 15.0

# Align with foundation ``MEMORY_PRESSURE_*`` + preemptive tight band.
_MEMORY_LOW_RATIO = 0.24
_MEMORY_LOW_MIN_MB = 2560
_MEMORY_NORMAL_RATIO = 0.26
_MEMORY_NORMAL_MIN_MB = 2560
_PREEMPTIVE_TIGHT_RATIO = 0.24
_PREEMPTIVE_TIGHT_MIN_MB = 2304
_MIN_TOTAL_RAM_MB_FOR_RELAXED = 12_288


class PerfTier(str, Enum):
    RELAXED = "relaxed"
    BALANCED = "balanced"
    TIGHT = "tight"


def _truthy_env(name: str) -> bool:
    raw = (os.environ.get(name) or "").strip().lower()
    return raw in {"1", "true", "yes", "on"}


def _parse_tier_env() -> PerfTier | None:
    raw = (os.environ.get(ENV_MFB_PERF_TIER) or "").strip().lower()
    if not raw:
        return None
    aliases = {
        "relaxed": PerfTier.RELAXED,
        "wide": PerfTier.RELAXED,
        "balanced": PerfTier.BALANCED,
        "normal": PerfTier.BALANCED,
        "default": PerfTier.BALANCED,
        "tight": PerfTier.TIGHT,
        "strict": PerfTier.TIGHT,
        "conservative": PerfTier.TIGHT,
    }
    return aliases.get(raw)


def _parse_vm_stat_value(line: str) -> int | None:
    part = line.split(":", 1)[-1].strip().replace(".", "")
    try:
        return int(part)
    except ValueError:
        return None


def _memory_mb() -> tuple[int | None, int | None]:
    if sys.platform == "darwin":
        return _memory_mb_macos()
    if sys.platform.startswith("linux"):
        return _memory_mb_linux()
    return None, None


def _memory_mb_macos() -> tuple[int | None, int | None]:
    """Match ``system_memory.rs``: Pages available, else free+inactive, else free."""
    total_mb: int | None = None
    try:
        out = subprocess.run(
            ["/usr/sbin/sysctl", "-n", "hw.memsize"],
            capture_output=True,
            text=True,
            check=False,
        )
        if out.returncode == 0:
            total_mb = int(out.stdout.strip()) // (1024 * 1024)
    except (OSError, ValueError):
        pass

    available_mb: int | None = None
    try:
        out = subprocess.run(
            ["/usr/bin/vm_stat"],
            capture_output=True,
            text=True,
            check=False,
        )
        if out.returncode != 0:
            return available_mb, total_mb
        page_size = 4096
        pages_available: int | None = None
        pages_free: int | None = None
        pages_inactive: int | None = None
        for line in out.stdout.splitlines():
            line = line.strip()
            if line.startswith("page size of "):
                m = re.search(r"page size of (\d+)", line)
                if m:
                    page_size = int(m.group(1))
            elif line.startswith("Pages available:"):
                pages_available = _parse_vm_stat_value(line)
            elif line.startswith("Pages free:"):
                pages_free = _parse_vm_stat_value(line)
            elif line.startswith("Pages inactive:"):
                pages_inactive = _parse_vm_stat_value(line)
        pages = pages_available
        if pages is None and pages_free is not None:
            if pages_inactive is not None:
                pages = pages_free + pages_inactive
            else:
                pages = pages_free
        if pages is not None:
            available_mb = (pages * page_size) // (1024 * 1024)
    except (OSError, ValueError, AttributeError):
        pass
    return available_mb, total_mb


def _memory_mb_linux() -> tuple[int | None, int | None]:
    available_mb: int | None = None
    total_mb: int | None = None
    try:
        with open("/proc/meminfo", encoding="utf-8") as fh:
            for line in fh:
                if line.startswith("MemAvailable:"):
                    available_mb = int(line.split()[1]) // 1024
                elif line.startswith("MemTotal:"):
                    total_mb = int(line.split()[1]) // 1024
    except OSError:
        return None, None
    return available_mb, total_mb


def _memory_pressure_level() -> str | None:
    available_mb, total_mb = _memory_mb()
    if available_mb is None or total_mb is None or total_mb <= 0:
        return None
    ratio = available_mb / total_mb
    if ratio >= _MEMORY_LOW_RATIO and available_mb >= _MEMORY_LOW_MIN_MB:
        return "low"
    if ratio >= _MEMORY_NORMAL_RATIO or available_mb >= _MEMORY_NORMAL_MIN_MB:
        return "normal"
    return "high"


def _preemptive_tight(available_mb: int, total_mb: int) -> bool:
    if total_mb <= 0:
        return False
    ratio = available_mb / total_mb
    return ratio < _PREEMPTIVE_TIGHT_RATIO or available_mb < _PREEMPTIVE_TIGHT_MIN_MB


def _clamp_tier_for_stability(tier: PerfTier, *, env_override: bool) -> PerfTier:
    if tier != PerfTier.RELAXED:
        return tier
    pressure = _memory_pressure_level()
    available_mb, total_mb = _memory_mb()
    if pressure == "high":
        return PerfTier.TIGHT
    if env_override:
        return PerfTier.RELAXED
    if _truthy_env(ENV_MFB_MULTI_INSTANCE) or (
        available_mb is not None
        and total_mb is not None
        and _preemptive_tight(available_mb, total_mb)
    ):
        return PerfTier.BALANCED
    if total_mb is not None and total_mb < _MIN_TOTAL_RAM_MB_FOR_RELAXED:
        return PerfTier.BALANCED
    return PerfTier.RELAXED


def detect_perf_tier(*, sister_training_load: bool = False) -> PerfTier:
    override = _parse_tier_env()
    if override is not None:
        return _clamp_tier_for_stability(override, env_override=True)
    if _truthy_env(ENV_MFB_LOW_MEMORY):
        return PerfTier.TIGHT
    if sister_training_load:
        return PerfTier.TIGHT
    multi = _truthy_env(ENV_MFB_MULTI_INSTANCE)
    available_mb, total_mb = _memory_mb()
    pressure = _memory_pressure_level()
    if pressure == "low" and not multi:
        return _clamp_tier_for_stability(PerfTier.RELAXED, env_override=False)
    if pressure == "high":
        return PerfTier.TIGHT
    if multi:
        return PerfTier.TIGHT
    if (
        pressure == "normal"
        and available_mb is not None
        and total_mb is not None
        and _preemptive_tight(available_mb, total_mb)
    ):
        return PerfTier.TIGHT
    if pressure == "low":
        return PerfTier.BALANCED
    return PerfTier.BALANCED


def _positive_int_env(name: str, default: int) -> int:
    raw = (os.environ.get(name) or "").strip()
    if not raw:
        return default
    try:
        value = int(raw)
    except ValueError:
        return default
    return value if value >= 1 else default


def _positive_float_env(name: str, default: float) -> float:
    raw = (os.environ.get(name) or "").strip()
    if not raw:
        return default
    try:
        value = float(raw)
    except ValueError:
        return default
    return value if value > 0 else default


@dataclass
class ScanGovernor:
    """Adaptive scan progress + cooperative yield during directory walks."""

    sister_training_load: bool = False
    _tier: PerfTier = PerfTier.BALANCED
    _last_reprobe_at: float = 0.0

    def __post_init__(self) -> None:
        self._tier = detect_perf_tier(sister_training_load=self.sister_training_load)
        self._last_reprobe_at = time.monotonic()

    def maybe_reprobe(self, *, force: bool = False) -> PerfTier:
        reprobe_secs = _positive_float_env(
            ENV_MFB_PERF_REPROBE_SECS, DEFAULT_REPROBE_SECS
        )
        if self.sister_training_load:
            reprobe_secs = min(reprobe_secs, 4.0)
        now = time.monotonic()
        if not force and now - self._last_reprobe_at < reprobe_secs:
            return self._tier
        self._tier = detect_perf_tier(sister_training_load=self.sister_training_load)
        self._last_reprobe_at = now
        return self._tier

    @property
    def tier(self) -> PerfTier:
        return self._tier

    def scan_interval(self) -> int:
        env_override = _positive_int_env(ENV_MFB_STATIC_TIER_SCAN_INTERVAL, 0)
        if env_override > 0:
            return env_override
        if self.sister_training_load:
            return 96
        if self._tier == PerfTier.RELAXED:
            return 512
        if self._tier == PerfTier.TIGHT:
            return 64
        return DEFAULT_SCAN_INTERVAL

    def heartbeat_secs(self) -> float:
        env_override = _positive_float_env(ENV_MFB_TRAINING_SCAN_HEARTBEAT_SECS, 0.0)
        if env_override > 0:
            return env_override
        if self.sister_training_load:
            return 6.0
        if self._tier == PerfTier.RELAXED:
            return 32.0
        if self._tier == PerfTier.TIGHT:
            return 6.0
        return DEFAULT_HEARTBEAT_SECS

    def yield_scan_slot(self, scanned: int) -> None:
        if scanned <= 0:
            return
        if self._tier == PerfTier.TIGHT or self.sister_training_load:
            if scanned % 16 != 0:
                return
            time.sleep(0.012 if self.sister_training_load else 0.006)
            return
        if self._tier == PerfTier.BALANCED and scanned % 48 == 0:
            time.sleep(0.002)

    def schedule_hint(self) -> str:
        sister = " sister_load=1" if self.sister_training_load else ""
        return (
            f"perf_tier={self._tier.value}{sister} "
            f"heartbeat_every={self.scan_interval()}files "
            f"heartbeat_max_silence={self.heartbeat_secs():.1f}s "
            f"(override {ENV_MFB_STATIC_TIER_SCAN_INTERVAL}, "
            f"{ENV_MFB_TRAINING_SCAN_HEARTBEAT_SECS}, "
            f"{ENV_MFB_PERF_TIER}, {ENV_MFB_PERF_REPROBE_SECS})"
        )
