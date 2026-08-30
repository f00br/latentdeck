from __future__ import annotations

import pytest

from latentdeck_comfy_toolkit.h3_timing import (
    H3_VISUAL_TEMPORAL_RULE,
    is_valid_h3_visual_temporal_slots,
)


@pytest.mark.parametrize("slots", [2, 7, 32, 72])
def test_h3_visual_temporal_contract_accepts_only_two_plus_five_n(slots: int) -> None:
    assert is_valid_h3_visual_temporal_slots(slots) is True
    assert H3_VISUAL_TEMPORAL_RULE == "T = 2 + 5n (n >= 0)"


@pytest.mark.parametrize("slots", [True, -3, 0, 1, 3, 6, 8, 31])
def test_h3_visual_temporal_contract_rejects_non_contract_lengths(slots: object) -> None:
    assert is_valid_h3_visual_temporal_slots(slots) is False
