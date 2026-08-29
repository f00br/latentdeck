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
            "devices": [
                {"ordinal": 0, "name": "Synthetic CUDA device", "total_memory_bytes": 1}
            ],
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
