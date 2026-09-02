from __future__ import annotations

import json
import uuid
from copy import deepcopy

import pytest
from latentdeck_codec_host import NativeCartridgeAccessFactory
from latentdeck_codec_host.runtime_v2 import CartridgeAccessFactory
from latentdeck_codec_sdk import CartridgeAccess, CodecSdkError

CARTRIDGE_ID = uuid.UUID("dfe47ccc-390d-4703-9ccb-5def8b51ffdf")
ARCHIVE_SHA256 = "a" * 64
PAYLOAD_SHA256 = "b" * 64
ARCHIVE_BYTES = 4096


def _manifest() -> dict[str, object]:
    return {
        "spec_version": "0.1.0",
        "cartridge_id": str(CARTRIDGE_ID),
        "codec": {
            "family": "synthetic",
            "profile": "test",
            "profile_version": "1.0.0",
        },
        "payloads": [
            {
                "path": "payloads/test.safetensors",
                "media_type": "application/vnd.safetensors",
                "byte_length": 128,
                "sha256": PAYLOAD_SHA256,
            }
        ],
        "tensors": [
            {
                "stream": "visual",
                "name": "video",
                "payload": "payloads/test.safetensors",
                "storage_dtype": "F16",
                "runtime_dtype": "F16",
                "shape": [1, 2],
            }
        ],
    }


def _validation() -> dict[str, object]:
    return {
        "validation_level": "full",
        "archive_bytes": ARCHIVE_BYTES,
        "archive_sha256": ARCHIVE_SHA256,
        "payload_path": "payloads/test.safetensors",
        "payload_bytes": 128,
        "payload_sha256": PAYLOAD_SHA256,
        "tensor_storage_bytes": 4,
    }


class FakeNativeHandle:
    def __init__(
        self,
        *,
        manifest: dict[str, object] | None = None,
        validation: dict[str, object] | None = None,
        descriptors: dict[str, object] | None = None,
    ) -> None:
        self.manifest = deepcopy(manifest or _manifest())
        self.validation = deepcopy(validation or _validation())
        self.descriptors = deepcopy(
            descriptors or {"video": {"dtype": "F16", "shape": [1, 2], "byte_length": 4}}
        )
        self.tensor = bytes([10, 20, 30, 40])
        self.reads: list[tuple[str, int, int, int | None]] = []
        self.close_count = 0

    def manifest_json(self) -> str:
        return json.dumps(self.manifest, separators=(",", ":"))

    def validation_json(self) -> str:
        return json.dumps(self.validation, separators=(",", ":"))

    def tensor_descriptors_json(self) -> str:
        return json.dumps(self.descriptors, separators=(",", ":"))

    def read_tensor_range(
        self,
        name: str,
        offset: int,
        byte_length: int,
        max_read_bytes: int | None = None,
    ) -> bytes:
        self.reads.append((name, offset, byte_length, max_read_bytes))
        assert name == "video"
        assert max_read_bytes == byte_length
        return self.tensor[offset : offset + byte_length]

    def close(self) -> None:
        self.close_count += 1


class FakeOpener:
    def __init__(self, handle: FakeNativeHandle) -> None:
        self.handle = handle
        self.calls: list[tuple[int, str]] = []

    def __call__(self, raw_handle: int, receipt: str) -> FakeNativeHandle:
        self.calls.append((raw_handle, receipt))
        return self.handle


def _open(factory: NativeCartridgeAccessFactory):
    return factory.open(
        retained_native_handle=1234,
        archive_bytes=ARCHIVE_BYTES,
        cartridge_id=CARTRIDGE_ID,
        archive_sha256=ARCHIVE_SHA256,
        integrity_access_receipt='{"access_abi_version":1}',
    )


def test_factory_exposes_only_immutable_tensor_relative_access() -> None:
    native = FakeNativeHandle()
    opener = FakeOpener(native)
    factory = NativeCartridgeAccessFactory(opener)

    access = _open(factory)

    assert isinstance(factory, CartridgeAccessFactory)
    assert isinstance(access, CartridgeAccess)
    assert opener.calls == [(1234, '{"access_abi_version":1}')]
    assert access.cartridge_id == CARTRIDGE_ID
    assert access.archive_sha256 == ARCHIVE_SHA256
    assert access.tensor_descriptor("video").shape == (1, 2)
    assert access.read_tensor_range("video", 1, 2).tobytes() == bytes([20, 30])
    assert native.reads == [("video", 1, 2, 2)]
    assert access.read_tensor_range("video", 0, 1).readonly
    with pytest.raises(TypeError):
        access.manifest["cartridge_id"] = "mutated"  # type: ignore[index]
    assert isinstance(access.manifest["payloads"], tuple)
    with pytest.raises(CodecSdkError, match="tensor.range_invalid"):
        access.read_tensor_range("video", 3, 2)
    assert not hasattr(access, "read_payload_range")

    factory.close(access)
    factory.close(access)
    assert native.close_count == 1
    with pytest.raises(CodecSdkError, match="source.closed"):
        access.tensor_descriptor("video")


@pytest.mark.parametrize(
    ("mutation", "message"),
    [
        (lambda handle: handle.manifest.__setitem__("cartridge_id", str(uuid.uuid4())), "ID"),
        (lambda handle: handle.validation.__setitem__("archive_sha256", "c" * 64), "SHA"),
        (lambda handle: handle.validation.__setitem__("archive_bytes", 4095), "length"),
        (
            lambda handle: handle.descriptors["video"].__setitem__("byte_length", 2),
            "length",
        ),
    ],
)
def test_factory_drops_consumed_native_handle_on_every_crosscheck_error(
    mutation, message: str
) -> None:
    native = FakeNativeHandle()
    mutation(native)
    opener = FakeOpener(native)

    with pytest.raises(CodecSdkError, match=message):
        _open(NativeCartridgeAccessFactory(opener))

    assert opener.calls == [(1234, '{"access_abi_version":1}')]
    assert native.close_count == 1


def test_factory_calls_consuming_opener_exactly_once_when_native_rejects() -> None:
    calls: list[tuple[int, str]] = []

    def reject(raw_handle: int, receipt: str):
        calls.append((raw_handle, receipt))
        raise OSError("native receipt rejected after consuming handle")

    with pytest.raises(CodecSdkError, match="source.integrity_access"):
        _open(NativeCartridgeAccessFactory(reject))

    assert calls == [(1234, '{"access_abi_version":1}')]
