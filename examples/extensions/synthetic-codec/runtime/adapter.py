"""Data-free CPU Codec adapter used by the public authoring example."""

from __future__ import annotations

import hashlib
import os
import uuid
from dataclasses import dataclass
from pathlib import Path

from latentdeck_codec_sdk import (
    Capability,
    CapturePayload,
    CodecDescriptor,
    CodecLoadRequest,
    CodecSdkError,
    DecodedAbi,
    DecodedBatch,
    ProfileInspection,
    ProfileKey,
    ProfileReceipt,
    SignalGeometry,
    TensorAbi,
    validate_codec_v2_descriptor,
    validate_profile_receipt,
)

PACK_ID = "org.example.latentdeck.synthetic"
PACK_VERSION = "0.1.0"
ADAPTER_ID = PACK_ID
ADAPTER_VERSION = PACK_VERSION
TORCH_BUILD = "2.13.0+cu130"
PROFILE = ProfileKey("synthetic", "example_latent", "0.1.0")
CAPABILITIES = (
    Capability.PLAYER,
    Capability.REALTIME,
    Capability.RESAMPLE,
    Capability.SNAPSHOT_CAPTURE,
    Capability.LIVE_CAPTURE,
)
GEOMETRY = SignalGeometry(
    channels=4,
    latent_height=2,
    latent_width=3,
    decoded_height=4,
    decoded_width=6,
    frame_rate_numerator=24,
    frame_rate_denominator=1,
    timing_contract="synthetic_ticks",
    timing_contract_version="0.1.0",
)
_torch = None


@dataclass(slots=True)
class SyntheticSource:
    source_id: uuid.UUID
    seed: int
    slot_count: int = 7
    closed: bool = False

    def close(self) -> None:
        self.closed = True


class SyntheticCaptureWriter:
    def __init__(self, request) -> None:
        request.validate()
        self.request = request
        self.payloads: list[bytes] = []
        self.finished = False
        self.output_path = Path(request.staging_root) / f"{request.capture_id}.synthetic"
        self.owns_output = False

    def append(self, tensor, *, reset_event=None) -> None:
        del reset_event
        if self.finished:
            raise CodecSdkError("capture.finalized", "capture writer is finalized")
        if _torch is None or not _torch.is_tensor(tensor):
            raise CodecSdkError("capture.tensor", "codec must be loaded before capture")
        expected = (1, 4, 1, 2, 3)
        if tuple(tensor.shape) != expected or tensor.dtype != _torch.float32:
            raise CodecSdkError("capture.tensor", "synthetic tensor ABI does not match")
        if not tensor.is_contiguous() or not bool(_torch.isfinite(tensor).all().item()):
            raise CodecSdkError("capture.tensor", "synthetic tensor must be finite and contiguous")
        self.payloads.append(tensor.detach().cpu().numpy().tobytes())

    def finish(self) -> CapturePayload:
        if self.finished or not self.payloads:
            raise CodecSdkError("capture.not_ready", "capture has no finalizable payload")
        encoded = b"".join(self.payloads)
        if len(encoded) > self.request.maximum_visual_bytes:
            raise CodecSdkError("capture.too_large", "capture exceeds its host-owned bound")
        created = False
        try:
            with self.output_path.open("xb") as output:
                created = True
                output.write(encoded)
                output.flush()
                os.fsync(output.fileno())
            self.owns_output = True
        except FileExistsError as error:
            raise CodecSdkError(
                "capture.target_exists", "capture output already exists"
            ) from error
        except OSError:
            if created:
                self.output_path.unlink(missing_ok=True)
            raise
        self.finished = True
        payload = CapturePayload(
            capture_id=self.request.capture_id,
            payload_path=str(self.output_path),
            payload_sha256=hashlib.sha256(encoded).hexdigest(),
            payload_byte_length=len(encoded),
            latent_slots=len(self.payloads),
            decoded_frame_count=len(self.payloads),
        )
        payload.validate()
        return payload

    def abort(self) -> None:
        self.finished = True
        self.payloads.clear()
        if self.owns_output:
            self.output_path.unlink(missing_ok=True)
            self.owns_output = False


class SyntheticCodecAdapter:
    def descriptor(self) -> CodecDescriptor:
        return validate_codec_v2_descriptor(
            CodecDescriptor(
                pack_id=PACK_ID,
                pack_version=PACK_VERSION,
                adapter_id=ADAPTER_ID,
                adapter_version=ADAPTER_VERSION,
                host_api_version="2.0",
                capabilities=CAPABILITIES,
                profiles=(PROFILE,),
            )
        )

    def inspect(self, cartridge) -> ProfileInspection:
        manifest = cartridge.manifest
        inspection = ProfileInspection(
            cartridge_id=cartridge.cartridge_id,
            archive_sha256=cartridge.archive_sha256,
            payload_sha256=manifest["payload_sha256"],
            profile_key=PROFILE,
            signal_geometry=GEOMETRY,
        )
        inspection.validate()
        return inspection

    def validate_profile(self, cartridge, inspection) -> ProfileReceipt:
        if cartridge.cartridge_id != inspection.cartridge_id:
            raise CodecSdkError("profile.identity_mismatch", "inspection is for another cartridge")
        receipt = ProfileReceipt(
            receipt_id=uuid.uuid5(uuid.NAMESPACE_OID, str(cartridge.cartridge_id)),
            cartridge_id=inspection.cartridge_id,
            archive_sha256=inspection.archive_sha256,
            payload_sha256=inspection.payload_sha256,
            pack_id=PACK_ID,
            pack_version=PACK_VERSION,
            adapter_id=ADAPTER_ID,
            adapter_version=ADAPTER_VERSION,
            profile_key=PROFILE,
            signal_geometry=GEOMETRY,
            tensor_abi=TensorAbi(
                python_version="3.13",
                torch_version=TORCH_BUILD,
                dtype="float32",
                shape=(1, 4, 1, 2, 3),
                device="cpu",
            ),
            decoded_abi=DecodedAbi(maximum_batch=24),
            capabilities=CAPABILITIES,
            estimated_host_bytes=1024,
            estimated_device_bytes=0,
        )
        return validate_profile_receipt(receipt, self.descriptor())

    def load(self, request: CodecLoadRequest) -> None:
        global _torch
        request.validate()
        if request.descriptor != self.descriptor() or request.assets:
            raise CodecSdkError("codec.untrusted", "load request does not bind this adapter")
        if request.device != "cpu" or request.device_ordinal != 0:
            raise CodecSdkError("tensor.device", "synthetic example is CPU-only")
        import torch

        if torch.__version__ != TORCH_BUILD:
            raise CodecSdkError("tensor.torch_abi", "Torch build differs from the package contract")
        _torch = torch

    def open_source(self, cartridge, receipt, source_id: uuid.UUID) -> SyntheticSource:
        validate_profile_receipt(receipt, self.descriptor())
        if receipt.cartridge_id != cartridge.cartridge_id:
            raise CodecSdkError("source.identity", "receipt is for another cartridge")
        seed = int(cartridge.read_tensor_range("seed", 0, 1)[0])
        return SyntheticSource(source_id=source_id, seed=seed)

    def read_slot(self, source: SyntheticSource, slot_index: int):
        if _torch is None:
            raise CodecSdkError("codec.not_loaded", "load the adapter before reading")
        if source.closed or not 0 <= slot_index < source.slot_count:
            raise CodecSdkError("source.slot", "source slot is outside the retained source")
        return _torch.full(
            (1, 4, 1, 2, 3),
            float(source.seed + slot_index),
            dtype=_torch.float32,
            device="cpu",
        ).contiguous()

    def decode_slot(self, tensor, maximum_frames: int) -> DecodedBatch:
        if _torch is None or not 1 <= maximum_frames <= 24:
            raise CodecSdkError("decode.request", "decode request is outside the bounded ABI")
        if tuple(tensor.shape) != (1, 4, 1, 2, 3):
            raise CodecSdkError("decode.tensor", "synthetic tensor shape does not match")
        value = max(0, min(255, int(float(tensor.mean().item()))))
        result = DecodedBatch(
            pixels=memoryview(bytearray([value, value, value, 255] * 24)),
            batch=1,
            height=4,
            width=6,
        )
        result.validate()
        return result

    def reset_decoder(self, stream_generation: int) -> None:
        if isinstance(stream_generation, bool) or not isinstance(stream_generation, int):
            raise CodecSdkError("decode.generation", "generation must be a positive integer")
        if stream_generation <= 0:
            raise CodecSdkError("decode.generation", "generation must be a positive integer")

    def create_capture_writer(self, request) -> SyntheticCaptureWriter:
        return SyntheticCaptureWriter(request)


def make_adapter() -> SyntheticCodecAdapter:
    """Return a fresh adapter for one authenticated Protocol 2 worker session."""

    return SyntheticCodecAdapter()
