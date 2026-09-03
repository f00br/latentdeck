from __future__ import annotations

import pytest

from latentdeck_codec_h3.decoder import DecodedRgbaBatch, H3Decoder

torch = pytest.importorskip("torch")


class _UnusedStream:
    def decode(self, _slot: object | None = None) -> object:
        raise AssertionError("RGBA conversion must not invoke the decoder stream")

    def reset(self) -> None:
        pass


def test_rgba_is_assembled_before_one_contiguous_host_batch_view(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    first = torch.zeros((1, 1, 3, 2, 2), dtype=torch.float32)
    first[0, 0, 0] = torch.tensor([[1.0, 0.0], [0.0, 1.0]])
    first[0, 0, 1] = torch.tensor([[0.0, 1.0], [0.0, 1.0]])
    first[0, 0, 2] = torch.tensor([[0.0, 0.0], [1.0, 1.0]])
    second = 1.0 - first
    original_empty = torch.empty
    allocations: list[tuple[tuple[int, ...], object]] = []

    def recording_empty(*shape: object, **kwargs: object) -> torch.Tensor:
        dimensions = (
            tuple(shape[0]) if len(shape) == 1 and isinstance(shape[0], tuple) else tuple(shape)
        )
        allocations.append((dimensions, kwargs.get("device")))
        return original_empty(*shape, **kwargs)

    monkeypatch.setattr(torch, "empty", recording_empty)
    decoder = H3Decoder(torch, torch.device("cpu"), object(), _UnusedStream())
    decoded = decoder._rgba8([first, second])

    assert isinstance(decoded, DecodedRgbaBatch)
    assert decoded.batch == 2
    assert decoded.pixels.readonly
    assert decoded.pixels.c_contiguous
    assert decoded.pixels.ndim == 1
    assert allocations == [((2, 2, 2, 4), first.device)]
    first_rgba = bytes(
        [
            255,
            0,
            0,
            255,
            0,
            255,
            0,
            255,
            0,
            0,
            255,
            255,
            255,
            255,
            255,
            255,
        ]
    )
    second_rgba = bytes(
        [
            0,
            255,
            255,
            255,
            255,
            0,
            255,
            255,
            255,
            255,
            0,
            255,
            0,
            0,
            0,
            255,
        ]
    )
    assert bytes(decoded.pixels) == first_rgba + second_rgba
