from __future__ import annotations

import importlib.util

import latentdeck_operator_d2


def test_p1_stream_surface_is_not_shipped_by_the_bundled_deck() -> None:
    assert importlib.util.find_spec("latentdeck_operator_d2.stream") is None
    for name in ("process_slot", "D2StreamEngine", "D2Context", "builtin_registry"):
        assert not hasattr(latentdeck_operator_d2, name)
