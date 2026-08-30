from __future__ import annotations

from pathlib import Path

import latentdeck_cartridge as cartridge_sdk
import pytest
import torch

from latentdeck_comfy_toolkit.cartridge_io import (
    H3AVSamples,
    ToolkitIOError,
    save_resampled_lc,
)
from latentdeck_comfy_toolkit.workflow_metadata import (
    annotate_operation,
    initialize_lc_metadata,
)

CARTRIDGE_ID = "550e8400-e29b-41d4-a716-446655440001"
PARENT = {
    "cartridge_id": "550e8400-e29b-41d4-a716-446655440000",
    "archive_sha256": "a" * 64,
    "role": "carrier",
}
OPERATION = {
    "operator_id": "org.latentdeck.builtin.ld_d2",
    "operator_version": "0.1.0",
    "seed": 17,
    "controls": {"algorithm": "XS5", "mode": "HYBRIDIZE", "mix": 0.4},
}
AUDIO = {
    "policy": "copied_from_carrier_exact",
    "source_cartridge": {
        "cartridge_id": PARENT["cartridge_id"],
        "archive_sha256": PARENT["archive_sha256"],
    },
}


class RecordingWriter:
    def __init__(self) -> None:
        self.path: Path | None = None
        self.tensors: dict[str, torch.Tensor] = {}

    def __call__(self, path: Path, tensors: dict[str, torch.Tensor]) -> None:
        self.path = path
        self.tensors = dict(tensors)
        path.write_bytes(b"synthetic-payload")


class RecordingSdk:
    def __init__(self) -> None:
        self.base_path: Path | None = None
        self.final_manifest: dict[str, object] | None = None

    def pack_raw_h3(
        self,
        payload_path: Path,
        output_path: Path,
        _preview_path: Path | None = None,
        *,
        cartridge_id: str | None = None,
        provenance: dict[str, object] | None = None,
        overwrite: bool = False,
    ) -> dict[str, object]:
        assert payload_path.read_bytes() == b"synthetic-payload"
        assert cartridge_id == CARTRIDGE_ID
        assert provenance is not None
        assert overwrite is False
        self.base_path = output_path
        output_path.write_bytes(b"temporary-base-lc")
        return {"status": "ok", "validation": {"validation_level": "full"}}

    def inspect(self, path: Path) -> dict[str, object]:
        assert path == self.base_path
        return {
            "status": "ok",
            "manifest": {
                "spec_version": "0.1.0",
                "cartridge_id": CARTRIDGE_ID,
                "codec": {
                    "family": "minimax_h3",
                    "profile": "h3_av_latent",
                    "profile_version": "0.1.0",
                },
                "payloads": [],
                "tensors": [],
                "timing": {},
                "audio": {"policy": "preserved_source"},
                "provenance": {
                    "created_by": {"name": "latentdeck-comfy-toolkit", "version": "0.1.0"},
                    "sources": [],
                },
                "parent_cartridges": [],
                "operation_history": [],
            },
        }

    def pack(
        self,
        manifest: dict[str, object],
        payload_path: Path,
        output_path: Path,
        _preview_path: Path | None = None,
        *,
        overwrite: bool = False,
    ) -> dict[str, object]:
        assert payload_path.read_bytes() == b"synthetic-payload"
        assert overwrite is False
        self.final_manifest = manifest
        output_path.write_bytes(b"final-lc")
        return {
            "status": "ok",
            "output": str(output_path),
            "validation": {"validation_level": "full", "archive_sha256": "f" * 64},
        }


def test_resample_writes_genealogy_operator_history_and_explicit_audio_policy(
    tmp_path: Path,
) -> None:
    video = torch.zeros((1, 24, 2, 1, 1), dtype=torch.float16)
    audio = torch.zeros((1, 32, 2, 8), dtype=torch.float32)
    writer = RecordingWriter()
    sdk = RecordingSdk()

    saved = save_resampled_lc(
        {"samples": H3AVSamples((video, audio))},
        tmp_path / "resampled.lc",
        parent_cartridges=[PARENT],
        operation_history=[OPERATION],
        audio_disposition=AUDIO,
        cartridge_id=CARTRIDGE_ID,
        sdk=sdk,
        tensor_writer=writer,
    )

    assert saved.output_path.read_bytes() == b"final-lc"
    assert saved.receipt["validation"]["validation_level"] == "full"
    assert sdk.final_manifest is not None
    assert sdk.final_manifest["parent_cartridges"] == [PARENT]
    assert sdk.final_manifest["operation_history"] == [OPERATION]
    assert sdk.final_manifest["audio"] == AUDIO
    assert sdk.final_manifest["provenance"]["created_by"] == {
        "name": "latentdeck-comfy-toolkit",
        "version": "0.1.0",
    }
    assert writer.tensors == {"audio": audio, "video": video}
    assert writer.path is not None and not writer.path.exists()
    assert sdk.base_path is not None and not sdk.base_path.exists()


def test_resample_derives_genealogy_history_and_audio_from_workflow_metadata(
    tmp_path: Path,
) -> None:
    sdk = RecordingSdk()
    source = initialize_lc_metadata(
        {
            "samples": H3AVSamples(
                (
                    torch.zeros((1, 24, 2, 1, 1), dtype=torch.float16),
                    torch.zeros((1, 32, 2, 8), dtype=torch.float32),
                )
            )
        },
        manifest={
            "cartridge_id": PARENT["cartridge_id"],
            "codec": {},
            "timing": {},
            "audio": {"policy": "preserved_source"},
        },
        validation={"archive_sha256": PARENT["archive_sha256"]},
    )
    operated = annotate_operation(
        source,
        sources=(("carrier", source),),
        structural_role="carrier",
        provenance={"operation": OPERATION},
    )

    save_resampled_lc(
        operated,
        tmp_path / "automatic.lc",
        cartridge_id=CARTRIDGE_ID,
        sdk=sdk,
        tensor_writer=RecordingWriter(),
    )

    assert sdk.final_manifest is not None
    assert sdk.final_manifest["parent_cartridges"] == [PARENT]
    assert sdk.final_manifest["operation_history"] == [OPERATION]
    assert sdk.final_manifest["audio"] == AUDIO


def test_resample_rejects_f32_visual_instead_of_hiding_a_storage_cast(tmp_path: Path) -> None:
    video = torch.zeros((1, 24, 2, 1, 1), dtype=torch.float32)

    with pytest.raises(ToolkitIOError) as caught:
        save_resampled_lc(
            {"samples": video},
            tmp_path / "invalid.lc",
            parent_cartridges=[PARENT],
            operation_history=[OPERATION],
            audio_disposition={"policy": "source_absent"},
            sdk=RecordingSdk(),
            tensor_writer=RecordingWriter(),
        )

    assert caught.value.code == "resample.video_dtype_invalid"
    assert not (tmp_path / "invalid.lc").exists()


def test_resample_roundtrips_through_the_authoritative_rust_sdk(tmp_path: Path) -> None:
    output = tmp_path / "authoritative.lc"
    saved = save_resampled_lc(
        {"samples": torch.zeros((1, 24, 2, 1, 1), dtype=torch.float16)},
        output,
        parent_cartridges=[PARENT],
        operation_history=[OPERATION],
        audio_disposition={"policy": "source_absent"},
        cartridge_id=CARTRIDGE_ID,
    )

    inspection = cartridge_sdk.inspect(output)
    assert saved.receipt["validation"]["validation_level"] == "full"
    assert inspection["manifest"]["parent_cartridges"] == [PARENT]
    assert inspection["manifest"]["operation_history"] == [OPERATION]
    assert cartridge_sdk.validate(output)["validation"]["validation_level"] == "full"


def test_resample_cannot_relabel_an_explicit_audio_drop_as_source_absent(tmp_path: Path) -> None:
    video = torch.zeros((1, 24, 2, 1, 1), dtype=torch.float16)
    latent = {
        "samples": video,
        "latentdeck": {
            "operation_chain": [
                {
                    "kind": "latentdeck.toolkit.explicit_crop",
                    "audio_action": "dropped_explicitly",
                }
            ]
        },
    }

    with pytest.raises(ToolkitIOError) as caught:
        save_resampled_lc(
            latent,
            tmp_path / "mislabelled.lc",
            parent_cartridges=[PARENT],
            operation_history=[OPERATION],
            audio_disposition={"policy": "source_absent"},
            sdk=RecordingSdk(),
            tensor_writer=RecordingWriter(),
        )

    assert caught.value.code == "resample.audio_policy_invalid"


def test_resample_preserves_raw_import_identity_as_provenance_not_fake_parent(
    tmp_path: Path,
) -> None:
    sdk = RecordingSdk()
    raw_sha256 = "b" * 64
    save_resampled_lc(
        {
            "samples": torch.zeros((1, 24, 2, 1, 1), dtype=torch.float16),
            "latentdeck": {
                "source_kind": "raw_h3_safetensors",
                "source": {"sha256": raw_sha256, "byte_length": 1024},
            },
        },
        tmp_path / "raw-derived.lc",
        parent_cartridges=[],
        operation_history=[OPERATION],
        audio_disposition={"policy": "source_absent"},
        cartridge_id=CARTRIDGE_ID,
        sdk=sdk,
        tensor_writer=RecordingWriter(),
    )

    assert sdk.final_manifest is not None
    assert sdk.final_manifest["parent_cartridges"] == []
    assert sdk.final_manifest["provenance"]["sources"] == [
        {
            "kind": "raw_h3_safetensors",
            "sha256": raw_sha256,
            "metadata": {"byte_length": 1024},
        }
    ]


def test_raw_av_can_resample_without_a_fake_cartridge_parent(tmp_path: Path) -> None:
    sdk = RecordingSdk()
    video = torch.zeros((1, 24, 2, 1, 1), dtype=torch.float16)
    audio = torch.zeros((1, 32, 2, 8), dtype=torch.float32)

    save_resampled_lc(
        {
            "samples": H3AVSamples((video, audio)),
            "latentdeck": {
                "source_kind": "raw_h3_safetensors",
                "source": {"sha256": "c" * 64, "byte_length": 4096},
            },
        },
        tmp_path / "raw-av-derived.lc",
        parent_cartridges=[],
        operation_history=[OPERATION],
        audio_disposition={"policy": "preserved_source"},
        cartridge_id=CARTRIDGE_ID,
        sdk=sdk,
        tensor_writer=RecordingWriter(),
    )

    assert sdk.final_manifest is not None
    assert sdk.final_manifest["audio"] == {"policy": "preserved_source"}
    assert sdk.final_manifest["parent_cartridges"] == []


def test_raw_av_resample_is_accepted_by_the_authoritative_profile_validator(
    tmp_path: Path,
) -> None:
    output = tmp_path / "raw-av-authoritative.lc"
    raw_sha256 = "d" * 64
    save_resampled_lc(
        {
            "samples": H3AVSamples(
                (
                    torch.zeros((1, 24, 2, 1, 1), dtype=torch.float16),
                    torch.zeros((1, 32, 2, 8), dtype=torch.float32),
                )
            ),
            "latentdeck": {
                "source_kind": "raw_h3_safetensors",
                "source": {"sha256": raw_sha256, "byte_length": 4096},
            },
        },
        output,
        parent_cartridges=[],
        operation_history=[OPERATION],
        audio_disposition={"policy": "preserved_source"},
        cartridge_id=CARTRIDGE_ID,
    )

    manifest = cartridge_sdk.inspect(output)["manifest"]
    assert manifest["audio"] == {"policy": "preserved_source"}
    assert manifest["provenance"]["sources"] == [
        {
            "kind": "raw_h3_safetensors",
            "sha256": raw_sha256,
            "metadata": {"byte_length": 4096},
        }
    ]
