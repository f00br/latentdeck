from __future__ import annotations

import importlib.util

import latentdeck_operator_q4


def test_p1_stream_surface_is_not_shipped_by_the_bundled_deck() -> None:
    assert importlib.util.find_spec("latentdeck_operator_q4.stream") is None
    for name in ("process_slot", "Q4StreamEngine", "Q4Context", "builtin_registry"):
        assert not hasattr(latentdeck_operator_q4, name)
