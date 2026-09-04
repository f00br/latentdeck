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
        "pack_version": "0.2.1",
        "platform": "windows-x86_64",
        "worker_protocol": 2,
        "codec_adapter_api": 1,
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
                "wheel": {
                    "file_name": "demo_runtime-1.2.3-py3-none-any.whl",
                    "url": (
                        "https://files.pythonhosted.org/packages/test/"
                        "demo_runtime-1.2.3-py3-none-any.whl"
                    ),
                    "byte_length": 123,
                    "sha256": "a" * 64,
                },
            }
        ],
        "local_projects": [],
        "prune": {
            "path_segments": ["tests", "__pycache__", "sboms"],
            "relative_paths": ["bin", "share/man", "distutils-precedence.pth"],
        },
    }


def _native_metadata(pack_version: str = "0.2.1") -> tuple[str, str]:
    artifact_name = "LatentDeck H3 Native Extensions"
    artifact_ref = f"pkg:generic/LatentDeck%20H3%20Native%20Extensions@{pack_version}"
    components = [
        {
            "type": "library",
            "bom-ref": "rust:latentdeck-cartridge-python@0.1.0",
            "name": "latentdeck-cartridge-python",
            "version": "0.1.0",
            "licenses": [{"license": {"name": "Apache-2.0"}}],
            "properties": [
                {"name": "latentdeck:ecosystem", "value": "rust"},
                {"name": "latentdeck:dependency-scope", "value": "artifact"},
                {"name": "latentdeck:selection-root", "value": "true"},
            ],
        },
        {
            "type": "library",
            "bom-ref": "rust:latentdeck-gpu-python@0.1.0",
            "name": "latentdeck-gpu-python",
            "version": "0.1.0",
            "licenses": [{"license": {"name": "Apache-2.0"}}],
            "properties": [
                {"name": "latentdeck:ecosystem", "value": "rust"},
                {"name": "latentdeck:dependency-scope", "value": "artifact"},
                {"name": "latentdeck:selection-root", "value": "true"},
            ],
        },
    ]
    sbom = {
        "bomFormat": "CycloneDX",
        "specVersion": "1.5",
        "serialNumber": "urn:uuid:00000000-0000-0000-0000-000000000001",
        "version": 1,
        "metadata": {
            "component": {
                "type": "application",
                "bom-ref": artifact_ref,
                "name": artifact_name,
                "version": pack_version,
                "licenses": [{"license": {"name": "Apache-2.0"}}],
                "properties": [
                    {"name": "latentdeck:artifact-scope", "value": "h3-native"},
                    {"name": "latentdeck:dependency-scope", "value": "artifact"},
                    {
                        "name": "latentdeck:target-platform",
                        "value": "x86_64-pc-windows-msvc",
                    },
                ],
            }
        },
        "components": components,
    }
    sbom_text = json.dumps(sbom, indent=2, sort_keys=True) + "\n"
    license_text = "Synthetic Apache-2.0 test text.\n"
    text_hash = hashlib.sha256(license_text.encode()).hexdigest()
    mappings = []
    for component, ecosystem in [
        (sbom["metadata"]["component"], "artifact"),
        *[(component, "rust") for component in components],
    ]:
        scope = next(
            item["value"]
            for item in component["properties"]
            if item["name"] == "latentdeck:dependency-scope"
        )
        mappings.append(
            {
                "bom-ref": component["bom-ref"],
                "name": component["name"],
                "version": component["version"],
                "ecosystem": ecosystem,
                "dependency_scope": scope,
                "license_expression": "Apache-2.0",
                "artifacts": [artifact_name],
                "disposition": "license_text_in_bundle",
                "rationale": "",
                "text_sha256s": [text_hash],
            }
        )
    license_bundle = {
        "schema_version": 1,
        "artifact": {"name": artifact_name, "version": pack_version},
        "policy": {
            "component_coverage": "exact-sbom-closure",
            "redistributed_components_require_text": True,
            "build_only_disposition": "not_redistributed_no_text_required",
            "text_canonicalization": "strict-utf8-lf-final-newline",
        },
        "sboms": [
            {
                "name": "NATIVE_RUST_SBOM.cdx.json",
                "artifact": artifact_name,
                "byte_length": len(sbom_text.encode()),
                "sha256": hashlib.sha256(sbom_text.encode()).hexdigest(),
            }
        ],
        "component_count": len(mappings),
        "text_count": 1,
        "components": mappings,
        "texts": [
            {
                "sha256": text_hash,
                "byte_length": len(license_text.encode()),
                "sources": [{"source_kind": "synthetic-test"}],
                "text": license_text,
            }
        ],
    }
    return sbom_text, json.dumps(license_bundle, indent=2, sort_keys=True) + "\n"


@pytest.mark.parametrize(
    ("mutation", "message"),
    [
        ("missing", "contract is not exact"),
        ("hash", "sha256"),
        ("length", "byte_length"),
        ("url", "exact HTTPS wheel"),
        ("sdist", "one wheel filename"),
    ],
)
def test_dependency_wheel_lock_fails_closed(mutation: str, message: str) -> None:
    lock = _lock(content_sha256="1" * 64)
    dependency = lock["dependencies"][0]
    if mutation == "missing":
        dependency.pop("wheel")
    elif mutation == "hash":
        dependency["wheel"]["sha256"] = "not-a-sha256"
    elif mutation == "length":
        dependency["wheel"]["byte_length"] = 0
    elif mutation == "url":
        dependency["wheel"]["url"] = (
            "https://files.pythonhosted.org/packages/test/other-1.2.3-py3-none-any.whl"
        )
    else:
        dependency["wheel"]["file_name"] = "demo-runtime-1.2.3.tar.gz"
        dependency["wheel"]["url"] = (
            "https://files.pythonhosted.org/packages/test/demo-runtime-1.2.3.tar.gz"
        )

    with pytest.raises(curator.CuratorError, match=message):
        curator._validate_lock(lock)


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
    native_sbom, native_licenses = _native_metadata()
    metadata = curator.build_metadata(
        lock,
        components,
        base_notice="# H3 notice\n\nNo model weight is included.\n",
        native_rust_sbom=native_sbom,
        native_rust_licenses=native_licenses,
        pack_version="0.2.1",
        source_commit="a" * 40,
    )

    inventory = json.loads(metadata["DEPENDENCY_INVENTORY.json"])
    sbom = json.loads(metadata["SBOM.cdx.json"])
    assert inventory["components"][0]["name"] == "CPython"
    assert inventory["components"][1]["name"] == "demo-runtime"
    assert inventory["source_commit"] == "a" * 40
    assert sbom["bomFormat"] == "CycloneDX"
    assert sbom["specVersion"] == "1.5"
    root_properties = {
        item["name"]: item["value"] for item in sbom["metadata"]["component"]["properties"]
    }
    assert root_properties["latentdeck:dependency-scope"] == "artifact"
    assert root_properties["latentdeck:included-dependency-scopes"] == (
        "artifact,runtime,build,runtime+build"
    )
    assert root_properties["latentdeck:excluded-dependency-scopes"] == "development"
    assert all(
        component["properties"]
        == [
            {"name": "latentdeck:ecosystem", "value": "python"},
            {"name": "latentdeck:dependency-scope", "value": "runtime"},
        ]
        for component in sbom["components"]
        if component["bom-ref"].startswith("pkg:")
    )
    assert inventory["native_rust"]["selection_roots"] == [
        "latentdeck-cartridge-python",
        "latentdeck-gpu-python",
    ]
    assert set(metadata) == {
        "DEPENDENCY_INVENTORY.json",
        "NATIVE_RUST_LICENSES.json",
        "NATIVE_RUST_SBOM.cdx.json",
        "SBOM.cdx.json",
        "THIRD_PARTY_NOTICES.md",
    }
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


@pytest.mark.parametrize(
    ("mutation", "message"),
    [
        ("missing_mapping", "mapping"),
        ("scope_drift", "mapping"),
        ("sbom_hash", "hash binding"),
    ],
)
def test_metadata_rejects_tampered_native_rust_evidence(
    tmp_path: Path, mutation: str, message: str
) -> None:
    native_sbom, native_licenses = _native_metadata()
    licenses = json.loads(native_licenses)
    if mutation == "missing_mapping":
        licenses["components"].pop()
        licenses["component_count"] -= 1
    elif mutation == "scope_drift":
        licenses["components"][0]["dependency_scope"] = "runtime"
    else:
        licenses["sboms"][0]["sha256"] = "0" * 64

    with pytest.raises(curator.CuratorError, match=message):
        curator.build_metadata(
            _lock(content_sha256="1" * 64),
            [],
            base_notice="# H3 notice\n",
            native_rust_sbom=native_sbom,
            native_rust_licenses=json.dumps(licenses),
            pack_version="0.2.1",
            source_commit="a" * 40,
        )


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
