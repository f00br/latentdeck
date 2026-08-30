from __future__ import annotations

import hashlib
import sys
import types
from pathlib import Path

import latentdeck_cartridge as cartridge_sdk
import pytest
import torch
from safetensors.torch import save_file

from latentdeck_comfy_toolkit.cartridge_io import (
    ToolkitIOError,
    import_raw_h3,
    inspect_lc,
    inspect_raw_h3,
    load_lc,
    parent_cartridge_ref,
)


def _manifest() -> dict[str, object]:
    return {
        "spec_version": "0.1.0",
        "cartridge_id": "550e8400-e29b-41d4-a716-446655440000",
        "codec": {
            "family": "minimax_h3",
            "profile": "h3_av_latent",
            "profile_version": "0.1.0",
        },
        "payloads": [],
        "tensors": [
            {
                "stream": "visual",
                "name": "video",
                "storage_dtype": "F32",
                "runtime_dtype": "F16",
                "shape": [1, 24, 2, 1, 1],
            },
            {
                "stream": "audio",
                "name": "audio",
                "storage_dtype": "F32",
                "runtime_dtype": "F32",
                "shape": [1, 32, 2, 8],
            },
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
        "audio": {"policy": "preserved_source"},
        "provenance": {
            "created_by": {"name": "test", "version": "0.1.0"},
            "sources": [],
        },
        "parent_cartridges": [],
        "operation_history": [],
    }


class FakeSdk:
    def __init__(self) -> None:
        video = torch.arange(48, dtype=torch.float32).reshape(1, 24, 2, 1, 1)
        audio = torch.arange(512, dtype=torch.float32).reshape(1, 32, 2, 8)
        self.response = {
            "status": "ok",
            "command": "read_h3",
            "manifest": _manifest(),
            "validation": {
                "validation_level": "full",
                "archive_bytes": 4096,
                "archive_sha256": "a" * 64,
                "payload_bytes": 2240,
                "payload_sha256": "b" * 64,
                "visual_runtime_bytes": 96,
            },
            "tensors": {
                "video": {
                    "data": video.numpy().tobytes(),
                    "dtype": "F32",
                    "shape": [1, 24, 2, 1, 1],
                },
                "audio": {
                    "data": audio.numpy().tobytes(),
                    "dtype": "F32",
                    "shape": [1, 32, 2, 8],
                },
            },
        }

    def read_h3(
        self,
        path: Path,
        *,
        max_visual_values: int | None = None,
        max_tensor_bytes: int | None = None,
    ) -> dict[str, object]:
        assert path.name == "source.lc"
        assert max_visual_values == 50_331_648
        assert max_tensor_bytes == 201_326_592
        return self.response


def test_load_lc_returns_validated_h3_av_latent_and_complete_inspection(tmp_path: Path) -> None:
    loaded = load_lc(tmp_path / "source.lc", sdk=FakeSdk())

    streams = loaded.latent["samples"].unbind()
    assert len(streams) == 2
    assert streams[0].shape == (1, 24, 2, 1, 1)
    assert streams[0].dtype == torch.float16
    assert streams[1].shape == (1, 32, 2, 8)
    assert streams[1].dtype == torch.float32
    assert loaded.report["codec"] == _manifest()["codec"]
    assert loaded.report["provenance"] == _manifest()["provenance"]
    assert loaded.report["validation"]["validation_level"] == "full"
    assert loaded.report["archive"]["sha256"] == "a" * 64
    assert loaded.report["runtime_casts"] == [
        {
            "tensor": "video",
            "from": "F32",
            "to": "F16",
            "authority": "minimax_h3/h3_av_latent/0.1.0",
        }
    ]
    assert loaded.report["compatibility"]["status"] == "compatible"


class FakeInspectSdk:
    def inspect(self, path: Path) -> dict[str, object]:
        assert path.name == "inspect-only.lc"
        return {
            "status": "ok",
            "validation_level": "structure",
            "manifest": _manifest(),
            "safetensors": {
                "video": {"dtype": "F32", "shape": [1, 24, 2, 1, 1]},
                "audio": {"dtype": "F32", "shape": [1, 32, 2, 8]},
            },
        }

    def validate(self, path: Path) -> dict[str, object]:
        assert path.name == "inspect-only.lc"
        return {
            "status": "ok",
            "validation": {
                "validation_level": "full",
                "archive_bytes": 4096,
                "archive_sha256": "c" * 64,
                "payload_bytes": 2240,
                "payload_sha256": "d" * 64,
                "visual_runtime_bytes": 96,
            },
        }

    def read_h3(self, _path: Path, **_limits: object) -> dict[str, object]:
        raise AssertionError("inspection must not materialize tensor bytes")


def test_inspect_lc_fully_validates_without_materializing_tensors(tmp_path: Path) -> None:
    report = inspect_lc(tmp_path / "inspect-only.lc", sdk=FakeInspectSdk())

    assert report["validation"]["validation_level"] == "full"
    assert report["archive"] == {"sha256": "c" * 64, "byte_length": 4096}
    assert report["tensor_headers"] == {
        "video": {"dtype": "F32", "shape": [1, 24, 2, 1, 1]},
        "audio": {"dtype": "F32", "shape": [1, 32, 2, 8]},
    }
    assert report["runtime_casts"] == []


class FakeRawSdk:
    def __init__(self) -> None:
        video = torch.arange(48, dtype=torch.float32).reshape(1, 24, 2, 1, 1)
        audio = torch.arange(512, dtype=torch.float16).reshape(1, 32, 2, 8)
        self.video = video.numpy().tobytes()
        self.audio = audio.numpy().tobytes()

    def inspect_raw_h3(self, path: Path) -> dict[str, object]:
        assert path.name == "legacy.safetensors"
        return {
            "status": "ok",
            "command": "inspect_raw_h3",
            "byte_length": len(self.video) + len(self.audio) + 128,
            "sha256": "e" * 64,
            "profile": {
                "codec_family": "minimax_h3",
                "profile": "h3_av_latent",
                "profile_version": "0.1.0",
                "visual": {
                    "latent_slots": 2,
                    "latent_height": 1,
                    "latent_width": 1,
                    "decoded_frames": 5,
                    "decoded_height": 16,
                    "decoded_width": 16,
                },
                "audio_latent_slots": 8,
            },
            "safetensors": {
                "video": {"dtype": "F32", "shape": [1, 24, 2, 1, 1]},
                "audio": {"dtype": "F16", "shape": [1, 32, 2, 8]},
            },
        }

    def read_raw_h3(
        self,
        path: Path,
        *,
        max_visual_values: int | None = None,
        max_tensor_bytes: int | None = None,
    ) -> dict[str, object]:
        assert max_visual_values == 50_331_648
        assert max_tensor_bytes == 201_326_592
        inspection = self.inspect_raw_h3(path)
        return {
            **inspection,
            "command": "read_raw_h3",
            "tensors": {
                "video": {
                    "data": self.video,
                    "dtype": "F32",
                    "shape": [1, 24, 2, 1, 1],
                },
                "audio": {
                    "data": self.audio,
                    "dtype": "F16",
                    "shape": [1, 32, 2, 8],
                },
            },
        }


def test_raw_h3_import_is_direct_and_preserves_av_without_an_intermediate_lc(
    tmp_path: Path,
) -> None:
    imported = import_raw_h3(tmp_path / "legacy.safetensors", sdk=FakeRawSdk())

    video, audio = imported.latent["samples"].unbind()
    assert video.dtype == torch.float16
    assert audio.dtype == torch.float16
    assert imported.report["source_kind"] == "raw_h3_safetensors"
    assert imported.report["source"] == {
        "sha256": "e" * 64,
        "byte_length": len(FakeRawSdk().video) + len(FakeRawSdk().audio) + 128,
    }
    assert imported.report["validation"]["validation_level"] == "full"
    assert imported.latent["latentdeck"]["source_kind"] == "raw_h3_safetensors"


def test_raw_h3_inspection_validates_and_hashes_without_loading_tensor_bytes(
    tmp_path: Path,
) -> None:
    sdk = FakeRawSdk()
    sdk.read_raw_h3 = lambda _path: (_ for _ in ()).throw(
        AssertionError("inspection must not materialize tensors")
    )

    report = inspect_raw_h3(tmp_path / "legacy.safetensors", sdk=sdk)

    assert report["validation"] == {"validation_level": "full"}
    assert report["source"]["sha256"] == "e" * 64
    assert report["profile"]["visual"]["decoded_frames"] == 5
    assert report["tensor_headers"]["audio"]["shape"] == [1, 32, 2, 8]


def test_av_load_uses_the_running_comfy_nested_tensor_type(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    comfy = types.ModuleType("comfy")
    comfy.__path__ = []
    nested_module = types.ModuleType("comfy.nested_tensor")

    class NativeNestedTensor:
        def __init__(self, tensors: tuple[torch.Tensor, torch.Tensor]) -> None:
            self.tensors = list(tensors)
            self.is_nested = True

        def unbind(self) -> list[torch.Tensor]:
            return self.tensors

    nested_module.NestedTensor = NativeNestedTensor
    monkeypatch.setitem(sys.modules, "comfy", comfy)
    monkeypatch.setitem(sys.modules, "comfy.nested_tensor", nested_module)

    loaded = load_lc(tmp_path / "source.lc", sdk=FakeSdk())

    assert isinstance(loaded.latent["samples"], NativeNestedTensor)


def test_raw_h3_import_roundtrips_through_the_authoritative_rust_reader(tmp_path: Path) -> None:
    source = tmp_path / "raw.safetensors"
    save_file(
        {
            "audio": torch.zeros((1, 32, 2, 8), dtype=torch.float16),
            "video": torch.zeros((1, 24, 2, 1, 1), dtype=torch.float32),
        },
        str(source),
    )

    imported = import_raw_h3(source)

    video, audio = imported.latent["samples"].unbind()
    assert video.dtype == torch.float16
    assert audio.dtype == torch.float16
    assert imported.report["profile"]["visual"]["decoded_frames"] == 5
    assert imported.report["source"]["sha256"] == cartridge_sdk_hash(source)


def cartridge_sdk_hash(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def test_lc_load_roundtrips_through_the_authoritative_retained_handle_reader(
    tmp_path: Path,
) -> None:
    source = tmp_path / "source.safetensors"
    cartridge = tmp_path / "source.lc"
    save_file(
        {
            "audio": torch.zeros((1, 32, 2, 8), dtype=torch.float16),
            "video": torch.zeros((1, 24, 2, 1, 1), dtype=torch.float32),
        },
        str(source),
    )
    cartridge_sdk.pack_raw_h3(source, cartridge)

    loaded = load_lc(cartridge)

    video, audio = loaded.latent["samples"].unbind()
    assert video.dtype == torch.float16
    assert audio.dtype == torch.float16
    assert loaded.report["validation"]["validation_level"] == "full"
    assert loaded.report["archive"]["sha256"] == cartridge_sdk.hash(cartridge)["sha256"]


def test_loaded_lc_exposes_an_exact_genealogy_reference(tmp_path: Path) -> None:
    loaded = load_lc(tmp_path / "source.lc", sdk=FakeSdk())

    assert parent_cartridge_ref(loaded.latent, role="donor_b") == {
        "cartridge_id": "550e8400-e29b-41d4-a716-446655440000",
        "archive_sha256": "a" * 64,
        "role": "donor_b",
    }


def test_lc_load_rejects_a_valid_but_oversized_research_tensor_before_byte_read(
    tmp_path: Path,
) -> None:
    class OversizedSdk:
        def read_h3(
            self,
            _path: Path,
            *,
            max_visual_values: int | None = None,
            max_tensor_bytes: int | None = None,
        ) -> dict[str, object]:
            assert max_visual_values == 50_331_648
            assert max_tensor_bytes == 201_326_592
            raise ToolkitIOError(
                "runtime_limit_exceeded", "visual tensor exceeds max_visual_values before read"
            )

    with pytest.raises(ToolkitIOError) as caught:
        load_lc(tmp_path / "oversized.lc", sdk=OversizedSdk())

    assert caught.value.code == "runtime_limit_exceeded"
