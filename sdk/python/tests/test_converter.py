from __future__ import annotations

import json
from pathlib import Path

import latentdeck_cartridge as cartridge


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


def _synthetic_av_payload() -> bytes:
    video = bytes(24 * 7 * 2)
    audio = bytes(32 * 2 * 37 * 2)
    header = json.dumps(
        {
            "video": {
                "data_offsets": [0, len(video)],
                "dtype": "F16",
                "shape": [1, 24, 7, 1, 1],
            },
            "audio": {
                "data_offsets": [len(video), len(video) + len(audio)],
                "dtype": "F16",
                "shape": [1, 32, 2, 37],
            },
        },
        separators=(",", ":"),
    ).encode()
    header += b" " * (-len(header) % 8)
    return len(header).to_bytes(8, "little") + header + video + audio


def test_converter_packs_one_existing_raw_h3_file_without_modifying_it(
    tmp_path: Path, capsys
) -> None:
    from latentdeck_cartridge.converter import convert_main

    source = tmp_path / "old-h3.safetensors"
    output_dir = tmp_path / "cartridges"
    original = _synthetic_video_payload()
    source.write_bytes(original)

    status = convert_main([str(source), "-o", str(output_dir)])

    report = json.loads(capsys.readouterr().out)
    output = output_dir / "old-h3.lc"
    assert status == 0
    assert report["status"] == "ok"
    assert report["converted"] == 1
    assert report["failed"] == 0
    assert report["items"][0]["source"] == str(source.resolve())
    assert report["items"][0]["output"] == str(output.resolve())
    assert source.read_bytes() == original
    assert cartridge.validate(output)["validation"]["validation_level"] == "full"


def test_converter_recurses_only_when_explicit_and_preserves_relative_names(
    tmp_path: Path, capsys
) -> None:
    from latentdeck_cartridge.converter import convert_main

    source_dir = tmp_path / "continuum-run"
    nested = source_dir / "chunks"
    nested.mkdir(parents=True)
    (source_dir / "root.safetensors").write_bytes(_synthetic_video_payload())
    (nested / "child.safetensors").write_bytes(_synthetic_video_payload())
    (nested / "manifest.json").write_text("{}", encoding="utf-8")
    output_dir = tmp_path / "converted"

    status = convert_main(
        [str(source_dir), "--recursive", "--output-directory", str(output_dir)]
    )

    report = json.loads(capsys.readouterr().out)
    assert status == 0
    assert report["converted"] == 2
    assert (output_dir / "root.lc").is_file()
    assert (output_dir / "chunks" / "child.lc").is_file()
    assert not (output_dir / "chunks" / "manifest.lc").exists()


def test_converter_rejects_output_collisions_before_writing_any_cartridge(
    tmp_path: Path, capsys
) -> None:
    from latentdeck_cartridge.converter import convert_main

    first = tmp_path / "run-a" / "clip.safetensors"
    second = tmp_path / "run-b" / "clip.safetensors"
    first.parent.mkdir()
    second.parent.mkdir()
    first.write_bytes(_synthetic_video_payload())
    second.write_bytes(_synthetic_video_payload())
    output_dir = tmp_path / "converted"

    status = convert_main(
        [str(first), str(second), "--output-dir", str(output_dir)]
    )

    report = json.loads(capsys.readouterr().out)
    assert status == 2
    assert report["status"] == "error"
    assert report["code"] == "output_collision"
    assert not (output_dir / "clip.lc").exists()


def test_converter_preserves_existing_h3_av_streams_and_reports_the_profile(
    tmp_path: Path, capsys
) -> None:
    from latentdeck_cartridge.converter import convert_main

    source = tmp_path / "existing-av.safetensors"
    output_dir = tmp_path / "converted"
    source.write_bytes(_synthetic_av_payload())

    status = convert_main([str(source), "--output-dir", str(output_dir)])

    report = json.loads(capsys.readouterr().out)
    output = output_dir / "existing-av.lc"
    inspection = cartridge.inspect(output)
    assert status == 0
    assert report["items"][0]["profile"]["visual"]["latent_slots"] == 7
    assert report["items"][0]["profile"]["audio_latent_slots"] == 37
    assert {tensor["name"] for tensor in inspection["manifest"]["tensors"]} == {
        "video",
        "audio",
    }
    assert inspection["manifest"]["audio"]["policy"] == "preserved_source"


def test_converter_preflights_existing_outputs_before_batch_writes(
    tmp_path: Path, capsys
) -> None:
    from latentdeck_cartridge.converter import convert_main

    first = tmp_path / "first.safetensors"
    second = tmp_path / "second.safetensors"
    first.write_bytes(_synthetic_video_payload())
    second.write_bytes(_synthetic_video_payload())
    output_dir = tmp_path / "converted"
    output_dir.mkdir()
    (output_dir / "second.lc").write_bytes(b"owned-by-user")

    status = convert_main([str(first), str(second), "-o", str(output_dir)])

    report = json.loads(capsys.readouterr().out)
    assert status == 2
    assert report["code"] == "output_exists"
    assert not (output_dir / "first.lc").exists()
    assert (output_dir / "second.lc").read_bytes() == b"owned-by-user"


def test_converter_rejects_an_empty_directory_instead_of_reporting_false_success(
    tmp_path: Path, capsys
) -> None:
    from latentdeck_cartridge.converter import convert_main

    source_dir = tmp_path / "empty"
    source_dir.mkdir()
    output_dir = tmp_path / "converted"

    status = convert_main([str(source_dir), "--output-dir", str(output_dir)])

    report = json.loads(capsys.readouterr().out)
    assert status == 2
    assert report["status"] == "error"
    assert report["code"] == "no_inputs"
    assert not list(output_dir.glob("*.lc"))
