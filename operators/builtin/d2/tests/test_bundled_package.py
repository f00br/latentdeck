from __future__ import annotations

import hashlib
import inspect
import json
import os
import re
import shutil
import subprocess
import sys
import textwrap
import zipfile
from collections import Counter
from pathlib import Path

import torch

PACKAGE_ROOT = Path(__file__).resolve().parents[1] / "package"
REPOSITORY_ROOT = Path(__file__).resolve().parents[4]
IDENTIFIER = re.compile(r"^[a-z][a-z0-9]*(?:[._-][a-z0-9]+)*$")


def strict_json(path: Path) -> dict[str, object]:
    def closed_object(pairs: list[tuple[str, object]]) -> dict[str, object]:
        result: dict[str, object] = {}
        for key, value in pairs:
            assert key not in result, f"duplicate JSON key {key!r} in {path}"
            result[key] = value
        return result

    loaded = json.loads(path.read_text(encoding="utf-8"), object_pairs_hook=closed_object)
    assert isinstance(loaded, dict)
    return loaded


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def extension_manager(*arguments: str) -> dict[str, object]:
    completed = subprocess.run(
        [
            "cargo",
            "run",
            "--quiet",
            "-p",
            "latentdeck-extension-manager",
            "--",
            *arguments,
        ],
        cwd=REPOSITORY_ROOT,
        check=False,
        capture_output=True,
        text=True,
    )
    assert completed.returncode == 0, completed.stderr
    loaded = json.loads(completed.stdout)
    assert isinstance(loaded, dict)
    return loaded


def stage_package(destination: Path, catalog_files: list[dict[str, object]]) -> Path:
    destination.mkdir()
    for relative in ["deck-pack.json", "integrity.json", *[item["path"] for item in catalog_files]]:
        source = PACKAGE_ROOT / relative
        target = destination / relative
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(source, target)
    return destination


def isolated_python(code: str) -> subprocess.CompletedProcess[str]:
    environment = dict(os.environ)
    environment["PYTHONDONTWRITEBYTECODE"] = "1"
    return subprocess.run(
        [sys.executable, "-I", "-S", "-B", "-c", code],
        cwd=REPOSITORY_ROOT,
        env=environment,
        check=False,
        capture_output=True,
        text=True,
    )


def test_d2_package_is_closed_and_faceplate_exposes_the_exact_contract() -> None:
    manifest = strict_json(PACKAGE_ROOT / "deck-pack.json")
    operator = strict_json(PACKAGE_ROOT / "operator.json")
    faceplate = strict_json(PACKAGE_ROOT / "faceplate.json")

    assert (manifest["deck_id"], manifest["deck_version"]) == (
        "org.latentdeck.deck.d2",
        "0.2.1",
    )
    assert manifest["runtime"] == {
        "kind": "python_operator_stream_v1",
        "operator_descriptor_path": "operator.json",
        "python_root": "python",
        "entrypoint": "latentdeck_operator_d2.operator:process_sources_host",
    }
    signal = manifest["signal"]
    assert isinstance(signal, dict)
    assert signal["slots"] == 2
    assert signal["default_permutation"] == ["carrier", "donor"]
    assert signal["profile_allowlist"] is None
    assert "geometry" not in signal
    assert [
        (
            item["batch"],
            item["channels"],
            item["temporal"],
            item["height"],
            item["width"],
            item["dtype"],
            item["device"],
        )
        for item in signal["geometry_allowlist"]
    ] == [
        (1, 24, 1, 50, 28, "fp16", "cuda"),
        (1, 24, 1, 48, 28, "fp16", "cuda"),
        (1, 24, 1, 48, 84, "fp16", "cuda"),
        (1, 24, 1, 30, 45, "fp16", "cuda"),
    ]

    assert set(operator) == {
        "schema_version",
        "deck_operator_api",
        "deck_id",
        "deck_version",
        "operator_id",
        "operator_version",
        "entrypoint",
        "source_count",
        "role_ids",
        "controls",
    }
    assert (operator["operator_id"], operator["operator_version"]) == (
        "org.latentdeck.builtin.ld_d2",
        "0.2.0",
    )
    controls = operator["controls"]
    assert isinstance(controls, list)
    control_by_id = {control["control_id"]: control for control in controls}
    assert len(control_by_id) == len(controls) == 16
    assert "routing" not in control_by_id

    sections = faceplate["sections"]
    assert isinstance(sections, list)
    widgets = [widget for section in sections for widget in section["widgets"]]
    assert {widget["slot_index"] for widget in widgets if widget["kind"] == "source_picker"} == {
        0,
        1,
    }
    role_editors = [widget for widget in widgets if widget["kind"] == "role_editor"]
    assert len(role_editors) == 1
    assert role_editors[0]["role_ids"] == ["carrier", "donor"]

    bindings: list[str] = []
    for widget in widgets:
        if "control_id" in widget:
            control_id = widget["control_id"]
            bindings.append(control_id)
            control = control_by_id[control_id]
            if widget["kind"] == "select":
                values = [option["value"] for option in widget["options"]]
                assert set(values) == set(control["options"])
                assert all(IDENTIFIER.fullmatch(value) for value in values)
            elif widget["kind"] in {"slider", "number"}:
                assert (widget["minimum"], widget["maximum"], widget["step"]) == (
                    control["minimum"],
                    control["maximum"],
                    control["step"],
                )
    assert Counter(bindings) == Counter({control_id: 1 for control_id in control_by_id})


def test_d2_integrity_catalog_and_deterministic_ld_are_exact(tmp_path: Path) -> None:
    manifest = strict_json(PACKAGE_ROOT / "deck-pack.json")
    catalog = strict_json(PACKAGE_ROOT / "integrity.json")
    catalog_files = catalog["files"]
    assert isinstance(catalog_files, list)
    catalog_paths = {record["path"] for record in catalog_files}
    assert not catalog_paths & {
        "python/latentdeck_operator_d2/descriptor.json",
        "python/latentdeck_operator_d2/descriptor.py",
        "python/latentdeck_operator_d2/descriptor.schema.json",
        "python/latentdeck_operator_d2/stream.py",
        "python/latentdeck_operator_d2/trusted.py",
    }
    assert [record["path"] for record in catalog_files] == sorted(
        record["path"] for record in catalog_files
    )
    actual_paths = sorted(
        path.relative_to(PACKAGE_ROOT).as_posix()
        for path in PACKAGE_ROOT.rglob("*")
        if path.is_file()
        and "__pycache__" not in path.parts
        and path.suffix != ".pyc"
        and path.name not in {"deck-pack.json", "integrity.json"}
    )
    assert [record["path"] for record in catalog_files] == actual_paths
    for record in catalog_files:
        path = PACKAGE_ROOT / record["path"]
        assert record["byte_length"] == path.stat().st_size
        assert record["sha256"] == sha256(path)
    assert manifest["integrity"]["catalog_sha256"] == sha256(PACKAGE_ROOT / "integrity.json")

    staged = stage_package(tmp_path / "source", catalog_files)
    first = tmp_path / "d2-first.ld"
    second = tmp_path / "d2-second.ld"
    first_receipt = extension_manager("pack", "--source", str(staged), "--output", str(first))
    second_receipt = extension_manager("pack", "--source", str(staged), "--output", str(second))
    assert first.read_bytes() == second.read_bytes()
    assert first_receipt["inspection"]["archive_sha256"] == sha256(first)
    assert second_receipt["inspection"]["archive_sha256"] == sha256(second)
    inspected = extension_manager(
        "inspect", "--archive", str(first), "--expected-sha256", sha256(first)
    )
    assert inspected["package"] == {
        "kind": "deck_pack",
        "package_id": "org.latentdeck.deck.d2",
        "package_version": "0.2.1",
    }
    assert inspected["file_count"] == 8

    implementation_path = "python/latentdeck_operator_d2/operator.py"
    unpacked = tmp_path / "unpacked"
    with zipfile.ZipFile(first) as archive:
        assert (
            archive.read(implementation_path) == (PACKAGE_ROOT / implementation_path).read_bytes()
        )
        archive.extractall(unpacked)
    package_python = unpacked / "python"
    deck_sdk = REPOSITORY_ROOT / "sdk" / "deck-python" / "src"
    torch_site = Path(torch.__file__).resolve().parents[1]
    code = textwrap.dedent(
        f"""
        import pathlib
        import sys

        package_python = pathlib.Path({str(package_python)!r}).resolve()
        sys.path[:0] = [{str(package_python)!r}, {str(deck_sdk)!r}, {str(torch_site)!r}]

        import torch
        from latentdeck_deck_sdk import DeckOperatorContext, RoleBinding
        import latentdeck_operator_d2

        loaded = pathlib.Path(latentdeck_operator_d2.__file__).resolve()
        assert loaded.is_relative_to(package_python), loaded
        assert "latentdeck_codec_host" not in sys.modules
        assert not hasattr(latentdeck_operator_d2, "process_slot")
        assert not hasattr(latentdeck_operator_d2, "D2Context")
        index = torch.arange(96, dtype=torch.float32).reshape(1, 8, 1, 3, 4)
        sources = (torch.sin(index).contiguous(), torch.cos(index).contiguous())
        context = DeckOperatorContext(
            codec_family="synthetic",
            profile="test_latent",
            profile_version="0.1.0",
            timing_contract="synthetic_causal",
            timing_contract_version="0.1.0",
            frame_rate_numerator=24,
            frame_rate_denominator=1,
            generation=1,
            sequence=1,
            seed=17,
            playheads=(0, 0),
            physical_slots=(1, 2),
            roles=(RoleBinding("carrier", 1), RoleBinding("donor", 2)),
            previous_sources=(None, None),
        )
        result = latentdeck_operator_d2.process_sources(
            sources,
            {{"algorithm": "xs5", "top_k": 4, "interaction": 0.5}},
            context,
        )
        assert result.output.shape == sources[0].shape
        assert result.output.dtype == sources[0].dtype
        assert result.output.is_contiguous()
        for name, module in tuple(sys.modules.items()):
            if name.startswith("latentdeck_operator_d2") and getattr(module, "__file__", None):
                assert pathlib.Path(module.__file__).resolve().is_relative_to(package_python)
        """
    )
    completed = isolated_python(code)
    assert completed.returncode == 0, completed.stderr


def test_d2_wheel_build_uses_the_same_authoritative_package_tree(tmp_path: Path) -> None:
    from latentdeck_operator_d2 import process_sources

    source = PACKAGE_ROOT / "python" / "latentdeck_operator_d2" / "operator.py"
    imported_source = Path(inspect.getfile(inspect.unwrap(process_sources))).resolve()
    assert process_sources.__module__ == "latentdeck_operator_d2.operator"
    assert imported_source.read_bytes() == source.read_bytes()

    wheel_directory = tmp_path / "wheel"
    completed = subprocess.run(
        [
            "uv",
            "build",
            "--package",
            "latentdeck-operator-d2",
            "--wheel",
            "--out-dir",
            str(wheel_directory),
        ],
        cwd=REPOSITORY_ROOT,
        check=False,
        capture_output=True,
        text=True,
    )
    assert completed.returncode == 0, completed.stderr
    wheel = next(wheel_directory.glob("latentdeck_operator_d2-0.2.0-*.whl"))
    with zipfile.ZipFile(wheel) as archive:
        assert archive.read("latentdeck_operator_d2/operator.py") == source.read_bytes()
        assert "latentdeck_operator_d2/stream.py" not in archive.namelist()
        assert "latentdeck_operator_d2/trusted.py" not in archive.namelist()

    deck_sdk = REPOSITORY_ROOT / "sdk" / "deck-python" / "src"
    torch_site = Path(torch.__file__).resolve().parents[1]
    code = textwrap.dedent(
        f"""
        import pathlib
        import sys
        wheel = pathlib.Path({str(wheel)!r}).resolve()
        sys.path[:0] = [str(wheel), {str(deck_sdk)!r}, {str(torch_site)!r}]
        import latentdeck_operator_d2
        assert str(wheel) in latentdeck_operator_d2.__file__
        assert latentdeck_operator_d2.DECK_ID == "org.latentdeck.deck.d2"
        assert "latentdeck_codec_host" not in sys.modules
        """
    )
    imported = isolated_python(code)
    assert imported.returncode == 0, imported.stderr
