from __future__ import annotations

import hashlib
import importlib.util
import json
import struct
import tomllib
import uuid
from collections.abc import Mapping
from pathlib import Path

import pytest
import torch
from latentdeck_codec_sdk import (
    Capability,
    CaptureRequest,
    CodecLoadRequest,
    CodecSdkError,
    ExternalAsset,
    RawImportAdapter,
    RawImportPreflightRequest,
    RawImportStageRequest,
    TensorAccessDescriptor,
)
from safetensors.torch import save_file

from latentdeck_codec_h3.adapter import (
    ADAPTER_VERSION,
    PACK_VERSION,
    TAEH3_ASSET_BYTE_LENGTH,
    TAEH3_ASSET_SHA256,
    H3CodecAdapter,
    make_adapter,
)


class MemoryAccess:
    def __init__(
        self,
        cartridge_id: uuid.UUID,
        archive_sha256: str,
        manifest: Mapping[str, object],
        tensors: Mapping[str, bytes],
        descriptors: Mapping[str, TensorAccessDescriptor],
    ) -> None:
        self._cartridge_id = cartridge_id
        self._archive_sha256 = archive_sha256
        self._manifest = manifest
        self._tensors = dict(tensors)
        self._descriptors = dict(descriptors)
        self.reads: list[tuple[str, int, int]] = []

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
        return self._descriptors[name]

    def read_tensor_range(self, name: str, offset: int, byte_length: int) -> memoryview:
        tensor = self._tensors[name]
        if (
            isinstance(offset, bool)
            or isinstance(byte_length, bool)
            or offset < 0
            or byte_length <= 0
            or offset + byte_length > len(tensor)
        ):
            raise ValueError("tensor range is outside the retained handle")
        self.reads.append((name, offset, byte_length))
        return memoryview(tensor)[offset : offset + byte_length]


class FakeDecoder:
    def __init__(self) -> None:
        self.reset_count = 0
        self.decode_count = 0

    def reset(self) -> None:
        self.reset_count += 1

    def decode_slot(self, tensor: torch.Tensor) -> tuple[bytes, ...]:
        self.decode_count += 1
        height = int(tensor.shape[3]) * 16
        width = int(tensor.shape[4]) * 16
        return (bytes([7, 8, 9, 255]) * (height * width),)


def _make_access(
    *,
    storage_dtype: str = "F16",
    temporal: int = 7,
    latent_height: int = 1,
    latent_width: int = 1,
    with_audio: bool = False,
) -> MemoryAccess:
    cartridge_id = uuid.uuid4()
    frame_count = 5 + 17 * ((temporal - 2) // 5)
    video_values = [
        float(channel * 10 + slot)
        for channel in range(24)
        for slot in range(temporal)
        for _height in range(latent_height)
        for _width in range(latent_width)
    ]
    if storage_dtype == "F16":
        video = b"".join(struct.pack("<e", value) for value in video_values)
    else:
        video = b"".join(struct.pack("<f", value) for value in video_values)
    audio = b""
    tensors: list[dict[str, object]] = [
        {
            "stream": "visual",
            "name": "video",
            "payload": "payloads/h3.safetensors",
            "storage_dtype": storage_dtype,
            "runtime_dtype": "F16",
            "shape": [1, 24, temporal, latent_height, latent_width],
        }
    ]
    audio_disposition: dict[str, object] = {"policy": "source_absent"}
    if with_audio:
        audio_slots = (frame_count * 5 + 1) // 3
        audio = b"\0" * (1 * 32 * 2 * audio_slots * 2)
        tensors.append(
            {
                "stream": "audio",
                "name": "audio",
                "payload": "payloads/h3.safetensors",
                "storage_dtype": "F16",
                "runtime_dtype": "F16",
                "shape": [1, 32, 2, audio_slots],
            }
        )
        audio_disposition = {"policy": "preserved_source"}
    payload = video + audio
    duration_divisor = __import__("math").gcd(frame_count, 24)
    manifest: dict[str, object] = {
        "spec_version": "0.1.0",
        "cartridge_id": str(cartridge_id),
        "codec": {
            "family": "minimax_h3",
            "profile": "h3_av_latent",
            "profile_version": "0.1.0",
        },
        "payloads": [
            {
                "path": "payloads/h3.safetensors",
                "media_type": "application/vnd.safetensors",
                "byte_length": len(payload),
                "sha256": hashlib.sha256(payload).hexdigest(),
            }
        ],
        "tensors": tensors,
        "timing": {
            "contract": "minimax_h3_causal",
            "contract_version": "0.1.0",
            "decoded_video": {
                "width": latent_width * 16,
                "height": latent_height * 16,
                "frame_count": frame_count,
                "frame_rate": {"numerator": 24, "denominator": 1},
                "duration": {
                    "numerator": frame_count // duration_divisor,
                    "denominator": 24 // duration_divisor,
                },
            },
        },
        "audio": audio_disposition,
        "provenance": {"created_by": {"name": "test", "version": "0.1.0"}, "sources": []},
        "parent_cartridges": [],
        "operation_history": [],
    }
    descriptor_map = {
        "video": TensorAccessDescriptor(
            "video",
            storage_dtype,
            (1, 24, temporal, latent_height, latent_width),
            len(video),
        )
    }
    tensor_map = {"video": video}
    if with_audio:
        descriptor_map["audio"] = TensorAccessDescriptor(
            "audio", "F16", (1, 32, 2, audio_slots), len(audio)
        )
        tensor_map["audio"] = audio
    return MemoryAccess(
        cartridge_id,
        hashlib.sha256(b"synthetic archive identity").hexdigest(),
        manifest,
        tensor_map,
        descriptor_map,
    )


def _adapter_harness(
    tmp_path: Path,
    decoder: FakeDecoder,
) -> tuple[
    H3CodecAdapter,
    CodecLoadRequest,
    list[tuple[str, int]],
    list[str],
    list[tuple[str, str, int]],
]:
    decoder_loads: list[tuple[str, int]] = []
    torch_loads: list[str] = []
    asset_preflights: list[tuple[str, str, int]] = []

    def decoder_factory(asset: ExternalAsset, ordinal: int) -> FakeDecoder:
        decoder_loads.append((asset.sha256, ordinal))
        return decoder

    def torch_loader() -> object:
        torch_loads.append(str(torch.__version__))
        return torch

    def asset_validator(path: str, sha256: str, byte_length: int) -> Path:
        asset_preflights.append((path, sha256, byte_length))
        return Path(path)

    adapter = H3CodecAdapter(
        torch_loader=torch_loader,
        decoder_factory=decoder_factory,
        tensor_transfer=lambda cpu, _torch, _ordinal: cpu.to(dtype=torch.float16).contiguous(),
        asset_validator=asset_validator,
    )
    asset = ExternalAsset(
        asset_id="taeh3",
        path=str(tmp_path / "taeh3.safetensors"),
        sha256=TAEH3_ASSET_SHA256,
        byte_length=TAEH3_ASSET_BYTE_LENGTH,
    )
    request = CodecLoadRequest(
        descriptor=adapter.descriptor(),
        assets=(asset,),
        device="cuda",
        device_ordinal=0,
    )
    return adapter, request, decoder_loads, torch_loads, asset_preflights


def _loaded_adapter(
    tmp_path: Path,
    decoder: FakeDecoder,
) -> tuple[H3CodecAdapter, list[tuple[str, int]]]:
    adapter, request, decoder_loads, _torch_loads, _asset_preflights = _adapter_harness(
        tmp_path, decoder
    )
    adapter.load(request)
    return adapter, decoder_loads


def test_descriptor_and_pack_owned_entrypoint_are_exact_v2() -> None:
    adapter = make_adapter()
    descriptor = adapter.descriptor()
    assert descriptor.pack_version == PACK_VERSION == "0.2.0"
    assert descriptor.adapter_version == ADAPTER_VERSION == "0.2.0"
    assert descriptor.host_api_version == "2.0"
    assert {capability.value for capability in descriptor.capabilities} == {
        "player",
        "realtime",
        "resample",
        "snapshot_capture",
        "live_capture",
        "raw_import",
    }
    assert Capability.RAW_IMPORT in descriptor.capabilities
    assert isinstance(adapter, RawImportAdapter)


def test_h3_pack_keeps_only_the_player_p1_bridge_and_generic_v2_adapter() -> None:
    package_root = Path(__file__).parents[1]
    project = tomllib.loads((package_root / "pyproject.toml").read_text(encoding="utf-8"))[
        "project"
    ]
    assert project["scripts"] == {"latentdeck-h3-worker": "latentdeck_codec_h3.worker:main"}
    assert all(
        "operator-d2" not in item and "operator-q4" not in item for item in project["dependencies"]
    )
    assert importlib.util.find_spec("latentdeck_codec_h3.d2_worker") is None
    assert importlib.util.find_spec("latentdeck_codec_h3.q4_worker") is None
    inventory = json.loads(
        (package_root / "packaging/windows-x64-cu130.lock.json").read_text(encoding="utf-8")
    )
    local_versions = {item["name"]: item["version"] for item in inventory["local_projects"]}
    assert local_versions == {
        "latentdeck-cartridge": "0.1.0",
        "latentdeck-codec-host": "0.1.0",
        "latentdeck-codec-sdk": "0.2.0",
        "latentdeck-deck-sdk": "0.2.0",
        "latentdeck-rgb-ring": "0.1.0",
        "latentdeck-codec-h3": "0.2.0",
    }


@pytest.mark.parametrize("storage_dtype", ["F16", "F32"])
def test_path_free_profile_receipt_and_streamed_slot_read(
    tmp_path: Path,
    storage_dtype: str,
) -> None:
    decoder = FakeDecoder()
    adapter, load_request, decoder_loads, torch_loads, asset_preflights = _adapter_harness(
        tmp_path, decoder
    )
    access = _make_access(storage_dtype=storage_dtype, with_audio=True)

    # Strict permit order: descriptor -> inspect -> validate_profile performs
    # no Torch import, asset access, decoder construction, or GPU allocation.
    assert decoder_loads == []
    assert torch_loads == []
    assert asset_preflights == []
    inspection = adapter.inspect(access)
    receipt = adapter.validate_profile(access, inspection)
    second_receipt = adapter.validate_profile(access, inspection)
    assert decoder_loads == []
    assert torch_loads == []
    assert asset_preflights == []
    assert second_receipt.receipt_id != receipt.receipt_id
    assert receipt.profile_key.codec_family == "minimax_h3"
    assert receipt.tensor_abi.shape == (1, 24, 1, 1, 1)
    assert receipt.tensor_abi.dtype == "float16"
    assert receipt.tensor_abi.device == "cuda"
    assert receipt.estimated_device_bytes == 24 * 1 * 1 * 2 + TAEH3_ASSET_BYTE_LENGTH

    adapter.load(load_request)
    assert torch_loads == [str(torch.__version__)]
    assert asset_preflights == [
        (load_request.assets[0].path, TAEH3_ASSET_SHA256, TAEH3_ASSET_BYTE_LENGTH)
    ]
    assert decoder_loads == []

    source = adapter.open_source(access, receipt, uuid.uuid4())
    assert len(decoder_loads) == 1
    slot = adapter.read_slot(source, 3)
    assert slot.dtype == torch.float16
    assert slot.is_contiguous()
    assert tuple(slot.shape) == (1, 24, 1, 1, 1)
    assert slot[0, 0, 0, 0, 0].item() == 3
    assert slot[0, 23, 0, 0, 0].item() == 233
    assert all(
        name == "video" and offset >= 0 and byte_length > 0
        for name, offset, byte_length in access.reads
    )
    assert not hasattr(access, "read_payload_range")
    assert not hasattr(source, "path")

    decoded = adapter.decode_slot(slot, 1)
    assert (decoded.batch, decoded.height, decoded.width) == (1, 16, 16)
    assert decoded.pixels.nbytes == 16 * 16 * 4
    adapter.reset_decoder(2)
    assert decoder.reset_count == 1
    with pytest.raises(CodecSdkError, match="decode.generation_invalid"):
        adapter.reset_decoder(2)
    source.close()
    with pytest.raises(CodecSdkError, match="source.closed"):
        adapter.read_slot(source, 0)


def test_invalid_h3_semantics_fail_before_decoder_allocation(tmp_path: Path) -> None:
    decoder = FakeDecoder()
    adapter, _request, decoder_loads, torch_loads, asset_preflights = _adapter_harness(
        tmp_path, decoder
    )
    access = _make_access()
    decoded = access.manifest["timing"]["decoded_video"]  # type: ignore[index]
    decoded["frame_count"] = 23  # type: ignore[index]
    with pytest.raises(CodecSdkError, match="profile.timing"):
        adapter.inspect(access)
    assert decoder_loads == []
    assert torch_loads == []
    assert asset_preflights == []


def test_load_rejects_external_asset_tamper_without_decoder(tmp_path: Path) -> None:
    asset_path = tmp_path / "taeh3.safetensors"
    asset_path.write_bytes(b"asset")
    adapter = H3CodecAdapter()
    with pytest.raises(CodecSdkError, match="codec.asset_incompatible"):
        adapter.load(
            CodecLoadRequest(
                descriptor=adapter.descriptor(),
                assets=(
                    ExternalAsset(
                        asset_id="taeh3",
                        path=str(asset_path),
                        sha256="0" * 64,
                        byte_length=asset_path.stat().st_size,
                    ),
                ),
                device="cuda",
                device_ordinal=0,
            )
        )


def test_capture_writer_stages_only_codec_payload_and_cleans_abort(tmp_path: Path) -> None:
    decoder = FakeDecoder()
    adapter, decoder_loads = _loaded_adapter(tmp_path, decoder)
    capture_id = uuid.uuid4()
    staging_root = tmp_path / f"capture-{capture_id}"
    staging_root.mkdir()
    sentinel = staging_root / "host-owned.sentinel"
    sentinel.write_text("keep", encoding="utf-8")
    writer = adapter.create_capture_writer(
        CaptureRequest(
            capture_id=capture_id,
            mode="snapshot",
            staging_root=str(staging_root.resolve()),
            maximum_latent_slots=7,
            maximum_visual_bytes=1024 * 1024,
            maximum_reset_events=2,
        )
    )
    slot = torch.ones((1, 24, 1, 1, 1), dtype=torch.float16).contiguous()
    writer.append(slot, reset_event={"reason": "start"})
    writer.append(slot)
    payload = writer.finish()
    payload_path = Path(payload.payload_path)
    assert payload.capture_id == capture_id
    assert payload.latent_slots == 2
    assert payload.decoded_frame_count == 5
    assert payload_path.is_file()
    assert payload_path.is_relative_to(staging_root.resolve())
    assert hashlib.sha256(payload_path.read_bytes()).hexdigest() == payload.payload_sha256
    assert decoder_loads == []
    writer.abort()
    assert not payload_path.exists()
    assert sentinel.read_text(encoding="utf-8") == "keep"
    assert staging_root.is_dir()


def test_capture_writer_rejects_non_h3_boundary_and_removes_partials(tmp_path: Path) -> None:
    adapter, _decoder_loads = _loaded_adapter(tmp_path, FakeDecoder())
    capture_id = uuid.uuid4()
    staging_root = tmp_path / f"capture-{capture_id}"
    staging_root.mkdir()
    sentinel = staging_root / "host-owned.sentinel"
    sentinel.write_text("keep", encoding="utf-8")
    writer = adapter.create_capture_writer(
        CaptureRequest(
            capture_id=capture_id,
            mode="live_capture",
            staging_root=str(staging_root.resolve()),
            maximum_latent_slots=7,
            maximum_visual_bytes=1024 * 1024,
        )
    )
    slot = torch.zeros((1, 24, 1, 1, 1), dtype=torch.float16).contiguous()
    for _ in range(3):
        writer.append(slot)
    with pytest.raises(CodecSdkError, match="capture.not_ready"):
        writer.finish()
    assert writer._spool is not None and writer._spool.raw_path.is_file()
    for _ in range(4):
        writer.append(slot)
    payload = writer.finish()
    assert payload.latent_slots == 7
    assert payload.decoded_frame_count == 22
    assert Path(payload.payload_path).is_relative_to(staging_root.resolve())
    writer.abort()
    assert sentinel.read_text(encoding="utf-8") == "keep"
    assert sorted(path.name for path in staging_root.iterdir()) == [sentinel.name]


def test_raw_import_preflight_and_stage_are_cpu_only_and_exact(tmp_path: Path) -> None:
    adapter, _load_request, decoder_loads, torch_loads, asset_preflights = _adapter_harness(
        tmp_path, FakeDecoder()
    )
    source = tmp_path / "raw-h3.safetensors"
    save_file({"video": torch.zeros((1, 24, 7, 1, 1), dtype=torch.float16)}, source)
    import_id = uuid.uuid4()
    preflight = adapter.preflight_raw_import(
        RawImportPreflightRequest(import_id, str(source.resolve()), source.stat().st_size)
    )

    assert preflight.import_id == import_id
    assert preflight.source_sha256 == hashlib.sha256(source.read_bytes()).hexdigest()
    assert preflight.metadata.profile_key.profile == "h3_av_latent"
    assert preflight.metadata.tensors[0].shape == (1, 24, 7, 1, 1)
    assert preflight.metadata.decoded_frame_count == 22
    assert preflight.metadata.audio_policy == "source_absent"
    assert decoder_loads == []
    assert torch_loads == []
    assert asset_preflights == []

    staging_root = tmp_path / "core-owned-staging"
    staging_root.mkdir()
    sentinel = staging_root / "core-owned-sentinel"
    sentinel.write_text("keep", encoding="utf-8")
    artifact = adapter.stage_raw_import(
        RawImportStageRequest(preflight, str(staging_root.resolve()))
    )
    staged = Path(artifact.staged_payload_path)
    assert staged.parent == staging_root.resolve()
    assert staged.read_bytes() == source.read_bytes()
    assert artifact.payload_sha256 == preflight.source_sha256
    assert decoder_loads == []
    assert torch_loads == []
    assert asset_preflights == []

    adapter.abort_raw_import(import_id)
    assert not staged.exists()
    assert staging_root.is_dir()
    assert sentinel.read_text(encoding="utf-8") == "keep"


def test_raw_import_stage_revalidates_source_and_leaves_no_partial(tmp_path: Path) -> None:
    adapter = H3CodecAdapter()
    source = tmp_path / "raw-h3.safetensors"
    save_file({"video": torch.zeros((1, 24, 7, 1, 1), dtype=torch.float16)}, source)
    import_id = uuid.uuid4()
    preflight = adapter.preflight_raw_import(
        RawImportPreflightRequest(import_id, str(source.resolve()), source.stat().st_size)
    )
    source.write_bytes(source.read_bytes() + b"tamper")
    staging_root = tmp_path / "staging"
    staging_root.mkdir()

    with pytest.raises(CodecSdkError, match="raw_import.source_changed"):
        adapter.stage_raw_import(RawImportStageRequest(preflight, str(staging_root.resolve())))

    assert list(staging_root.iterdir()) == []
