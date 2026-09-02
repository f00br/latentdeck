from __future__ import annotations

import hashlib
import importlib
import sys
import uuid
from collections.abc import Mapping
from pathlib import Path

import pytest
from latentdeck_codec_host.runtime_v2 import (
    ProcessReceipt,
    Protocol2Worker,
    TrustedCodecEntrypoint,
    TrustedDeckEntrypoint,
    _device_matches_codec_load,
)
from latentdeck_codec_sdk import Capability, ProtocolError, TensorAccessDescriptor

PROFILE = {
    "codec_family": "test_codec",
    "profile": "synthetic_latent",
    "profile_version": "1.0.0",
}


@pytest.mark.parametrize(
    ("device", "expected_device", "expected_ordinal", "matches"),
    [
        ("cpu", "cpu", 0, True),
        ("cpu:0", "cpu", 0, True),
        ("cpu", "cpu", 1, False),
        ("cuda:0", "cuda", 0, True),
        ("cuda:1", "cuda", 1, True),
        ("cuda:1", "cuda", 0, False),
        ("cuda", "cuda", 0, False),
        ("cuda:0", "cpu", 0, False),
    ],
)
def test_exact_codec_load_device_rejects_malicious_cuda_ordinal(
    device: str, expected_device: str, expected_ordinal: int, matches: bool
) -> None:
    torch = importlib.import_module("torch")
    assert (
        _device_matches_codec_load(
            torch.device(device), expected_device, expected_ordinal
        )
        is matches
    )

SYNTHETIC_CODEC = r"""
from __future__ import annotations

import hashlib
import importlib.metadata
import uuid
from dataclasses import dataclass
from pathlib import Path

from latentdeck_codec_sdk import (
    Capability,
    CapturePayload,
    CodecSdkError,
    CodecDescriptor,
    DecodedAbi,
    DecodedBatch,
    ProfileInspection,
    ProfileKey,
    ProfileReceipt,
    RawImportArtifact,
    RawImportMetadata,
    RawImportPreflight,
    RawImportTensor,
    SignalGeometry,
    TensorAbi,
)

EVENTS = []
CAPTURE_FINISH_ERROR = False
TORCH_IMPORTED_DURING_MODULE_LOAD = "torch" in __import__("sys").modules
_torch = None

PROFILE = ProfileKey("test_codec", "synthetic_latent", "1.0.0")
DECLARED_TORCH_BUILD = importlib.metadata.version("torch")
CAPABILITIES = (
    Capability.PLAYER,
    Capability.REALTIME,
    Capability.RESAMPLE,
    Capability.SNAPSHOT_CAPTURE,
    Capability.LIVE_CAPTURE,
    Capability.RAW_IMPORT,
)


@dataclass
class Handle:
    source_id: uuid.UUID
    seed: int
    slot_count: int = 4
    closed: bool = False

    def close(self):
        self.closed = True


class Writer:
    def __init__(self, request):
        self.request = request
        self.payloads = []
        self.reset_events = []
        self.finished = False
        self.output_path = Path(request.staging_root) / f"{request.capture_id}.payload"

    def append(self, tensor, *, reset_event=None):
        if self.finished:
            raise RuntimeError("writer is finalized")
        EVENTS.append("capture.append")
        assert tuple(tensor.shape) == (1, 4, 1, 2, 3)
        assert tensor.dtype == _torch.float32
        assert tensor.device.type == "cpu"
        assert tensor.is_contiguous()
        assert bool(_torch.isfinite(tensor).all().item())
        self.payloads.append(tensor.detach().cpu().numpy().tobytes())
        if reset_event is not None:
            self.reset_events.append(dict(reset_event))

    def finish(self):
        if self.finished:
            raise RuntimeError("writer is already finalized")
        if CAPTURE_FINISH_ERROR:
            raise CodecSdkError("capture.synthetic_failure", "synthetic finish failed")
        if len(self.payloads) < 2 or (len(self.payloads) - 2) % 5 != 0:
            raise CodecSdkError("capture.not_ready", "synthetic causal boundary not reached")
        self.finished = True
        encoded = b"".join(self.payloads)
        self.output_path.write_bytes(encoded)
        return CapturePayload(
            capture_id=self.request.capture_id,
            payload_path=str(self.output_path),
            payload_sha256=hashlib.sha256(encoded).hexdigest(),
            payload_byte_length=len(encoded),
            latent_slots=len(self.payloads),
            decoded_frame_count=len(self.payloads),
        )

    def abort(self):
        EVENTS.append("capture.abort")
        self.finished = True
        self.payloads.clear()
        self.output_path.unlink(missing_ok=True)


class Adapter:
    def __init__(self):
        self.raw_imports = {}

    def descriptor(self):
        return CodecDescriptor(
            pack_id="test.synthetic.codec",
            pack_version="0.2.0",
            adapter_id="synthetic",
            adapter_version="0.2.0",
            host_api_version="2.0",
            capabilities=CAPABILITIES,
            profiles=(PROFILE,),
        )

    def inspect(self, cartridge):
        manifest = cartridge.manifest
        geometry = manifest["geometry"]
        return ProfileInspection(
            cartridge_id=cartridge.cartridge_id,
            archive_sha256=cartridge.archive_sha256,
            payload_sha256=manifest["payload_sha256"],
            profile_key=PROFILE,
            signal_geometry=SignalGeometry(**geometry),
        )

    def validate_profile(self, cartridge, inspection):
        geometry = inspection.signal_geometry
        return ProfileReceipt(
            receipt_id=uuid.uuid5(uuid.NAMESPACE_OID, str(cartridge.cartridge_id)),
            cartridge_id=cartridge.cartridge_id,
            archive_sha256=cartridge.archive_sha256,
            payload_sha256=inspection.payload_sha256,
            pack_id="test.synthetic.codec",
            pack_version="0.2.0",
            adapter_id="synthetic",
            adapter_version="0.2.0",
            profile_key=PROFILE,
            signal_geometry=geometry,
            tensor_abi=TensorAbi(
                python_version="3.13",
                torch_version=DECLARED_TORCH_BUILD,
                dtype="float32",
                shape=(1, 4, 1, 2, 3),
                device="cpu",
            ),
            decoded_abi=DecodedAbi(maximum_batch=4),
            capabilities=CAPABILITIES,
            estimated_host_bytes=512,
            estimated_device_bytes=0,
        )

    def load(self, request):
        global _torch
        import torch
        if request.device != "cpu" or request.device_ordinal != 0:
            raise ValueError("synthetic codec is CPU-only")
        _torch = torch
        EVENTS.append("codec.load")

    def open_source(self, cartridge, receipt, source_id):
        seed = int(cartridge.read_tensor_range("seed", 0, 1)[0])
        EVENTS.append("source.open")
        return Handle(source_id=source_id, seed=seed)

    def read_slot(self, source, slot_index):
        if source.closed or not 0 <= slot_index < source.slot_count:
            raise ValueError("invalid source slot")
        EVENTS.append("source.read")
        return _torch.full(
            (1, 4, 1, 2, 3),
            float(source.seed + slot_index),
            dtype=_torch.float32,
            device="cpu",
        ).contiguous()

    def decode_slot(self, tensor, maximum_frames):
        EVENTS.append("decode")
        assert 1 <= maximum_frames <= 24
        assert tuple(tensor.shape) == (1, 4, 1, 2, 3)
        value = max(0, min(255, int(float(tensor.mean().item()))))
        pixels = memoryview(bytearray([value, value, value, 255] * (4 * 6)))
        return DecodedBatch(pixels=pixels, batch=1, height=4, width=6)

    def reset_decoder(self, stream_generation):
        assert stream_generation > 0
        EVENTS.append("decoder.reset")

    def create_capture_writer(self, request):
        EVENTS.append("capture.create")
        return Writer(request)

    def preflight_raw_import(self, request):
        request.validate()
        source = Path(request.source_path)
        encoded = source.read_bytes()
        if len(encoded) > request.maximum_source_bytes:
            raise ValueError("raw source exceeds bound")
        preflight = RawImportPreflight(
            receipt_id=uuid.uuid4(),
            import_id=request.import_id,
            pack_id="test.synthetic.codec",
            pack_version="0.2.0",
            adapter_id="synthetic",
            adapter_version="0.2.0",
            source_sha256=hashlib.sha256(encoded).hexdigest(),
            source_byte_length=len(encoded),
            metadata=RawImportMetadata(
                profile_key=PROFILE,
                payload_entry="payloads/synthetic.safetensors",
                payload_media_type="application/vnd.safetensors",
                tensors=(RawImportTensor("visual", "seed", "F16", "F16", (1,)),),
                timing_contract="synthetic_ticks",
                timing_contract_version="1.0.0",
                decoded_width=1,
                decoded_height=1,
                decoded_frame_count=1,
                frame_rate_numerator=24,
                frame_rate_denominator=1,
                duration_numerator=1,
                duration_denominator=24,
                audio_policy="source_absent",
            ),
        )
        preflight.validate()
        self.raw_imports[request.import_id] = (source, preflight, None)
        EVENTS.append("raw_import.preflight")
        return preflight

    def stage_raw_import(self, request):
        request.validate()
        source, preflight, _artifact = self.raw_imports[request.preflight.import_id]
        assert preflight == request.preflight
        root = Path(request.staging_root)
        destination = root / f"{preflight.import_id}.safetensors"
        encoded = source.read_bytes()
        destination.write_bytes(encoded)
        artifact = RawImportArtifact(
            receipt_id=preflight.receipt_id,
            import_id=preflight.import_id,
            staged_payload_path=str(destination),
            payload_sha256=hashlib.sha256(encoded).hexdigest(),
            payload_byte_length=len(encoded),
        )
        self.raw_imports[preflight.import_id] = (source, preflight, artifact)
        EVENTS.append("raw_import.stage")
        return artifact

    def abort_raw_import(self, import_id):
        state = self.raw_imports.pop(import_id, None)
        if state is not None and state[2] is not None:
            Path(state[2].staged_payload_path).unlink(missing_ok=True)
        EVENTS.append("raw_import.abort")


def make_adapter():
    EVENTS.append("adapter.create")
    return Adapter()
"""

SYNTHETIC_DECK = r"""
from __future__ import annotations

from latentdeck_deck_sdk import DeckOperatorResult


def process_sources(sources, controls, context):
    output = sources[0]
    for source in sources[1:]:
        output = output + source
    output = (output / len(sources) + float(controls.get("bias", 0.0))).contiguous()
    return DeckOperatorResult(
        output=output,
        provenance={
            "operator": "test.synthetic.average",
            "source_count": len(sources),
            "sequence": context.sequence,
            "generation": context.generation,
            "physical_slots": list(context.physical_slots),
            "history_present": [item is not None for item in context.previous_sources],
        },
    )
"""


class MemoryAccess:
    def __init__(self, cartridge_id: uuid.UUID, archive_sha256: str, seed: int) -> None:
        self._cartridge_id = cartridge_id
        self._archive_sha256 = archive_sha256
        self._payload = bytes([seed, 0])
        self.reads: list[tuple[str, int, int]] = []
        self._manifest: Mapping[str, object] = {
            "payload_sha256": f"{seed:02x}" * 32,
            "geometry": {
                "channels": 4,
                "latent_height": 2,
                "latent_width": 3,
                "decoded_height": 4,
                "decoded_width": 6,
                "frame_rate_numerator": 24,
                "frame_rate_denominator": 1,
                "timing_contract": "synthetic_ticks",
                "timing_contract_version": "1.0.0",
            },
        }

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
        if name != "seed":
            raise KeyError(name)
        return TensorAccessDescriptor("seed", "F16", (1,), 2)

    def read_tensor_range(self, name: str, offset: int, byte_length: int) -> memoryview:
        if (
            name != "seed"
            or isinstance(offset, bool)
            or isinstance(byte_length, bool)
            or offset < 0
            or byte_length <= 0
            or offset + byte_length > len(self._payload)
        ):
            raise ValueError("tensor range is outside retained bounds")
        self.reads.append((name, offset, byte_length))
        return memoryview(self._payload)[offset : offset + byte_length]


class Sink:
    def __init__(self) -> None:
        self.deliveries: list[tuple[uuid.UUID, int, int, object]] = []
        self.rings: dict[uuid.UUID, tuple[str, int, int]] = {}
        self.configure_attempts: list[tuple[uuid.UUID, int, int, int]] = []
        self.discarded_handles: list[tuple[int, int, int]] = []
        self.released: list[uuid.UUID] = []
        self.generations: dict[uuid.UUID, int] = {}
        self.reject_next_configure = False

    def configure(
        self,
        *,
        ring_id,
        kind,
        mapping_handle,
        ready_event_handle,
        consumed_event_handle,
        slot_count,
        slot_bytes,
    ):
        handles = (mapping_handle, ready_event_handle, consumed_event_handle)
        self.configure_attempts.append((ring_id, *handles))
        try:
            assert all(handle > 0 for handle in handles)
            if self.reject_next_configure:
                self.reject_next_configure = False
                raise ValueError("synthetic ring validation failed")
            self.rings[ring_id] = (kind, slot_count, slot_bytes)
            self.generations[ring_id] = 1
        except Exception:
            self.discard_transferred_handles(
                mapping_handle=mapping_handle,
                ready_event_handle=ready_event_handle,
                consumed_event_handle=consumed_event_handle,
            )
            raise

    def discard_transferred_handles(
        self, *, mapping_handle, ready_event_handle, consumed_event_handle
    ):
        self.discarded_handles.append((mapping_handle, ready_event_handle, consumed_event_handle))

    def release(self, ring_id):
        self.released.append(ring_id)
        self.rings.pop(ring_id)
        self.generations.pop(ring_id)

    def set_generation(self, ring_id, new_generation):
        assert new_generation > self.generations[ring_id]
        self.generations[ring_id] = new_generation

    def publish(self, *, ring_id, session_id, stream_generation, sequence, batch):
        assert self.rings[ring_id][0] == "decoded_rgba"
        assert stream_generation == self.generations[ring_id]
        self.deliveries.append((session_id, stream_generation, sequence, batch))
        return len(self.deliveries)


class Factory:
    def __init__(self) -> None:
        self.handles: dict[int, MemoryAccess] = {}
        self.opened: list[MemoryAccess] = []
        self.closed: list[MemoryAccess] = []
        self._next_handle = 100

    def add(self, access: MemoryAccess) -> int:
        handle = self._next_handle
        self._next_handle += 1
        self.handles[handle] = access
        return handle

    def open(
        self,
        *,
        retained_native_handle,
        archive_bytes,
        cartridge_id,
        archive_sha256,
        integrity_access_receipt,
    ):
        access = self.handles.pop(retained_native_handle)
        self.opened.append(access)
        try:
            assert archive_bytes == 1
            assert integrity_access_receipt == '{"receipt_version":1}'
            return access
        except Exception:
            self.close(access)
            raise

    def close(self, access):
        self.closed.append(access)


class Harness:
    def __init__(self, worker: Protocol2Worker) -> None:
        self.worker = worker
        self.sequence = 1

    def command(self, name: str, payload: Mapping[str, object]) -> tuple[dict[str, object], object]:
        message_id = uuid.uuid4()
        envelope = {
            "protocol": "latentdeck.worker",
            "protocol_version": 2,
            "session_id": str(self.worker.session_id),
            "sequence": self.sequence,
            "message_id": str(message_id),
            "sender_uptime_ns": self.sequence,
            "message": {
                "kind": "command",
                "body": {"name": name, "payload": dict(payload)},
            },
        }
        self.sequence += 1
        reply = self.worker.handle_envelope(envelope)
        result = self.worker.take_command_result(message_id)
        return reply, None if result is None else result.value


def _write_packages(root: Path) -> None:
    (root / "synthetic_codec_package.py").write_text(SYNTHETIC_CODEC, encoding="utf-8")
    (root / "synthetic_deck_package.py").write_text(SYNTHETIC_DECK, encoding="utf-8")


def _worker(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    *,
    deck_entrypoints: tuple[TrustedDeckEntrypoint, ...] | None = None,
) -> tuple[Protocol2Worker, Sink, Harness, Factory]:
    _write_packages(tmp_path)
    monkeypatch.syspath_prepend(str(tmp_path))
    sys.modules.pop("synthetic_codec_package", None)
    sys.modules.pop("synthetic_deck_package", None)
    sink = Sink()
    factory = Factory()
    worker = Protocol2Worker(
        session_id=uuid.uuid4(),
        codec_entrypoints=(
            TrustedCodecEntrypoint(
                pack_id="test.synthetic.codec",
                pack_version="0.2.0",
                adapter_id="synthetic",
                adapter_version="0.2.0",
                entrypoint="synthetic_codec_package:make_adapter",
            ),
        ),
        deck_entrypoints=(
            TrustedDeckEntrypoint(
                deck_id="test.synthetic.deck",
                deck_version="0.2.0",
                operator_id="test.synthetic.average",
                operator_version="0.2.0",
                entrypoint="synthetic_deck_package:process_sources",
            ),
        )
        if deck_entrypoints is None
        else deck_entrypoints,
        cartridge_access_factory=factory,
        ring_transport=sink,
    )
    harness = Harness(worker)
    reply, _ = harness.command(
        "session.configure",
        {
            "selected_protocol_version": 2,
            "app_version": "0.2.0",
            "heartbeat_interval_ms": 1_000,
            "heartbeat_hard_timeout_ms": 10_000,
            "max_frame_bytes": 262_144,
            "max_inflight_batches": 4,
            "requested_capabilities": [capability.value for capability in Capability],
        },
    )
    _assert_ack(reply, "session.configure")
    reply, _ = harness.command(
        "ring.configure",
        {
            "ring_id": str(uuid.uuid4()),
            "kind": "decoded_rgba",
            "mapping_handle": 10,
            "ready_event_handle": 11,
            "consumed_event_handle": 12,
            "slot_count": 4,
            "slot_bytes": 4_096,
        },
    )
    _assert_ack(reply, "ring.configure")
    return worker, sink, harness, factory


def _load_codec(harness: Harness) -> None:
    reply, _ = harness.command(
        "codec.load",
        {
            "pack_id": "test.synthetic.codec",
            "pack_version": "0.2.0",
            "adapter_id": "synthetic",
            "adapter_version": "0.2.0",
            "device": "cpu",
            "device_ordinal": 0,
            "external_assets": [],
        },
    )
    _assert_ack(reply, "codec.load")


def _describe_codec(harness: Harness) -> None:
    reply, _ = harness.command(
        "codec.descriptor",
        {
            "pack_id": "test.synthetic.codec",
            "pack_version": "0.2.0",
            "adapter_id": "synthetic",
        },
    )
    _assert_ack(reply, "codec.descriptor")


def _bind_and_validate(
    worker: Protocol2Worker,
    harness: Harness,
    factory: Factory,
    *,
    slot: int,
    seed: int,
) -> tuple[uuid.UUID, MemoryAccess, dict[str, object]]:
    source_id = uuid.uuid4()
    cartridge_id = uuid.uuid4()
    archive_sha = f"{seed + 32:02x}" * 32
    access = MemoryAccess(cartridge_id, archive_sha, seed)
    reply, _ = harness.command(
        "source.open",
        {
            "source_id": str(source_id),
            "cartridge_id": str(cartridge_id),
            "archive_sha256": archive_sha,
            "archive_bytes": 1,
            "retained_native_handle": factory.add(access),
            "integrity_access_receipt": '{"receipt_version":1}',
        },
    )
    _assert_ack(reply, "source.open")
    reply, _ = harness.command(
        "profile.inspect",
        {
            "source_id": str(source_id),
            "cartridge_id": str(cartridge_id),
            "archive_sha256": archive_sha,
        },
    )
    _assert_ack(reply, "profile.inspect")
    reply, receipt = harness.command(
        "profile.validate",
        {
            "source_id": str(source_id),
            "expected_profile": PROFILE,
            "required_capabilities": ["player", "realtime", "snapshot_capture", "live_capture"],
        },
    )
    _assert_ack(reply, "profile.validate")
    return (
        source_id,
        access,
        {
            "physical_slot": slot,
            "source_id": str(source_id),
            "cartridge_id": str(cartridge_id),
            "archive_sha256": archive_sha,
            "profile_receipt_id": str(receipt.receipt_id),
            "loop_enabled": True,
        },
    )


def _open_source(
    harness: Harness,
    factory: Factory,
    *,
    source_id: uuid.UUID,
    access: MemoryAccess,
    cartridge_id: uuid.UUID | None = None,
    archive_sha256: str | None = None,
) -> dict[str, object]:
    reply, _ = harness.command(
        "source.open",
        {
            "source_id": str(source_id),
            "cartridge_id": str(cartridge_id or access.cartridge_id),
            "archive_sha256": archive_sha256 or access.archive_sha256,
            "archive_bytes": 1,
            "retained_native_handle": factory.add(access),
            "integrity_access_receipt": '{"receipt_version":1}',
        },
    )
    return reply


def _deck_load(
    harness: Harness,
    sources: list[dict[str, object]],
    *,
    runtime: dict[str, object] | None = None,
) -> tuple[uuid.UUID, dict[str, object]]:
    deck_id = uuid.uuid4()
    roles = [{"role": "carrier", "physical_slot": 1}]
    roles.extend(
        {"role": f"donor_{index}", "physical_slot": index} for index in range(2, len(sources) + 1)
    )
    payload = {
        "deck_session_id": str(deck_id),
        "deck_id": "test.synthetic.deck",
        "deck_version": "0.2.0",
        "operator_id": "test.synthetic.average",
        "operator_version": "0.2.0",
        "sources": sources,
        "roles": roles,
        "controls": [{"name": "bias", "value": {"kind": "number", "value": 0.25}}],
        "seed": 123,
        "stream_generation": 1,
    }
    if runtime is not None:
        payload["runtime"] = runtime
    reply, _ = harness.command("deck.load", payload)
    _assert_ack(reply, "deck.load")
    return deck_id, payload


def _assert_ack(reply: Mapping[str, object], command: str) -> None:
    message = reply["message"]
    assert message["kind"] == "ack", message
    assert message["body"]["ack"]["name"] == command


def _assert_no_bulk_data(value: object) -> None:
    if isinstance(value, Mapping):
        for item in value.values():
            _assert_no_bulk_data(item)
    elif isinstance(value, list | tuple):
        for item in value:
            _assert_no_bulk_data(item)
    else:
        assert not isinstance(value, bytes | bytearray | memoryview)
        assert value.__class__.__module__.split(".", maxsplit=1)[0] != "torch"


def test_metadata_discovery_does_not_import_torch(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    _worker_instance, _sink, harness, _factory = _worker(tmp_path, monkeypatch)
    torch_present_before = "torch" in sys.modules

    reply, descriptor = harness.command(
        "codec.descriptor",
        {
            "pack_id": "test.synthetic.codec",
            "pack_version": "0.2.0",
            "adapter_id": "synthetic",
        },
    )

    _assert_ack(reply, "codec.descriptor")
    assert descriptor.adapter_version == "0.2.0"
    synthetic = sys.modules["synthetic_codec_package"]
    assert synthetic.EVENTS.count("adapter.create") == 1
    assert synthetic.TORCH_IMPORTED_DURING_MODULE_LOAD is torch_present_before
    assert ("torch" in sys.modules) is torch_present_before
    _assert_no_bulk_data(reply)

    _describe_codec(harness)
    assert synthetic.EVENTS.count("adapter.create") == 1


def test_optional_raw_import_runtime_is_capability_gated_and_stages_only_inside_core_root(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    _worker_instance, _sink, harness, _factory = _worker(tmp_path, monkeypatch)
    _describe_codec(harness)
    synthetic = sys.modules["synthetic_codec_package"]
    synthetic.EVENTS.clear()
    source = tmp_path / "raw.safetensors"
    source.write_bytes(b"synthetic raw payload")
    import_id = uuid.uuid4()
    reply, preflight = harness.command(
        "raw_import.preflight",
        {
            "import_id": str(import_id),
            "source_path": str(source.resolve()),
            "maximum_source_bytes": source.stat().st_size,
        },
    )
    _assert_ack(reply, "raw_import.preflight")
    assert preflight.import_id == import_id
    assert synthetic.EVENTS == ["raw_import.preflight"]
    assert "codec.load" not in synthetic.EVENTS

    staging_root = tmp_path / "core-staging"
    staging_root.mkdir()
    sentinel = staging_root / "sentinel"
    sentinel.write_text("keep", encoding="utf-8")
    reply, artifact = harness.command(
        "raw_import.stage",
        {
            "import_id": str(import_id),
            "receipt_id": str(preflight.receipt_id),
            "staging_root": str(staging_root.resolve()),
        },
    )
    _assert_ack(reply, "raw_import.stage")
    staged = Path(artifact.staged_payload_path)
    assert staged.parent == staging_root.resolve()
    assert staged.read_bytes() == source.read_bytes()

    reply, _ = harness.command(
        "raw_import.abort",
        {"import_id": str(import_id), "receipt_id": str(preflight.receipt_id)},
    )
    _assert_ack(reply, "raw_import.abort")
    assert not staged.exists()
    assert staging_root.is_dir()
    assert sentinel.read_text(encoding="utf-8") == "keep"


def test_codec_without_raw_import_capability_rejects_with_stable_error(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    _worker_instance, _sink, harness, _factory = _worker(tmp_path, monkeypatch)
    synthetic = importlib.import_module("synthetic_codec_package")
    original = synthetic.Adapter.descriptor

    def without_raw_import(adapter):
        descriptor = original(adapter)
        return synthetic.CodecDescriptor(
            pack_id=descriptor.pack_id,
            pack_version=descriptor.pack_version,
            adapter_id=descriptor.adapter_id,
            adapter_version=descriptor.adapter_version,
            host_api_version=descriptor.host_api_version,
            capabilities=tuple(
                item
                for item in descriptor.capabilities
                if item is not synthetic.Capability.RAW_IMPORT
            ),
            profiles=descriptor.profiles,
        )

    synthetic.Adapter.descriptor = without_raw_import
    _describe_codec(harness)
    source = tmp_path / "raw.safetensors"
    source.write_bytes(b"raw")
    reply, _ = harness.command(
        "raw_import.preflight",
        {
            "import_id": str(uuid.uuid4()),
            "source_path": str(source.resolve()),
            "maximum_source_bytes": source.stat().st_size,
        },
    )
    assert reply["message"]["kind"] == "error"
    assert reply["message"]["body"]["error"]["code"] == "codec.capability_unsupported"
    assert "raw_import.preflight" not in synthetic.EVENTS


def test_raw_import_runtime_rejects_an_adapter_path_outside_core_root_without_deleting_it(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    _worker_instance, _sink, harness, _factory = _worker(tmp_path, monkeypatch)
    _describe_codec(harness)
    synthetic = sys.modules["synthetic_codec_package"]
    source = tmp_path / "raw.safetensors"
    source.write_bytes(b"raw")
    import_id = uuid.uuid4()
    reply, preflight = harness.command(
        "raw_import.preflight",
        {
            "import_id": str(import_id),
            "source_path": str(source.resolve()),
            "maximum_source_bytes": source.stat().st_size,
        },
    )
    _assert_ack(reply, "raw_import.preflight")

    staging_root = tmp_path / "core-staging"
    staging_root.mkdir()
    outside = tmp_path / "adapter-owned-outside.safetensors"
    outside.write_bytes(b"outside")

    def stage_outside(_adapter, request):
        return synthetic.RawImportArtifact(
            receipt_id=request.preflight.receipt_id,
            import_id=request.preflight.import_id,
            staged_payload_path=str(outside.resolve()),
            payload_sha256=hashlib.sha256(outside.read_bytes()).hexdigest(),
            payload_byte_length=outside.stat().st_size,
        )

    monkeypatch.setattr(synthetic.Adapter, "stage_raw_import", stage_outside)
    reply, result = harness.command(
        "raw_import.stage",
        {
            "import_id": str(import_id),
            "receipt_id": str(preflight.receipt_id),
            "staging_root": str(staging_root.resolve()),
        },
    )

    assert result is None
    assert reply["message"]["kind"] == "error"
    assert reply["message"]["body"]["error"]["code"] == "source.invalid"
    assert outside.read_bytes() == b"outside"
    assert not any(staging_root.iterdir())


def test_synthetic_non_h3_player_uses_retained_access_and_out_of_band_rgba(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    worker, sink, harness, factory = _worker(tmp_path, monkeypatch)
    _describe_codec(harness)
    _source_id, access, source = _bind_and_validate(worker, harness, factory, slot=1, seed=7)
    _load_codec(harness)
    player_id = uuid.uuid4()
    reply, _ = harness.command(
        "player.open",
        {
            "player_session_id": str(player_id),
            "source": source,
            "stream_generation": 1,
        },
    )
    _assert_ack(reply, "player.open")
    assert access.reads == [("seed", 0, 1)]

    reply, result = harness.command(
        "player.step",
        {
            "player_session_id": str(player_id),
            "stream_generation": 1,
            "maximum_decoded_frames": 4,
        },
    )
    _assert_ack(reply, "player.step")
    assert isinstance(result, ProcessReceipt)
    assert result.latent_shape == (1, 4, 1, 2, 3)
    assert result.latent_dtype == "float32"
    assert result.latent_device == "cpu"
    assert result.decoded_shape == (1, 4, 6, 4)
    assert len(sink.deliveries) == 1
    assert sink.deliveries[0][3].pixels.nbytes == 1 * 4 * 6 * 4
    _assert_no_bulk_data(reply)
    _assert_no_bulk_data(result)

    reply, _ = harness.command(
        "player.reset",
        {"player_session_id": str(player_id), "new_stream_generation": 2},
    )
    _assert_ack(reply, "player.reset")
    reply, status = harness.command("player.status", {})
    _assert_ack(reply, "player.status")
    assert status.stream_generation == 2
    assert status.playhead == 0
    assert sink.generations[next(iter(sink.rings))] == 2

    reply, result = harness.command(
        "player.step",
        {
            "player_session_id": str(player_id),
            "stream_generation": 2,
            "maximum_decoded_frames": 4,
        },
    )
    _assert_ack(reply, "player.step")
    assert isinstance(result, ProcessReceipt)
    assert sink.deliveries[-1][1:3] == (2, 1)


def test_codec_load_cannot_allocate_before_a_crosschecked_profile_receipt(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    worker, _sink, harness, factory = _worker(tmp_path, monkeypatch)
    _describe_codec(harness)
    synthetic = sys.modules["synthetic_codec_package"]
    synthetic.EVENTS.clear()

    reply, _ = harness.command(
        "codec.load",
        {
            "pack_id": "test.synthetic.codec",
            "pack_version": "0.2.0",
            "adapter_id": "synthetic",
            "adapter_version": "0.2.0",
            "device": "cpu",
            "device_ordinal": 0,
            "external_assets": [],
        },
    )
    assert reply["message"]["kind"] == "error"
    assert reply["message"]["body"]["error"]["code"] == "profile.invalid"
    assert "codec.load" not in synthetic.EVENTS

    _source_id, _access, source = _bind_and_validate(worker, harness, factory, slot=1, seed=9)
    assert "codec.load" not in synthetic.EVENTS

    reply, _ = harness.command(
        "player.open",
        {
            "player_session_id": str(uuid.uuid4()),
            "source": source,
            "stream_generation": 1,
        },
    )
    assert reply["message"]["kind"] == "error"
    assert reply["message"]["body"]["error"]["code"] == "codec.not_loaded"
    assert "source.open" not in synthetic.EVENTS

    _load_codec(harness)
    assert synthetic.EVENTS.count("codec.load") == 1


def test_cpu_codec_load_rejects_nonzero_ordinal_before_adapter_allocation(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    worker, _sink, harness, factory = _worker(tmp_path, monkeypatch)
    _describe_codec(harness)
    _bind_and_validate(worker, harness, factory, slot=1, seed=9)
    synthetic = sys.modules["synthetic_codec_package"]
    synthetic.EVENTS.clear()

    reply, _ = harness.command(
        "codec.load",
        {
            "pack_id": "test.synthetic.codec",
            "pack_version": "0.2.0",
            "adapter_id": "synthetic",
            "adapter_version": "0.2.0",
            "device": "cpu",
            "device_ordinal": 1,
            "external_assets": [],
        },
    )

    assert reply["message"]["kind"] == "error"
    assert reply["message"]["body"]["error"]["code"] == "profile.invalid"
    assert "codec.load" not in synthetic.EVENTS
    assert worker._codec_load_request is None


@pytest.mark.parametrize("source_count", [1, 2, 4, 16])
def test_synthetic_non_h3_deck_processes_bounded_source_arities_exactly(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, source_count: int
) -> None:
    worker, sink, harness, factory = _worker(tmp_path, monkeypatch)
    _describe_codec(harness)
    sources = [
        _bind_and_validate(worker, harness, factory, slot=index, seed=index)[2]
        for index in range(1, source_count + 1)
    ]
    _load_codec(harness)
    deck_id, _load = _deck_load(harness, sources)

    reply, result = harness.command(
        "deck.process",
        {
            "deck_session_id": str(deck_id),
            "deck_revision": 1,
            "stream_generation": 1,
        },
    )

    _assert_ack(reply, "deck.process")
    assert isinstance(result, ProcessReceipt)
    assert result.latent_shape == (1, 4, 1, 2, 3)
    assert result.latent_dtype == "float32"
    assert result.latent_device == "cpu"
    assert result.provenance["source_count"] == source_count
    assert result.provenance["physical_slots"] == list(range(1, source_count + 1))
    assert result.provenance["history_present"] == [False] * source_count
    assert len(sink.deliveries) == 1
    _assert_no_bulk_data(reply)
    _assert_no_bulk_data(result)

    reply, second = harness.command(
        "deck.process",
        {
            "deck_session_id": str(deck_id),
            "deck_revision": 1,
            "stream_generation": 1,
        },
    )
    _assert_ack(reply, "deck.process")
    assert second.provenance["history_present"] == [True] * source_count


def test_deck_transport_and_causal_loop_reset_preserve_independent_physical_playheads(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    worker, sink, harness, factory = _worker(tmp_path, monkeypatch)
    _describe_codec(harness)
    sources = [
        _bind_and_validate(worker, harness, factory, slot=index, seed=index)[2]
        for index in range(1, 3)
    ]
    _load_codec(harness)
    deck_id, _load = _deck_load(harness, sources)

    reply, _ = harness.command(
        "deck.transport.set",
        {
            "deck_session_id": str(deck_id),
            "deck_revision": 1,
            "sources": [
                {"physical_slot": 1, "playing": False, "loop_enabled": True},
                {"physical_slot": 2, "playing": True, "loop_enabled": False},
            ],
        },
    )
    _assert_ack(reply, "deck.transport.set")

    statuses = []
    provenances = []
    for _ in range(4):
        reply, result = harness.command(
            "deck.process",
            {
                "deck_session_id": str(deck_id),
                "deck_revision": 1,
                "stream_generation": 1,
            },
        )
        _assert_ack(reply, "deck.process")
        statuses.append(reply["message"]["body"]["ack"]["payload"]["status"])
        provenances.append(result.provenance)

    assert [
        (status["playheads"][0]["latent_slot"], status["playheads"][1]["latent_slot"])
        for status in statuses
    ] == [(0, 1), (0, 2), (0, 3), (0, 3)]
    assert statuses[-1]["source_transport"] == [
        {"physical_slot": 1, "playing": False, "loop_enabled": True},
        {"physical_slot": 2, "playing": False, "loop_enabled": False},
    ]
    assert provenances[0]["history_present"] == [False, False]
    assert provenances[1]["history_present"] == [True, True]

    reply, _ = harness.command(
        "deck.transport.set",
        {
            "deck_session_id": str(deck_id),
            "deck_revision": 1,
            "sources": [
                {"physical_slot": 1, "playing": True, "loop_enabled": True},
                {"physical_slot": 2, "playing": False, "loop_enabled": False},
            ],
        },
    )
    _assert_ack(reply, "deck.transport.set")
    for _ in range(4):
        reply, _result = harness.command(
            "deck.process",
            {
                "deck_session_id": str(deck_id),
                "deck_revision": 1,
                "stream_generation": 1,
            },
        )
        _assert_ack(reply, "deck.process")
    status = reply["message"]["body"]["ack"]["payload"]["status"]
    assert status["playheads"] == [
        {"physical_slot": 1, "latent_slot": 0, "loop_enabled": True, "end_of_stream": False},
        {"physical_slot": 2, "latent_slot": 3, "loop_enabled": False, "end_of_stream": True},
    ]

    synthetic = sys.modules["synthetic_codec_package"]
    synthetic.EVENTS.clear()
    ring_id = next(iter(sink.rings))
    reply, _ = harness.command(
        "deck.reset",
        {
            "deck_session_id": str(deck_id),
            "deck_revision": 1,
            "new_stream_generation": 2,
            "preserve_playheads": True,
        },
    )
    _assert_ack(reply, "deck.reset")
    reset_status = reply["message"]["body"]["ack"]["payload"]
    assert reset_status["stream_generation"] == 2
    assert reset_status["stream_sequence"] == 0
    assert reset_status["playheads"] == status["playheads"]
    assert reset_status["source_transport"] == status["source_transport"]
    assert sink.generations[ring_id] == 2
    assert synthetic.EVENTS == ["decoder.reset"]

    reply, result = harness.command(
        "deck.process",
        {
            "deck_session_id": str(deck_id),
            "deck_revision": 1,
            "stream_generation": 2,
        },
    )
    _assert_ack(reply, "deck.process")
    assert result.provenance["history_present"] == [False, False]


def test_empty_registry_imports_only_the_host_bound_temp_package(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    python_root = tmp_path / "installed-external-deck" / "python"
    python_root.mkdir(parents=True)
    module_name = "installed_external_deck"
    (python_root / f"{module_name}.py").write_text(SYNTHETIC_DECK, encoding="utf-8")
    sys.modules.pop(module_name, None)

    worker, _sink, harness, factory = _worker(tmp_path, monkeypatch, deck_entrypoints=())
    _describe_codec(harness)
    sources = [
        _bind_and_validate(worker, harness, factory, slot=index, seed=index)[2]
        for index in range(1, 3)
    ]
    _load_codec(harness)
    deck_id, _load = _deck_load(
        harness,
        sources,
        runtime={
            "deck_id": "test.synthetic.deck",
            "deck_version": "0.2.0",
            "operator_id": "test.synthetic.average",
            "operator_version": "0.2.0",
            "python_root": str(python_root.resolve()),
            "entrypoint": f"{module_name}:process_sources",
            "package_manifest_sha256": "a" * 64,
            "integrity_catalog_sha256": "b" * 64,
        },
    )

    reply, result = harness.command(
        "deck.process",
        {
            "deck_session_id": str(deck_id),
            "deck_revision": 1,
            "stream_generation": 1,
        },
    )
    _assert_ack(reply, "deck.process")
    assert result.provenance["source_count"] == 2
    imported = Path(sys.modules[module_name].__file__).resolve()
    assert imported.is_relative_to(python_root.resolve())


def test_bound_runtime_failure_never_falls_back_to_legacy_registry(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    worker, _sink, harness, factory = _worker(tmp_path, monkeypatch)
    _describe_codec(harness)
    sources = [
        _bind_and_validate(worker, harness, factory, slot=index, seed=index)[2]
        for index in range(1, 3)
    ]
    _load_codec(harness)
    payload_runtime = {
        "deck_id": "test.synthetic.deck",
        "deck_version": "0.2.0",
        "operator_id": "test.synthetic.average",
        "operator_version": "0.2.0",
        "python_root": str(tmp_path.resolve()),
        "entrypoint": "missing_bound_deck:process_sources",
        "package_manifest_sha256": "a" * 64,
        "integrity_catalog_sha256": "b" * 64,
    }
    deck_id = uuid.uuid4()
    payload = {
        "deck_session_id": str(deck_id),
        "deck_id": "test.synthetic.deck",
        "deck_version": "0.2.0",
        "operator_id": "test.synthetic.average",
        "operator_version": "0.2.0",
        "runtime": payload_runtime,
        "sources": sources,
        "roles": [
            {"role": "carrier", "physical_slot": 1},
            {"role": "donor_2", "physical_slot": 2},
        ],
        "controls": [],
        "seed": 1,
        "stream_generation": 1,
    }
    reply, _ = harness.command("deck.load", payload)
    assert reply["message"]["kind"] == "error"
    assert reply["message"]["body"]["error"]["code"] == "deck.invalid"
    assert "synthetic_deck_package" not in sys.modules


def test_snapshot_and_live_capture_receive_post_operator_latent_before_decode(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    worker, _sink, harness, factory = _worker(tmp_path, monkeypatch)
    _describe_codec(harness)
    sources = [
        _bind_and_validate(worker, harness, factory, slot=index, seed=index)[2]
        for index in range(1, 3)
    ]
    _load_codec(harness)
    deck_id, _load = _deck_load(harness, sources)
    synthetic = sys.modules["synthetic_codec_package"]

    snapshot_id = uuid.uuid4()
    snapshot_root = tmp_path / "snapshot-staging"
    snapshot_root.mkdir()
    reply, _ = harness.command(
        "capture.start",
        {
            "deck_session_id": str(deck_id),
            "deck_revision": 1,
            "capture_id": str(snapshot_id),
            "mode": "snapshot",
            "staging_root": str(snapshot_root.resolve()),
            "maximum_latent_slots": 8,
            "maximum_visual_bytes": 1_000_000,
            "maximum_reset_events": 4,
        },
    )
    _assert_ack(reply, "capture.start")
    synthetic.EVENTS.clear()
    reply, _ = harness.command(
        "deck.process",
        {
            "deck_session_id": str(deck_id),
            "deck_revision": 1,
            "stream_generation": 1,
        },
    )
    _assert_ack(reply, "deck.process")
    assert synthetic.EVENTS.index("capture.append") < synthetic.EVENTS.index("decode")
    assert reply["message"]["body"]["ack"]["payload"]["status"]["capture_state"] == ("capturing")
    reply, _ = harness.command(
        "deck.process",
        {
            "deck_session_id": str(deck_id),
            "deck_revision": 1,
            "stream_generation": 1,
        },
    )
    _assert_ack(reply, "deck.process")
    reply, snapshot_session = harness.command(
        "capture.status",
        {
            "deck_session_id": str(deck_id),
            "deck_revision": 1,
            "capture_id": str(snapshot_id),
        },
    )
    _assert_ack(reply, "capture.status")
    snapshot = snapshot_session.payload
    assert snapshot is not None
    assert snapshot.capture_id == snapshot_id
    assert snapshot.latent_slots == 2
    assert snapshot.decoded_frame_count == 2
    assert Path(snapshot.payload_path).is_file()
    assert Path(snapshot.payload_path).is_relative_to(snapshot_root.resolve())

    live_id = uuid.uuid4()
    live_root = tmp_path / "live-staging"
    live_root.mkdir()
    reply, _ = harness.command(
        "capture.start",
        {
            "deck_session_id": str(deck_id),
            "deck_revision": 1,
            "capture_id": str(live_id),
            "mode": "live_capture",
            "staging_root": str(live_root.resolve()),
            "maximum_latent_slots": 8,
            "maximum_visual_bytes": 1_000_000,
            "maximum_reset_events": 4,
        },
    )
    _assert_ack(reply, "capture.start")
    for _ in range(2):
        reply, _result = harness.command(
            "deck.process",
            {
                "deck_session_id": str(deck_id),
                "deck_revision": 1,
                "stream_generation": 1,
            },
        )
        _assert_ack(reply, "deck.process")
    reply, _ = harness.command(
        "deck.reset",
        {
            "deck_session_id": str(deck_id),
            "deck_revision": 1,
            "new_stream_generation": 2,
            "preserve_playheads": False,
        },
    )
    _assert_ack(reply, "deck.reset")
    reply, _result = harness.command(
        "deck.process",
        {
            "deck_session_id": str(deck_id),
            "deck_revision": 1,
            "stream_generation": 2,
        },
    )
    _assert_ack(reply, "deck.process")
    reply, live_session = harness.command(
        "capture.stop",
        {
            "deck_session_id": str(deck_id),
            "deck_revision": 1,
            "capture_id": str(live_id),
        },
    )
    _assert_ack(reply, "capture.stop")
    assert live_session.payload is None
    assert live_session.state.value == "finalizing"
    for _ in range(4):
        reply, _result = harness.command(
            "deck.process",
            {
                "deck_session_id": str(deck_id),
                "deck_revision": 1,
                "stream_generation": 2,
            },
        )
        _assert_ack(reply, "deck.process")
    reply, live_session = harness.command(
        "capture.status",
        {
            "deck_session_id": str(deck_id),
            "deck_revision": 1,
            "capture_id": str(live_id),
        },
    )
    _assert_ack(reply, "capture.status")
    live = live_session.payload
    assert live is not None
    assert live.capture_id == live_id
    assert live.latent_slots == 7
    assert live.decoded_frame_count == 7
    assert Path(live.payload_path).is_file()
    assert Path(live.payload_path).is_relative_to(live_root.resolve())


@pytest.mark.parametrize("terminal_name", ["COMPLETED", "ABORTED", "FAULTED"])
def test_terminal_capture_is_cleared_by_deck_reset_and_capture_can_restart(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, terminal_name: str
) -> None:
    worker, _sink, harness, factory = _worker(tmp_path, monkeypatch)
    _describe_codec(harness)
    source = _bind_and_validate(worker, harness, factory, slot=1, seed=1)[2]
    _load_codec(harness)
    deck_id, _load = _deck_load(harness, [source])

    first_capture_id = uuid.uuid4()
    first_root = tmp_path / "first-capture"
    first_root.mkdir()
    reply, capture = harness.command(
        "capture.start",
        {
            "deck_session_id": str(deck_id),
            "deck_revision": 1,
            "capture_id": str(first_capture_id),
            "mode": "live_capture",
            "staging_root": str(first_root.resolve()),
            "maximum_latent_slots": 8,
            "maximum_visual_bytes": 1_000_000,
            "maximum_reset_events": 4,
        },
    )
    _assert_ack(reply, "capture.start")
    terminal = getattr(type(capture.state), terminal_name)
    capture.state = terminal
    worker._capture_state = terminal

    reply, _ = harness.command(
        "deck.reset",
        {
            "deck_session_id": str(deck_id),
            "deck_revision": 1,
            "new_stream_generation": 2,
            "preserve_playheads": True,
        },
    )
    _assert_ack(reply, "deck.reset")
    reset_status = reply["message"]["body"]["ack"]["payload"]
    assert reset_status["capture_state"] == "idle"
    assert worker._deck is not None
    assert worker._deck.capture is None
    assert worker.status()["capture"] == "idle"

    next_capture_id = uuid.uuid4()
    next_root = tmp_path / "next-capture"
    next_root.mkdir()
    reply, _ = harness.command(
        "capture.start",
        {
            "deck_session_id": str(deck_id),
            "deck_revision": 1,
            "capture_id": str(next_capture_id),
            "mode": "snapshot",
            "staging_root": str(next_root.resolve()),
            "maximum_latent_slots": 8,
            "maximum_visual_bytes": 1_000_000,
            "maximum_reset_events": 4,
        },
    )
    _assert_ack(reply, "capture.start")


@pytest.mark.parametrize("active_name", ["CAPTURING", "FINALIZING"])
def test_active_capture_survives_causal_deck_reset_with_reset_event(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, active_name: str
) -> None:
    worker, _sink, harness, factory = _worker(tmp_path, monkeypatch)
    _describe_codec(harness)
    source = _bind_and_validate(worker, harness, factory, slot=1, seed=1)[2]
    _load_codec(harness)
    deck_id, _load = _deck_load(harness, [source])

    capture_id = uuid.uuid4()
    capture_root = tmp_path / "active-capture"
    capture_root.mkdir()
    reply, capture = harness.command(
        "capture.start",
        {
            "deck_session_id": str(deck_id),
            "deck_revision": 1,
            "capture_id": str(capture_id),
            "mode": "live_capture",
            "staging_root": str(capture_root.resolve()),
            "maximum_latent_slots": 8,
            "maximum_visual_bytes": 1_000_000,
            "maximum_reset_events": 4,
        },
    )
    _assert_ack(reply, "capture.start")
    active = getattr(type(capture.state), active_name)
    capture.state = active
    worker._capture_state = active

    reply, _ = harness.command(
        "deck.reset",
        {
            "deck_session_id": str(deck_id),
            "deck_revision": 1,
            "new_stream_generation": 2,
            "preserve_playheads": True,
        },
    )
    _assert_ack(reply, "deck.reset")
    reset_status = reply["message"]["body"]["ack"]["payload"]
    assert reset_status["capture_state"] == active.value
    reply, capture = harness.command(
        "capture.status",
        {
            "deck_session_id": str(deck_id),
            "deck_revision": 1,
            "capture_id": str(capture_id),
        },
    )
    _assert_ack(reply, "capture.status")
    assert capture.reset_events == 1
    assert capture.pending_reset_event == {"generation": 2, "sequence": 0}


def test_capture_finish_failure_aborts_owned_payload_but_preserves_host_root(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    worker, _sink, harness, factory = _worker(tmp_path, monkeypatch)
    _describe_codec(harness)
    sources = [
        _bind_and_validate(worker, harness, factory, slot=index, seed=index)[2]
        for index in range(1, 3)
    ]
    _load_codec(harness)
    deck_id, _load = _deck_load(harness, sources)
    capture_id = uuid.uuid4()
    staging_root = tmp_path / "fatal-capture-staging"
    staging_root.mkdir()
    sentinel = staging_root / "host-owned.sentinel"
    sentinel.write_text("preserve", encoding="utf-8")
    reply, _ = harness.command(
        "capture.start",
        {
            "deck_session_id": str(deck_id),
            "deck_revision": 1,
            "capture_id": str(capture_id),
            "mode": "live_capture",
            "staging_root": str(staging_root.resolve()),
            "maximum_latent_slots": 8,
            "maximum_visual_bytes": 1_000_000,
            "maximum_reset_events": 4,
        },
    )
    _assert_ack(reply, "capture.start")
    reply, _ = harness.command(
        "deck.process",
        {
            "deck_session_id": str(deck_id),
            "deck_revision": 1,
            "stream_generation": 1,
        },
    )
    _assert_ack(reply, "deck.process")
    synthetic = sys.modules["synthetic_codec_package"]
    monkeypatch.setattr(synthetic, "CAPTURE_FINISH_ERROR", True)

    reply, _ = harness.command(
        "capture.stop",
        {
            "deck_session_id": str(deck_id),
            "deck_revision": 1,
            "capture_id": str(capture_id),
        },
    )
    assert reply["message"]["kind"] == "error"
    assert reply["message"]["body"]["error"]["code"] == "capture.invalid_state"
    assert "capture.abort" in synthetic.EVENTS
    assert sentinel.read_text(encoding="utf-8") == "preserve"
    assert sorted(path.name for path in staging_root.iterdir()) == [sentinel.name]


def test_p2_errors_are_bounded_and_protocol_one_never_falls_back(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    worker, _sink, harness, factory = _worker(tmp_path, monkeypatch)
    _describe_codec(harness)
    _bind_and_validate(worker, harness, factory, slot=1, seed=11)
    reply, _ = harness.command(
        "codec.load",
        {
            "pack_id": "test.synthetic.codec",
            "pack_version": "0.2.0",
            "adapter_id": "synthetic",
            "adapter_version": "9.9.9",
            "device": "cpu",
            "device_ordinal": 0,
            "external_assets": [],
        },
    )
    assert reply["message"]["kind"] == "error"
    error = reply["message"]["body"]["error"]
    assert error["code"] == "codec.untrusted"
    assert len(error["message"].encode()) <= 4_096
    assert len(error["details"]) <= 16
    _assert_no_bulk_data(reply)

    p1 = {
        "protocol": "latentdeck.worker",
        "protocol_version": 1,
        "session_id": str(worker.session_id),
        "sequence": harness.sequence,
        "message_id": str(uuid.uuid4()),
        "sender_uptime_ns": 1,
        "message": {
            "kind": "command",
            "body": {"name": "session.status", "payload": {}},
        },
    }
    with pytest.raises(ProtocolError, match="unsupported"):
        worker.handle_envelope(p1)


def test_source_ring_and_metrics_lifecycle_remains_control_only(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    worker, sink, harness, factory = _worker(tmp_path, monkeypatch)
    _describe_codec(harness)
    source_id, access, _source = _bind_and_validate(worker, harness, factory, slot=1, seed=3)
    _load_codec(harness)

    reply, metrics = harness.command("metrics.get", {})
    _assert_ack(reply, "metrics.get")
    assert metrics["commands_total"] >= 7
    assert metrics["commands_failed_total"] == 0
    _assert_no_bulk_data(reply)

    reply, _ = harness.command("source.close", {"source_id": str(source_id)})
    _assert_ack(reply, "source.close")
    assert factory.closed == [access]

    ring_id = next(iter(sink.rings))
    reply, _ = harness.command("ring.release", {"ring_id": str(ring_id)})
    _assert_ack(reply, "ring.release")
    assert sink.rings == {}
    _assert_no_bulk_data(reply)


def test_source_open_consumes_and_closes_duplicate_target_handle(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    _worker_runtime, _sink, harness, factory = _worker(tmp_path, monkeypatch)
    source_id = uuid.uuid4()
    cartridge_id = uuid.uuid4()
    archive_sha256 = "31" * 32
    original = MemoryAccess(cartridge_id, archive_sha256, 1)
    _assert_ack(
        _open_source(
            harness,
            factory,
            source_id=source_id,
            access=original,
        ),
        "source.open",
    )

    duplicate = MemoryAccess(cartridge_id, archive_sha256, 2)
    reply = _open_source(
        harness,
        factory,
        source_id=source_id,
        access=duplicate,
    )

    assert reply["message"]["kind"] == "error"
    assert reply["message"]["body"]["error"]["code"] == "source.invalid"
    assert factory.opened[-1] is duplicate
    assert duplicate in factory.closed
    assert original not in factory.closed
    assert factory.handles == {}


@pytest.mark.parametrize("failure", ["validation", "identity"])
def test_source_open_closes_target_handle_on_pre_registration_failure(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, failure: str
) -> None:
    _worker_runtime, _sink, harness, factory = _worker(tmp_path, monkeypatch)
    source_id = uuid.uuid4()
    access = MemoryAccess(uuid.uuid4(), "41" * 32, 3)
    cartridge_id = access.cartridge_id
    if failure == "validation":
        access._manifest = []  # type: ignore[assignment]
    else:
        cartridge_id = uuid.uuid4()

    reply = _open_source(
        harness,
        factory,
        source_id=source_id,
        access=access,
        cartridge_id=cartridge_id,
    )

    assert reply["message"]["kind"] == "error"
    assert reply["message"]["body"]["error"]["code"] == "source.invalid"
    assert factory.opened == [access]
    assert factory.closed == [access]
    assert factory.handles == {}


@pytest.mark.parametrize("duplicate", ["id", "kind"])
def test_ring_configure_discards_target_handles_before_runtime_rejection(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, duplicate: str
) -> None:
    _worker_runtime, sink, harness, _factory = _worker(tmp_path, monkeypatch)
    existing_ring_id = next(iter(sink.rings))
    ring_id = existing_ring_id if duplicate == "id" else uuid.uuid4()
    transferred = (20, 21, 22)

    reply, _ = harness.command(
        "ring.configure",
        {
            "ring_id": str(ring_id),
            "kind": "decoded_rgba",
            "mapping_handle": transferred[0],
            "ready_event_handle": transferred[1],
            "consumed_event_handle": transferred[2],
            "slot_count": 4,
            "slot_bytes": 4_096,
        },
    )

    assert reply["message"]["kind"] == "error"
    assert sink.discarded_handles == [transferred]
    assert sink.configure_attempts == [(existing_ring_id, 10, 11, 12)]
    assert sink.released == []
    assert existing_ring_id in sink.rings


def test_ring_transport_reclaims_all_target_handles_on_configure_failure(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    _worker_runtime, sink, harness, _factory = _worker(tmp_path, monkeypatch)
    sink.reject_next_configure = True
    ring_id = uuid.uuid4()
    transferred = (30, 31, 32)

    reply, _ = harness.command(
        "ring.configure",
        {
            "ring_id": str(ring_id),
            "kind": "latent_tensor",
            "mapping_handle": transferred[0],
            "ready_event_handle": transferred[1],
            "consumed_event_handle": transferred[2],
            "slot_count": 4,
            "slot_bytes": 4_096,
        },
    )

    assert reply["message"]["kind"] == "error"
    assert sink.configure_attempts[-1] == (ring_id, *transferred)
    assert sink.discarded_handles == [transferred]
    assert ring_id not in sink.rings
