"""Concrete Core-retained cartridge access for Protocol 2 workers."""

from __future__ import annotations

import json
import math
import uuid
from collections.abc import Callable, Mapping
from types import MappingProxyType
from typing import Protocol

from latentdeck_codec_sdk import CartridgeAccess, CodecSdkError, TensorAccessDescriptor

MAX_NATIVE_METADATA_BYTES = 1024 * 1024
MAX_TENSOR_DESCRIPTORS = 256


class _NativeValidatedHandle(Protocol):
    def manifest_json(self) -> str: ...

    def validation_json(self) -> str: ...

    def tensor_descriptors_json(self) -> str: ...

    def read_tensor_range(
        self,
        name: str,
        offset: int,
        byte_length: int,
        max_read_bytes: int | None = None,
    ) -> bytes: ...


type NativeHandleOpener = Callable[[int, str], _NativeValidatedHandle]


class NativeCartridgeAccess:
    """Immutable adapter view over one target-owned native LC handle."""

    def __init__(
        self,
        handle: _NativeValidatedHandle,
        *,
        archive_bytes: int,
        cartridge_id: uuid.UUID,
        archive_sha256: str,
    ) -> None:
        manifest = _json_object(handle.manifest_json(), "native manifest")
        validation = _json_object(handle.validation_json(), "native validation receipt")
        raw_descriptors = _json_object(
            handle.tensor_descriptors_json(), "native tensor descriptors"
        )
        _crosscheck_identity(
            manifest,
            validation,
            archive_bytes=archive_bytes,
            cartridge_id=cartridge_id,
            archive_sha256=archive_sha256,
        )
        descriptors = _tensor_descriptors(raw_descriptors)
        _crosscheck_manifest_tensors(manifest, validation, descriptors)

        self._handle: _NativeValidatedHandle | None = handle
        self._cartridge_id = cartridge_id
        self._archive_sha256 = archive_sha256
        self._manifest = _freeze_mapping(manifest)
        self._descriptors = MappingProxyType(descriptors)

    @property
    def cartridge_id(self) -> uuid.UUID:
        return self._cartridge_id

    @property
    def archive_sha256(self) -> str:
        return self._archive_sha256

    @property
    def manifest(self) -> Mapping[str, object]:
        return self._manifest

    def tensor_descriptor(self, name: str) -> TensorAccessDescriptor:
        self._require_open()
        if not isinstance(name, str):
            raise TypeError("tensor name must be text")
        try:
            return self._descriptors[name]
        except KeyError as error:
            raise CodecSdkError(
                "tensor.not_found", "named tensor is absent from validated access"
            ) from error

    def read_tensor_range(self, name: str, offset: int, byte_length: int) -> memoryview:
        handle = self._require_open()
        descriptor = self.tensor_descriptor(name)
        if (
            isinstance(offset, bool)
            or not isinstance(offset, int)
            or isinstance(byte_length, bool)
            or not isinstance(byte_length, int)
            or offset < 0
            or byte_length <= 0
            or offset + byte_length > descriptor.byte_length
        ):
            raise CodecSdkError(
                "tensor.range_invalid", "tensor-relative read is outside validated bounds"
            )
        try:
            encoded = handle.read_tensor_range(
                name,
                offset,
                byte_length,
                max_read_bytes=byte_length,
            )
        except Exception as error:
            raise CodecSdkError(
                "tensor.read_failed", "native validated tensor range could not be read"
            ) from error
        if not isinstance(encoded, bytes) or len(encoded) != byte_length:
            raise CodecSdkError(
                "tensor.read_invalid", "native tensor range returned incomplete bytes"
            )
        return memoryview(encoded)

    def close(self) -> None:
        handle = self._handle
        self._handle = None
        if handle is not None:
            _drop_native_handle(handle)

    def _require_open(self) -> _NativeValidatedHandle:
        if self._handle is None:
            raise CodecSdkError("source.closed", "retained cartridge access is closed")
        return self._handle


class NativeCartridgeAccessFactory:
    """Consume Core-duplicated handles into codec-neutral Python access."""

    def __init__(self, opener: NativeHandleOpener | None = None) -> None:
        self._opener = opener or _open_native_handle

    def open(
        self,
        *,
        retained_native_handle: int,
        archive_bytes: int,
        cartridge_id: uuid.UUID,
        archive_sha256: str,
        integrity_access_receipt: str,
    ) -> CartridgeAccess:
        # The native opener is deliberately first: it consumes the transferred
        # OS handle before any Python-side identity check can reject metadata.
        try:
            handle = self._opener(retained_native_handle, integrity_access_receipt)
        except Exception as error:
            raise CodecSdkError(
                "source.integrity_access", "Core-validated handle transfer was rejected"
            ) from error
        try:
            return NativeCartridgeAccess(
                handle,
                archive_bytes=archive_bytes,
                cartridge_id=cartridge_id,
                archive_sha256=archive_sha256,
            )
        except Exception:
            _drop_native_handle(handle)
            raise

    def close(self, access: CartridgeAccess) -> None:
        if not isinstance(access, NativeCartridgeAccess):
            raise TypeError("access was not created by NativeCartridgeAccessFactory")
        access.close()


def _open_native_handle(raw_handle: int, receipt: str) -> _NativeValidatedHandle:
    from latentdeck_cartridge import open_validated_handle_from_raw

    return open_validated_handle_from_raw(raw_handle, receipt)


def _drop_native_handle(handle: _NativeValidatedHandle) -> None:
    close = getattr(handle, "close", None)
    if callable(close):
        close()
    # PyO3 currently releases the owned HANDLE when its final Python reference
    # is dropped. The caller clears that reference immediately after this call.


def _json_object(encoded: object, label: str) -> dict[str, object]:
    if (
        not isinstance(encoded, str)
        or not encoded
        or len(encoded.encode("utf-8")) > MAX_NATIVE_METADATA_BYTES
    ):
        raise CodecSdkError("source.native_metadata", f"{label} is outside its byte bound")
    try:
        value = json.loads(encoded, parse_constant=_reject_json_constant)
    except (UnicodeError, json.JSONDecodeError, ValueError) as error:
        raise CodecSdkError("source.native_metadata", f"{label} is invalid JSON") from error
    if not isinstance(value, dict) or any(not isinstance(key, str) for key in value):
        raise CodecSdkError("source.native_metadata", f"{label} must be a JSON object")
    return value


def _reject_json_constant(value: str) -> object:
    raise ValueError(f"non-finite JSON constant {value}")


def _crosscheck_identity(
    manifest: Mapping[str, object],
    validation: Mapping[str, object],
    *,
    archive_bytes: int,
    cartridge_id: uuid.UUID,
    archive_sha256: str,
) -> None:
    if not isinstance(cartridge_id, uuid.UUID) or cartridge_id.int == 0:
        raise CodecSdkError("source.identity", "expected cartridge ID is not a non-nil UUID")
    if not isinstance(archive_bytes, int) or isinstance(archive_bytes, bool) or archive_bytes <= 0:
        raise CodecSdkError("source.identity", "expected archive length is invalid")
    _canonical_sha256(archive_sha256, "expected archive SHA-256")
    manifest_id = manifest.get("cartridge_id")
    try:
        parsed_manifest_id = uuid.UUID(str(manifest_id))
    except (AttributeError, ValueError) as error:
        raise CodecSdkError("source.identity", "manifest cartridge ID is invalid") from error
    if str(parsed_manifest_id) != manifest_id or parsed_manifest_id != cartridge_id:
        raise CodecSdkError("source.identity", "manifest cartridge ID does not match Core")
    if validation.get("validation_level") != "full":
        raise CodecSdkError("source.identity", "native handle is not fully validated")
    if validation.get("archive_bytes") != archive_bytes:
        raise CodecSdkError("source.identity", "archive length does not match Core")
    validation_sha = validation.get("archive_sha256")
    _canonical_sha256(validation_sha, "validated archive SHA-256")
    if validation_sha != archive_sha256:
        raise CodecSdkError("source.identity", "archive SHA-256 does not match Core")


def _tensor_descriptors(
    raw: Mapping[str, object],
) -> dict[str, TensorAccessDescriptor]:
    if not 1 <= len(raw) <= MAX_TENSOR_DESCRIPTORS:
        raise CodecSdkError("source.tensor_descriptors", "tensor descriptor count is invalid")
    result: dict[str, TensorAccessDescriptor] = {}
    for name, raw_descriptor in raw.items():
        if not isinstance(raw_descriptor, dict):
            raise CodecSdkError(
                "source.tensor_descriptors", "native tensor descriptor must be an object"
            )
        shape = raw_descriptor.get("shape")
        descriptor = TensorAccessDescriptor(
            name=name,
            dtype=str(raw_descriptor.get("dtype")),
            shape=tuple(shape) if isinstance(shape, list) else (),
            byte_length=raw_descriptor.get("byte_length"),  # type: ignore[arg-type]
        )
        descriptor.validate()
        result[name] = descriptor
    return result


def _crosscheck_manifest_tensors(
    manifest: Mapping[str, object],
    validation: Mapping[str, object],
    descriptors: Mapping[str, TensorAccessDescriptor],
) -> None:
    payloads = manifest.get("payloads")
    tensors = manifest.get("tensors")
    if not isinstance(payloads, list) or len(payloads) != 1 or not isinstance(payloads[0], dict):
        raise CodecSdkError("source.manifest", "manifest must declare one validated payload")
    payload = payloads[0]
    if (
        payload.get("path") != validation.get("payload_path")
        or payload.get("byte_length") != validation.get("payload_bytes")
        or payload.get("sha256") != validation.get("payload_sha256")
    ):
        raise CodecSdkError("source.manifest", "manifest payload does not match validation")
    if not isinstance(tensors, list) or len(tensors) != len(descriptors):
        raise CodecSdkError("source.manifest", "manifest tensor set does not match validation")
    manifest_names: set[str] = set()
    for raw in tensors:
        if not isinstance(raw, dict):
            raise CodecSdkError("source.manifest", "manifest tensor descriptor is invalid")
        name = raw.get("name")
        if not isinstance(name, str) or name in manifest_names or name not in descriptors:
            raise CodecSdkError("source.manifest", "manifest tensor set does not match validation")
        descriptor = descriptors[name]
        shape = raw.get("shape")
        if (
            raw.get("payload") != payload.get("path")
            or raw.get("storage_dtype") != descriptor.dtype
            or not isinstance(shape, list)
            or tuple(shape) != descriptor.shape
        ):
            raise CodecSdkError(
                "source.manifest", "manifest tensor does not match validated descriptor"
            )
        manifest_names.add(name)
    if manifest_names != set(descriptors):
        raise CodecSdkError("source.manifest", "manifest tensor set does not match validation")


def _canonical_sha256(value: object, label: str) -> str:
    if (
        not isinstance(value, str)
        or len(value) != 64
        or any(character not in "0123456789abcdef" for character in value)
    ):
        raise CodecSdkError("source.identity", f"{label} is invalid")
    return value


def _freeze_mapping(value: Mapping[str, object]) -> Mapping[str, object]:
    return MappingProxyType({key: _freeze_json(item) for key, item in value.items()})


def _freeze_json(value: object) -> object:
    if isinstance(value, dict):
        if any(not isinstance(key, str) for key in value):
            raise CodecSdkError("source.native_metadata", "JSON object key is not text")
        return _freeze_mapping(value)
    if isinstance(value, list):
        return tuple(_freeze_json(item) for item in value)
    if isinstance(value, float) and not math.isfinite(value):
        raise CodecSdkError("source.native_metadata", "JSON number is not finite")
    if value is None or isinstance(value, (str, int, float, bool)):
        return value
    raise CodecSdkError("source.native_metadata", "native metadata is not JSON-safe")


__all__ = ["NativeCartridgeAccess", "NativeCartridgeAccessFactory"]
