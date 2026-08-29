from __future__ import annotations

import io
import struct
import uuid

import msgpack
import pytest
from latentdeck_codec_host.protocol import (
    Bootstrap,
    EnvelopeWriter,
    ProtocolError,
    SequenceValidator,
    encode_bootstrap,
    read_bootstrap,
    read_frame,
    write_frame,
)

SESSION_ID = "9ca8c228-04c7-4b59-909f-6fbef591a43e"
MESSAGE_ID = "f78e4568-f58e-4fe7-888a-5cf1cc2db76b"


def command_envelope(sequence: int = 1, message_id: str = MESSAGE_ID) -> dict[str, object]:
    return {
        "protocol": "latentdeck.worker",
        "protocol_version": 1,
        "session_id": SESSION_ID,
        "sequence": sequence,
        "message_id": message_id,
        "sender_uptime_ns": 1,
        "message": {
            "kind": "command",
            "body": {"name": "codec.inspect", "payload": {}},
        },
    }


def d2_controls() -> dict[str, object]:
    return {
        "algorithm": "LINEAR",
        "mix": 0.5,
        "mode": "HYBRIDIZE",
        "routing": "A",
        "interaction": 0.0,
        "preserve": 0.55,
        "chaos": 0.0,
        "xs1_channel_a": 0,
        "xs1_channel_b": 1,
        "xs1_angle_degrees": 30.0,
        "xs2_radius": 1,
        "xs3_high_gain": 0.5,
        "xs4_epsilon": 0.000001,
        "xs5_routing": "TOPK",
        "temperature": 0.12,
        "top_k": 8,
        "sinkhorn_iterations": 5,
    }


def d2_load_payload() -> dict[str, object]:
    return {
        "deck_id": "main-d2",
        "operator_id": "org.latentdeck.builtin.ld_d2",
        "operator_version": "0.1.0",
        "source_a": {
            "cartridge_path": "C:/private/a.lc",
            "cartridge_id": "11111111-1111-4111-8111-111111111111",
            "expected_archive_sha256": "a" * 64,
        },
        "source_b": {
            "cartridge_path": "C:/private/b.lc",
            "cartridge_id": "22222222-2222-4222-8222-222222222222",
            "expected_archive_sha256": "b" * 64,
        },
        "controls": d2_controls(),
        "transport": {"playing_a": True, "playing_b": True, "loop_a": True, "loop_b": True},
        "seed": 42,
        "stream_generation": 1,
    }


def test_bootstrap_roundtrip_is_bounded_and_secret_is_binary() -> None:
    bootstrap = Bootstrap(SESSION_ID, rf"\\.\pipe\LatentDeck.Worker.{SESSION_ID}", b"x" * 32)
    encoded = encode_bootstrap(bootstrap)
    assert len(encoded) <= 4100
    assert read_bootstrap(io.BytesIO(encoded)) == bootstrap


@pytest.mark.parametrize("split", range(5))
def test_frame_reader_handles_short_reads_at_every_prefix_boundary(split: int) -> None:
    encoded = io.BytesIO()
    write_frame(encoded, command_envelope())
    raw = encoded.getvalue()

    class SplitReader(io.BytesIO):
        def read(self, size: int = -1) -> bytes:
            return super().read(min(size, max(1, split)))

    assert read_frame(SplitReader(raw)) == command_envelope()


def test_rejects_zero_oversized_and_truncated_frames() -> None:
    for length in (0, 262_145, 0xFFFF_FFFF):
        with pytest.raises(ProtocolError, match="length"):
            read_frame(io.BytesIO(struct.pack("<I", length)))
    with pytest.raises(ProtocolError, match="ended early"):
        read_frame(io.BytesIO(struct.pack("<I", 3) + b"x"))


def test_rejects_duplicate_map_keys_extension_nil_and_nonfinite() -> None:
    packer = msgpack.Packer(use_bin_type=True)
    duplicate = (
        packer.pack_map_header(2)
        + packer.pack("same")
        + packer.pack(1)
        + packer.pack("same")
        + packer.pack(2)
    )
    with pytest.raises(ProtocolError, match="invalid MessagePack"):
        read_frame(io.BytesIO(struct.pack("<I", len(duplicate)) + duplicate))
    for value in ({"value": msgpack.ExtType(1, b"")}, {"value": None}, {"value": float("nan")}):
        encoded = msgpack.packb(value, use_bin_type=True)
        with pytest.raises(ProtocolError):
            read_frame(io.BytesIO(struct.pack("<I", len(encoded)) + encoded))


def test_command_schema_sequence_and_message_ids_are_strict() -> None:
    validator = SequenceValidator(SESSION_ID)
    assert validator.validate_command(command_envelope())["name"] == "codec.inspect"
    with pytest.raises(ProtocolError, match="contiguous"):
        validator.validate_command(command_envelope(sequence=3, message_id=str(uuid.uuid4())))

    wrong = command_envelope(sequence=2, message_id=str(uuid.uuid4()))
    wrong["unexpected"] = True
    with pytest.raises(ProtocolError, match="closed schema"):
        validator.validate_command(wrong)


def test_d2_commands_have_closed_nested_wire_schemas() -> None:
    envelope = command_envelope()
    envelope["message"]["body"] = {  # type: ignore[index]
        "name": "deck.d2.load",
        "payload": d2_load_payload(),
    }
    assert SequenceValidator(SESSION_ID).validate_command(envelope)["name"] == "deck.d2.load"

    unknown = command_envelope()
    payload = d2_load_payload()
    controls = payload["controls"]
    assert isinstance(controls, dict)
    controls["hidden_downscale"] = True
    unknown["message"]["body"] = {"name": "deck.d2.load", "payload": payload}  # type: ignore[index]
    with pytest.raises(ProtocolError, match="closed schema"):
        SequenceValidator(SESSION_ID).validate_command(unknown)

    nonfinite = command_envelope()
    payload = d2_load_payload()
    controls = payload["controls"]
    assert isinstance(controls, dict)
    controls["interaction"] = float("nan")
    nonfinite["message"]["body"] = {"name": "deck.d2.load", "payload": payload}  # type: ignore[index]
    with pytest.raises(ProtocolError, match="finite bound"):
        SequenceValidator(SESSION_ID).validate_command(nonfinite)

    step = command_envelope()
    step["message"]["body"] = {  # type: ignore[index]
        "name": "deck.d2.process_slot",
        "payload": {"deck_id": "main-d2", "deck_revision": 1, "stream_generation": 1},
    }
    assert SequenceValidator(SESSION_ID).validate_command(step)["name"] == "deck.d2.process_slot"


def test_d2_capture_commands_have_closed_bounded_wire_schemas() -> None:
    capture_id = "33333333-3333-4333-8333-333333333333"
    start_payload = {
        "deck_id": "main-d2",
        "deck_revision": 1,
        "capture_id": capture_id,
        "mode": "snapshot",
        "temporary_root": "C:/trusted/latentdeck/captures",
        "max_latent_slots": 128,
        "max_visual_bytes": 16 * 1024 * 1024,
    }
    cases = [
        ("deck.d2.capture.start", start_payload),
        (
            "deck.d2.capture.stop",
            {"deck_id": "main-d2", "deck_revision": 1, "capture_id": capture_id},
        ),
        (
            "deck.d2.capture.status",
            {"deck_id": "main-d2", "deck_revision": 1, "capture_id": capture_id},
        ),
    ]
    for name, payload in cases:
        envelope = command_envelope()
        envelope["message"]["body"] = {"name": name, "payload": payload}  # type: ignore[index]
        assert SequenceValidator(SESSION_ID).validate_command(envelope)["name"] == name

    invalid_cases = [
        {**start_payload, "capture_id": "AAAAAAAA-AAAA-4AAA-8AAA-AAAAAAAAAAAA"},
        {**start_payload, "mode": "FRAME_DUMP"},
        {**start_payload, "max_latent_slots": 1},
        {**start_payload, "max_visual_bytes": 0},
        {**start_payload, "hidden_rgb_fallback": True},
    ]
    for payload in invalid_cases:
        envelope = command_envelope()
        envelope["message"]["body"] = {  # type: ignore[index]
            "name": "deck.d2.capture.start",
            "payload": payload,
        }
        with pytest.raises(ProtocolError):
            SequenceValidator(SESSION_ID).validate_command(envelope)


def test_writer_serializes_ack_event_and_error_envelopes() -> None:
    stream = io.BytesIO()
    writer = EnvelopeWriter(stream, SESSION_ID)
    writer.event(
        "worker.hello",
        {
            "auth_token": b"x" * 32,
            "worker_version": "0.1.0",
            "protocol_min": 1,
            "protocol_max": 1,
            "pid": 1,
            "os": "windows",
            "arch": "x86_64",
            "python_version": "3.13",
            "available_adapters": ["org.latentdeck.h3"],
        },
    )
    writer.ack(MESSAGE_ID, "codec.inspect", {"ok": True})
    writer.error(
        MESSAGE_ID,
        "codec.load",
        code="codec.load_failed",
        message="load failed",
        retryable=False,
        fatal=False,
        worker_state="ready",
    )
    stream.seek(0)
    first, second, third = read_frame(stream), read_frame(stream), read_frame(stream)
    assert first["sequence"] == 1
    assert second["sequence"] == 2
    assert third["sequence"] == 3
    assert first["message"]["kind"] == "event"  # type: ignore[index]
    assert second["message"]["kind"] == "ack"  # type: ignore[index]
    assert third["message"]["kind"] == "error"  # type: ignore[index]


def test_writer_accepts_the_closed_codec_inspection_nesting() -> None:
    stream = io.BytesIO()
    writer = EnvelopeWriter(stream, SESSION_ID)
    writer.ack(
        MESSAGE_ID,
        "codec.inspect",
        {
            "torch_version": "2.13.0+cu130",
            "cuda_available": True,
            "cuda_runtime": "13.0",
            "devices": [{"ordinal": 0, "name": "Synthetic CUDA device", "total_memory_bytes": 1}],
            "adapters": [
                {
                    "adapter_id": "org.latentdeck.h3",
                    "adapter_version": "0.1.0",
                    "profiles": [
                        {
                            "codec_family": "minimax_h3",
                            "profile": "h3_av_latent",
                            "profile_version": "0.1.0",
                        }
                    ],
                }
            ],
        },
    )
    stream.seek(0)
    assert read_frame(stream)["message"]["kind"] == "ack"  # type: ignore[index]


def test_writer_still_rejects_excessive_nesting() -> None:
    value: object = True
    for _ in range(18):
        value = [value]
    with pytest.raises(ProtocolError, match="nesting"):
        write_frame(io.BytesIO(), {"value": value})
