"""Shared terminal UI tokens for MFB Python scripts (U10).

Aligns with Rust ``modern_ui::brand::HEX_BLUE`` / ``colors::MFB_BLUE`` RGB (67, 160, 255).
"""

from __future__ import annotations

import os
import sys

# Matches crates/foundation/src/modern_ui.rs brand::HEX_BLUE
BRAND_BLUE = "#43a0ff"


def colors_enabled() -> bool:
    """True when decorative ANSI/Rich color is allowed."""
    if os.environ.get("NO_COLOR") is not None:
        return False
    if os.environ.get("MODERN_FORMAT_PLAIN_UI", "").lower() in (
        "1",
        "true",
        "yes",
        "on",
    ):
        return False
    return sys.stdout.isatty()


def plain_mode_enabled() -> bool:
    """True when the UI should render without emojis or advanced drawing chars."""
    if os.environ.get("MODERN_FORMAT_PLAIN_UI", "").lower() in (
        "1",
        "true",
        "yes",
        "on",
    ):
        return True
    return bool(not sys.stdout.isatty())


def pick_symbol(emoji: str, ascii_fallback: str) -> str:
    """Pick between an emoji and an ASCII fallback based on plain mode."""
    if plain_mode_enabled():
        return ascii_fallback
    return emoji
