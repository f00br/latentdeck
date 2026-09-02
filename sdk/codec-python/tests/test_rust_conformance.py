from __future__ import annotations

import json
import subprocess
from pathlib import Path

import msgpack

from latentdeck_codec_sdk.protocol import (
    COMMAND_FIELDS,
    encode_json,
    encode_messagepack,
    make_conformance_ack_envelope,
    make_conformance_envelope,
    make_conformance_error_envelope,
)

WORKSPACE_ROOT = Path(__file__).resolve().parents[3]


def _raw_import_command_fixture() -> dict[str, object]:
    return {
        "protocol": "latentdeck.worker",
        "protocol_version": 2,
        "session_id": "9ca8c228-04c7-4b59-909f-6fbef591a43e",
        "sequence": 4,
        "message_id": "10000000-0000-4000-8000-000000000008",
        "sender_uptime_ns": 423_456,
        "message": {
            "kind": "command",
            "body": {
                "name": "raw_import.preflight",
                "payload": {
                    "import_id": "10000000-0000-4000-8000-000000000009",
                    "source_path": "C:\\latentdeck-conformance\\raw.safetensors",
                    "maximum_source_bytes": 1024,
                },
            },
        },
    }


def _raw_import_ack_fixture() -> dict[str, object]:
    return {
        "protocol": "latentdeck.worker",
        "protocol_version": 2,
        "session_id": "9ca8c228-04c7-4b59-909f-6fbef591a43e",
        "sequence": 5,
        "message_id": "10000000-0000-4000-8000-00000000000a",
        "sender_uptime_ns": 523_456,
        "message": {
            "kind": "ack",
            "body": {
                "reply_to": "10000000-0000-4000-8000-000000000008",
                "ack": {
                    "name": "raw_import.preflight",
                    "payload": {
                        "receipt_id": "10000000-0000-4000-8000-00000000000b",
                        "import_id": "10000000-0000-4000-8000-000000000009",
                        "pack_id": "org.example.synthetic",
                        "pack_version": "0.2.0",
                        "adapter_id": "org.example.synthetic.adapter",
                        "adapter_version": "0.2.0",
                        "source_sha256": "11" * 32,
                        "source_byte_length": 512,
                        "metadata": {
                            "profile_key": {
                                "codec_family": "synthetic",
                                "profile": "test_latent",
                                "profile_version": "0.1.0",
                            },
                            "payload_entry": "payloads/synthetic.safetensors",
                            "payload_media_type": "application/vnd.safetensors",
                            "tensors": [
                                {
                                    "stream": "visual",
                                    "name": "video",
                                    "storage_dtype": "F16",
                                    "runtime_dtype": "F16",
                                    "shape": [1, 4, 2, 1, 1],
                                }
                            ],
                            "timing_contract": "synthetic_ticks",
                            "timing_contract_version": "0.1.0",
                            "decoded_width": 8,
                            "decoded_height": 8,
                            "decoded_frame_count": 2,
                            "frame_rate_numerator": 24,
                            "frame_rate_denominator": 1,
                            "duration_numerator": 1,
                            "duration_denominator": 12,
                            "audio_policy": "source_absent",
                        },
                    },
                },
                "status": {
                    "session": "ready",
                    "codec": "unloaded",
                    "player": "empty",
                    "deck": "empty",
                    "capture": "idle",
                    "open_session_count": 0,
                    "foreground_output_session": None,
                    "output_lease_pinned": False,
                },
            },
        },
    }


def _rust(mode: str, payload: bytes | None = None) -> bytes:
    completed = subprocess.run(
        [
            "cargo",
            "run",
            "--quiet",
            "-p",
            "latentdeck-control",
            "--example",
            "worker_protocol_v2_conformance",
            "--",
            mode,
        ],
        cwd=WORKSPACE_ROOT,
        input=payload,
        capture_output=True,
        check=False,
        timeout=120,
    )
    if completed.returncode != 0:
        raise AssertionError(completed.stderr.decode(errors="replace"))
    return completed.stdout


def test_rust_generated_json_and_messagepack_equal_the_python_fixture() -> None:
    expected = make_conformance_envelope()
    assert json.loads(_rust("emit-json")) == expected
    assert msgpack.unpackb(_rust("emit-msgpack"), raw=False) == expected


def test_rust_and_python_expose_the_same_closed_protocol2_command_set() -> None:
    rust_names = json.loads(_rust("emit-command-names-json"))
    assert len(rust_names) == 32
    assert len(set(rust_names)) == 32
    assert set(rust_names) == set(COMMAND_FIELDS)
    assert all(".d2." not in name and ".q4." not in name for name in rust_names)


def test_rust_accepts_python_generated_json_and_messagepack() -> None:
    expected = make_conformance_envelope()
    assert json.loads(_rust("validate-json", encode_json(expected))) == expected
    assert json.loads(_rust("validate-msgpack", encode_messagepack(expected))) == expected


def test_error_and_status_enums_conform_in_both_wire_formats() -> None:
    expected = make_conformance_error_envelope()
    assert json.loads(_rust("emit-error-json")) == expected
    assert msgpack.unpackb(_rust("emit-error-msgpack"), raw=False) == expected
    assert json.loads(_rust("validate-error-json", encode_json(expected))) == expected
    assert json.loads(_rust("validate-error-msgpack", encode_messagepack(expected))) == expected


def test_typed_ack_conforms_in_both_wire_formats() -> None:
    expected = make_conformance_ack_envelope()
    assert json.loads(_rust("emit-ack-json")) == expected
    assert msgpack.unpackb(_rust("emit-ack-msgpack"), raw=False) == expected
    assert json.loads(_rust("validate-ack-json", encode_json(expected))) == expected
    assert json.loads(_rust("validate-ack-msgpack", encode_messagepack(expected))) == expected


def test_raw_import_command_and_ack_conform_exactly_in_both_wire_formats() -> None:
    command = _raw_import_command_fixture()
    ack = _raw_import_ack_fixture()
    assert json.loads(_rust("emit-raw-import-json")) == command
    assert msgpack.unpackb(_rust("emit-raw-import-msgpack"), raw=False) == command
    assert json.loads(_rust("validate-raw-import-json", encode_json(command))) == command
    assert json.loads(_rust("validate-raw-import-msgpack", encode_messagepack(command))) == command
    assert json.loads(_rust("emit-raw-import-ack-json")) == ack
    assert msgpack.unpackb(_rust("emit-raw-import-ack-msgpack"), raw=False) == ack
    assert json.loads(_rust("validate-raw-import-ack-json", encode_json(ack))) == ack
    assert json.loads(_rust("validate-raw-import-ack-msgpack", encode_messagepack(ack))) == ack
