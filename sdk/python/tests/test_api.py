from __future__ import annotations

import hashlib
import json
import uuid
from pathlib import Path

import latentdeck_cartridge as cartridge
import pytest


def _synthetic_video_payload() -> bytes:
    tensor = bytes(24 * 2 * 2)
    header = json.dumps(
        {
            "video": {
                "data_offsets": [0, len(tensor)],
                "dtype": "F16",
                "shape": [1, 24, 2, 1, 1],
            }
        },
        separators=(",", ":"),
    ).encode()
    header += b" " * (-len(header) % 8)
    return len(header).to_bytes(8, "little") + header + tensor


def _synthetic_t7_av_payload() -> bytes:
    audio = bytes(32 * 2 * 37 * 2)
    video = bytes(24 * 7 * 2)
    audio_end = len(audio)
    video_end = audio_end + len(video)
    header = json.dumps(
        {
            "audio": {
                "data_offsets": [0, audio_end],
                "dtype": "F16",
                "shape": [1, 32, 2, 37],
            },
            "video": {
                "data_offsets": [audio_end, video_end],
                "dtype": "F16",
                "shape": [1, 24, 7, 1, 1],
            },
        },
        separators=(",", ":"),
    ).encode()
    header += b" " * (-len(header) % 8)
    return len(header).to_bytes(8, "little") + header + audio + video


def _manifest(payload: bytes) -> dict[str, object]:
    return {
        "spec_version": "0.1.0",
        "cartridge_id": "550e8400-e29b-41d4-a716-446655440000",
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
        "tensors": [
            {
                "stream": "visual",
                "name": "video",
                "payload": "payloads/h3.safetensors",
                "storage_dtype": "F16",
                "runtime_dtype": "F16",
                "shape": [1, 24, 2, 1, 1],
            }
        ],
        "timing": {
            "contract": "minimax_h3_causal",
            "contract_version": "0.1.0",
            "decoded_video": {
                "width": 16,
                "height": 16,
                "frame_count": 5,
                "frame_rate": {"numerator": 24, "denominator": 1},
                "duration": {"numerator": 5, "denominator": 24},
            },
        },
        "audio": {"policy": "source_absent"},
        "provenance": {
            "created_by": {"name": "latentdeck-cartridge", "version": "0.1.0"},
            "sources": [],
        },
        "parent_cartridges": [],
        "operation_history": [],
    }


def _pack_synthetic(tmp_path: Path) -> tuple[Path, Path, dict[str, object]]:
    payload = _synthetic_video_payload()
    payload_path = tmp_path / "source.safetensors"
    output_path = tmp_path / "result.lc"
    payload_path.write_bytes(payload)
    manifest = _manifest(payload)
    receipt = cartridge.pack(manifest, payload_path, output_path)
    assert receipt["status"] == "ok"
    assert receipt["command"] == "pack"
    return output_path, payload_path, manifest


def test_python_api_roundtrips_through_the_single_rust_implementation(tmp_path: Path) -> None:
    output_path, payload_path, manifest = _pack_synthetic(tmp_path)

    inspection = cartridge.inspect(output_path)
    validation = cartridge.validate(output_path)
    measured = cartridge.hash(output_path)

    assert inspection["validation_level"] == "structure"
    assert inspection["manifest"] == manifest
    assert inspection["safetensors"]["video"]["shape"] == [1, 24, 2, 1, 1]
    assert validation["validation"]["validation_level"] == "full"
    assert validation["cartridge_id"] == manifest["cartridge_id"]
    assert measured["byte_length"] == output_path.stat().st_size
    assert measured["sha256"] == hashlib.sha256(output_path.read_bytes()).hexdigest()
    assert payload_path.read_bytes() == _synthetic_video_payload()


def test_read_h3_returns_validated_tensor_bytes_without_archive_extraction(tmp_path: Path) -> None:
    output_path, _, manifest = _pack_synthetic(tmp_path)

    loaded = cartridge.read_h3(output_path)

    assert loaded["status"] == "ok"
    assert loaded["command"] == "read_h3"
    assert loaded["manifest"] == manifest
    assert loaded["validation"]["validation_level"] == "full"
    assert loaded["tensors"]["video"] == {
        "data": bytes(24 * 2 * 2),
        "dtype": "F16",
        "shape": [1, 24, 2, 1, 1],
    }
    assert "audio" not in loaded["tensors"]
    assert sorted(path.name for path in tmp_path.iterdir()) == ["result.lc", "source.safetensors"]


def test_public_error_carries_stable_code_detail_and_locations(tmp_path: Path) -> None:
    missing = tmp_path / "missing.lc"

    with pytest.raises(cartridge.CartridgeError) as caught:
        cartridge.validate(missing)

    error = caught.value
    assert error.code == "io_open"
    assert error.detail
    assert error.entry is None
    assert error.tensor is None
    assert error.json_pointer is None


def test_pack_requires_explicit_overwrite(tmp_path: Path) -> None:
    output_path, payload_path, manifest = _pack_synthetic(tmp_path)

    with pytest.raises(cartridge.CartridgeError) as caught:
        cartridge.pack(manifest, payload_path, output_path)
    assert caught.value.code == "target_exists"

    receipt = cartridge.pack(manifest, payload_path, output_path, overwrite=True)
    assert receipt["validation"]["validation_level"] == "full"


def test_raw_h3_authoring_uses_the_authoritative_t7_audio_cadence(tmp_path: Path) -> None:
    payload_path = tmp_path / "t7-av.safetensors"
    output_path = tmp_path / "t7-av.lc"
    payload_path.write_bytes(_synthetic_t7_av_payload())

    receipt = cartridge.pack_raw_h3(payload_path, output_path)
    inspection = cartridge.inspect(output_path)

    assert receipt["validation"]["validation_level"] == "full"
    assert inspection["profile"]["visual"]["latent_slots"] == 7
    assert inspection["profile"]["visual"]["decoded_frames"] == 22
    assert inspection["profile"]["audio_latent_slots"] == 37
    assert inspection["manifest"]["audio"]["policy"] == "preserved_source"


def test_raw_h3_inspection_validates_without_creating_a_cartridge(tmp_path: Path) -> None:
    payload_path = tmp_path / "inspect-only.safetensors"
    payload = _synthetic_t7_av_payload()
    payload_path.write_bytes(payload)

    inspection = cartridge.inspect_raw_h3(payload_path)

    assert inspection["status"] == "ok"
    assert inspection["command"] == "inspect_raw_h3"
    assert inspection["byte_length"] == len(payload)
    assert inspection["sha256"] == hashlib.sha256(payload).hexdigest()
    assert inspection["profile"]["visual"]["latent_slots"] == 7
    assert inspection["profile"]["visual"]["decoded_frames"] == 22
    assert inspection["profile"]["audio_latent_slots"] == 37
    assert inspection["safetensors"]["video"]["shape"] == [1, 24, 7, 1, 1]
    assert inspection["safetensors"]["audio"]["shape"] == [1, 32, 2, 37]
    assert list(tmp_path.iterdir()) == [payload_path]


def test_read_raw_h3_returns_av_tensor_bytes_without_converting_to_lc(tmp_path: Path) -> None:
    payload_path = tmp_path / "raw-av.safetensors"
    payload_path.write_bytes(_synthetic_t7_av_payload())

    loaded = cartridge.read_raw_h3(payload_path)

    assert loaded["status"] == "ok"
    assert loaded["command"] == "read_raw_h3"
    assert loaded["profile"]["codec_family"] == "minimax_h3"
    assert loaded["profile"]["profile"] == "h3_av_latent"
    assert loaded["profile"]["visual"]["decoded_frames"] == 22
    assert loaded["profile"]["audio_latent_slots"] == 37
    assert loaded["safetensors"]["video"]["shape"] == [1, 24, 7, 1, 1]
    assert loaded["safetensors"]["audio"]["shape"] == [1, 32, 2, 37]
    assert loaded["tensors"]["video"] == {
        "data": bytes(24 * 7 * 2),
        "dtype": "F16",
        "shape": [1, 24, 7, 1, 1],
    }
    assert loaded["tensors"]["audio"] == {
        "data": bytes(32 * 2 * 37 * 2),
        "dtype": "F16",
        "shape": [1, 32, 2, 37],
    }
    assert list(tmp_path.iterdir()) == [payload_path]


def test_tensor_read_limits_precede_materializing_valid_payloads(tmp_path: Path) -> None:
    payload_path = tmp_path / "bounded-raw-av.safetensors"
    output_path = tmp_path / "bounded.lc"
    payload_path.write_bytes(_synthetic_t7_av_payload())
    cartridge.pack_raw_h3(payload_path, output_path)

    for reader, source in (
        (cartridge.read_raw_h3, payload_path),
        (cartridge.read_h3, output_path),
    ):
        with pytest.raises(cartridge.CartridgeError) as visual_limit:
            reader(source, max_visual_values=(24 * 7) - 1)
        assert visual_limit.value.code == "runtime_limit_exceeded"
        assert visual_limit.value.tensor == "video"

        with pytest.raises(cartridge.CartridgeError) as byte_limit:
            reader(source, max_tensor_bytes=(32 * 2 * 37 * 2) - 1)
        assert byte_limit.value.code == "runtime_limit_exceeded"
        assert byte_limit.value.tensor == "audio"

        loaded = reader(
            source,
            max_visual_values=24 * 7,
            max_tensor_bytes=32 * 2 * 37 * 2,
        )
        assert loaded["status"] == "ok"


def test_raw_h3_default_identity_is_deterministic_uuid_v8(tmp_path: Path) -> None:
    payload_path = tmp_path / "source.safetensors"
    first_output = tmp_path / "first.lc"
    second_output = tmp_path / "second.lc"
    payload_path.write_bytes(_synthetic_video_payload())

    cartridge.pack_raw_h3(payload_path, first_output)
    cartridge.pack_raw_h3(payload_path, second_output)
    first_id = cartridge.inspect(first_output)["manifest"]["cartridge_id"]
    second_id = cartridge.inspect(second_output)["manifest"]["cartridge_id"]
    parsed = uuid.UUID(first_id)

    assert first_id == second_id
    assert parsed.version == 8
    assert parsed.variant == uuid.RFC_4122


def test_raw_h3_native_options_reject_duplicate_provenance_keys(tmp_path: Path) -> None:
    from latentdeck_cartridge import _native

    payload_path = tmp_path / "source.safetensors"
    output_path = tmp_path / "duplicate-options.lc"
    payload_path.write_bytes(_synthetic_video_payload())
    duplicate_options = (
        '{"created_at":"2026-08-30T08:00:00Z",'
        '"created_at":"2026-08-30T09:00:00Z"}'
    )

    with pytest.raises(cartridge.CartridgeError) as caught:
        _native.pack_raw_h3_json(
            str(payload_path),
            str(output_path),
            None,
            None,
            duplicate_options,
            False,
        )

    assert caught.value.code == "manifest_duplicate_key"
    assert caught.value.json_pointer == "/provenance/created_at"


def test_raw_h3_roundtrips_bounded_authoring_provenance_without_raw_prompt(
    tmp_path: Path,
) -> None:
    payload = _synthetic_video_payload()
    payload_path = tmp_path / "recorded.safetensors"
    output_path = tmp_path / "recorded.lc"
    payload_path.write_bytes(payload)
    raw_prompt = "private recorder prompt must not enter the cartridge"
    workflow_sha256 = "a" * 64
    prompt_sha256 = hashlib.sha256(raw_prompt.encode()).hexdigest()

    cartridge.pack_raw_h3(
        payload_path,
        output_path,
        provenance={
            "created_by": {
                "name": "comfyui-latent-cartridge",
                "version": "0.1.0",
            },
            "created_at": "2026-08-30T08:00:00Z",
            "source_kind": "comfyui_h3_latent",
            "source_metadata": {
                "workflow_sha256": workflow_sha256,
                "prompt_sha256": prompt_sha256,
            },
        },
    )
    manifest = cartridge.inspect(output_path)["manifest"]
    source = manifest["provenance"]["sources"][0]

    assert manifest["provenance"]["created_by"] == {
        "name": "comfyui-latent-cartridge",
        "version": "0.1.0",
    }
    assert manifest["provenance"]["created_at"] == "2026-08-30T08:00:00Z"
    assert source["kind"] == "comfyui_h3_latent"
    assert source["sha256"] == hashlib.sha256(payload).hexdigest()
    assert source["metadata"] == {
        "prompt_sha256": prompt_sha256,
        "workflow_sha256": workflow_sha256,
    }
    assert raw_prompt.encode() not in output_path.read_bytes()


def test_raw_h3_rejects_unknown_provenance_fields_with_escaped_pointer(
    tmp_path: Path,
) -> None:
    payload_path = tmp_path / "source.safetensors"
    output_path = tmp_path / "unknown-provenance.lc"
    payload_path.write_bytes(_synthetic_video_payload())

    with pytest.raises(cartridge.CartridgeError) as caught:
        cartridge.pack_raw_h3(
            payload_path,
            output_path,
            provenance={"raw/prompt~text": "must not be accepted"},
        )

    assert caught.value.code == "manifest_unknown_field"
    assert caught.value.json_pointer == "/provenance/raw~1prompt~0text"
    assert not output_path.exists()


def test_raw_h3_rejects_non_utc_authoring_timestamp(tmp_path: Path) -> None:
    payload_path = tmp_path / "source.safetensors"
    output_path = tmp_path / "bad-timestamp.lc"
    payload_path.write_bytes(_synthetic_video_payload())

    with pytest.raises(cartridge.CartridgeError) as caught:
        cartridge.pack_raw_h3(
            payload_path,
            output_path,
            provenance={"created_at": "2026-08-30T15:00:00+07:00"},
        )

    assert caught.value.code == "manifest_invalid"
    assert caught.value.json_pointer == "/provenance/created_at"
    assert not output_path.exists()


def test_latentdeck_pack_console_surface_builds_manifest_automatically(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    from latentdeck_cartridge.__main__ import pack_main

    payload_path = tmp_path / "source.safetensors"
    output_path = tmp_path / "console.lc"
    payload_path.write_bytes(_synthetic_video_payload())

    status = pack_main(
        [str(payload_path), "--profile", "h3", "-o", str(output_path)]
    )

    assert status == 0
    assert json.loads(capsys.readouterr().out)["command"] == "pack"
    inspection = cartridge.inspect(output_path)
    assert inspection["manifest"]["provenance"]["created_by"]["name"] == "latentdeck-pack"
    assert inspection["manifest"]["provenance"]["sources"][0]["kind"] == "raw_h3_safetensors"
    assert cartridge.validate(output_path)["validation"]["validation_level"] == "full"
