"""Pack-owned MiniMax H3 adapter for LatentDeck Codec SDK v2.

The adapter receives only Core-retained :class:`CartridgeAccess` objects.  It
never learns an LC path and reads only bounded ranges from the already
integrity-validated Safetensors payload.  The external decoder asset is the
only path-bearing input; Protocol 2 Core hashes and retains its exact bytes
before this authenticated worker receives the descriptor.
"""

from __future__ import annotations

import hashlib
import json
import math
import os
import sys
import uuid
from collections.abc import Callable, Mapping, Sequence
from contextlib import suppress
from dataclasses import dataclass
from pathlib import Path
from typing import Any

from latentdeck_codec_sdk import (
    Capability,
    CapturePayload,
    CaptureRequest,
    CartridgeAccess,
    CodecDescriptor,
    CodecLoadRequest,
    CodecSdkError,
    DecodedAbi,
    DecodedBatch,
    ExternalAsset,
    ProfileInspection,
    ProfileKey,
    ProfileReceipt,
    RawImportArtifact,
    RawImportMetadata,
    RawImportPreflight,
    RawImportPreflightRequest,
    RawImportStageRequest,
    RawImportTensor,
    SignalGeometry,
    TensorAbi,
    TensorAccessDescriptor,
    validate_profile_receipt,
)

from .decoder import (
    DecodedRgbaBatch,
    H3Decoder,
    configure_torch_cpu_threads,
    configure_torch_environment,
    validate_decoder_asset,
)

PACK_ID = "org.latentdeck.h3"
PACK_VERSION = "0.2.0"
ADAPTER_ID = "org.latentdeck.h3"
ADAPTER_VERSION = "0.2.0"
HOST_API_VERSION = "2.0"
TORCH_EXACT_BUILD = "2.13.0+cu130"
TAEH3_ASSET_SHA256 = "4fd022bfcab08772fe0536b17ea1a3bbb5625be11e397868d1c5d891863d4c13"
TAEH3_ASSET_BYTE_LENGTH = 22_709_752
PROFILE_KEY = ProfileKey("minimax_h3", "h3_av_latent", "0.1.0")
TIMING_CONTRACT = "minimax_h3_causal"
TIMING_VERSION = "0.1.0"
PAYLOAD_PATH = "payloads/h3.safetensors"
PAYLOAD_MEDIA_TYPE = "application/vnd.safetensors"
MAX_DECODED_AXIS = 4096
MAX_DECODED_PIXELS = 16_777_216
MAX_TEMPORAL_AXIS = 1_048_576
MAX_CAPTURE_EVENTS = 32
H3_CHANNELS = 24

CAPABILITIES = (
    Capability.PLAYER,
    Capability.REALTIME,
    Capability.RESAMPLE,
    Capability.SNAPSHOT_CAPTURE,
    Capability.LIVE_CAPTURE,
    Capability.RAW_IMPORT,
)


@dataclass(frozen=True, slots=True)
class _H3Profile:
    cartridge_id: uuid.UUID
    archive_sha256: str
    payload_sha256: str
    geometry: SignalGeometry
    video: TensorAccessDescriptor
    audio: TensorAccessDescriptor | None


@dataclass(frozen=True, slots=True)
class _ResidentVideoKey:
    archive_sha256: str
    payload_sha256: str
    storage_dtype: str
    shape: tuple[int, ...]
    byte_length: int


@dataclass(slots=True)
class _ResidentVideo:
    tensor: Any
    references: int


@dataclass(slots=True)
class _H3RawImport:
    source_path: Path
    preflight: RawImportPreflight
    staged_payload: Path | None = None
    partial_payload: Path | None = None


class H3SourceHandle:
    """One path-free view of an integrity-validated, GPU-resident H3 payload."""

    def __init__(
        self,
        source_id: uuid.UUID,
        cartridge: CartridgeAccess,
        profile: _H3Profile,
        resident_video: Any,
        release_resident_video: Callable[[], None],
    ) -> None:
        self._source_id = source_id
        self.cartridge = cartridge
        self.profile = profile
        self.resident_video: Any | None = resident_video
        self._release_resident_video: Callable[[], None] | None = release_resident_video
        self.closed = False

    @property
    def source_id(self) -> uuid.UUID:
        return self._source_id

    @property
    def slot_count(self) -> int:
        return self.profile.video.shape[2]

    def close(self) -> None:
        if self.closed:
            return
        self.closed = True
        self.resident_video = None
        release = self._release_resident_video
        self._release_resident_video = None
        if release is not None:
            release()


class _H3CaptureWriter:
    """Lazy bounded H3 spool; geometry is fixed by the first operator slot."""

    def __init__(
        self,
        request: CaptureRequest,
    ) -> None:
        request.validate()
        self._request = request
        requested_root = Path(request.staging_root)
        if requested_root.is_symlink() or not requested_root.is_dir():
            raise CodecSdkError(
                "capture.staging_root", "host capture staging root must already exist"
            )
        try:
            self._root = requested_root.resolve(strict=True)
        except OSError as error:
            raise CodecSdkError(
                "capture.staging_root", "host capture staging root cannot be resolved"
            ) from error
        self._spool: Any | None = None
        self._payload: CapturePayload | None = None
        self._aborted = False
        self._reset_events = 0

    def append(
        self,
        tensor: object,
        *,
        reset_event: Mapping[str, object] | None = None,
    ) -> None:
        """Validate an independently supplied slot, then append it once."""

        self._append(tensor, reset_event=reset_event, values_validated=False)

    def append_validated(
        self,
        tensor: object,
        *,
        reset_event: Mapping[str, object] | None = None,
    ) -> None:
        """Append a slot whose finite-value gate already passed in the host."""

        self._append(tensor, reset_event=reset_event, values_validated=True)

    def _append(
        self,
        tensor: object,
        *,
        reset_event: Mapping[str, object] | None,
        values_validated: bool,
    ) -> None:
        if self._aborted or self._payload is not None:
            raise CodecSdkError("capture.invalid_state", "capture writer is not appendable")
        torch = _torch_module()
        if (
            not torch.is_tensor(tensor)
            or tensor.ndim != 5
            or tuple(tensor.shape[:3]) != (1, H3_CHANNELS, 1)
            or tensor.dtype != torch.float16
            or not tensor.is_contiguous()
        ):
            raise CodecSdkError(
                "capture.tensor_invalid",
                "H3 capture slots must be contiguous F16 [1,24,1,H,W]",
            )
        if reset_event is not None:
            _bounded_json_object(reset_event, "reset_event")
            self._reset_events += 1
            if self._reset_events > min(
                self._request.maximum_reset_events,
                MAX_CAPTURE_EVENTS,
            ):
                raise CodecSdkError("capture.event_limit", "capture reset-event bound exceeded")
        if self._spool is None:
            from .resample_spool import H3ResampleSpool

            self._spool = H3ResampleSpool(
                self._root,
                str(self._request.capture_id),
                latent_height=int(tensor.shape[3]),
                latent_width=int(tensor.shape[4]),
                max_latent_slots=self._request.maximum_latent_slots,
                max_visual_bytes=self._request.maximum_visual_bytes,
            )
        try:
            self._spool.append_slot(tensor, values_validated=values_validated)
        except Exception as error:
            self.abort()
            raise CodecSdkError(
                "capture.write_failed", "post-operator H3 slot could not be staged"
            ) from error

    def finish(self) -> CapturePayload:
        if self._payload is not None:
            return self._payload
        if self._aborted or self._spool is None:
            raise CodecSdkError("capture.invalid_state", "capture has no staged slots")
        if not self._spool.can_finish:
            raise CodecSdkError(
                "capture.not_ready",
                "H3 capture has not reached an exact T=2+5n finish boundary",
            )
        try:
            receipt = self._spool.finish()
            payload = CapturePayload(
                capture_id=self._request.capture_id,
                payload_path=str(receipt.payload_path),
                payload_sha256=receipt.sha256,
                payload_byte_length=receipt.byte_length,
                latent_slots=receipt.shape[2],
                decoded_frame_count=receipt.decoded_frame_count,
            )
            payload.validate()
        except Exception as error:
            self.abort()
            raise CodecSdkError(
                "capture.write_failed",
                "H3 capture payload could not be finalized",
            ) from error
        self._payload = payload
        return payload

    def abort(self) -> None:
        if self._spool is not None:
            self._spool.abort()
        self._spool = None
        if self._payload is not None:
            with suppress(FileNotFoundError):
                Path(self._payload.payload_path).unlink()
        self._payload = None
        self._aborted = True


class H3CodecAdapter:
    """Codec SDK v2 implementation for MiniMax H3 Profile 0.1."""

    def __init__(
        self,
        *,
        torch_loader: Callable[[], Any] | None = None,
        decoder_factory: Callable[[ExternalAsset, int], Any] | None = None,
        tensor_transfer: Callable[[Any, Any, int], Any] | None = None,
        asset_validator: Callable[[str, str, int], Path] = validate_decoder_asset,
        torch_configurator: Callable[[Any], int] | None = None,
    ) -> None:
        self._torch_loader = torch_loader or _torch_module
        self._decoder_factory = decoder_factory or self._load_decoder
        self._tensor_transfer = tensor_transfer or self._transfer_to_cuda
        self._torch_configurator = torch_configurator or configure_torch_cpu_threads
        # Kept as a compatibility-only injection seam for pack-local tests.
        # Protocol 2 Core is the sole full-hash authority, so this callback must
        # never run in the authenticated adapter path.
        self._legacy_asset_validator = asset_validator
        self._request: CodecLoadRequest | None = None
        self._torch: Any | None = None
        self._asset: ExternalAsset | None = None
        self._decoder: Any | None = None
        self._resident_videos: dict[_ResidentVideoKey, _ResidentVideo] = {}
        self._last_reset_generation = 0
        self._raw_imports: dict[uuid.UUID, _H3RawImport] = {}

    def descriptor(self) -> CodecDescriptor:
        descriptor = CodecDescriptor(
            pack_id=PACK_ID,
            pack_version=PACK_VERSION,
            adapter_id=ADAPTER_ID,
            adapter_version=ADAPTER_VERSION,
            host_api_version=HOST_API_VERSION,
            capabilities=CAPABILITIES,
            profiles=(PROFILE_KEY,),
        )
        descriptor.validate()
        return descriptor

    def inspect(self, cartridge: CartridgeAccess) -> ProfileInspection:
        profile = _inspect_h3_cartridge(cartridge)
        inspection = ProfileInspection(
            cartridge_id=profile.cartridge_id,
            archive_sha256=profile.archive_sha256,
            payload_sha256=profile.payload_sha256,
            profile_key=PROFILE_KEY,
            signal_geometry=profile.geometry,
        )
        inspection.validate()
        return inspection

    def validate_profile(
        self,
        cartridge: CartridgeAccess,
        inspection: ProfileInspection,
    ) -> ProfileReceipt:
        profile = _inspect_h3_cartridge(cartridge)
        if (
            inspection.cartridge_id != profile.cartridge_id
            or inspection.archive_sha256 != profile.archive_sha256
            or inspection.payload_sha256 != profile.payload_sha256
            or inspection.profile_key != PROFILE_KEY
            or inspection.signal_geometry != profile.geometry
        ):
            raise CodecSdkError(
                "profile.inspection_mismatch",
                "H3 inspection no longer binds the retained cartridge",
            )
        resident_bytes = (
            H3_CHANNELS
            * profile.video.shape[2]
            * profile.geometry.latent_height
            * profile.geometry.latent_width
            * 2
        )
        receipt = ProfileReceipt(
            # The identity fields below bind the exact bytes.  The receipt ID
            # is deliberately unique so one cartridge may occupy more than
            # one physical Deck slot in the same worker session.
            receipt_id=uuid.uuid4(),
            cartridge_id=profile.cartridge_id,
            archive_sha256=profile.archive_sha256,
            payload_sha256=profile.payload_sha256,
            pack_id=PACK_ID,
            pack_version=PACK_VERSION,
            adapter_id=ADAPTER_ID,
            adapter_version=ADAPTER_VERSION,
            profile_key=PROFILE_KEY,
            signal_geometry=profile.geometry,
            tensor_abi=TensorAbi(
                python_version="3.13",
                torch_version=TORCH_EXACT_BUILD,
                dtype="float16",
                shape=(
                    1,
                    H3_CHANNELS,
                    1,
                    profile.geometry.latent_height,
                    profile.geometry.latent_width,
                ),
                device="cuda",
            ),
            decoded_abi=DecodedAbi(maximum_batch=24),
            capabilities=CAPABILITIES,
            estimated_host_bytes=profile.video.byte_length,
            estimated_device_bytes=resident_bytes + TAEH3_ASSET_BYTE_LENGTH,
        )
        return validate_profile_receipt(receipt, self.descriptor())

    def load(self, request: CodecLoadRequest) -> None:
        if self._request is not None:
            raise CodecSdkError("codec.already_loaded", "H3 adapter is already loaded")
        request.validate()
        if request.descriptor != self.descriptor():
            raise CodecSdkError("codec.identity_mismatch", "H3 descriptor identity is not exact")
        if request.device != "cuda":
            raise CodecSdkError("codec.device_unsupported", "H3 Profile 0.1 requires CUDA")
        if len(request.assets) != 1 or request.assets[0].asset_id != "taeh3":
            raise CodecSdkError(
                "codec.asset_unbound", "exactly one hash-bound taeh3 asset is required"
            )
        if sys.version_info[:2] != (3, 13):
            raise CodecSdkError("codec.python_abi", "H3 Codec Pack v2 requires CPython 3.13")
        torch = self._torch_loader()
        if str(torch.__version__) != TORCH_EXACT_BUILD:
            raise CodecSdkError(
                "codec.torch_abi",
                "installed Torch build does not match the H3 pack declaration",
            )
        asset = request.assets[0]
        if asset.sha256 != TAEH3_ASSET_SHA256 or asset.byte_length != TAEH3_ASSET_BYTE_LENGTH:
            raise CodecSdkError(
                "codec.asset_incompatible",
                "external taeh3 identity does not match the H3 pack declaration",
            )
        try:
            self._torch_configurator(torch)
        except (RuntimeError, TypeError, ValueError) as error:
            raise CodecSdkError(
                "codec.torch_threads",
                "H3 Codec Pack could not configure Torch CPU threads",
            ) from error
        # Do not construct the decoder here: source profile semantics must be
        # validated and receipted before any codec-owned GPU allocation.
        self._request = request
        self._torch = torch
        self._asset = asset

    def open_source(
        self,
        cartridge: CartridgeAccess,
        receipt: ProfileReceipt,
        source_id: uuid.UUID,
    ) -> H3SourceHandle:
        request, _torch = self._loaded_runtime()
        profile = _inspect_h3_cartridge(cartridge)
        validate_profile_receipt(receipt, self.descriptor())
        if (
            receipt.cartridge_id != profile.cartridge_id
            or receipt.archive_sha256 != profile.archive_sha256
            or receipt.payload_sha256 != profile.payload_sha256
            or receipt.signal_geometry != profile.geometry
            or receipt.tensor_abi.device != request.device
        ):
            raise CodecSdkError(
                "source.receipt_mismatch", "source does not match the exact H3 profile receipt"
            )
        if not isinstance(source_id, uuid.UUID) or source_id.int == 0:
            raise CodecSdkError("source.id_invalid", "source_id must be a non-nil UUID")
        self._ensure_decoder()
        resident_video, release_resident_video = self._acquire_resident_video(
            cartridge,
            profile,
        )
        return H3SourceHandle(
            source_id,
            cartridge,
            profile,
            resident_video,
            release_resident_video,
        )

    def read_slot(self, source: H3SourceHandle, slot_index: int) -> object:
        _request, torch = self._loaded_runtime()
        if not isinstance(source, H3SourceHandle) or source.closed:
            raise CodecSdkError("source.closed", "H3 source handle is closed")
        if (
            isinstance(slot_index, bool)
            or not isinstance(slot_index, int)
            or not 0 <= slot_index < source.slot_count
        ):
            raise CodecSdkError("source.slot_invalid", "H3 slot index is outside the source")
        video = source.profile.video
        _, _channels, _temporal, height, width = video.shape
        resident_video = source.resident_video
        if resident_video is None:
            raise CodecSdkError("source.closed", "H3 source handle is closed")
        # Operators are replaceable and may accidentally mutate their input.
        # Keep the shared resident source immutable by exposing one small
        # device-to-device slot clone instead of its backing storage view.
        tensor = (
            resident_video[:, slot_index].unsqueeze(2).clone(memory_format=torch.contiguous_format)
        )
        if (
            not torch.is_tensor(tensor)
            or tuple(tensor.shape) != (1, H3_CHANNELS, 1, height, width)
            or tensor.dtype != torch.float16
            or not tensor.is_contiguous()
        ):
            raise CodecSdkError(
                "source.tensor_invalid",
                "H3 resident slot violates shape, dtype, or contiguity bounds",
            )
        return tensor

    def decode_slot(self, tensor: object, maximum_frames: int) -> DecodedBatch:
        _request, torch = self._loaded_runtime()
        if (
            isinstance(maximum_frames, bool)
            or not isinstance(maximum_frames, int)
            or not 1 <= maximum_frames <= 24
        ):
            raise CodecSdkError("decode.batch_invalid", "maximum_frames must be in [1, 24]")
        if (
            not torch.is_tensor(tensor)
            or tensor.ndim != 5
            or tuple(tensor.shape[:3]) != (1, H3_CHANNELS, 1)
            or tensor.dtype != torch.float16
            or not tensor.is_contiguous()
        ):
            raise CodecSdkError("decode.tensor_invalid", "decoded H3 slot violates tensor ABI")
        decoder = self._ensure_decoder()
        try:
            decoded = decoder.decode_slot(tensor)
        except Exception as error:
            raise CodecSdkError("decode.failed", "TAEH3 failed to decode the H3 slot") from error
        if (
            not isinstance(decoded, DecodedRgbaBatch)
            or not isinstance(decoded.batch, int)
            or isinstance(decoded.batch, bool)
            or not 1 <= decoded.batch <= maximum_frames
        ):
            raise CodecSdkError(
                "decode.batch_exceeded", "TAEH3 frame count exceeds the negotiated batch"
            )
        height = int(tensor.shape[3]) * 16
        width = int(tensor.shape[4]) * 16
        frame_bytes = height * width * 4
        if (
            not isinstance(decoded.pixels, memoryview)
            or decoded.pixels.format != "B"
            or not decoded.pixels.readonly
            or not decoded.pixels.c_contiguous
            or decoded.pixels.nbytes != decoded.batch * frame_bytes
        ):
            raise CodecSdkError("decode.rgba_invalid", "TAEH3 returned an invalid RGBA8 frame")
        batch = DecodedBatch(
            pixels=decoded.pixels,
            batch=decoded.batch,
            height=height,
            width=width,
        )
        batch.validate()
        return batch

    def reset_decoder(self, stream_generation: int) -> None:
        if (
            isinstance(stream_generation, bool)
            or not isinstance(stream_generation, int)
            or stream_generation <= self._last_reset_generation
        ):
            raise CodecSdkError(
                "decode.generation_invalid", "decoder reset generation must increase"
            )
        if self._decoder is not None:
            self._decoder.reset()
        self._last_reset_generation = stream_generation

    def create_capture_writer(self, request: CaptureRequest) -> _H3CaptureWriter:
        self._loaded_runtime()
        return _H3CaptureWriter(request)

    def preflight_raw_import(self, request: RawImportPreflightRequest) -> RawImportPreflight:
        self._require_cpu_import_state()
        request.validate()
        if request.import_id in self._raw_imports:
            raise CodecSdkError("raw_import.duplicate", "raw import ID is already active")
        try:
            source = Path(request.source_path)
            if source.is_symlink():
                raise OSError("source is a link")
            source = source.resolve(strict=True)
            if not source.is_file():
                raise OSError("source is not a regular file")
            if not 1 <= source.stat().st_size <= request.maximum_source_bytes:
                raise OSError("source exceeds import bound")
            inspected = _inspect_raw_h3_source(source)
            metadata = _raw_h3_import_metadata(inspected)
            preflight = RawImportPreflight(
                receipt_id=uuid.uuid4(),
                import_id=request.import_id,
                pack_id=PACK_ID,
                pack_version=PACK_VERSION,
                adapter_id=ADAPTER_ID,
                adapter_version=ADAPTER_VERSION,
                source_sha256=str(inspected["sha256"]),
                source_byte_length=int(inspected["byte_length"]),
                metadata=metadata,
            )
            preflight.validate()
            if preflight.source_byte_length > request.maximum_source_bytes:
                raise OSError("measured source exceeds import bound")
        except CodecSdkError:
            raise
        except Exception as error:
            raise CodecSdkError(
                "raw_import.source_invalid", "raw H3 source failed bounded CPU preflight"
            ) from error
        self._raw_imports[request.import_id] = _H3RawImport(source, preflight)
        return preflight

    def stage_raw_import(self, request: RawImportStageRequest) -> RawImportArtifact:
        self._require_cpu_import_state()
        request.validate()
        state = self._raw_imports.get(request.preflight.import_id)
        if state is None or state.preflight != request.preflight:
            raise CodecSdkError("raw_import.receipt_invalid", "raw import receipt is not active")
        if state.staged_payload is not None:
            raise CodecSdkError("raw_import.already_staged", "raw import is already staged")
        try:
            root = Path(request.staging_root)
            if root.is_symlink():
                raise OSError("staging root is a link")
            root = root.resolve(strict=True)
            if not root.is_dir():
                raise OSError("staging root is not a directory")
        except OSError as error:
            raise CodecSdkError(
                "raw_import.staging_root_invalid", "Core staging root is invalid"
            ) from error
        destination = root / f"{state.preflight.import_id}.safetensors"
        partial = root / f".{state.preflight.import_id}.partial.safetensors"
        state.partial_payload = partial
        linked = False
        try:
            hasher = hashlib.sha256()
            copied = 0
            with state.source_path.open("rb") as source, partial.open("xb") as output:
                while chunk := source.read(64 * 1024):
                    copied += len(chunk)
                    if copied > state.preflight.source_byte_length:
                        raise CodecSdkError(
                            "raw_import.source_changed", "raw H3 source changed after preflight"
                        )
                    output.write(chunk)
                    hasher.update(chunk)
                output.flush()
                os.fsync(output.fileno())
            if (
                copied != state.preflight.source_byte_length
                or hasher.hexdigest() != state.preflight.source_sha256
            ):
                raise CodecSdkError(
                    "raw_import.source_changed", "raw H3 source changed after preflight"
                )
            staged_inspection = _inspect_raw_h3_source(partial)
            if (
                int(staged_inspection["byte_length"]) != copied
                or str(staged_inspection["sha256"]) != state.preflight.source_sha256
                or _raw_h3_import_metadata(staged_inspection) != state.preflight.metadata
            ):
                raise CodecSdkError(
                    "raw_import.source_changed", "staged H3 payload differs from preflight"
                )
            os.link(partial, destination)
            linked = True
            partial.unlink()
            state.partial_payload = None
            artifact = RawImportArtifact(
                receipt_id=state.preflight.receipt_id,
                import_id=state.preflight.import_id,
                staged_payload_path=str(destination),
                payload_sha256=state.preflight.source_sha256,
                payload_byte_length=state.preflight.source_byte_length,
            )
            artifact.validate()
            state.staged_payload = destination
            return artifact
        except CodecSdkError:
            raise
        except Exception as error:
            raise CodecSdkError(
                "raw_import.stage_failed", "raw H3 payload could not be staged safely"
            ) from error
        finally:
            with suppress(FileNotFoundError):
                partial.unlink()
            state.partial_payload = None
            if linked and state.staged_payload is None:
                with suppress(FileNotFoundError):
                    destination.unlink()

    def abort_raw_import(self, import_id: uuid.UUID) -> None:
        state = self._raw_imports.pop(import_id, None)
        if state is None:
            return
        for owned in (state.partial_payload, state.staged_payload):
            if owned is not None:
                with suppress(FileNotFoundError):
                    owned.unlink()

    def _require_cpu_import_state(self) -> None:
        if self._request is not None or self._torch is not None or self._decoder is not None:
            raise CodecSdkError(
                "raw_import.codec_loaded", "raw import requires an unloaded CPU-only adapter"
            )

    def _loaded_runtime(self) -> tuple[CodecLoadRequest, Any]:
        if self._request is None or self._torch is None or self._asset is None:
            raise CodecSdkError("codec.not_loaded", "H3 codec has not been loaded")
        return self._request, self._torch

    @staticmethod
    def _resident_video_key(profile: _H3Profile) -> _ResidentVideoKey:
        return _ResidentVideoKey(
            archive_sha256=profile.archive_sha256,
            payload_sha256=profile.payload_sha256,
            storage_dtype=profile.video.dtype,
            shape=profile.video.shape,
            byte_length=profile.video.byte_length,
        )

    def _acquire_resident_video(
        self,
        cartridge: CartridgeAccess,
        profile: _H3Profile,
    ) -> tuple[Any, Callable[[], None]]:
        key = self._resident_video_key(profile)
        cached = self._resident_videos.get(key)
        if cached is None:
            cached = _ResidentVideo(
                tensor=self._load_resident_video(cartridge, profile),
                references=0,
            )
            self._resident_videos[key] = cached
        cached.references += 1
        return cached.tensor, lambda: self._release_resident_video(key)

    def _load_resident_video(self, cartridge: CartridgeAccess, profile: _H3Profile) -> Any:
        request, torch = self._loaded_runtime()
        video = profile.video
        encoded = cartridge.read_tensor_range("video", 0, video.byte_length)
        if (
            not isinstance(encoded, memoryview)
            or encoded.nbytes != video.byte_length
            or not encoded.c_contiguous
            or not encoded.readonly
        ):
            raise CodecSdkError(
                "source.range_invalid",
                "retained H3 tensor range is incomplete",
            )
        try:
            owned_buffer = bytearray(encoded)
        finally:
            encoded.release()
        storage_dtype = torch.float16 if video.dtype == "F16" else torch.float32
        expected_shape = (1, video.shape[2], H3_CHANNELS, video.shape[3], video.shape[4])
        try:
            channel_major = torch.frombuffer(owned_buffer, dtype=storage_dtype).reshape(video.shape)
            slot_major = torch.empty(
                expected_shape,
                dtype=torch.float16,
                device="cpu",
            )
            slot_major.copy_(channel_major.permute(0, 2, 1, 3, 4))
            resident = self._tensor_transfer(slot_major, torch, request.device_ordinal)
        except Exception as error:
            raise CodecSdkError(
                "source.residency_failed",
                "validated H3 tensor could not become GPU resident",
            ) from error
        if (
            not torch.is_tensor(resident)
            or tuple(resident.shape) != expected_shape
            or resident.dtype != torch.float16
            or not resident.is_contiguous()
        ):
            raise CodecSdkError(
                "source.tensor_invalid",
                "resident H3 tensor violates slot-major runtime bounds",
            )
        return resident

    def _release_resident_video(self, key: _ResidentVideoKey) -> None:
        cached = self._resident_videos.get(key)
        if cached is None:
            return
        if cached.references <= 1:
            del self._resident_videos[key]
        else:
            cached.references -= 1

    def _ensure_decoder(self) -> Any:
        request, _torch = self._loaded_runtime()
        if self._decoder is None:
            assert self._asset is not None
            try:
                self._decoder = self._decoder_factory(self._asset, request.device_ordinal)
            except Exception as error:
                raise CodecSdkError(
                    "codec.load_failed", "TAEH3 decoder could not be created"
                ) from error
        return self._decoder

    @staticmethod
    def _load_decoder(asset: ExternalAsset, device_ordinal: int) -> H3Decoder:
        if sys.platform == "win32":
            return H3Decoder.load_host_validated(
                asset.path,
                asset.sha256,
                asset.byte_length,
                device_ordinal,
            )
        return H3Decoder.load(
            asset.path,
            asset.sha256,
            asset.byte_length,
            device_ordinal,
        )

    @staticmethod
    def _transfer_to_cuda(cpu: Any, torch: Any, device_ordinal: int) -> Any:
        return cpu.to(
            device=torch.device(f"cuda:{device_ordinal}"),
            dtype=torch.float16,
        ).contiguous()


def make_adapter() -> H3CodecAdapter:
    """Create the exact adapter referenced by ``codec-pack.json`` v2."""

    return H3CodecAdapter()


def _inspect_h3_cartridge(cartridge: CartridgeAccess) -> _H3Profile:
    if not isinstance(cartridge.cartridge_id, uuid.UUID) or cartridge.cartridge_id.int == 0:
        raise CodecSdkError("profile.cartridge_id", "retained cartridge identity is invalid")
    _canonical_sha256(cartridge.archive_sha256, "archive_sha256")
    manifest = _mapping(cartridge.manifest, "manifest")
    manifest_id = _uuid_value(manifest.get("cartridge_id"), "manifest cartridge_id")
    if manifest_id != cartridge.cartridge_id:
        raise CodecSdkError("profile.cartridge_id", "manifest and retained identity disagree")
    codec = _mapping(manifest.get("codec"), "codec")
    if (
        codec.get("family"),
        codec.get("profile"),
        codec.get("profile_version"),
    ) != (
        PROFILE_KEY.codec_family,
        PROFILE_KEY.profile,
        PROFILE_KEY.profile_version,
    ):
        raise CodecSdkError("profile.unsupported", "cartridge is not MiniMax H3 Profile 0.1")

    payloads = _sequence(manifest.get("payloads"), "payloads")
    if len(payloads) != 1:
        raise CodecSdkError("profile.payload", "H3 requires exactly one payload")
    payload = _mapping(payloads[0], "payload")
    if payload.get("path") != PAYLOAD_PATH or payload.get("media_type") != PAYLOAD_MEDIA_TYPE:
        raise CodecSdkError("profile.payload", "H3 payload identity is invalid")
    payload_sha256 = _canonical_sha256(payload.get("sha256"), "payload sha256")
    _positive_int(payload.get("byte_length"), "payload byte_length")

    tensors = _sequence(manifest.get("tensors"), "tensors")
    by_name: dict[str, Mapping[str, object]] = {}
    for raw in tensors:
        tensor = _mapping(raw, "tensor descriptor")
        name = tensor.get("name")
        if not isinstance(name, str) or name in by_name:
            raise CodecSdkError("profile.tensor", "H3 tensor names must be unique")
        by_name[name] = tensor
    if set(by_name) not in ({"video"}, {"video", "audio"}):
        raise CodecSdkError("profile.tensor", "H3 permits only video and optional audio")
    visual_manifest = by_name["video"]
    visual_shape = _shape(visual_manifest.get("shape"), 5, "video shape")
    if (
        visual_manifest.get("stream") != "visual"
        or visual_manifest.get("payload") != PAYLOAD_PATH
        or visual_manifest.get("storage_dtype") not in {"F16", "F32"}
        or visual_manifest.get("runtime_dtype") != "F16"
        or visual_shape[:2] != (1, H3_CHANNELS)
        or not 2 <= visual_shape[2] <= MAX_TEMPORAL_AXIS
        or (visual_shape[2] - 2) % 5
    ):
        raise CodecSdkError("profile.video_invalid", "H3 video descriptor is invalid")

    timing = _mapping(manifest.get("timing"), "timing")
    if (
        timing.get("contract") != TIMING_CONTRACT
        or timing.get("contract_version") != TIMING_VERSION
    ):
        raise CodecSdkError("profile.timing", "H3 causal timing identity is invalid")
    decoded = _mapping(timing.get("decoded_video"), "decoded_video")
    latent_height, latent_width = visual_shape[3], visual_shape[4]
    decoded_height = latent_height * 16
    decoded_width = latent_width * 16
    if (
        decoded_height > MAX_DECODED_AXIS
        or decoded_width > MAX_DECODED_AXIS
        or decoded_height * decoded_width > MAX_DECODED_PIXELS
        or decoded.get("height") != decoded_height
        or decoded.get("width") != decoded_width
    ):
        raise CodecSdkError("profile.geometry", "H3 decoded geometry is invalid")
    frame_count = 5 + 17 * ((visual_shape[2] - 2) // 5)
    frame_rate = _mapping(decoded.get("frame_rate"), "frame_rate")
    duration = _mapping(decoded.get("duration"), "duration")
    duration_numerator, duration_denominator = _reduced(frame_count, 24)
    if (
        decoded.get("frame_count") != frame_count
        or frame_rate.get("numerator") != 24
        or frame_rate.get("denominator") != 1
        or duration.get("numerator") != duration_numerator
        or duration.get("denominator") != duration_denominator
    ):
        raise CodecSdkError("profile.timing", "H3 frame cadence or duration is invalid")

    audio_manifest = by_name.get("audio")
    if audio_manifest is not None:
        audio_shape = _shape(audio_manifest.get("shape"), 4, "audio shape")
        expected_audio_slots = (frame_count * 5 + 1) // 3
        if (
            audio_manifest.get("stream") != "audio"
            or audio_manifest.get("payload") != PAYLOAD_PATH
            or audio_manifest.get("storage_dtype") not in {"F16", "F32"}
            or audio_manifest.get("runtime_dtype") != audio_manifest.get("storage_dtype")
            or audio_shape[:3] != (1, 32, 2)
            or audio_shape[3] != expected_audio_slots
        ):
            raise CodecSdkError("profile.audio_invalid", "H3 audio descriptor is invalid")
    _validate_audio_disposition(manifest.get("audio"), audio_manifest is not None)

    video = cartridge.tensor_descriptor("video")
    video.validate()
    if video.name != "video":
        raise CodecSdkError("profile.tensor", "validated video tensor identity disagrees")
    if video.dtype != visual_manifest.get("storage_dtype") or video.shape != visual_shape:
        raise CodecSdkError("profile.tensor", "validated video tensor descriptor disagrees")
    audio: TensorAccessDescriptor | None = None
    if audio_manifest is not None:
        audio = cartridge.tensor_descriptor("audio")
        audio.validate()
        if (
            audio.name != "audio"
            or audio.dtype != audio_manifest.get("storage_dtype")
            or audio.shape != _shape(audio_manifest.get("shape"), 4, "audio shape")
        ):
            raise CodecSdkError("profile.tensor", "validated audio tensor descriptor disagrees")
    return _H3Profile(
        cartridge_id=cartridge.cartridge_id,
        archive_sha256=cartridge.archive_sha256,
        payload_sha256=payload_sha256,
        geometry=SignalGeometry(
            channels=H3_CHANNELS,
            latent_height=latent_height,
            latent_width=latent_width,
            decoded_height=decoded_height,
            decoded_width=decoded_width,
            frame_rate_numerator=24,
            frame_rate_denominator=1,
            timing_contract=TIMING_CONTRACT,
            timing_contract_version=TIMING_VERSION,
        ),
        video=video,
        audio=audio,
    )


def _inspect_raw_h3_source(source: Path) -> Mapping[str, object]:
    try:
        from latentdeck_cartridge import inspect_raw_h3

        inspected = inspect_raw_h3(source)
    except Exception as error:
        raise CodecSdkError(
            "raw_import.source_invalid", "raw H3 source failed structural or finite validation"
        ) from error
    value = _mapping(inspected, "raw H3 inspection")
    if value.get("status") != "ok" or value.get("command") != "inspect_raw_h3":
        raise CodecSdkError(
            "raw_import.source_invalid", "raw H3 inspector returned an invalid receipt"
        )
    _positive_int(value.get("byte_length"), "raw source byte length")
    _canonical_sha256(value.get("sha256"), "raw source sha256")
    return value


def _raw_h3_import_metadata(inspected: Mapping[str, object]) -> RawImportMetadata:
    profile = _mapping(inspected.get("profile"), "raw H3 profile")
    if (
        profile.get("codec_family"),
        profile.get("profile"),
        profile.get("profile_version"),
    ) != (PROFILE_KEY.codec_family, PROFILE_KEY.profile, PROFILE_KEY.profile_version):
        raise CodecSdkError("raw_import.profile_invalid", "raw source is not H3 Profile 0.1")
    visual = _mapping(profile.get("visual"), "raw H3 visual profile")
    safetensors = _mapping(inspected.get("safetensors"), "raw H3 Safetensors receipt")
    video = _mapping(safetensors.get("video"), "raw H3 video tensor")
    video_shape = _shape(video.get("shape"), 5, "raw H3 video shape")
    video_dtype = str(video.get("dtype"))
    if video.get("name") != "video" or video_dtype not in {"F16", "F32"}:
        raise CodecSdkError("raw_import.video_invalid", "raw H3 video tensor is invalid")
    tensors = [
        RawImportTensor(
            stream="visual",
            name="video",
            storage_dtype=video_dtype,
            runtime_dtype="F16",
            shape=video_shape,
        )
    ]
    audio = safetensors.get("audio")
    audio_policy = "source_absent"
    if audio is not None:
        audio_value = _mapping(audio, "raw H3 audio tensor")
        audio_shape = _shape(audio_value.get("shape"), 4, "raw H3 audio shape")
        audio_dtype = str(audio_value.get("dtype"))
        if audio_value.get("name") != "audio" or audio_dtype not in {"F16", "F32"}:
            raise CodecSdkError("raw_import.audio_invalid", "raw H3 audio tensor is invalid")
        tensors.append(
            RawImportTensor(
                stream="audio",
                name="audio",
                storage_dtype=audio_dtype,
                runtime_dtype=audio_dtype,
                shape=audio_shape,
            )
        )
        audio_policy = "preserved_source"
    decoded_frames = _positive_int(visual.get("decoded_frames"), "decoded frames")
    duration_numerator, duration_denominator = _reduced(decoded_frames, 24)
    metadata = RawImportMetadata(
        profile_key=PROFILE_KEY,
        payload_entry=PAYLOAD_PATH,
        payload_media_type=PAYLOAD_MEDIA_TYPE,
        tensors=tuple(tensors),
        timing_contract=TIMING_CONTRACT,
        timing_contract_version=TIMING_VERSION,
        decoded_width=_positive_int(visual.get("decoded_width"), "decoded width"),
        decoded_height=_positive_int(visual.get("decoded_height"), "decoded height"),
        decoded_frame_count=decoded_frames,
        frame_rate_numerator=24,
        frame_rate_denominator=1,
        duration_numerator=duration_numerator,
        duration_denominator=duration_denominator,
        audio_policy=audio_policy,
    )
    metadata.validate()
    return metadata


def _validate_audio_disposition(value: object, audio_present: bool) -> None:
    audio = _mapping(value, "audio disposition")
    policy = audio.get("policy")
    if audio_present and policy not in {"preserved_source", "copied_from_carrier_exact"}:
        raise CodecSdkError("profile.audio_policy", "audio tensor contradicts audio policy")
    if not audio_present and policy not in {"source_absent", "omitted_timing_mismatch"}:
        raise CodecSdkError("profile.audio_policy", "missing audio contradicts audio policy")


def _bounded_json_object(value: Mapping[str, object], label: str) -> None:
    if not isinstance(value, Mapping) or any(not isinstance(key, str) for key in value):
        raise CodecSdkError("capture.event_invalid", f"{label} must be a JSON object")
    try:
        encoded = json.dumps(value, allow_nan=False, separators=(",", ":")).encode()
    except (TypeError, ValueError) as error:
        raise CodecSdkError("capture.event_invalid", f"{label} is not JSON-safe") from error
    if len(encoded) > 32_768:
        raise CodecSdkError("capture.event_invalid", f"{label} exceeds 32 KiB")


def _torch_module() -> Any:
    configure_torch_environment()
    try:
        import torch
    except ImportError as error:
        raise CodecSdkError("codec.torch_missing", "H3 Codec Pack is missing Torch") from error
    return torch


def _mapping(value: object, label: str) -> Mapping[str, object]:
    if not isinstance(value, Mapping) or any(not isinstance(key, str) for key in value):
        raise CodecSdkError("profile.schema", f"{label} must be an object")
    return value


def _sequence(value: object, label: str) -> tuple[object, ...]:
    if not isinstance(value, Sequence) or isinstance(value, (str, bytes, bytearray)):
        raise CodecSdkError("profile.schema", f"{label} must be an array")
    return tuple(value)


def _shape(value: object, rank: int, label: str) -> tuple[int, ...]:
    if (
        not isinstance(value, Sequence)
        or isinstance(value, (str, bytes, bytearray))
        or len(value) != rank
    ):
        raise CodecSdkError("profile.tensor", f"{label} rank is invalid")
    return tuple(_positive_int(axis, label) for axis in value)


def _positive_int(value: object, label: str) -> int:
    if isinstance(value, bool) or not isinstance(value, int) or value <= 0:
        raise CodecSdkError("profile.schema", f"{label} must be a positive integer")
    return value


def _uuid_value(value: object, label: str) -> uuid.UUID:
    try:
        parsed = uuid.UUID(str(value))
    except (ValueError, AttributeError) as error:
        raise CodecSdkError("profile.schema", f"{label} is not a UUID") from error
    if parsed.int == 0 or str(parsed) != value:
        raise CodecSdkError("profile.schema", f"{label} is not canonical")
    return parsed


def _canonical_sha256(value: object, label: str) -> str:
    if (
        not isinstance(value, str)
        or len(value) != 64
        or any(character not in "0123456789abcdef" for character in value)
    ):
        raise CodecSdkError("profile.hash", f"{label} is not a canonical SHA-256")
    return value


def _reduced(numerator: int, denominator: int) -> tuple[int, int]:
    divisor = math.gcd(numerator, denominator)
    return numerator // divisor, denominator // divisor


__all__ = [
    "ADAPTER_ID",
    "ADAPTER_VERSION",
    "H3CodecAdapter",
    "H3SourceHandle",
    "PACK_ID",
    "PACK_VERSION",
    "PROFILE_KEY",
    "TAEH3_ASSET_BYTE_LENGTH",
    "TAEH3_ASSET_SHA256",
    "make_adapter",
]
