from __future__ import annotations

import copy
import json

import msgpack
import pytest

from latentdeck_codec_sdk.protocol import (
    MAX_ARRAY_ITEMS,
    ProtocolError,
    WorkerStreamValidator,
    decode_json,
    decode_messagepack,
    encode_json,
    encode_messagepack,
    make_conformance_envelope,
)


def _status() -> dict[str, object]:
    return {
        "session": "ready",
        "codec": "ready",
        "player": "empty",
        "deck": "empty",
        "capture": "idle",
        "open_session_count": 0,
        "foreground_output_session": None,
        "output_lease_pinned": False,
    }


def _message(kind: str, body: dict[str, object]) -> dict[str, object]:
    envelope = make_conformance_envelope()
    envelope["message"] = {"kind": kind, "body": body}
    return envelope


def _worker_hello(*, token: object = "ab" * 32) -> dict[str, object]:
    return _message(
        "event",
        {
            "caused_by": None,
            "event": {
                "name": "worker.hello",
                "payload": {
                    "auth_token": token,
                    "worker_pid": 1234,
                    "worker_identity": "org.latentdeck.codec-host",
                    "runtime_identity": "cpython-3.13",
                    "protocol_min": 2,
                    "protocol_max": 2,
                },
            },
        },
    )


def test_worker_hello_is_closed_and_requires_exact_lowercase_hex_auth() -> None:
    expected = _worker_hello()
    assert decode_json(encode_json(expected)) == expected
    assert decode_messagepack(encode_messagepack(expected)) == expected

    for invalid in ("AB" * 32, "ab" * 31, b"\xab" * 32):
        with pytest.raises(ProtocolError, match="auth token"):
            encode_messagepack(_worker_hello(token=invalid))

    unknown = _worker_hello()
    unknown["message"]["body"]["event"]["payload"]["publisher"] = "self-declared"
    with pytest.raises(ProtocolError):
        encode_json(unknown)


def test_worker_stream_requires_hello_first_and_contiguous_unique_frames() -> None:
    validator = WorkerStreamValidator("9ca8c228-04c7-4b59-909f-6fbef591a43e", "ab" * 32)
    heartbeat = _message(
        "event", {"caused_by": None, "event": {"name": "worker.heartbeat", "payload": _status()}}
    )
    with pytest.raises(ProtocolError, match="worker.hello"):
        validator.validate(heartbeat)

    hello = _worker_hello()
    assert validator.validate(hello) == hello

    repeated = _worker_hello()
    repeated["sequence"] = 2
    repeated["message_id"] = "10000000-0000-4000-8000-000000000099"
    with pytest.raises(ProtocolError, match="exactly once"):
        validator.validate(repeated)

    gap = copy.deepcopy(heartbeat)
    gap["sequence"] = 3
    gap["message_id"] = "10000000-0000-4000-8000-000000000098"
    with pytest.raises(ProtocolError, match="contiguous"):
        validator.validate(gap)


def test_worker_stream_compares_the_hello_secret_without_reflecting_it() -> None:
    validator = WorkerStreamValidator("9ca8c228-04c7-4b59-909f-6fbef591a43e", "cd" * 32)
    with pytest.raises(ProtocolError, match="authentication failed") as failure:
        validator.validate(_worker_hello())
    assert "ab" * 32 not in str(failure.value)
    assert "cd" * 32 not in str(failure.value)


def test_json_and_messagepack_round_trip_the_same_envelope() -> None:
    expected = make_conformance_envelope()
    assert decode_json(encode_json(expected)) == expected
    assert decode_messagepack(encode_messagepack(expected)) == expected


def test_unknown_fields_are_rejected_at_envelope_and_payload_boundaries() -> None:
    envelope = make_conformance_envelope()
    envelope["hidden_fallback"] = True
    with pytest.raises(ProtocolError):
        encode_json(envelope)

    payload = make_conformance_envelope()
    payload["message"]["body"]["payload"]["hidden_resize"] = True
    with pytest.raises(ProtocolError):
        encode_messagepack(payload)


def test_duplicate_json_and_messagepack_keys_are_rejected() -> None:
    encoded = encode_json(make_conformance_envelope())
    duplicate = encoded[:-1] + b',"sequence":1}'
    with pytest.raises(ProtocolError):
        decode_json(duplicate)

    packer = msgpack.Packer(use_bin_type=True)
    duplicate_map = b"".join(
        (
            packer.pack_map_header(2),
            packer.pack("protocol"),
            packer.pack("latentdeck.worker"),
            packer.pack("protocol"),
            packer.pack("latentdeck.worker"),
        )
    )
    with pytest.raises(ProtocolError):
        decode_messagepack(duplicate_map)


def test_non_finite_and_oversized_values_are_rejected() -> None:
    non_finite = make_conformance_envelope()
    non_finite["message"]["body"]["payload"]["app_version"] = float("nan")
    with pytest.raises(ProtocolError):
        encode_json(non_finite)

    oversized = make_conformance_envelope()
    oversized["message"]["body"]["payload"]["requested_capabilities"] = ["player"] * (
        MAX_ARRAY_ITEMS + 1
    )
    with pytest.raises(ProtocolError):
        encode_messagepack(oversized)


def test_unknown_command_is_rejected_without_fallback() -> None:
    envelope = copy.deepcopy(make_conformance_envelope())
    envelope["message"]["body"]["name"] = "deck.d2.load"
    with pytest.raises(ProtocolError, match="unknown"):
        encode_json(envelope)


def test_dynamic_deck_runtime_binding_round_trips_and_is_strict(tmp_path) -> None:
    payload = {
        "deck_session_id": "10000000-0000-4000-8000-000000000030",
        "deck_id": "com.example.deck",
        "deck_version": "0.2.0",
        "operator_id": "com.example.operator",
        "operator_version": "0.2.0",
        "runtime": {
            "deck_id": "com.example.deck",
            "deck_version": "0.2.0",
            "operator_id": "com.example.operator",
            "operator_version": "0.2.0",
            "python_root": str(tmp_path.resolve()),
            "entrypoint": "external_deck.operator:process_sources",
            "package_manifest_sha256": "a" * 64,
            "integrity_catalog_sha256": "b" * 64,
        },
        "sources": [
            {
                "physical_slot": 1,
                "source_id": "10000000-0000-4000-8000-000000000031",
                "cartridge_id": "10000000-0000-4000-8000-000000000032",
                "archive_sha256": "c" * 64,
                "profile_receipt_id": "10000000-0000-4000-8000-000000000033",
                "loop_enabled": True,
            }
        ],
        "roles": [{"role": "carrier", "physical_slot": 1}],
        "controls": [],
        "seed": 7,
        "stream_generation": 1,
    }
    envelope = _message("command", {"name": "deck.load", "payload": payload})
    assert decode_json(encode_json(envelope)) == envelope
    assert decode_messagepack(encode_messagepack(envelope)) == envelope

    legacy = copy.deepcopy(envelope)
    del legacy["message"]["body"]["payload"]["runtime"]
    assert decode_json(encode_json(legacy)) == legacy

    mismatch = copy.deepcopy(envelope)
    mismatch["message"]["body"]["payload"]["runtime"]["operator_version"] = "0.3.0"
    with pytest.raises(ProtocolError, match="identity"):
        encode_json(mismatch)

    relative = copy.deepcopy(envelope)
    relative["message"]["body"]["payload"]["runtime"]["python_root"] = "relative/python"
    with pytest.raises(ProtocolError, match="absolute"):
        encode_json(relative)

    unknown = copy.deepcopy(envelope)
    unknown["message"]["body"]["payload"]["runtime"]["fallback"] = True
    with pytest.raises(ProtocolError, match="closed schema"):
        encode_json(unknown)


def test_deck_transport_is_independent_and_keyed_by_physical_slot() -> None:
    envelope = _message(
        "command",
        {
            "name": "deck.transport.set",
            "payload": {
                "deck_session_id": "10000000-0000-4000-8000-000000000030",
                "deck_revision": 1,
                "sources": [
                    {"physical_slot": 1, "playing": False, "loop_enabled": True},
                    {"physical_slot": 2, "playing": True, "loop_enabled": False},
                ],
            },
        },
    )
    assert decode_json(encode_json(envelope)) == envelope
    assert decode_messagepack(encode_messagepack(envelope)) == envelope

    duplicate = copy.deepcopy(envelope)
    duplicate["message"]["body"]["payload"]["sources"][1]["physical_slot"] = 1
    with pytest.raises(ProtocolError, match="physical slots must be unique"):
        encode_json(duplicate)


def test_deck_reset_requires_explicit_playhead_preservation_policy() -> None:
    envelope = _message(
        "command",
        {
            "name": "deck.reset",
            "payload": {
                "deck_session_id": "10000000-0000-4000-8000-000000000030",
                "deck_revision": 1,
                "new_stream_generation": 2,
                "preserve_playheads": True,
            },
        },
    )
    assert decode_json(encode_json(envelope)) == envelope
    assert decode_messagepack(encode_messagepack(envelope)) == envelope

    missing_policy = copy.deepcopy(envelope)
    del missing_policy["message"]["body"]["payload"]["preserve_playheads"]
    with pytest.raises(ProtocolError, match="closed schema"):
        encode_json(missing_policy)


def test_capture_protocol_binds_host_staging_and_decoded_timing(tmp_path) -> None:
    capture_id = "10000000-0000-4000-8000-000000000040"
    staging_root = str((tmp_path / "capture-staging").resolve())
    staged_payload = str((tmp_path / "capture-staging" / "capture.safetensors").resolve())
    command = _message(
        "command",
        {
            "name": "capture.start",
            "payload": {
                "deck_session_id": "10000000-0000-4000-8000-000000000030",
                "deck_revision": 1,
                "capture_id": capture_id,
                "mode": "snapshot",
                "staging_root": staging_root,
                "maximum_latent_slots": 128,
                "maximum_visual_bytes": 64 * 1024 * 1024,
                "maximum_reset_events": 32,
            },
        },
    )
    assert decode_json(encode_json(command)) == command
    assert decode_messagepack(encode_messagepack(command)) == command

    completed = _message(
        "ack",
        {
            "reply_to": "10000000-0000-4000-8000-000000000002",
            "ack": {
                "name": "capture.status",
                "payload": {
                    "deck_session_id": "10000000-0000-4000-8000-000000000030",
                    "deck_revision": 1,
                    "capture_id": capture_id,
                    "state": "completed",
                    "mode": "snapshot",
                    "latent_slots": 2,
                    "reset_events": 0,
                    "artifact": {
                        "staged_payload_path": staged_payload,
                        "payload_sha256": "a" * 64,
                        "payload_byte_length": 4096,
                        "latent_slots": 2,
                        "decoded_frame_count": 5,
                    },
                },
            },
            "status": _status(),
        },
    )
    assert decode_json(encode_json(completed)) == completed

    relative = copy.deepcopy(command)
    relative["message"]["body"]["payload"]["staging_root"] = "relative/capture"
    with pytest.raises(ProtocolError, match="staging_root"):
        encode_json(relative)
    unknown = copy.deepcopy(command)
    unknown["message"]["body"]["payload"]["worker_path"] = "not-authoritative"
    with pytest.raises(ProtocolError, match="closed schema"):
        encode_json(unknown)


def test_source_open_requires_a_bounded_canonical_integrity_access_receipt() -> None:
    source_open = _message(
        "command",
        {
            "name": "source.open",
            "payload": {
                "source_id": "10000000-0000-4000-8000-000000000010",
                "cartridge_id": "10000000-0000-4000-8000-000000000011",
                "archive_sha256": "a" * 64,
                "archive_bytes": 4096,
                "retained_native_handle": 1234,
                "integrity_access_receipt": '{"receipt_version":1}',
            },
        },
    )
    assert decode_messagepack(encode_messagepack(source_open)) == source_open

    noncanonical = copy.deepcopy(source_open)
    noncanonical["message"]["body"]["payload"]["integrity_access_receipt"] = (
        '{ "receipt_version": 1 }'
    )
    with pytest.raises(ProtocolError, match="canonical JSON"):
        encode_messagepack(noncanonical)


def test_json_decoder_rejects_a_second_top_level_value() -> None:
    encoded = encode_json(make_conformance_envelope()) + json.dumps({}).encode()
    with pytest.raises(ProtocolError):
        decode_json(encoded)


@pytest.mark.parametrize(
    ("name", "payload"),
    [
        (
            "source.open",
            {
                "source_id": "10000000-0000-4000-8000-000000000010",
                "cartridge_id": "10000000-0000-4000-8000-000000000011",
                "archive_sha256": "a" * 64,
                "archive_bytes": 4096,
                "retained_native_handle": 1234,
                "integrity_access_receipt": '{"receipt_version":1}',
            },
        ),
        ("source.close", {"source_id": "10000000-0000-4000-8000-000000000010"}),
        (
            "ring.configure",
            {
                "ring_id": "10000000-0000-4000-8000-000000000012",
                "kind": "latent_tensor",
                "mapping_handle": 2001,
                "ready_event_handle": 2002,
                "consumed_event_handle": 2003,
                "slot_count": 4,
                "slot_bytes": 8192,
            },
        ),
        ("ring.release", {"ring_id": "10000000-0000-4000-8000-000000000012"}),
        ("metrics.get", {}),
    ],
)
def test_native_handle_lifecycle_commands_never_carry_media_bytes(
    name: str, payload: dict[str, object]
) -> None:
    envelope = _message("command", {"name": name, "payload": payload})
    encoded = encode_messagepack(envelope)
    assert decode_messagepack(encoded) == envelope
    assert b"tensor_bytes" not in encoded
    assert b"rgba_bytes" not in encoded

    smuggled = copy.deepcopy(envelope)
    smuggled["message"]["body"]["payload"]["tensor_bytes"] = b"x"
    with pytest.raises(ProtocolError, match="closed schema"):
        encode_messagepack(smuggled)


def test_typed_codec_descriptor_ack_is_closed_and_requires_full_v2_capabilities() -> None:
    payload = {
        "pack_id": "org.example.synthetic",
        "pack_version": "0.2.0",
        "adapter_id": "org.example.synthetic.adapter",
        "adapter_version": "0.2.0",
        "host_api_version": "2.0",
        "capabilities": [
            "player",
            "realtime",
            "resample",
            "snapshot_capture",
            "live_capture",
        ],
        "profiles": [
            {
                "codec_family": "synthetic",
                "profile": "test_latent",
                "profile_version": "0.1.0",
            }
        ],
    }
    envelope = _message(
        "ack",
        {
            "reply_to": "10000000-0000-4000-8000-000000000002",
            "ack": {"name": "codec.descriptor", "payload": payload},
            "status": _status(),
        },
    )
    assert decode_json(encode_json(envelope)) == envelope

    missing = copy.deepcopy(envelope)
    missing["message"]["body"]["ack"]["payload"]["capabilities"].remove("live_capture")
    with pytest.raises(ProtocolError, match="required capability"):
        encode_json(missing)

    legacy = copy.deepcopy(envelope)
    body = legacy["message"]["body"]
    body["name"] = body["ack"]["name"]
    del body["ack"]
    with pytest.raises(ProtocolError, match="closed schema"):
        encode_json(legacy)


def test_typed_profile_receipt_ack_cross_checks_tensor_geometry() -> None:
    receipt = {
        "receipt_id": "10000000-0000-4000-8000-000000000021",
        "cartridge_id": "10000000-0000-4000-8000-000000000022",
        "archive_sha256": "a" * 64,
        "payload_sha256": "b" * 64,
        "pack_id": "org.example.synthetic",
        "pack_version": "0.2.0",
        "adapter_id": "org.example.synthetic.adapter",
        "adapter_version": "0.2.0",
        "profile_key": {
            "codec_family": "synthetic",
            "profile": "test_latent",
            "profile_version": "0.1.0",
        },
        "signal_geometry": {
            "channels": 4,
            "latent_height": 8,
            "latent_width": 8,
            "decoded_height": 64,
            "decoded_width": 64,
            "frame_rate_numerator": 24,
            "frame_rate_denominator": 1,
            "timing_contract": "synthetic_causal",
            "timing_contract_version": "0.1.0",
        },
        "tensor_abi": {
            "python_major": 3,
            "python_minor": 13,
            "torch_version": "2.13.0+cu130",
            "dtype": "float16",
            "shape": [1, 4, 1, 8, 8],
            "contiguous": True,
            "device": "cuda",
        },
        "decoded_abi": {"pixel_format": "rgba8", "maximum_batch": 24},
        "capabilities": ["player", "realtime"],
        "estimated_host_bytes": 4096,
        "estimated_device_bytes": 8192,
    }
    envelope = _message(
        "ack",
        {
            "reply_to": "10000000-0000-4000-8000-000000000002",
            "ack": {"name": "profile.validate", "payload": receipt},
            "status": _status(),
        },
    )
    assert decode_messagepack(encode_messagepack(envelope)) == envelope

    mismatched = copy.deepcopy(envelope)
    mismatched["message"]["body"]["ack"]["payload"]["tensor_abi"]["shape"][1] = 8
    with pytest.raises(ProtocolError, match="signal geometry"):
        encode_messagepack(mismatched)


def _raw_import_metadata() -> dict[str, object]:
    return {
        "profile_key": {
            "codec_family": "minimax_h3",
            "profile": "h3_av_latent",
            "profile_version": "0.1.0",
        },
        "payload_entry": "payloads/h3.safetensors",
        "payload_media_type": "application/vnd.safetensors",
        "tensors": [
            {
                "stream": "visual",
                "name": "video",
                "storage_dtype": "F16",
                "runtime_dtype": "F16",
                "shape": [1, 24, 7, 8, 8],
            }
        ],
        "timing_contract": "minimax_h3_causal",
        "timing_contract_version": "0.1.0",
        "decoded_width": 128,
        "decoded_height": 128,
        "decoded_frame_count": 22,
        "frame_rate_numerator": 24,
        "frame_rate_denominator": 1,
        "duration_numerator": 11,
        "duration_denominator": 12,
        "audio_policy": "source_absent",
    }


def test_raw_import_preflight_stage_and_abort_are_closed_and_conformant(tmp_path) -> None:
    import_id = "10000000-0000-4000-8000-000000000070"
    receipt_id = "10000000-0000-4000-8000-000000000071"
    source = str((tmp_path / "source.safetensors").resolve())
    staging_root = str((tmp_path / "staging").resolve())
    staged = str((tmp_path / "staging" / "staged.safetensors").resolve())
    commands = [
        (
            "raw_import.preflight",
            {
                "import_id": import_id,
                "source_path": source,
                "maximum_source_bytes": 64 * 1024 * 1024,
            },
        ),
        (
            "raw_import.stage",
            {
                "import_id": import_id,
                "receipt_id": receipt_id,
                "staging_root": staging_root,
            },
        ),
        ("raw_import.abort", {"import_id": import_id, "receipt_id": receipt_id}),
    ]
    for name, payload in commands:
        envelope = _message("command", {"name": name, "payload": payload})
        assert decode_json(encode_json(envelope)) == envelope
        assert decode_messagepack(encode_messagepack(envelope)) == envelope

    ack_payloads = [
        (
            "raw_import.preflight",
            {
                "receipt_id": receipt_id,
                "import_id": import_id,
                "pack_id": "org.latentdeck.h3",
                "pack_version": "0.2.0",
                "adapter_id": "org.latentdeck.h3",
                "adapter_version": "0.2.0",
                "source_sha256": "a" * 64,
                "source_byte_length": 4096,
                "metadata": _raw_import_metadata(),
            },
        ),
        (
            "raw_import.stage",
            {
                "receipt_id": receipt_id,
                "import_id": import_id,
                "staged_payload_path": staged,
                "payload_sha256": "b" * 64,
                "payload_byte_length": 4096,
            },
        ),
        ("raw_import.abort", {"receipt_id": receipt_id, "import_id": import_id}),
    ]
    for name, payload in ack_payloads:
        envelope = _message(
            "ack",
            {
                "reply_to": "10000000-0000-4000-8000-000000000002",
                "ack": {"name": name, "payload": payload},
                "status": _status(),
            },
        )
        assert decode_json(encode_json(envelope)) == envelope
        assert decode_messagepack(encode_messagepack(envelope)) == envelope

    escaping = _message(
        "ack",
        {
            "reply_to": "10000000-0000-4000-8000-000000000002",
            "ack": {
                "name": "raw_import.preflight",
                "payload": {**ack_payloads[0][1], "metadata": _raw_import_metadata()},
            },
            "status": _status(),
        },
    )
    escaping["message"]["body"]["ack"]["payload"]["metadata"]["payload_entry"] = (
        "../outside.safetensors"
    )
    with pytest.raises(ProtocolError, match="payload entry"):
        encode_json(escaping)
