from __future__ import annotations

import sys

import pytest
from latentdeck_rgb_ring import BINDING_ABI_VERSION, RingError, WindowsRgbRingProducer


def test_native_surface_is_abi_1_and_requires_the_open_factory() -> None:
    assert BINDING_ABI_VERSION == "1"
    with pytest.raises(TypeError):
        WindowsRgbRingProducer()


def test_open_reports_a_stable_native_error() -> None:
    with pytest.raises(RingError) as captured:
        WindowsRgbRingProducer.open(0, 0, 4096, 1, 1, 1)

    expected_code = (
        "ring_invalid_handle" if sys.platform == "win32" else "ring_unsupported_platform"
    )
    assert captured.value.code == expected_code
    assert captured.value.detail
    assert str(captured.value).startswith(f"{expected_code}: ")
