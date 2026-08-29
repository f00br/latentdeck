from __future__ import annotations

import json
import sys
import types
from dataclasses import dataclass
from datetime import UTC, datetime
from pathlib import Path
from typing import Any

import pytest
from latentdeck_comfy_cartridge.recorder import (
    H3Recorder,
    RecorderError,
    _comfy_output_directory,
    _write_safetensors,
)

CARTRIDGE_ID = "550e8400-e29b-41d4-a716-446655440000"


@dataclass(frozen=True)
class TensorStub:
    shape: tuple[int, ...]
    dtype: str


class NestedTensorStub:
    is_nested = True

    def __init__(self, *streams: TensorStub) -> None:
        self._streams = streams

    def unbind(self) -> tuple[TensorStub, ...]:
        return self._streams


class RecordingWriter:
    def __init__(self) -> None:
        self.paths: list[Path] = []
        self.tensors: dict[str, object] = {}

    def __call__(self, path: Path, tensors: dict[str, object]) -> None:
        self.paths.append(path)
        self.tensors = dict(tensors)
        path.write_bytes(b"synthetic-safetensors")


class RecordingPack:
    def __init__(self, *, fail: bool = False) -> None:
        self.calls: list[dict[str, Any]] = []
        self.fail = fail

    def __call__(
        self,
        payload_path: Path,
        output_path: Path,
        preview_path: Path | None = None,
        *,
        cartridge_id: str | None = None,
        provenance: dict[str, object] | None = None,
        overwrite: bool = False,
    ) -> dict[str, object]:
        self.calls.append(
            {
                "payload_path": payload_path,
                "output_path": output_path,
                "preview_path": preview_path,
                "cartridge_id": cartridge_id,
                "provenance": provenance,
                "overwrite": overwrite,
            }
        )
        if self.fail:
            raise RuntimeError("synthetic SDK failure")
        output_path.write_bytes(b"synthetic-lc")
        return {
            "status": "ok",
            "command": "pack",
            "output": str(output_path),
            "validation": {
                "validation_level": "full",
                "archive_bytes": len(b"synthetic-lc"),
                "archive_sha256": "0" * 64,
                "payload_bytes": payload_path.stat().st_size,
                "payload_sha256": "1" * 64,
                "visual_runtime_bytes": 1,
            },
        }


class RacingPack(RecordingPack):
    def __call__(self, *args: Any, **kwargs: Any) -> dict[str, object]:
        output_path = args[1]
        output_path.write_bytes(b"raced-sentinel")
        raise RuntimeError("synthetic race")


def make_recorder(tmp_path: Path, writer: RecordingWriter, pack: RecordingPack) -> H3Recorder:
    return H3Recorder(
        pack=pack,
        tensor_writer=writer,
        output_directory=lambda: tmp_path,
        cartridge_id_factory=lambda: CARTRIDGE_ID,
        clock=lambda: datetime(2026, 8, 30, 3, 0, tzinfo=UTC),
    )


def test_records_visual_h3_latent_through_native_authoring_with_safe_provenance(
    tmp_path: Path,
) -> None:
    video = TensorStub((1, 24, 32, 50, 28), "torch.float16")
    latent = {"samples": video, "batch_index": [0]}
    writer = RecordingWriter()
    pack = RecordingPack()
    recorder = make_recorder(tmp_path, writer, pack)

    result = recorder.record(
        latent,
        "cartridge",
        prompt={"node": {"inputs": {"sensitive_text": "synthetic-not-recorded"}}},
    )

    assert result.output_path.exists()
    assert result.receipt["status"] == "ok"
    assert writer.tensors == {"video": video}
    assert len(pack.calls) == 1
    call = pack.calls[0]
    provenance = call["provenance"]
    assert provenance["created_by"] == {
        "name": "comfyui-latent-cartridge",
        "version": "0.1.0",
    }
    assert provenance["created_at"] == "2026-08-30T03:00:00Z"
    assert provenance["source_kind"] == "comfyui_h3_latent"
    metadata = provenance["source_metadata"]
    assert len(metadata["workflow_sha256"]) == 64
    assert "synthetic-not-recorded" not in json.dumps(provenance)
    assert call["cartridge_id"] == CARTRIDGE_ID
    assert call["preview_path"] is None
    assert call["overwrite"] is False
    assert not call["payload_path"].exists()


def test_records_nested_h3_av_with_discriminating_audio_cadence(tmp_path: Path) -> None:
    video = TensorStub((1, 24, 7, 2, 3), "float32")
    audio = TensorStub((1, 32, 2, 37), "float16")
    latent = {"samples": NestedTensorStub(video, audio)}
    writer = RecordingWriter()
    pack = RecordingPack()
    recorder = make_recorder(tmp_path, writer, pack)

    recorder.record(latent, "av")

    assert list(writer.tensors) == ["audio", "video"]
    assert writer.tensors["video"] is video
    assert writer.tensors["audio"] is audio


def test_rejects_floor_based_audio_length_before_writing(tmp_path: Path) -> None:
    video = TensorStub((1, 24, 7, 2, 3), "float32")
    floor_audio = TensorStub((1, 32, 2, 36), "float16")
    writer = RecordingWriter()
    pack = RecordingPack()
    recorder = make_recorder(tmp_path, writer, pack)

    with pytest.raises(RecorderError, match="H3 audio T must be 37"):
        recorder.record({"samples": NestedTensorStub(video, floor_audio)}, "av")

    assert writer.paths == []
    assert pack.calls == []


def test_rejects_invalid_shapes_before_writing(tmp_path: Path) -> None:
    writer = RecordingWriter()
    pack = RecordingPack()
    recorder = make_recorder(tmp_path, writer, pack)
    latent = {"samples": TensorStub((1, 23, 32, 50, 28), "float16")}

    with pytest.raises(RecorderError, match=r"\[1,24,T,H,W\]"):
        recorder.record(latent, "invalid")

    assert writer.paths == []
    assert pack.calls == []
    assert list(tmp_path.rglob("*")) == []


def test_sdk_failure_cleans_temporary_payload_and_leaves_no_output(tmp_path: Path) -> None:
    writer = RecordingWriter()
    pack = RecordingPack(fail=True)
    recorder = make_recorder(tmp_path, writer, pack)
    latent = {"samples": TensorStub((1, 24, 2, 1, 1), "float16")}

    with pytest.raises(RuntimeError, match="synthetic SDK failure"):
        recorder.record(latent, "failure")

    assert len(writer.paths) == 1
    assert not writer.paths[0].exists()
    assert not pack.calls[0]["output_path"].exists()


def test_recorder_never_deletes_a_target_created_during_sdk_race(tmp_path: Path) -> None:
    writer = RecordingWriter()
    pack = RacingPack()
    recorder = make_recorder(tmp_path, writer, pack)
    latent = {"samples": TensorStub((1, 24, 2, 1, 1), "float16")}

    with pytest.raises(RuntimeError, match="synthetic race"):
        recorder.record(latent, "race")

    target = next(tmp_path.rglob("*.lc"))
    assert target.read_bytes() == b"raced-sentinel"
    assert not writer.paths[0].exists()


def test_rejects_output_traversal_without_writing(tmp_path: Path) -> None:
    writer = RecordingWriter()
    pack = RecordingPack()
    recorder = make_recorder(tmp_path, writer, pack)
    latent = {"samples": TensorStub((1, 24, 2, 1, 1), "float16")}

    with pytest.raises(RecorderError, match="relative output prefix"):
        recorder.record(latent, "../outside")

    assert writer.paths == []
    assert pack.calls == []


def test_sanitizes_the_output_basename(tmp_path: Path) -> None:
    writer = RecordingWriter()
    pack = RecordingPack()
    recorder = make_recorder(tmp_path, writer, pack)
    latent = {"samples": TensorStub((1, 24, 2, 1, 1), "float16")}

    result = recorder.record(latent, "my cartridge! 01")

    assert result.output_path.name == f"my_cartridge_01_{CARTRIDGE_ID}.lc"


def test_escapes_windows_reserved_output_basenames(tmp_path: Path) -> None:
    writer = RecordingWriter()
    pack = RecordingPack()
    recorder = make_recorder(tmp_path, writer, pack)
    latent = {"samples": TensorStub((1, 24, 2, 1, 1), "float16")}

    result = recorder.record(latent, "CON.txt")

    assert result.output_path.name == f"_CON.txt_{CARTRIDGE_ID}.lc"


def test_default_output_uses_a_dedicated_comfy_subdirectory(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    folder_paths = types.ModuleType("folder_paths")
    folder_paths.get_output_directory = lambda: str(tmp_path)
    monkeypatch.setitem(sys.modules, "folder_paths", folder_paths)

    assert _comfy_output_directory() == tmp_path / "latentdeck" / "cartridges"


def test_safetensors_writer_never_relayouts_a_tensor(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    calls: dict[str, object] = {}
    safetensors = types.ModuleType("safetensors")
    safetensors.__path__ = []
    safetensors_torch = types.ModuleType("safetensors.torch")

    def save_file(tensors: dict[str, object], path: str, metadata: dict[str, str]) -> None:
        calls.update(tensors=tensors, path=path, metadata=metadata)

    safetensors_torch.save_file = save_file
    monkeypatch.setitem(sys.modules, "safetensors", safetensors)
    monkeypatch.setitem(sys.modules, "safetensors.torch", safetensors_torch)

    class ContiguousTensor:
        def detach(self) -> ContiguousTensor:
            return self

        def is_contiguous(self) -> bool:
            return True

        def contiguous(self) -> None:
            raise AssertionError("Recorder must not make a hidden contiguous copy")

    tensor = ContiguousTensor()
    path = tmp_path / "temporary.safetensors"

    _write_safetensors(path, {"video": tensor})

    assert calls["tensors"] == {"video": tensor}
    assert calls["path"] == str(path)
