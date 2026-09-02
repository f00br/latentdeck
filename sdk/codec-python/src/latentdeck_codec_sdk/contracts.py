"""Codec-neutral adapter, source, decode, and capture interfaces."""

from __future__ import annotations

import math
import os
import uuid
from collections.abc import Mapping
from dataclasses import dataclass
from typing import Protocol, runtime_checkable

from .protocol import Capability

MAX_IDENTIFIER_BYTES = 128
MAX_VERSION_BYTES = 64
MAX_EXTERNAL_ASSETS = 16
MAX_DECODE_BATCH = 24
MAX_RAW_IMPORT_SOURCE_BYTES = 64 * 1024**3
MAX_RAW_IMPORT_TENSORS = 64
REQUIRED_CODEC_V2_CAPABILITIES = frozenset(
    {
        Capability.PLAYER,
        Capability.REALTIME,
        Capability.RESAMPLE,
        Capability.SNAPSHOT_CAPTURE,
        Capability.LIVE_CAPTURE,
    }
)


class CodecSdkError(ValueError):
    """A stable path-free Codec SDK contract failure."""

    def __init__(self, code: str, detail: str) -> None:
        super().__init__(f"{code}: {detail}")
        self.code = code
        self.detail = detail


def _identifier(value: object, field: str) -> str:
    if not isinstance(value, str) or not value or len(value.encode()) > MAX_IDENTIFIER_BYTES:
        raise CodecSdkError("identity.invalid", f"{field} is not a bounded identifier")
    allowed = frozenset("abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789._:-")
    if any(character not in allowed for character in value):
        raise CodecSdkError("identity.invalid", f"{field} contains an invalid character")
    return value


def _version(value: object, field: str) -> str:
    if not isinstance(value, str) or not value or len(value.encode()) > MAX_VERSION_BYTES:
        raise CodecSdkError("version.invalid", f"{field} is not a bounded version")
    allowed = frozenset("abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789.+-_")
    if any(character not in allowed for character in value):
        raise CodecSdkError("version.invalid", f"{field} contains an invalid character")
    return value


def _sha256(value: object, field: str) -> str:
    if (
        not isinstance(value, str)
        or len(value) != 64
        or any(character not in "0123456789abcdef" for character in value)
    ):
        raise CodecSdkError("hash.invalid", f"{field} must be a canonical SHA-256")
    return value


def _uuid(value: object, field: str) -> uuid.UUID:
    if not isinstance(value, uuid.UUID) or value.int == 0:
        raise CodecSdkError("identity.invalid", f"{field} must be a non-nil UUID")
    return value


def _positive(value: object, field: str, maximum: int | None = None) -> int:
    if isinstance(value, bool) or not isinstance(value, int) or value <= 0:
        raise CodecSdkError("value.invalid", f"{field} must be a positive integer")
    if maximum is not None and value > maximum:
        raise CodecSdkError("value.out_of_range", f"{field} exceeds {maximum}")
    return value


def _absolute_path(value: object, field: str) -> str:
    if (
        not isinstance(value, str)
        or not value
        or len(value.encode()) > 32_768
        or "\0" in value
        or not os.path.isabs(value)
    ):
        raise CodecSdkError(field, f"{field} must be a bounded absolute path")
    return value


def _archive_entry(value: object, field: str) -> str:
    if not isinstance(value, str) or not value or len(value.encode()) > 512:
        raise CodecSdkError(field, f"{field} must be a bounded archive entry")
    if (
        "\\" in value
        or value.startswith("/")
        or value.endswith("/")
        or any(part in {"", ".", ".."} for part in value.split("/"))
        or ":" in value
    ):
        raise CodecSdkError(field, f"{field} is not a safe relative archive entry")
    return value


@dataclass(frozen=True, slots=True)
class ProfileKey:
    codec_family: str
    profile: str
    profile_version: str

    def validate(self) -> None:
        _identifier(self.codec_family, "codec_family")
        _identifier(self.profile, "profile")
        _version(self.profile_version, "profile_version")


@dataclass(frozen=True, slots=True)
class SignalGeometry:
    channels: int
    latent_height: int
    latent_width: int
    decoded_height: int
    decoded_width: int
    frame_rate_numerator: int
    frame_rate_denominator: int
    timing_contract: str
    timing_contract_version: str

    def validate(self) -> None:
        for field in (
            "channels",
            "latent_height",
            "latent_width",
            "decoded_height",
            "decoded_width",
            "frame_rate_numerator",
            "frame_rate_denominator",
        ):
            _positive(getattr(self, field), field)
        _identifier(self.timing_contract, "timing_contract")
        _version(self.timing_contract_version, "timing_contract_version")


@dataclass(frozen=True, slots=True)
class TensorAbi:
    python_version: str
    torch_version: str
    dtype: str
    shape: tuple[int, int, int, int, int]
    device: str
    contiguous: bool = True

    def validate(self) -> None:
        if self.python_version != "3.13":
            raise CodecSdkError("tensor.python_abi", "Protocol 2 requires CPython 3.13")
        _version(self.torch_version, "torch_version")
        if self.dtype not in {"float16", "bfloat16", "float32"}:
            raise CodecSdkError("tensor.dtype", "dtype is outside the Protocol 2 tensor ABI")
        if (
            len(self.shape) != 5
            or self.shape[0] != 1
            or self.shape[2] != 1
            or any(
                isinstance(dimension, bool) or not isinstance(dimension, int) or dimension <= 0
                for dimension in self.shape
            )
        ):
            raise CodecSdkError("tensor.shape", "shape must be finite [1,C,1,H,W]")
        if self.device not in {"cpu", "cuda"}:
            raise CodecSdkError("tensor.device", "device must be cpu or cuda")
        if self.contiguous is not True:
            raise CodecSdkError("tensor.contiguous", "Protocol 2 tensors must be contiguous")


@dataclass(frozen=True, slots=True)
class DecodedAbi:
    pixel_format: str = "rgba8"
    maximum_batch: int = MAX_DECODE_BATCH

    def validate(self) -> None:
        if self.pixel_format != "rgba8":
            raise CodecSdkError("decoded.pixel_format", "decoded output must be RGBA8")
        _positive(self.maximum_batch, "maximum_batch", MAX_DECODE_BATCH)


@dataclass(frozen=True, slots=True)
class ExternalAsset:
    asset_id: str
    path: str
    sha256: str
    byte_length: int

    def validate(self) -> None:
        _identifier(self.asset_id, "asset_id")
        if not isinstance(self.path, str) or not self.path or len(self.path.encode()) > 32_768:
            raise CodecSdkError("asset.path", "asset path is outside the protocol bound")
        _sha256(self.sha256, "asset sha256")
        _positive(self.byte_length, "asset byte_length")


@dataclass(frozen=True, slots=True)
class CodecDescriptor:
    pack_id: str
    pack_version: str
    adapter_id: str
    adapter_version: str
    host_api_version: str
    capabilities: tuple[Capability, ...]
    profiles: tuple[ProfileKey, ...]

    def validate(self) -> None:
        _identifier(self.pack_id, "pack_id")
        _version(self.pack_version, "pack_version")
        _identifier(self.adapter_id, "adapter_id")
        _version(self.adapter_version, "adapter_version")
        if self.host_api_version != "2.0":
            raise CodecSdkError("codec.host_api", "codec must target host API 2.0")
        if any(not isinstance(capability, Capability) for capability in self.capabilities):
            raise CodecSdkError("codec.capability_type", "capabilities must use Capability")
        if len(set(self.capabilities)) != len(self.capabilities):
            raise CodecSdkError("codec.capability_duplicate", "capabilities must be unique")
        missing = REQUIRED_CODEC_V2_CAPABILITIES.difference(self.capabilities)
        if missing:
            names = ", ".join(sorted(capability.value for capability in missing))
            raise CodecSdkError(
                "codec.capability_missing", f"missing required capabilities: {names}"
            )
        if not self.profiles or len(self.profiles) > 64:
            raise CodecSdkError("codec.profiles", "descriptor must expose 1..64 profiles")
        identities: set[tuple[str, str, str]] = set()
        for profile in self.profiles:
            profile.validate()
            identity = (profile.codec_family, profile.profile, profile.profile_version)
            if identity in identities:
                raise CodecSdkError("codec.profile_duplicate", "profiles must be unique")
            identities.add(identity)


@dataclass(frozen=True, slots=True)
class ProfileInspection:
    cartridge_id: uuid.UUID
    archive_sha256: str
    payload_sha256: str
    profile_key: ProfileKey
    signal_geometry: SignalGeometry

    def validate(self) -> None:
        _uuid(self.cartridge_id, "cartridge_id")
        _sha256(self.archive_sha256, "archive_sha256")
        _sha256(self.payload_sha256, "payload_sha256")
        self.profile_key.validate()
        self.signal_geometry.validate()


@dataclass(frozen=True, slots=True)
class ProfileReceipt:
    receipt_id: uuid.UUID
    cartridge_id: uuid.UUID
    archive_sha256: str
    payload_sha256: str
    pack_id: str
    pack_version: str
    adapter_id: str
    adapter_version: str
    profile_key: ProfileKey
    signal_geometry: SignalGeometry
    tensor_abi: TensorAbi
    decoded_abi: DecodedAbi
    capabilities: tuple[Capability, ...]
    estimated_host_bytes: int
    estimated_device_bytes: int

    def validate(self) -> None:
        _uuid(self.receipt_id, "receipt_id")
        _uuid(self.cartridge_id, "cartridge_id")
        _sha256(self.archive_sha256, "archive_sha256")
        _sha256(self.payload_sha256, "payload_sha256")
        _identifier(self.pack_id, "pack_id")
        _version(self.pack_version, "pack_version")
        _identifier(self.adapter_id, "adapter_id")
        _version(self.adapter_version, "adapter_version")
        self.profile_key.validate()
        self.signal_geometry.validate()
        self.tensor_abi.validate()
        self.decoded_abi.validate()
        if self.tensor_abi.shape[1:] != (
            self.signal_geometry.channels,
            1,
            self.signal_geometry.latent_height,
            self.signal_geometry.latent_width,
        ):
            raise CodecSdkError(
                "profile.tensor_geometry", "tensor ABI does not match signal geometry"
            )
        if (
            not self.capabilities
            or any(not isinstance(capability, Capability) for capability in self.capabilities)
            or len(set(self.capabilities)) != len(self.capabilities)
        ):
            raise CodecSdkError("profile.capabilities", "receipt capabilities must be unique")
        for field in ("estimated_host_bytes", "estimated_device_bytes"):
            value = getattr(self, field)
            if isinstance(value, bool) or not isinstance(value, int) or value < 0:
                raise CodecSdkError("profile.memory", f"{field} must be a non-negative integer")


@dataclass(frozen=True, slots=True)
class CodecLoadRequest:
    descriptor: CodecDescriptor
    assets: tuple[ExternalAsset, ...]
    device: str
    device_ordinal: int

    def validate(self) -> None:
        self.descriptor.validate()
        if not 0 <= len(self.assets) <= MAX_EXTERNAL_ASSETS:
            raise CodecSdkError("asset.count", "codec load accepts at most 16 external assets")
        seen: set[str] = set()
        for asset in self.assets:
            asset.validate()
            if asset.asset_id in seen:
                raise CodecSdkError("asset.duplicate", "external asset IDs must be unique")
            seen.add(asset.asset_id)
        if self.device not in {"cpu", "cuda"}:
            raise CodecSdkError("tensor.device", "device must be cpu or cuda")
        if isinstance(self.device_ordinal, bool) or not 0 <= self.device_ordinal <= 255:
            raise CodecSdkError("tensor.device", "device ordinal must be in [0, 255]")


@dataclass(frozen=True, slots=True)
class CaptureRequest:
    capture_id: uuid.UUID
    mode: str
    staging_root: str
    maximum_latent_slots: int
    maximum_visual_bytes: int
    maximum_reset_events: int = 32

    def validate(self) -> None:
        _uuid(self.capture_id, "capture_id")
        if self.mode not in {"snapshot", "live_capture"}:
            raise CodecSdkError("capture.mode", "capture mode is unsupported")
        _absolute_path(self.staging_root, "capture.staging_root")
        _positive(self.maximum_latent_slots, "maximum_latent_slots", 1_048_576)
        _positive(self.maximum_visual_bytes, "maximum_visual_bytes", 15 * 1024**3)
        _positive(self.maximum_reset_events, "maximum_reset_events", 32)


@dataclass(frozen=True, slots=True)
class CapturePayload:
    capture_id: uuid.UUID
    payload_path: str
    payload_sha256: str
    payload_byte_length: int
    latent_slots: int
    decoded_frame_count: int

    def validate(self) -> None:
        _uuid(self.capture_id, "capture_id")
        _absolute_path(self.payload_path, "capture.path")
        _sha256(self.payload_sha256, "payload_sha256")
        _positive(self.payload_byte_length, "payload_byte_length")
        _positive(self.latent_slots, "latent_slots")
        _positive(self.decoded_frame_count, "decoded_frame_count")


@dataclass(frozen=True, slots=True)
class RawImportTensor:
    stream: str
    name: str
    storage_dtype: str
    runtime_dtype: str
    shape: tuple[int, ...]

    def validate(self) -> None:
        if self.stream not in {"visual", "audio"}:
            raise CodecSdkError("raw_import.tensor_stream", "raw import tensor stream is invalid")
        _identifier(self.name, "raw import tensor name")
        if self.storage_dtype not in {"F16", "F32"} or self.runtime_dtype not in {"F16", "F32"}:
            raise CodecSdkError("raw_import.tensor_dtype", "raw import tensor dtype is invalid")
        if (
            not isinstance(self.shape, tuple)
            or not 1 <= len(self.shape) <= 8
            or any(
                isinstance(dimension, bool) or not isinstance(dimension, int) or dimension <= 0
                for dimension in self.shape
            )
        ):
            raise CodecSdkError("raw_import.tensor_shape", "raw import shape has invalid axes")
        values = math.prod(self.shape)
        if values > MAX_RAW_IMPORT_SOURCE_BYTES:
            raise CodecSdkError("raw_import.tensor_shape", "raw import tensor shape is unbounded")


@dataclass(frozen=True, slots=True)
class RawImportMetadata:
    profile_key: ProfileKey
    payload_entry: str
    payload_media_type: str
    tensors: tuple[RawImportTensor, ...]
    timing_contract: str
    timing_contract_version: str
    decoded_width: int
    decoded_height: int
    decoded_frame_count: int
    frame_rate_numerator: int
    frame_rate_denominator: int
    duration_numerator: int
    duration_denominator: int
    audio_policy: str

    def validate(self) -> None:
        self.profile_key.validate()
        _archive_entry(self.payload_entry, "raw_import.payload_entry")
        if self.payload_media_type != "application/vnd.safetensors":
            raise CodecSdkError(
                "raw_import.payload_media_type", "raw imports must stage a Safetensors payload"
            )
        if not 1 <= len(self.tensors) <= MAX_RAW_IMPORT_TENSORS:
            raise CodecSdkError("raw_import.tensor_count", "raw import tensor count is invalid")
        names: set[str] = set()
        visual_count = 0
        audio_count = 0
        for tensor in self.tensors:
            if not isinstance(tensor, RawImportTensor):
                raise CodecSdkError("raw_import.tensor_type", "raw import tensor is invalid")
            tensor.validate()
            if tensor.name in names:
                raise CodecSdkError(
                    "raw_import.tensor_duplicate", "raw import tensors repeat a name"
                )
            names.add(tensor.name)
            visual_count += tensor.stream == "visual"
            audio_count += tensor.stream == "audio"
        if visual_count != 1 or audio_count > 1:
            raise CodecSdkError(
                "raw_import.tensor_streams",
                "raw import requires one visual and at most one audio tensor",
            )
        _identifier(self.timing_contract, "raw import timing contract")
        _version(self.timing_contract_version, "raw import timing contract version")
        for field in (
            "decoded_width",
            "decoded_height",
            "decoded_frame_count",
            "frame_rate_numerator",
            "frame_rate_denominator",
            "duration_numerator",
            "duration_denominator",
        ):
            _positive(getattr(self, field), f"raw import {field}")
        if self.audio_policy not in {"source_absent", "preserved_source"}:
            raise CodecSdkError("raw_import.audio_policy", "raw import audio policy is invalid")
        if (audio_count == 1) != (self.audio_policy == "preserved_source"):
            raise CodecSdkError(
                "raw_import.audio_policy", "raw import audio policy does not match tensors"
            )


@dataclass(frozen=True, slots=True)
class RawImportPreflightRequest:
    import_id: uuid.UUID
    source_path: str
    maximum_source_bytes: int

    def validate(self) -> None:
        _uuid(self.import_id, "raw import ID")
        _absolute_path(self.source_path, "raw_import.source_path")
        _positive(
            self.maximum_source_bytes,
            "raw import maximum_source_bytes",
            MAX_RAW_IMPORT_SOURCE_BYTES,
        )


@dataclass(frozen=True, slots=True)
class RawImportPreflight:
    receipt_id: uuid.UUID
    import_id: uuid.UUID
    pack_id: str
    pack_version: str
    adapter_id: str
    adapter_version: str
    source_sha256: str
    source_byte_length: int
    metadata: RawImportMetadata

    def validate(self) -> None:
        _uuid(self.receipt_id, "raw import receipt ID")
        _uuid(self.import_id, "raw import ID")
        _identifier(self.pack_id, "raw import pack ID")
        _version(self.pack_version, "raw import pack version")
        _identifier(self.adapter_id, "raw import adapter ID")
        _version(self.adapter_version, "raw import adapter version")
        _sha256(self.source_sha256, "raw import source SHA-256")
        _positive(
            self.source_byte_length,
            "raw import source byte length",
            MAX_RAW_IMPORT_SOURCE_BYTES,
        )
        self.metadata.validate()


@dataclass(frozen=True, slots=True)
class RawImportStageRequest:
    preflight: RawImportPreflight
    staging_root: str

    def validate(self) -> None:
        self.preflight.validate()
        _absolute_path(self.staging_root, "raw_import.staging_root")


@dataclass(frozen=True, slots=True)
class RawImportArtifact:
    receipt_id: uuid.UUID
    import_id: uuid.UUID
    staged_payload_path: str
    payload_sha256: str
    payload_byte_length: int

    def validate(self) -> None:
        _uuid(self.receipt_id, "raw import receipt ID")
        _uuid(self.import_id, "raw import ID")
        _absolute_path(self.staged_payload_path, "raw_import.staged_payload_path")
        _sha256(self.payload_sha256, "raw import payload SHA-256")
        _positive(
            self.payload_byte_length,
            "raw import payload byte length",
            MAX_RAW_IMPORT_SOURCE_BYTES,
        )


@dataclass(frozen=True, slots=True)
class DecodedBatch:
    pixels: memoryview
    batch: int
    height: int
    width: int

    def validate(self) -> None:
        if not isinstance(self.pixels, memoryview):
            raise CodecSdkError("decoded.type", "decoded pixels must be a memoryview")
        if self.pixels.format != "B" or not self.pixels.c_contiguous:
            raise CodecSdkError(
                "decoded.layout", "decoded pixels must be contiguous unsigned bytes"
            )
        _positive(self.batch, "batch", MAX_DECODE_BATCH)
        _positive(self.height, "height")
        _positive(self.width, "width")
        expected = self.batch * self.height * self.width * 4
        if self.pixels.nbytes != expected:
            raise CodecSdkError("decoded.byte_length", "RGBA8 byte length does not match shape")


@dataclass(frozen=True, slots=True)
class TensorAccessDescriptor:
    """Core-validated metadata for one bounded tensor byte range.

    Offsets are deliberately absent: adapters can address bytes only relative
    to the named tensor and never learn archive or payload layout.
    """

    name: str
    dtype: str
    shape: tuple[int, ...]
    byte_length: int

    def validate(self) -> None:
        _identifier(self.name, "tensor name")
        byte_width = {"F16": 2, "F32": 4}.get(self.dtype)
        if byte_width is None:
            raise CodecSdkError("tensor.storage_dtype", "storage dtype must be F16 or F32")
        if (
            not isinstance(self.shape, tuple)
            or not 1 <= len(self.shape) <= 8
            or any(
                isinstance(dimension, bool) or not isinstance(dimension, int) or dimension <= 0
                for dimension in self.shape
            )
        ):
            raise CodecSdkError("tensor.storage_shape", "storage shape must have 1..8 axes")
        expected = math.prod(self.shape) * byte_width
        if (
            isinstance(self.byte_length, bool)
            or not isinstance(self.byte_length, int)
            or self.byte_length <= 0
            or self.byte_length != expected
        ):
            raise CodecSdkError(
                "tensor.storage_length", "storage byte length does not match dtype and shape"
            )


@runtime_checkable
class CartridgeAccess(Protocol):
    """Bounded read-only access retained by Core after integrity validation."""

    @property
    def cartridge_id(self) -> uuid.UUID: ...

    @property
    def archive_sha256(self) -> str: ...

    @property
    def manifest(self) -> Mapping[str, object]: ...

    def tensor_descriptor(self, name: str) -> TensorAccessDescriptor: ...

    def read_tensor_range(self, name: str, offset: int, byte_length: int) -> memoryview: ...


@runtime_checkable
class SourceHandle(Protocol):
    @property
    def source_id(self) -> uuid.UUID: ...

    @property
    def slot_count(self) -> int: ...

    def close(self) -> None: ...


@runtime_checkable
class CaptureWriter(Protocol):
    def append(
        self, tensor: object, *, reset_event: Mapping[str, object] | None = None
    ) -> None: ...

    def finish(self) -> CapturePayload: ...

    def abort(self) -> None: ...


@runtime_checkable
class RawImportAdapter(Protocol):
    """Optional CPU-only raw-source staging surface for codecs declaring raw_import."""

    def preflight_raw_import(self, request: RawImportPreflightRequest) -> RawImportPreflight: ...

    def stage_raw_import(self, request: RawImportStageRequest) -> RawImportArtifact: ...

    def abort_raw_import(self, import_id: uuid.UUID) -> None: ...


@runtime_checkable
class CodecAdapter(Protocol):
    """Complete Codec Pack v2 adapter surface."""

    def descriptor(self) -> CodecDescriptor: ...

    def inspect(self, cartridge: CartridgeAccess) -> ProfileInspection: ...

    def validate_profile(
        self, cartridge: CartridgeAccess, inspection: ProfileInspection
    ) -> ProfileReceipt: ...

    def load(self, request: CodecLoadRequest) -> None: ...

    def open_source(
        self,
        cartridge: CartridgeAccess,
        receipt: ProfileReceipt,
        source_id: uuid.UUID,
    ) -> SourceHandle: ...

    def read_slot(self, source: SourceHandle, slot_index: int) -> object: ...

    def decode_slot(self, tensor: object, maximum_frames: int) -> DecodedBatch: ...

    def reset_decoder(self, stream_generation: int) -> None: ...

    def create_capture_writer(self, request: CaptureRequest) -> CaptureWriter: ...


def validate_codec_v2_descriptor(descriptor: CodecDescriptor) -> CodecDescriptor:
    descriptor.validate()
    return descriptor


def validate_profile_receipt(
    receipt: ProfileReceipt, descriptor: CodecDescriptor | None = None
) -> ProfileReceipt:
    receipt.validate()
    if descriptor is not None:
        descriptor.validate()
        if (
            receipt.pack_id,
            receipt.pack_version,
            receipt.adapter_id,
            receipt.adapter_version,
        ) != (
            descriptor.pack_id,
            descriptor.pack_version,
            descriptor.adapter_id,
            descriptor.adapter_version,
        ):
            raise CodecSdkError("profile.identity_mismatch", "receipt does not bind the descriptor")
        if receipt.profile_key not in descriptor.profiles:
            raise CodecSdkError("profile.unsupported", "receipt profile is not declared")
        if not set(receipt.capabilities).issubset(descriptor.capabilities):
            raise CodecSdkError("profile.capability_mismatch", "receipt exceeds codec capabilities")
    return receipt
