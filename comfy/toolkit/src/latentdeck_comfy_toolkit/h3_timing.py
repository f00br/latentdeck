"""Shared MiniMax H3 visual temporal-contract helpers."""

from __future__ import annotations

H3_VISUAL_TEMPORAL_RULE = "T = 2 + 5n (n >= 0)"


def is_valid_h3_visual_temporal_slots(slots: object) -> bool:
    """Return whether ``slots`` is an exact codec-valid H3 visual length."""

    return (
        isinstance(slots, int)
        and not isinstance(slots, bool)
        and slots >= 2
        and (slots - 2) % 5 == 0
    )


__all__ = ["H3_VISUAL_TEMPORAL_RULE", "is_valid_h3_visual_temporal_slots"]
