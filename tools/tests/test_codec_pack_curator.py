from __future__ import annotations

import base64
import csv
import hashlib
import importlib.util
import io
import json
import zipfile
from pathlib import Path

import pytest

MODULE_PATH = Path(__file__).parents[1] / "codec_pack_curator.py"
SPEC = importlib.util.spec_from_file_location("codec_pack_curator", MODULE_PATH)
assert SPEC is not None and SPEC.loader is not None
curator = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(curator)


def _record_hash(payload: bytes) -> str:
    encoded = base64.urlsafe_b64encode(hashlib.sha256(payload).digest()).rstrip(b"=")
    return f"sha256={encoded.decode('ascii')}"


def _content_digest(files: dict[str, bytes]) -> str:
    rows = [
        f"{path}\0{hashlib.sha256(payload).hexdigest()}\0{len(payload)}"
        for path, payload in sorted(files.items())
    ]
    return hashlib.sha256(("\n".join(rows) + "\n").encode()).hexdigest()


def _write_distribution(
    root: Path,
    *,
    name: str,
    version: str,
    package_name: str,
    license_expression: str,
) -> tuple[str, str]:
    dist_info = f"{name.replace('-', '_')}-{version}.dist-info"
    files = {
        f"{package_name}/__init__.py": f'__version__ = "{version}"\n'.encode(),
        f"{dist_info}/METADATA": (
            "Metadata-Version: 2.4\n"
            f"Name: {name}\n"
            f"Version: {version}\n"
            f"License-Expression: {license_expression}\n"
            "License-File: LICENSE.txt\n"
            "Project-URL: Source, https://example.invalid/source\n\n"
        ).encode(),
        f"{dist_info}/licenses/LICENSE.txt": b"Synthetic redistributable license text.\n",
        f"{dist_info}/WHEEL": b"Wheel-Version: 1.0\nTag: py3-none-any\n",
    }
    for relative, payload in files.items():
        path = root / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(payload)

    generated = {
        f"{dist_info}/INSTALLER": b"uv\n",
        f"{dist_info}/REQUESTED": b"",
        f"{dist_info}/direct_url.json": b'{"url":"file:///private/build.whl"}\n',
        f"{dist_info}/uv_cache.json": b'{"timestamp":1}\n',
        f"{dist_info}/DELVEWHEEL": b"Arguments: ['C:\\\\Users\\\\builder\\\\wheel']\n",
        f"{dist_info}/sboms/build.json": b'{"source":"path+file:///C:/private/source"}\n',
        f"bin/{package_name}.exe": b"generated launcher",
    }
    for relative, payload in generated.items():
        path = root / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(payload)

    rows: list[list[str]] = []
    for relative, payload in sorted({**files, **generated}.items()):
        rows.append([relative, _record_hash(payload), str(len(payload))])
    rows.append([f"{dist_info}/RECORD", "", ""])
    buffer = io.StringIO(newline="")
    csv.writer(buffer, lineterminator="\n").writerows(rows)
    (root / dist_info / "RECORD").write_text(buffer.getvalue(), encoding="utf-8")
    return dist_info, _content_digest(files)


def _lock(*, content_sha256: str) -> dict[str, object]:
    return {
        "schema_version": 1,
        "platform": "windows-x86_64",
        "uv_version": "0.11.8",
        "python_runtime": {
            "version": "3.13.14",
            "archive_filename": "python-3.13.14-embed-amd64.zip",
            "source_url": "https://www.python.org/ftp/python/3.13.14/python-3.13.14-embed-amd64.zip",
            "sha256": "0" * 64,
            "license_expression": "PSF-2.0",
            "license_path": "LICENSE.txt",
            "prune_entries": ["ctypes/macholib/fetch_macholib.bat"],
        },
        "dependencies": [
            {
                "name": "demo-runtime",
                "version": "1.2.3",
                "source_url": "https://pypi.org/project/demo-runtime/1.2.3/",
                "license_expression": "MIT",
                "content_sha256": content_sha256,
            }
        ],
        "local_projects": [],
        "prune": {
            "path_segments": ["tests", "__pycache__", "sboms"],
            "relative_paths": ["bin", "share/man", "distutils-precedence.pth"],
        },
    }


def test_curates_exact_recorded_distribution_and_removes_installer_paths(tmp_path: Path) -> None:
    site_packages = tmp_path / "site-packages"
    _, expected_digest = _write_distribution(
        site_packages,
        name="demo-runtime",
        version="1.2.3",
        package_name="demo_runtime",
        license_expression="MIT",
    )

    components = curator.curate_site_packages(site_packages, _lock(content_sha256=expected_digest))

    assert [component["name"] for component in components] == ["demo-runtime"]
    assert components[0]["content_sha256"] == expected_digest
    assert components[0]["license_files"] == ["demo_runtime-1.2.3.dist-info/licenses/LICENSE.txt"]
    assert not (site_packages / "bin").exists()
    assert not any(site_packages.glob("*.dist-info/direct_url.json"))
    assert not any(site_packages.glob("*.dist-info/INSTALLER"))
    assert not any(site_packages.glob("*.dist-info/REQUESTED"))
    assert not any(site_packages.glob("*.dist-info/uv_cache.json"))
    assert not any(site_packages.glob("*.dist-info/DELVEWHEEL"))
    assert not any(site_packages.glob("*.dist-info/sboms"))


def test_rejects_record_tampering_unexpected_files_and_forbidden_payloads(tmp_path: Path) -> None:
    for case in ("tampered", "unowned", "weight"):
        site_packages = tmp_path / case
        _, expected_digest = _write_distribution(
            site_packages,
            name="demo-runtime",
            version="1.2.3",
            package_name="demo_runtime",
            license_expression="MIT",
        )
        if case == "tampered":
            (site_packages / "demo_runtime/__init__.py").write_text("changed\n", encoding="utf-8")
        elif case == "unowned":
            (site_packages / "private.txt").write_text("not in RECORD\n", encoding="utf-8")
        else:
            (site_packages / "model.safetensors").write_bytes(b"forbidden")

        with pytest.raises(curator.CuratorError):
            curator.curate_site_packages(site_packages, _lock(content_sha256=expected_digest))


def test_rejects_duplicate_lock_names_and_distribution_version_drift(tmp_path: Path) -> None:
    site_packages = tmp_path / "site-packages"
    _, expected_digest = _write_distribution(
        site_packages,
        name="demo-runtime",
        version="1.2.3",
        package_name="demo_runtime",
        license_expression="MIT",
    )
    duplicate = _lock(content_sha256=expected_digest)
    duplicate["dependencies"] = [
        duplicate["dependencies"][0],
        dict(duplicate["dependencies"][0]),
    ]
    with pytest.raises(curator.CuratorError, match="duplicate"):
        curator.curate_site_packages(site_packages, duplicate)

    drift = _lock(content_sha256=expected_digest)
    drift["dependencies"][0]["version"] = "1.2.4"
    with pytest.raises(curator.CuratorError, match="version"):
        curator.curate_site_packages(site_packages, drift)


def test_prepares_verified_embed_runtime_with_native_module_path(tmp_path: Path) -> None:
    archive = tmp_path / "python-3.13.14-embed-amd64.zip"
    stdlib_buffer = io.BytesIO()
    with zipfile.ZipFile(stdlib_buffer, "w", compression=zipfile.ZIP_DEFLATED) as stdlib:
        stdlib.writestr("encodings/__init__.pyc", b"synthetic bytecode")
        stdlib.writestr("ctypes/macholib/fetch_macholib.bat", b"not used at runtime")
    with zipfile.ZipFile(archive, "w", compression=zipfile.ZIP_STORED) as bundle:
        for name, payload in {
            "python.exe": b"synthetic PE",
            "python313.dll": b"synthetic DLL",
            "python313.zip": stdlib_buffer.getvalue(),
            "python313._pth": b"python313.zip\n.\n#import site\n",
            "_ctypes.pyd": b"synthetic native stdlib module",
            "LICENSE.txt": b"PSF license fixture\n",
        }.items():
            bundle.writestr(name, payload)
    lock = _lock(content_sha256="1" * 64)
    lock["python_runtime"]["sha256"] = hashlib.sha256(archive.read_bytes()).hexdigest()

    destination = tmp_path / "runtime"
    curator.prepare_python_runtime(archive, destination, lock)

    assert (destination / "_ctypes.pyd").read_bytes() == b"synthetic native stdlib module"
    assert (destination / "python313._pth").read_text(encoding="utf-8") == (
        "python313.zip\n.\nLib/site-packages\n"
    )
    with zipfile.ZipFile(destination / "python313.zip") as stdlib:
        assert stdlib.namelist() == ["encodings/__init__.pyc"]
    assert not (destination / "pythonw.exe").exists()


def test_runtime_archive_hash_and_path_traversal_fail_closed(tmp_path: Path) -> None:
    archive = tmp_path / "runtime.zip"
    with zipfile.ZipFile(archive, "w") as bundle:
        bundle.writestr("../escape.txt", b"escape")
    lock = _lock(content_sha256="1" * 64)
    lock["python_runtime"]["sha256"] = hashlib.sha256(archive.read_bytes()).hexdigest()
    with pytest.raises(curator.CuratorError, match="path"):
        curator.prepare_python_runtime(archive, tmp_path / "runtime", lock)

    lock["python_runtime"]["sha256"] = "f" * 64
    with pytest.raises(curator.CuratorError, match="SHA-256"):
        curator.prepare_python_runtime(archive, tmp_path / "other-runtime", lock)


def test_metadata_is_deterministic_path_free_and_atomic(tmp_path: Path) -> None:
    site_packages = tmp_path / "site-packages"
    _, expected_digest = _write_distribution(
        site_packages,
        name="demo-runtime",
        version="1.2.3",
        package_name="demo_runtime",
        license_expression="MIT",
    )
    lock = _lock(content_sha256=expected_digest)
    components = curator.curate_site_packages(site_packages, lock)
    metadata = curator.build_metadata(
        lock,
        components,
        base_notice="# H3 notice\n\nNo model weight is included.\n",
        pack_version="0.1.0",
    )

    inventory = json.loads(metadata["DEPENDENCY_INVENTORY.json"])
    sbom = json.loads(metadata["SBOM.cdx.json"])
    assert inventory["components"][0]["name"] == "CPython"
    assert inventory["components"][1]["name"] == "demo-runtime"
    assert sbom["bomFormat"] == "CycloneDX"
    assert sbom["specVersion"] == "1.5"
    assert "demo_runtime-1.2.3.dist-info/licenses/LICENSE.txt" in metadata["THIRD_PARTY_NOTICES.md"]
    serialized = "\n".join(metadata.values())
    assert "file:///" not in serialized
    assert str(tmp_path) not in serialized

    output = tmp_path / "metadata"
    curator.write_metadata_atomic(output, metadata)
    first = {path.name: path.read_bytes() for path in output.iterdir()}
    assert not list(output.glob("*.partial"))
    with pytest.raises(curator.CuratorError, match="overwrite"):
        curator.write_metadata_atomic(output, metadata)
    assert first == {path.name: path.read_bytes() for path in output.iterdir()}


def test_sensitive_metadata_rejects_private_paths_without_rejecting_code_examples(
    tmp_path: Path,
) -> None:
    code = tmp_path / "example.py"
    code.write_text(
        'pattern = r"C:\\\\Users\\\\example"\npassword = "password"\n',
        encoding="utf-8",
    )
    curator._assert_portable_text(code, tmp_path)

    metadata = tmp_path / "receipt.json"
    metadata.write_text('{"source":"C:\\\\Users\\\\private\\\\wheel.whl"}\n', encoding="utf-8")
    with pytest.raises(curator.CuratorError, match="private path"):
        curator._assert_portable_text(metadata, tmp_path)
