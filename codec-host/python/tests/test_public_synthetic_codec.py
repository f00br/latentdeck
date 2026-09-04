from __future__ import annotations

import importlib.util
import io
import os
import shutil
import struct
import subprocess
import sys
import uuid
from collections.abc import Mapping
from dataclasses import dataclass
from pathlib import Path

import msgpack
import pytest
from latentdeck_codec_host.runtime_v2 import (
    Protocol2Worker,
    StreamConnection,
    TrustedCodecEntrypoint,
    TrustedDeckEntrypoint,
    run_protocol2_service,
)
from latentdeck_codec_sdk import (
    Capability,
    CaptureRequest,
    CodecLoadRequest,
    CodecSdkError,
    TensorAccessDescriptor,
    WorkerStreamValidator,
    decode_messagepack,
    encode_messagepack,
)

REPOSITORY_ROOT = Path(__file__).resolve().parents[3]
ADAPTER_PATH = (
    REPOSITORY_ROOT / "examples/extensions/synthetic-codec/runtime/adapter.py"
)
DECK_OPERATOR_PATH = (
    REPOSITORY_ROOT
    / "examples/extensions/starter-deck/python/latentdeck_example_identity_deck/operator.py"
)
SERVICE_CHILD_ENV = "LATENTDECK_SYNTHETIC_SERVICE_CHILD"


class _Cartridge:
    def __init__(self) -> None:
        self.cartridge_id = uuid.uuid4()
        self.archive_sha256 = "ab" * 32
        self.manifest = {"payload_sha256": "cd" * 32}

    def tensor_descriptor(self, name: str) -> TensorAccessDescriptor:
        if name != "seed":
            raise KeyError(name)
        return TensorAccessDescriptor("seed", "F16", (1,), 2)

    def read_tensor_range(self, name: str, offset: int, byte_length: int) -> memoryview:
        assert (name, offset, byte_length) == ("seed", 0, 1)
        return memoryview(b"\x03")


class _Factory:
    def __init__(self, cartridge: _Cartridge) -> None:
        self.cartridge = cartridge
        self.closed = []

    def open(self, **arguments):
        assert arguments["retained_native_handle"] == 101
        assert arguments["archive_bytes"] == 1
        assert arguments["cartridge_id"] == self.cartridge.cartridge_id
        assert arguments["archive_sha256"] == self.cartridge.archive_sha256
        return self.cartridge

    def close(self, access) -> None:
        self.closed.append(access)


class _Sink:
    def __init__(self) -> None:
        self.generations: dict[uuid.UUID, int] = {}
        self.generation_updates: list[tuple[uuid.UUID, int]] = []
        self.deliveries = []

    def configure(self, *, ring_id, **arguments) -> None:
        assert arguments["kind"] == "decoded_rgba"
        self.generations[ring_id] = 1

    def discard_transferred_handles(self, **_arguments) -> None:
        pass

    def release(self, ring_id) -> None:
        self.generations.pop(ring_id, None)

    def set_generation(self, ring_id, new_generation) -> None:
        self.generations[ring_id] = new_generation
        self.generation_updates.append((ring_id, new_generation))

    def publish(self, **arguments) -> int:
        self.deliveries.append(arguments)
        return len(self.deliveries)


class _Harness:
    def __init__(self, worker: Protocol2Worker) -> None:
        self.worker = worker
        self.sequence = 1

    def command(self, name: str, payload: Mapping[str, object]):
        message_id = uuid.uuid4()
        reply = self.worker.handle_envelope(
            {
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
        )
        self.sequence += 1
        result = self.worker.take_command_result(message_id)
        return reply, None if result is None else result.value


def _assert_ack(reply, name: str) -> None:
    assert reply["message"]["kind"] == "ack", reply
    assert reply["message"]["body"]["ack"]["name"] == name


def _framed(value: object) -> bytes:
    encoded = encode_messagepack(value)
    return struct.pack("<I", len(encoded)) + encoded


def _service_bootstrap(session_id: uuid.UUID, token: str) -> bytes:
    encoded = msgpack.packb(
        {
            "bootstrap_version": 2,
            "protocol_version": 2,
            "session_id": str(session_id),
            "pipe_name": rf"\\.\pipe\latentdeck-worker-{session_id}",
            "auth_token": token,
        },
        use_bin_type=True,
        strict_types=True,
    )
    return struct.pack("<I", len(encoded)) + encoded


def _service_command(
    session_id: uuid.UUID,
    sequence: int,
    name: str,
    payload: Mapping[str, object],
) -> bytes:
    return _framed(
        {
            "protocol": "latentdeck.worker",
            "protocol_version": 2,
            "session_id": str(session_id),
            "sequence": sequence,
            "message_id": str(uuid.UUID(int=sequence + 1000)),
            "sender_uptime_ns": sequence,
            "message": {
                "kind": "command",
                "body": {"name": name, "payload": dict(payload)},
            },
        }
    )


def _service_frames(encoded: bytes) -> list[dict[str, object]]:
    stream = io.BytesIO(encoded)
    frames: list[dict[str, object]] = []
    while prefix := stream.read(4):
        assert len(prefix) == 4
        size = struct.unpack("<I", prefix)[0]
        frames.append(decode_messagepack(stream.read(size)))
    return frames


@dataclass
class _Connector:
    session_id: uuid.UUID
    reader: io.BytesIO
    writer: io.BytesIO

    def connect(self, pipe_name: str) -> StreamConnection:
        assert pipe_name == rf"\\.\pipe\latentdeck-worker-{self.session_id}"
        return StreamConnection(self.reader, self.writer)


def _load_example():
    module_name = "latentdeck_public_synthetic_codec"
    specification = importlib.util.spec_from_file_location(module_name, ADAPTER_PATH)
    assert specification is not None and specification.loader is not None
    module = importlib.util.module_from_spec(specification)
    sys.modules[module_name] = module
    previous = sys.dont_write_bytecode
    sys.dont_write_bytecode = True
    try:
        specification.loader.exec_module(module)
    finally:
        sys.dont_write_bytecode = previous
    return module


def _load_deck_example():
    module_name = "latentdeck_public_identity_deck"
    specification = importlib.util.spec_from_file_location(module_name, DECK_OPERATOR_PATH)
    assert specification is not None and specification.loader is not None
    module = importlib.util.module_from_spec(specification)
    sys.modules[module_name] = module
    previous = sys.dont_write_bytecode
    sys.dont_write_bytecode = True
    try:
        specification.loader.exec_module(module)
    finally:
        sys.dont_write_bytecode = previous
    return module


def test_public_synthetic_codec_exercises_cpu_decode_capture_abort_and_reset(
    tmp_path: Path,
) -> None:
    module = _load_example()
    adapter = module.make_adapter()
    descriptor = adapter.descriptor()
    cartridge = _Cartridge()
    inspection = adapter.inspect(cartridge)
    receipt = adapter.validate_profile(cartridge, inspection)
    adapter.load(
        CodecLoadRequest(
            descriptor=descriptor,
            assets=(),
            device="cpu",
            device_ordinal=0,
        )
    )
    source = adapter.open_source(cartridge, receipt, uuid.uuid4())
    tensor = adapter.read_slot(source, 2)
    assert tuple(tensor.shape) == (1, 4, 1, 2, 3)
    decoded = adapter.decode_slot(tensor, 1)
    assert (decoded.batch, decoded.height, decoded.width, decoded.pixels.nbytes) == (1, 4, 6, 96)
    adapter.reset_decoder(2)

    capture = adapter.create_capture_writer(
        CaptureRequest(
            capture_id=uuid.uuid4(),
            mode="snapshot",
            staging_root=str(tmp_path.resolve()),
            maximum_latent_slots=8,
            maximum_visual_bytes=1_000_000,
        )
    )
    capture.append(tensor)
    payload = capture.finish()
    assert Path(payload.payload_path).is_file()
    assert payload.latent_slots == 1
    preserved = capture.output_path.read_bytes()
    collision = adapter.create_capture_writer(capture.request)
    collision.append(tensor)
    with pytest.raises(CodecSdkError) as failure:
        collision.finish()
    assert failure.value.code == "capture.target_exists"
    assert capture.output_path.read_bytes() == preserved

    aborted = adapter.create_capture_writer(
        CaptureRequest(
            capture_id=uuid.uuid4(),
            mode="live_capture",
            staging_root=str(tmp_path.resolve()),
            maximum_latent_slots=8,
            maximum_visual_bytes=1_000_000,
        )
    )
    aborted.append(tensor)
    aborted.abort()
    assert not aborted.output_path.exists()
    source.close()
    assert source.closed


def test_public_synthetic_codec_enters_through_authenticated_protocol_two(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    module = _load_example()
    _load_deck_example()
    aborted: list[uuid.UUID] = []
    original_abort = module.SyntheticCaptureWriter.abort

    def observe_abort(writer) -> None:
        aborted.append(writer.request.capture_id)
        original_abort(writer)

    monkeypatch.setattr(module.SyntheticCaptureWriter, "abort", observe_abort)
    cartridge = _Cartridge()
    session_id = uuid.uuid4()
    sink = _Sink()
    worker = Protocol2Worker(
        session_id=session_id,
        codec_entrypoints=(
            TrustedCodecEntrypoint(
                pack_id=module.PACK_ID,
                pack_version=module.PACK_VERSION,
                adapter_id=module.ADAPTER_ID,
                adapter_version=module.ADAPTER_VERSION,
                entrypoint="latentdeck_public_synthetic_codec:make_adapter",
            ),
        ),
        deck_entrypoints=(
            TrustedDeckEntrypoint(
                deck_id="org.example.latentdeck.identity",
                deck_version="0.1.0",
                operator_id="org.example.latentdeck.identity",
                operator_version="0.1.0",
                entrypoint="latentdeck_public_identity_deck:process_sources_host",
            ),
        ),
        cartridge_access_factory=_Factory(cartridge),
        ring_transport=sink,
    )
    harness = _Harness(worker)
    reply, _ = harness.command(
        "session.configure",
        {
            "selected_protocol_version": 2,
            "app_version": "0.1.0",
            "heartbeat_interval_ms": 1000,
            "heartbeat_hard_timeout_ms": 10000,
            "max_frame_bytes": 262144,
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
            "slot_bytes": 4096,
        },
    )
    _assert_ack(reply, "ring.configure")
    reply, descriptor = harness.command(
        "codec.descriptor",
        {
            "pack_id": module.PACK_ID,
            "pack_version": module.PACK_VERSION,
            "adapter_id": module.ADAPTER_ID,
        },
    )
    _assert_ack(reply, "codec.descriptor")
    assert descriptor == module.make_adapter().descriptor()

    source_id = uuid.uuid4()
    reply, _ = harness.command(
        "source.open",
        {
            "source_id": str(source_id),
            "cartridge_id": str(cartridge.cartridge_id),
            "archive_sha256": cartridge.archive_sha256,
            "archive_bytes": 1,
            "retained_native_handle": 101,
            "integrity_access_receipt": '{"receipt_version":1}',
        },
    )
    _assert_ack(reply, "source.open")
    reply, _ = harness.command(
        "profile.inspect",
        {
            "source_id": str(source_id),
            "cartridge_id": str(cartridge.cartridge_id),
            "archive_sha256": cartridge.archive_sha256,
        },
    )
    _assert_ack(reply, "profile.inspect")
    reply, receipt = harness.command(
        "profile.validate",
        {
            "source_id": str(source_id),
            "expected_profile": {
                "codec_family": "synthetic",
                "profile": "example_latent",
                "profile_version": "0.1.0",
            },
            "required_capabilities": [
                "player",
                "realtime",
                "resample",
                "snapshot_capture",
                "live_capture",
            ],
        },
    )
    _assert_ack(reply, "profile.validate")
    assert receipt.profile_key == module.PROFILE
    reply, _ = harness.command(
        "codec.load",
        {
            "pack_id": module.PACK_ID,
            "pack_version": module.PACK_VERSION,
            "adapter_id": module.ADAPTER_ID,
            "adapter_version": module.ADAPTER_VERSION,
            "device": "cpu",
            "device_ordinal": 0,
            "external_assets": [],
        },
    )
    _assert_ack(reply, "codec.load")

    source = {
        "physical_slot": 1,
        "source_id": str(source_id),
        "cartridge_id": str(cartridge.cartridge_id),
        "archive_sha256": cartridge.archive_sha256,
        "profile_receipt_id": str(receipt.receipt_id),
        "loop_enabled": True,
    }
    deck_session_id = uuid.uuid4()
    reply, _ = harness.command(
        "deck.load",
        {
            "deck_session_id": str(deck_session_id),
            "deck_id": "org.example.latentdeck.identity",
            "deck_version": "0.1.0",
            "operator_id": "org.example.latentdeck.identity",
            "operator_version": "0.1.0",
            "sources": [source],
            "roles": [{"role": "source", "physical_slot": 1}],
            "controls": [
                {"name": "mode", "value": {"kind": "text", "value": "identity"}}
            ],
            "seed": 17,
            "stream_generation": 1,
        },
    )
    _assert_ack(reply, "deck.load")

    snapshot_id = uuid.uuid4()
    snapshot_root = tmp_path / "snapshot"
    snapshot_root.mkdir()
    reply, _ = harness.command(
        "capture.start",
        {
            "deck_session_id": str(deck_session_id),
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
    reply, result = harness.command(
        "deck.process",
        {
            "deck_session_id": str(deck_session_id),
            "deck_revision": 1,
            "stream_generation": 1,
        },
    )
    _assert_ack(reply, "deck.process")
    assert result.decoded_shape == (1, 4, 6, 4)
    assert len(sink.deliveries) == 1
    reply, snapshot = harness.command(
        "capture.status",
        {
            "deck_session_id": str(deck_session_id),
            "deck_revision": 1,
            "capture_id": str(snapshot_id),
        },
    )
    _assert_ack(reply, "capture.status")
    assert snapshot.state.value == "completed"
    assert snapshot.payload.capture_id == snapshot_id
    assert Path(snapshot.payload.payload_path).is_file()

    reply, _ = harness.command(
        "deck.reset",
        {
            "deck_session_id": str(deck_session_id),
            "deck_revision": 1,
            "new_stream_generation": 2,
            "preserve_playheads": False,
        },
    )
    _assert_ack(reply, "deck.reset")
    reply, replay = harness.command(
        "deck.process",
        {
            "deck_session_id": str(deck_session_id),
            "deck_revision": 1,
            "stream_generation": 2,
        },
    )
    _assert_ack(reply, "deck.process")
    assert replay.decoded_shape == (1, 4, 6, 4)
    assert len(sink.deliveries) == 2
    assert next(iter(sink.generations.values())) == 2

    live_id = uuid.uuid4()
    live_root = tmp_path / "live"
    live_root.mkdir()
    collision = live_root / f"{live_id}.synthetic"
    collision.write_bytes(b"preexisting")
    reply, _ = harness.command(
        "capture.start",
        {
            "deck_session_id": str(deck_session_id),
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
    reply, _ = harness.command(
        "deck.process",
        {
            "deck_session_id": str(deck_session_id),
            "deck_revision": 1,
            "stream_generation": 2,
        },
    )
    _assert_ack(reply, "deck.process")
    reply, result = harness.command(
        "capture.stop",
        {
            "deck_session_id": str(deck_session_id),
            "deck_revision": 1,
            "capture_id": str(live_id),
        },
    )
    assert result is None
    assert reply["message"]["kind"] == "error"
    error = reply["message"]["body"]["error"]
    assert error["code"] == "capture.invalid_state"
    assert error["details"] == [
        {"key": "extension_code", "value": "capture.target_exists"}
    ]
    assert aborted == [live_id]
    assert collision.read_bytes() == b"preexisting"
    reply, faulted = harness.command(
        "capture.status",
        {
            "deck_session_id": str(deck_session_id),
            "deck_revision": 1,
            "capture_id": str(live_id),
        },
    )
    _assert_ack(reply, "capture.status")
    assert faulted.state.value == "faulted"


def test_public_synthetic_codec_runs_authenticated_service_bootstrap_from_temp_runtime(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """Run the service path under a real copied CPython, not a fake PE fixture."""

    if os.environ.get(SERVICE_CHILD_ENV) != "1":
        runtime = tmp_path / "package" / "runtime"
        runtime.mkdir(parents=True)
        runtime_python = runtime / "python.exe"
        base_python = Path(getattr(sys, "_base_executable", sys.executable)).resolve()
        runtime_dll = base_python.with_name(
            f"python{sys.version_info.major}{sys.version_info.minor}.dll"
        )
        assert runtime_dll.is_file(), f"CPython runtime DLL not found: {runtime_dll}"
        shutil.copy2(base_python, runtime_python)
        shutil.copy2(runtime_dll, runtime / runtime_dll.name)
        environment = os.environ.copy()
        environment[SERVICE_CHILD_ENV] = "1"
        environment["PYTHONDONTWRITEBYTECODE"] = "1"
        environment["PYTHONPATH"] = os.pathsep.join(
            str(Path(entry).resolve()) for entry in sys.path if entry
        )
        completed = subprocess.run(
            [
                str(runtime_python),
                "-m",
                "pytest",
                "--quiet",
                f"{Path(__file__).resolve()}::test_public_synthetic_codec_runs_authenticated_service_bootstrap_from_temp_runtime",
            ],
            cwd=REPOSITORY_ROOT,
            env=environment,
            capture_output=True,
            text=True,
            timeout=60,
            check=False,
        )
        assert completed.returncode == 0, completed.stdout + completed.stderr
        return

    module = _load_example()
    _load_deck_example()
    aborted: list[uuid.UUID] = []
    original_abort = module.SyntheticCaptureWriter.abort

    def observe_abort(writer) -> None:
        aborted.append(writer.request.capture_id)
        original_abort(writer)

    monkeypatch.setattr(module.SyntheticCaptureWriter, "abort", observe_abort)
    session_id = uuid.UUID("e117c45c-31cc-451f-894e-9708646a5ad0")
    token = "8f" * 32
    cartridge = _Cartridge()
    receipt_id = uuid.uuid5(uuid.NAMESPACE_OID, str(cartridge.cartridge_id))
    source_id = uuid.UUID("da1d57c9-1b6e-4677-965a-3aa144761b55")
    deck_session_id = uuid.UUID("311c8737-55a9-45f7-a7a1-0b4d08a0693a")
    ring_id = uuid.UUID("411c8737-55a9-45f7-a7a1-0b4d08a0693a")
    snapshot_id = uuid.UUID("511c8737-55a9-45f7-a7a1-0b4d08a0693a")
    live_id = uuid.UUID("611c8737-55a9-45f7-a7a1-0b4d08a0693a")
    snapshot_root = tmp_path / "snapshot-service"
    live_root = tmp_path / "live-service"
    snapshot_root.mkdir()
    live_root.mkdir()
    sink = _Sink()
    factory = _Factory(cartridge)
    worker_holder: list[Protocol2Worker] = []

    def worker_factory(worker_session_id: uuid.UUID) -> Protocol2Worker:
        worker = Protocol2Worker(
            session_id=worker_session_id,
            codec_entrypoints=(
                TrustedCodecEntrypoint(
                    pack_id=module.PACK_ID,
                    pack_version=module.PACK_VERSION,
                    adapter_id=module.ADAPTER_ID,
                    adapter_version=module.ADAPTER_VERSION,
                    entrypoint="latentdeck_public_synthetic_codec:make_adapter",
                ),
            ),
            deck_entrypoints=(
                TrustedDeckEntrypoint(
                    deck_id="org.example.latentdeck.identity",
                    deck_version="0.1.0",
                    operator_id="org.example.latentdeck.identity",
                    operator_version="0.1.0",
                    entrypoint="latentdeck_public_identity_deck:process_sources_host",
                ),
            ),
            cartridge_access_factory=factory,
            ring_transport=sink,
        )
        worker_holder.append(worker)
        return worker

    source = {
        "physical_slot": 1,
        "source_id": str(source_id),
        "cartridge_id": str(cartridge.cartridge_id),
        "archive_sha256": cartridge.archive_sha256,
        "profile_receipt_id": str(receipt_id),
        "loop_enabled": True,
    }
    commands: list[tuple[str, Mapping[str, object]]] = [
        (
            "session.configure",
            {
                "selected_protocol_version": 2,
                "app_version": "0.1.0",
                "heartbeat_interval_ms": 1000,
                "heartbeat_hard_timeout_ms": 10000,
                "max_frame_bytes": 262144,
                "max_inflight_batches": 4,
                "requested_capabilities": [
                    capability.value for capability in Capability
                ],
            },
        ),
        (
            "ring.configure",
            {
                "ring_id": str(ring_id),
                "kind": "decoded_rgba",
                "mapping_handle": 10,
                "ready_event_handle": 11,
                "consumed_event_handle": 12,
                "slot_count": 4,
                "slot_bytes": 4096,
            },
        ),
        (
            "codec.descriptor",
            {
                "pack_id": module.PACK_ID,
                "pack_version": module.PACK_VERSION,
                "adapter_id": module.ADAPTER_ID,
            },
        ),
        (
            "source.open",
            {
                "source_id": str(source_id),
                "cartridge_id": str(cartridge.cartridge_id),
                "archive_sha256": cartridge.archive_sha256,
                "archive_bytes": 1,
                "retained_native_handle": 101,
                "integrity_access_receipt": '{"receipt_version":1}',
            },
        ),
        (
            "profile.inspect",
            {
                "source_id": str(source_id),
                "cartridge_id": str(cartridge.cartridge_id),
                "archive_sha256": cartridge.archive_sha256,
            },
        ),
        (
            "profile.validate",
            {
                "source_id": str(source_id),
                "expected_profile": {
                    "codec_family": "synthetic",
                    "profile": "example_latent",
                    "profile_version": "0.1.0",
                },
                "required_capabilities": [
                    "player",
                    "realtime",
                    "resample",
                    "snapshot_capture",
                    "live_capture",
                ],
            },
        ),
        (
            "codec.load",
            {
                "pack_id": module.PACK_ID,
                "pack_version": module.PACK_VERSION,
                "adapter_id": module.ADAPTER_ID,
                "adapter_version": module.ADAPTER_VERSION,
                "device": "cpu",
                "device_ordinal": 0,
                "external_assets": [],
            },
        ),
        (
            "deck.load",
            {
                "deck_session_id": str(deck_session_id),
                "deck_id": "org.example.latentdeck.identity",
                "deck_version": "0.1.0",
                "operator_id": "org.example.latentdeck.identity",
                "operator_version": "0.1.0",
                "sources": [source],
                "roles": [{"role": "source", "physical_slot": 1}],
                "controls": [
                    {
                        "name": "mode",
                        "value": {"kind": "text", "value": "identity"},
                    }
                ],
                "seed": 17,
                "stream_generation": 1,
            },
        ),
        (
            "capture.start",
            {
                "deck_session_id": str(deck_session_id),
                "deck_revision": 1,
                "capture_id": str(snapshot_id),
                "mode": "snapshot",
                "staging_root": str(snapshot_root.resolve()),
                "maximum_latent_slots": 8,
                "maximum_visual_bytes": 1_000_000,
                "maximum_reset_events": 4,
            },
        ),
        (
            "deck.process",
            {
                "deck_session_id": str(deck_session_id),
                "deck_revision": 1,
                "stream_generation": 1,
            },
        ),
        (
            "capture.status",
            {
                "deck_session_id": str(deck_session_id),
                "deck_revision": 1,
                "capture_id": str(snapshot_id),
            },
        ),
        (
            "deck.reset",
            {
                "deck_session_id": str(deck_session_id),
                "deck_revision": 1,
                "new_stream_generation": 2,
                "preserve_playheads": False,
            },
        ),
        (
            "deck.process",
            {
                "deck_session_id": str(deck_session_id),
                "deck_revision": 1,
                "stream_generation": 2,
            },
        ),
        (
            "capture.start",
            {
                "deck_session_id": str(deck_session_id),
                "deck_revision": 1,
                "capture_id": str(live_id),
                "mode": "live_capture",
                "staging_root": str(live_root.resolve()),
                "maximum_latent_slots": 8,
                "maximum_visual_bytes": 1_000_000,
                "maximum_reset_events": 4,
            },
        ),
        (
            "deck.process",
            {
                "deck_session_id": str(deck_session_id),
                "deck_revision": 1,
                "stream_generation": 2,
            },
        ),
        ("session.shutdown", {"reason": "host_exit"}),
    ]
    encoded_commands = b"".join(
        _service_command(session_id, sequence, name, payload)
        for sequence, (name, payload) in enumerate(commands, start=1)
    )
    output = io.BytesIO()
    connector = _Connector(session_id, io.BytesIO(encoded_commands), output)

    assert (
        run_protocol2_service(
            io.BytesIO(_service_bootstrap(session_id, token)),
            worker_factory=worker_factory,
            connector=connector,
            worker_identity="org.example.latentdeck.synthetic.worker",
        )
        == 0
    )
    stream_validator = WorkerStreamValidator(str(session_id), token)
    frames = [
        stream_validator.validate(frame)
        for frame in _service_frames(output.getvalue())
    ]
    hello = frames[0]["message"]["body"]["event"]
    assert hello["name"] == "worker.hello"
    assert hello["payload"]["auth_token"] == token
    transcript = frames[1:]
    assert {frame["message"]["kind"] for frame in transcript} <= {"ack", "event"}
    acknowledgements = [
        frame for frame in transcript if frame["message"]["kind"] == "ack"
    ]
    assert [
        reply["message"]["body"]["ack"]["name"] for reply in acknowledgements
    ] == [
        name for name, _payload in commands
    ]
    assert [
        reply["message"]["body"]["reply_to"] for reply in acknowledgements
    ] == [
        str(uuid.UUID(int=sequence + 1000))
        for sequence in range(1, len(commands) + 1)
    ]

    configure_ack_position = transcript.index(acknowledgements[0])
    shutdown_ack_position = transcript.index(acknowledgements[-1])
    for position, frame in enumerate(transcript):
        if frame["message"]["kind"] == "ack":
            continue
        assert configure_ack_position < position < shutdown_ack_position
        body = frame["message"]["body"]
        assert body["caused_by"] is None
        assert body["event"]["name"] == "worker.heartbeat"
        assert set(body["event"]["payload"]) == {
            "session",
            "codec",
            "player",
            "deck",
            "capture",
            "open_session_count",
            "foreground_output_session",
            "output_lease_pinned",
        }
    assert len(sink.deliveries) == 3
    assert (ring_id, 2) in sink.generation_updates
    assert ring_id not in sink.generations
    assert (snapshot_root / f"{snapshot_id}.synthetic").is_file()
    assert aborted == [live_id]
    assert not (live_root / f"{live_id}.synthetic").exists()
    assert worker_holder and worker_holder[0].status()["session"] == "stopped"
    assert output.getvalue().count(token.encode()) == 1
