"""Lazy CUDA TAEH3 runtime used only inside the isolated codec worker."""

from __future__ import annotations

import hashlib
from dataclasses import dataclass
from pathlib import Path
from typing import Any

from .cartridge import H3Cycle, H3VideoSource
from .presentation import H3PresentationCadence


class CodecRuntimeError(RuntimeError):
    """The explicitly selected H3 runtime or asset cannot be used."""


@dataclass(frozen=True)
class RuntimeDevice:
    ordinal: int
    name: str
    total_memory_bytes: int


@dataclass(frozen=True)
class RuntimeInspection:
    torch_version: str | None
    cuda_available: bool
    cuda_runtime: str | None
    devices: tuple[RuntimeDevice, ...]


@dataclass(frozen=True)
class DecodedCycle:
    timing: H3Cycle
    rgba_frames: tuple[bytes, ...]


def inspect_runtime() -> RuntimeInspection:
    """Inspect Torch/CUDA without loading the external decoder weight."""

    try:
        import torch
    except ImportError:
        return RuntimeInspection(None, False, None, ())
    cuda_available = bool(torch.cuda.is_available())
    devices: list[RuntimeDevice] = []
    if cuda_available:
        for ordinal in range(torch.cuda.device_count()):
            properties = torch.cuda.get_device_properties(ordinal)
            devices.append(
                RuntimeDevice(
                    ordinal=ordinal,
                    name=str(properties.name),
                    total_memory_bytes=int(properties.total_memory),
                )
            )
    cuda_runtime = getattr(torch.version, "cuda", None)
    return RuntimeInspection(str(torch.__version__), cuda_available, cuda_runtime, tuple(devices))


def validate_decoder_asset(
    path: str | Path,
    expected_sha256: str,
    expected_byte_length: int,
) -> Path:
    """Revalidate an explicit external asset immediately before model load."""

    asset_path = Path(path)
    if expected_byte_length <= 0:
        raise CodecRuntimeError("decoder asset byte length must be positive")
    if len(expected_sha256) != 64 or any(
        character not in "0123456789abcdef" for character in expected_sha256
    ):
        raise CodecRuntimeError("decoder asset SHA-256 is not canonical")
    if not asset_path.is_file() or asset_path.stat().st_size != expected_byte_length:
        raise CodecRuntimeError("decoder asset is missing or its byte length changed")
    digest = hashlib.sha256()
    with asset_path.open("rb") as stream:
        while chunk := stream.read(1024 * 1024):
            digest.update(chunk)
    if digest.hexdigest() != expected_sha256:
        raise CodecRuntimeError("decoder asset hash changed after selection")
    return asset_path


class H3Decoder:
    """One exact weight binding and one causal playback slot."""

    def __init__(self, torch: Any, device: Any, model: Any, stream: Any) -> None:
        self._torch = torch
        self._device = device
        self._model = model
        self._cadence = H3PresentationCadence(stream)
        self._source: H3VideoSource | None = None
        self._video: Any = None
        self._next_cycle_index = 0

    @classmethod
    def load(
        cls,
        weight_path: str | Path,
        expected_sha256: str,
        expected_byte_length: int,
        device_ordinal: int,
    ) -> H3Decoder:
        """Load the one explicit, hash-bound TAEH3 asset onto CUDA FP16."""

        asset_path = validate_decoder_asset(
            weight_path,
            expected_sha256,
            expected_byte_length,
        )
        try:
            import torch
            from safetensors.torch import load_file
        except ImportError as error:
            raise CodecRuntimeError("H3 Codec Pack is missing Torch or Safetensors") from error
        if not torch.cuda.is_available():
            raise CodecRuntimeError("CUDA is unavailable in the H3 Codec Pack")
        if not 0 <= device_ordinal < torch.cuda.device_count():
            raise CodecRuntimeError("selected CUDA device ordinal is unavailable")

        from ._vendor.taehv import TAEHV, StreamingTAEHV

        device = torch.device(f"cuda:{device_ordinal}")
        try:
            state = load_file(str(asset_path), device="cpu")
            model = TAEHV(
                checkpoint_path=None,
                patch_size=2,
                latent_channels=24,
                encoder_time_downscale=(True, True, False),
                decoder_time_upscale=(False, True, True),
            )
            model.is_h3 = True
            patched_state = model.patch_tgrow_layers(state)
            model.load_state_dict(patched_state, strict=True)
            model.eval().to(device=device, dtype=torch.float16)
            stream = StreamingTAEHV(model)
        except (KeyError, RuntimeError, ValueError) as error:
            raise CodecRuntimeError("external decoder asset is incompatible with TAEH3") from error
        return cls(torch, device, model, stream)

    def bind_source(self, source: H3VideoSource) -> None:
        """Bind one visual tensor; audio is never materialized by this runtime."""

        dtype = self._torch.float16 if source.storage_dtype == "F16" else self._torch.float32
        cpu_video = self._torch.frombuffer(bytearray(source.video_bytes), dtype=dtype).reshape(
            source.shape
        )
        self._source = source
        self._video = cpu_video
        self.reset()

    def reset(self) -> None:
        """Clear every causal queue and return to the codec-valid prime cycle."""

        self._cadence.reset()
        self._next_cycle_index = 0

    def decode_slot(self, slot: Any) -> tuple[bytes, ...]:
        """Decode one already-processed F16 H3 slot into RGBA8 frames.

        This is the D2/Q4 pre-decode boundary. The caller owns latent math;
        this method never converts RGB back into a latent or changes spatial
        geometry.
        """

        if self._model is None:
            raise CodecRuntimeError("H3 decoder is closed")
        if (
            not isinstance(slot, self._torch.Tensor)
            or slot.ndim != 5
            or tuple(slot.shape[:3]) != (1, 24, 1)
            or slot.shape[3] <= 0
            or slot.shape[4] <= 0
        ):
            raise CodecRuntimeError("processed H3 slot must be [1,24,1,H,W]")
        if slot.dtype != self._torch.float16:
            raise CodecRuntimeError("processed H3 slot runtime dtype must be F16")
        if not bool(self._torch.isfinite(slot).all().item()):
            raise CodecRuntimeError("processed H3 slot contains NaN or Inf")
        model_slot = slot.permute(0, 2, 1, 3, 4).contiguous()
        model_slot = model_slot.to(device=self._device, dtype=self._torch.float16)
        frames: list[Any] = []
        with self._torch.inference_mode():
            frames.append(self._cadence.feed_slot(model_slot))
            while (frame := self._cadence.pop_pending()) is not None:
                frames.append(frame)
            return self._rgba8(frames)

    def decode_cycle(self, cycle_index: int) -> DecodedCycle:
        """Decode exactly the next H3 cycle into interleaved RGBA8 frames."""

        if self._source is None or self._video is None:
            raise CodecRuntimeError("no H3 cartridge slot is bound")
        if cycle_index != self._next_cycle_index:
            raise CodecRuntimeError("decode cycle is out of order")
        timing = self._source.cycle(cycle_index)
        rgba_frames: list[bytes] = []
        for latent_index in range(
            timing.latent_start,
            timing.latent_start + timing.latent_count,
        ):
            slot = self._video[:, :, latent_index : latent_index + 1]
            slot = slot.to(dtype=self._torch.float16)
            rgba_frames.extend(self.decode_slot(slot))
        if len(rgba_frames) != timing.decoded_frame_count:
            raise CodecRuntimeError("TAEH3 output violated the H3 cadence contract")
        self._next_cycle_index += 1
        return DecodedCycle(timing=timing, rgba_frames=tuple(rgba_frames))

    def _rgba8(self, frames: list[Any]) -> tuple[bytes, ...]:
        rgb = self._torch.cat(frames, dim=1).squeeze(0)
        rgb = rgb.mul(255).round().to(self._torch.uint8).permute(0, 2, 3, 1)
        rgb = rgb.contiguous().cpu()
        rgba = self._torch.empty((*rgb.shape[:-1], 4), dtype=self._torch.uint8)
        rgba[..., :3].copy_(rgb)
        rgba[..., 3].fill_(255)
        return tuple(frame.numpy().tobytes(order="C") for frame in rgba)

    def close(self) -> None:
        """Release slot/model references before worker shutdown or recovery."""

        self._source = None
        self._video = None
        self._cadence.reset()
        self._model = None
        self._torch.cuda.empty_cache()


__all__ = [
    "CodecRuntimeError",
    "DecodedCycle",
    "H3Decoder",
    "RuntimeDevice",
    "RuntimeInspection",
    "inspect_runtime",
    "validate_decoder_asset",
]
