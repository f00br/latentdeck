"""Deterministic, fail-closed curation for the Windows H3 Codec Pack.

This tool deliberately operates on an already isolated staging tree.  It does
not discover model assets, ComfyUI, or a generator installation, and it never
downloads anything.  Network/cache orchestration stays in the PowerShell
builder, while this module verifies the exact bytes selected for distribution.
"""

from __future__ import annotations

import argparse
import base64
import csv
import hashlib
import io
import json
import os
import re
import shutil
import sys
import uuid
import zipfile
from email.parser import BytesParser
from email.policy import compat32
from pathlib import Path, PurePosixPath
from typing import Any

MAX_RUNTIME_ARCHIVE_BYTES = 64 * 1024 * 1024
MAX_RUNTIME_ENTRIES = 128
MAX_METADATA_BYTES = 4 * 1024 * 1024
MAX_SITE_FILES = 32_768
FORBIDDEN_EXTENSIONS = frozenset(
    {
        ".lc",
        ".h3latent",
        ".safetensors",
        ".ckpt",
        ".pt",
        ".pth",
        ".onnx",
        ".engine",
        ".plan",
        ".gguf",
        ".bin",
        ".mp4",
        ".mov",
        ".mkv",
        ".avi",
        ".webm",
        ".wav",
        ".flac",
        ".mp3",
        ".whl",
        ".npy",
        ".npz",
        ".pkl",
        ".pickle",
        ".png",
        ".jpg",
        ".jpeg",
        ".webp",
        ".gif",
        ".bmp",
        ".tif",
        ".tiff",
        ".exr",
        ".hdr",
        ".psd",
        ".ps1",
        ".cmd",
        ".bat",
        ".sh",
    }
)
TEXT_EXTENSIONS = frozenset(
    {
        ".cfg",
        ".ini",
        ".json",
        ".md",
        ".txt",
        ".toml",
        ".yaml",
        ".yml",
        "._pth",
        ".py",
        ".pyi",
        ".pem",
        ".key",
        ".crt",
        ".xml",
        ".html",
        ".rst",
        ".cmake",
        ".pc",
        ".h",
        ".hpp",
        ".c",
        ".cc",
        ".cpp",
        ".rs",
        ".js",
        ".mjs",
        ".ts",
        ".css",
    }
)
SENSITIVE_TEXT_EXTENSIONS = frozenset(
    {
        ".cfg",
        ".ini",
        ".json",
        ".md",
        ".txt",
        ".toml",
        ".yaml",
        ".yml",
        "._pth",
        ".pem",
        ".key",
        ".crt",
        ".xml",
        ".cmake",
        ".pc",
    }
)
GENERATED_DIST_INFO_FILES = frozenset(
    {"DELVEWHEEL", "INSTALLER", "REQUESTED", "direct_url.json", "uv_cache.json"}
)
METADATA_FILENAMES = frozenset(
    {"DEPENDENCY_INVENTORY.json", "SBOM.cdx.json", "THIRD_PARTY_NOTICES.md"}
)
HEX_SHA256 = re.compile(r"^[0-9a-f]{64}$")
NORMALIZED_NAME = re.compile(r"[-_.]+")
ABSOLUTE_WINDOWS_PATH = re.compile(r"(?:^|[\s\"'(=])(?:file:///)?[A-Za-z]:[\\/]")
ABSOLUTE_POSIX_USER_PATH = re.compile(r"/(?:Users|home)/[^/\s]+/", re.IGNORECASE)
UNC_PATH = re.compile(
    r"\\\\[A-Za-z0-9][A-Za-z0-9._-]{0,63}"
    r"\\[A-Za-z0-9$][A-Za-z0-9$._-]{0,63}(?:\\|[\s\"'])"
)
CREDENTIAL = re.compile(
    r"\b(?:api[_-]?key|access[_-]?token|secret|password)\b\s*[:=]\s*[\"'][^\"'\r\n\s]{8,}",
    re.IGNORECASE,
)


class CuratorError(RuntimeError):
    """A candidate runtime is not a reproducible public distribution input."""


def _normalize_name(value: str) -> str:
    return NORMALIZED_NAME.sub("-", value).lower()


def _require_string(value: object, context: str, *, maximum: int = 4096) -> str:
    if not isinstance(value, str) or not value or len(value) > maximum or "\0" in value:
        raise CuratorError(f"{context} must be bounded non-empty text")
    return value


def _require_sha256(value: object, context: str) -> str:
    text = _require_string(value, context, maximum=64)
    if not HEX_SHA256.fullmatch(text):
        raise CuratorError(f"{context} must be a lowercase SHA-256")
    return text


def _portable_relative(value: str, context: str) -> str:
    if "\\" in value or "\0" in value or value.startswith(("/", "~")):
        raise CuratorError(f"{context} is not a portable relative path")
    path = PurePosixPath(value)
    if not value or path.is_absolute() or any(part in {"", ".", ".."} for part in path.parts):
        raise CuratorError(f"{context} is not a portable relative path")
    if path.parts[0].endswith(":"):
        raise CuratorError(f"{context} is not a portable relative path")
    return path.as_posix()


def _strict_json(path: Path) -> Any:
    try:
        payload = path.read_bytes()
    except OSError as error:
        raise CuratorError(f"could not read JSON input: {path.name}") from error
    if not payload or len(payload) > MAX_METADATA_BYTES:
        raise CuratorError(f"JSON input is empty or oversized: {path.name}")
    try:
        text = payload.decode("utf-8", errors="strict")
        return json.loads(
            text,
            parse_constant=lambda value: (_ for _ in ()).throw(
                CuratorError(f"JSON input contains non-finite number {value}")
            ),
            object_pairs_hook=_reject_duplicate_pairs,
        )
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        raise CuratorError(f"JSON input is not strict UTF-8 JSON: {path.name}") from error


def _reject_duplicate_pairs(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    result: dict[str, Any] = {}
    for key, value in pairs:
        if key in result:
            raise CuratorError(f"JSON input contains duplicate key {key!r}")
        result[key] = value
    return result


def load_lock(path: str | Path) -> dict[str, Any]:
    lock = _strict_json(Path(path))
    _validate_lock(lock)
    return lock


def _validate_lock(lock: object) -> dict[str, Any]:
    if not isinstance(lock, dict):
        raise CuratorError("curation lock must be a JSON object")
    required = {
        "schema_version",
        "platform",
        "uv_version",
        "python_runtime",
        "dependencies",
        "local_projects",
        "prune",
    }
    if set(lock) != required:
        raise CuratorError("curation lock has missing or unknown top-level fields")
    if lock["schema_version"] != 1 or lock["platform"] != "windows-x86_64":
        raise CuratorError("curation lock schema or platform is unsupported")
    _require_string(lock["uv_version"], "uv_version", maximum=32)
    runtime = lock["python_runtime"]
    if not isinstance(runtime, dict) or set(runtime) != {
        "version",
        "archive_filename",
        "source_url",
        "sha256",
        "license_expression",
        "license_path",
        "prune_entries",
    }:
        raise CuratorError("python_runtime contract is not exact")
    version = _require_string(runtime["version"], "python_runtime.version", maximum=32)
    if not re.fullmatch(r"3\.13\.\d+", version):
        raise CuratorError("python_runtime.version must be a CPython 3.13 patch release")
    _portable_relative(
        _require_string(runtime["archive_filename"], "python_runtime.archive_filename"),
        "python_runtime.archive_filename",
    )
    if not _require_string(runtime["source_url"], "python_runtime.source_url").startswith(
        "https://"
    ):
        raise CuratorError("python_runtime.source_url must use HTTPS")
    _require_sha256(runtime["sha256"], "python_runtime.sha256")
    _require_string(runtime["license_expression"], "python_runtime.license_expression")
    _portable_relative(
        _require_string(runtime["license_path"], "python_runtime.license_path"),
        "python_runtime.license_path",
    )
    prune_entries = runtime["prune_entries"]
    if not isinstance(prune_entries, list) or len(prune_entries) > 32:
        raise CuratorError("python_runtime.prune_entries must be a bounded array")
    seen_runtime_prunes: set[str] = set()
    for value in prune_entries:
        portable = _portable_relative(
            _require_string(value, "python_runtime.prune_entries"),
            "python_runtime.prune_entries",
        )
        if portable.lower() in seen_runtime_prunes:
            raise CuratorError("python_runtime.prune_entries contains duplicate paths")
        seen_runtime_prunes.add(portable.lower())

    expected_names: set[str] = set()
    for collection_name in ("dependencies", "local_projects"):
        collection = lock[collection_name]
        if not isinstance(collection, list) or len(collection) > 128:
            raise CuratorError(f"{collection_name} must be a bounded array")
        for index, component in enumerate(collection):
            if not isinstance(component, dict):
                raise CuratorError(f"{collection_name}[{index}] must be an object")
            required_component = {"name", "version", "source_url", "license_expression"}
            allowed_component = required_component | {"content_sha256"}
            if not required_component.issubset(component) or not set(component).issubset(
                allowed_component
            ):
                raise CuratorError(f"{collection_name}[{index}] contract is not exact")
            name = _normalize_name(_require_string(component["name"], "component.name"))
            if name in expected_names:
                raise CuratorError(f"curation lock contains duplicate component {name!r}")
            expected_names.add(name)
            _require_string(component["version"], "component.version", maximum=128)
            source_url = _require_string(component["source_url"], "component.source_url")
            if not source_url.startswith("https://"):
                raise CuratorError("component.source_url must use HTTPS")
            _require_string(component["license_expression"], "component.license_expression")
            if "content_sha256" in component:
                _require_sha256(component["content_sha256"], "component.content_sha256")

    prune = lock["prune"]
    if not isinstance(prune, dict) or set(prune) != {"path_segments", "relative_paths"}:
        raise CuratorError("prune contract is not exact")
    for key in ("path_segments", "relative_paths"):
        values = prune[key]
        if not isinstance(values, list) or len(values) > 256:
            raise CuratorError(f"prune.{key} must be a bounded array")
        seen: set[str] = set()
        for value in values:
            portable = _portable_relative(_require_string(value, f"prune.{key}"), f"prune.{key}")
            if "/" in portable and key == "path_segments":
                raise CuratorError("prune.path_segments entries must be one path segment")
            if portable.lower() in seen:
                raise CuratorError(f"prune.{key} contains duplicate paths")
            seen.add(portable.lower())
    return lock


def _assert_regular_tree(root: Path) -> list[Path]:
    if not root.is_dir() or root.is_symlink():
        raise CuratorError("site-packages root must be a regular directory")
    files: list[Path] = []
    for directory, dir_names, file_names in os.walk(root, followlinks=False):
        directory_path = Path(directory)
        for name in list(dir_names):
            candidate = directory_path / name
            if candidate.is_symlink():
                raise CuratorError("site-packages contains a symbolic link")
        for name in file_names:
            candidate = directory_path / name
            if candidate.is_symlink() or not candidate.is_file():
                raise CuratorError("site-packages contains a non-regular file")
            files.append(candidate)
            if len(files) > MAX_SITE_FILES:
                raise CuratorError("site-packages exceeds the bounded file count")
    return files


def _decode_record_hash(value: str, context: str) -> bytes:
    algorithm, separator, encoded = value.partition("=")
    if separator != "=" or algorithm != "sha256" or not encoded:
        raise CuratorError(f"{context} uses a non-SHA-256 RECORD hash")
    try:
        return base64.urlsafe_b64decode(encoded + "=" * (-len(encoded) % 4))
    except ValueError as error:
        raise CuratorError(f"{context} has malformed RECORD hash encoding") from error


def _read_record(dist_info: Path, root: Path) -> list[list[str]]:
    record = dist_info / "RECORD"
    try:
        text = record.read_text(encoding="utf-8", errors="strict")
        rows = list(csv.reader(io.StringIO(text, newline="")))
    except (OSError, UnicodeDecodeError, csv.Error) as error:
        raise CuratorError(f"{dist_info.name}/RECORD is invalid") from error
    if not rows or len(rows) > MAX_SITE_FILES:
        raise CuratorError(f"{dist_info.name}/RECORD is empty or oversized")
    seen: set[str] = set()
    for row in rows:
        if len(row) != 3:
            raise CuratorError(f"{dist_info.name}/RECORD has a malformed row")
        relative = _portable_relative(row[0], f"{dist_info.name}/RECORD path")
        if relative.lower() in seen:
            raise CuratorError(f"{dist_info.name}/RECORD contains duplicate paths")
        seen.add(relative.lower())
        path = root / PurePosixPath(relative)
        if row[1] or row[2]:
            if not row[1] or not row[2] or not row[2].isdigit():
                raise CuratorError(f"{dist_info.name}/RECORD has an incomplete hash row")
            if not path.is_file() or path.is_symlink():
                raise CuratorError(f"{dist_info.name}/RECORD references a missing file")
            payload = path.read_bytes()
            expected = _decode_record_hash(row[1], f"{dist_info.name}/RECORD")
            if len(payload) != int(row[2]) or hashlib.sha256(payload).digest() != expected:
                raise CuratorError(f"{dist_info.name}/RECORD integrity mismatch at {relative}")
        elif relative != f"{dist_info.name}/RECORD":
            raise CuratorError(f"{dist_info.name}/RECORD has an unhashed payload row")
    return rows


def _distribution_metadata(dist_info: Path) -> Any:
    metadata_path = dist_info / "METADATA"
    try:
        payload = metadata_path.read_bytes()
        if not payload or len(payload) > MAX_METADATA_BYTES:
            raise CuratorError(f"{dist_info.name}/METADATA is empty or oversized")
        return BytesParser(policy=compat32).parsebytes(payload)
    except OSError as error:
        raise CuratorError(f"{dist_info.name}/METADATA is missing") from error


def _should_prune(relative: str, lock: dict[str, Any]) -> bool:
    parts = PurePosixPath(relative).parts
    segments = {str(value).lower() for value in lock["prune"]["path_segments"]}
    if any(part.lower() in segments for part in parts):
        return True
    lowered = relative.lower()
    for configured in lock["prune"]["relative_paths"]:
        prefix = str(configured).lower().rstrip("/")
        if lowered == prefix or lowered.startswith(prefix + "/"):
            return True
    if len(parts) >= 2 and parts[0].lower().endswith(".dist-info"):
        return parts[-1] in GENERATED_DIST_INFO_FILES
    return False


def _remove_path(path: Path, root: Path) -> None:
    try:
        path.relative_to(root)
    except ValueError as error:
        raise CuratorError("refusing to prune outside site-packages") from error
    if path.is_dir() and not path.is_symlink():
        shutil.rmtree(path)
    elif path.exists() or path.is_symlink():
        path.unlink()


def _rewrite_record(dist_info: Path, root: Path, rows: list[list[str]]) -> list[list[str]]:
    retained: list[list[str]] = []
    record_relative = f"{dist_info.name}/RECORD"
    for row in rows:
        relative = row[0]
        path = root / PurePosixPath(relative)
        if relative == record_relative:
            retained.append([record_relative, "", ""])
        elif path.is_file() and not path.is_symlink():
            retained.append(row)
    retained.sort(key=lambda row: row[0])
    buffer = io.StringIO(newline="")
    csv.writer(buffer, lineterminator="\n").writerows(retained)
    (dist_info / "RECORD").write_text(buffer.getvalue(), encoding="utf-8", newline="")
    return retained


def _assert_portable_text(path: Path, root: Path) -> None:
    relative = path.relative_to(root).as_posix()
    extension = path.suffix.lower()
    if extension not in TEXT_EXTENSIONS:
        return
    if path.stat().st_size > MAX_METADATA_BYTES:
        raise CuratorError(f"portable text candidate is oversized: {relative}")
    try:
        text = path.read_text(encoding="utf-8", errors="strict")
    except UnicodeDecodeError as error:
        raise CuratorError(f"portable text is not strict UTF-8: {relative}") from error
    if extension in SENSITIVE_TEXT_EXTENSIONS and (
        ABSOLUTE_WINDOWS_PATH.search(text)
        or ABSOLUTE_POSIX_USER_PATH.search(text)
        or UNC_PATH.search(text)
        or CREDENTIAL.search(text)
    ):
        raise CuratorError(f"portable text contains a private path or credential: {relative}")


def curate_site_packages(root: str | Path, lock: dict[str, Any]) -> list[dict[str, Any]]:
    """Validate and normalize one clean ``site-packages`` staging tree in place."""

    lock = _validate_lock(lock)
    root = Path(root).resolve(strict=True)
    initial_files = _assert_regular_tree(root)
    for file in initial_files:
        relative = file.relative_to(root).as_posix()
        if file.suffix.lower() in FORBIDDEN_EXTENSIONS and not _should_prune(relative, lock):
            raise CuratorError(f"site-packages contains forbidden payload: {relative}")

    expected: dict[str, dict[str, Any]] = {}
    local_names: set[str] = set()
    for component in lock["dependencies"]:
        expected[_normalize_name(component["name"])] = component
    for component in lock["local_projects"]:
        name = _normalize_name(component["name"])
        expected[name] = component
        local_names.add(name)

    discovered: dict[str, dict[str, Any]] = {}
    dist_infos = sorted(root.glob("*.dist-info"), key=lambda path: path.name.lower())
    for dist_info in dist_infos:
        if not dist_info.is_dir() or dist_info.is_symlink():
            raise CuratorError("distribution metadata must be a regular directory")
        metadata = _distribution_metadata(dist_info)
        raw_name = metadata.get("Name")
        version = metadata.get("Version")
        if not raw_name or not version:
            raise CuratorError(f"{dist_info.name}/METADATA lacks Name or Version")
        name = _normalize_name(str(raw_name))
        if name in discovered:
            raise CuratorError(f"site-packages contains duplicate distribution {name!r}")
        component = expected.get(name)
        if component is None:
            raise CuratorError(f"site-packages contains unexpected distribution {name!r}")
        if str(version) != component["version"]:
            expected_version = component["version"]
            raise CuratorError(
                f"distribution version drift for {name}: "
                f"expected {expected_version}, found {version}"
            )
        rows = _read_record(dist_info, root)
        discovered[name] = {
            "dist_info": dist_info,
            "metadata": metadata,
            "rows": rows,
            "contract": component,
        }
    missing = sorted(set(expected) - set(discovered))
    if missing:
        raise CuratorError(f"site-packages is missing locked distributions: {', '.join(missing)}")

    # Only remove bytes after every original wheel RECORD has passed integrity.
    prune_candidates: set[Path] = set()
    for file in initial_files:
        relative = file.relative_to(root).as_posix()
        if _should_prune(relative, lock):
            prune_candidates.add(file)
    for candidate in sorted(prune_candidates, key=lambda path: len(path.parts), reverse=True):
        _remove_path(candidate, root)
    for directory in sorted(
        [path for path in root.rglob("*") if path.is_dir()],
        key=lambda path: len(path.parts),
        reverse=True,
    ):
        if directory != root and not any(directory.iterdir()):
            directory.rmdir()

    ownership: dict[str, str] = {}
    components: list[dict[str, Any]] = []
    for name in sorted(discovered):
        item = discovered[name]
        dist_info: Path = item["dist_info"]
        rows = _rewrite_record(dist_info, root, item["rows"])
        digest_rows: list[str] = []
        for relative, record_hash, byte_length in rows:
            owner = ownership.get(relative.lower())
            if owner is not None:
                raise CuratorError(f"distribution payload has multiple owners: {relative}")
            ownership[relative.lower()] = name
            if relative == f"{dist_info.name}/RECORD":
                continue
            payload = (root / PurePosixPath(relative)).read_bytes()
            digest_rows.append(f"{relative}\0{hashlib.sha256(payload).hexdigest()}\0{len(payload)}")
            if not record_hash or int(byte_length) != len(payload):
                raise CuratorError(f"curated RECORD is incomplete for {relative}")
        content_sha256 = hashlib.sha256(
            ("\n".join(sorted(digest_rows)) + "\n").encode()
        ).hexdigest()
        contract = item["contract"]
        expected_digest = contract.get("content_sha256")
        if expected_digest is not None and content_sha256 != expected_digest:
            raise CuratorError(f"locked content SHA-256 mismatch for {name}")

        metadata = item["metadata"]
        license_files: set[str] = set()
        for declared in metadata.get_all("License-File", []) or []:
            declared = _portable_relative(str(declared), f"{name} License-File")
            for candidate in (
                dist_info / "licenses" / PurePosixPath(declared),
                dist_info / PurePosixPath(declared),
            ):
                if candidate.is_file() and not candidate.is_symlink():
                    license_files.add(candidate.relative_to(root).as_posix())
                    break
            else:
                raise CuratorError(f"declared license file is missing for {name}: {declared}")
        licenses_directory = dist_info / "licenses"
        if licenses_directory.is_dir():
            for candidate in licenses_directory.rglob("*"):
                if candidate.is_file() and not candidate.is_symlink():
                    license_files.add(candidate.relative_to(root).as_posix())
        if not license_files and name not in local_names:
            raise CuratorError(f"redistributed dependency has no bundled license text: {name}")

        components.append(
            {
                "name": str(metadata["Name"]),
                "version": str(metadata["Version"]),
                "kind": "repository" if name in local_names else "dependency",
                "source_url": contract["source_url"],
                "license_expression": contract["license_expression"],
                "content_sha256": content_sha256,
                "license_files": sorted(license_files),
            }
        )

    final_files = _assert_regular_tree(root)
    for file in final_files:
        relative = file.relative_to(root).as_posix()
        if relative.lower() not in ownership:
            raise CuratorError(f"site-packages contains unowned file: {relative}")
        if file.suffix.lower() in FORBIDDEN_EXTENSIONS:
            raise CuratorError(f"site-packages contains forbidden payload: {relative}")
        _assert_portable_text(file, root)
    return components


def _curate_stdlib_archive(path: Path, prune_entries: list[str]) -> None:
    configured = {entry.lower(): entry for entry in prune_entries}
    found: set[str] = set()
    retained: list[tuple[str, zipfile.ZipInfo, bytes]] = []
    try:
        with zipfile.ZipFile(path, "r") as source:
            entries = source.infolist()
            if not entries or len(entries) > 10_000:
                raise CuratorError("CPython standard-library ZIP has an invalid entry count")
            seen: set[str] = set()
            total_bytes = 0
            for entry in entries:
                if entry.is_dir():
                    continue
                name = _portable_relative(entry.filename, "CPython standard-library path")
                lowered = name.lower()
                if lowered in seen:
                    raise CuratorError("CPython standard-library ZIP contains duplicate paths")
                seen.add(lowered)
                if entry.flag_bits & 0x1:
                    raise CuratorError("CPython standard-library ZIP contains an encrypted entry")
                total_bytes += entry.file_size
                if total_bytes > 512 * 1024 * 1024:
                    raise CuratorError("CPython standard-library ZIP exceeds its byte bound")
                if lowered in configured:
                    found.add(lowered)
                    continue
                if Path(name).suffix.lower() in {".bat", ".cmd", ".ps1", ".sh"}:
                    raise CuratorError(
                        f"CPython standard-library ZIP contains an unapproved script: {name}"
                    )
                if entry.compress_type not in {zipfile.ZIP_STORED, zipfile.ZIP_DEFLATED}:
                    raise CuratorError("CPython standard-library ZIP uses unsupported compression")
                retained.append((name, entry, source.read(entry)))
    except (OSError, zipfile.BadZipFile) as error:
        raise CuratorError("CPython standard-library input is not a valid ZIP") from error

    missing = sorted(configured[key] for key in configured.keys() - found)
    if missing:
        raise CuratorError(
            "CPython standard-library ZIP is missing configured prune entries: "
            + ", ".join(missing)
        )

    partial = path.with_name(f".{path.name}.partial-{uuid.uuid4().hex}")
    try:
        with zipfile.ZipFile(partial, "x", allowZip64=True) as output:
            for name, source_info, payload in sorted(retained, key=lambda item: item[0]):
                info = zipfile.ZipInfo(name, date_time=(1980, 1, 1, 0, 0, 0))
                info.compress_type = source_info.compress_type
                info.create_system = source_info.create_system
                info.external_attr = source_info.external_attr
                info.internal_attr = source_info.internal_attr
                output.writestr(
                    info, payload, compress_type=source_info.compress_type, compresslevel=9
                )
        partial.replace(path)
    except Exception:
        partial.unlink(missing_ok=True)
        raise


def prepare_python_runtime(
    archive_path: str | Path,
    destination: str | Path,
    lock: dict[str, Any],
) -> Path:
    """Extract one authenticated official CPython embed archive atomically."""

    lock = _validate_lock(lock)
    archive_path = Path(archive_path).resolve(strict=True)
    destination = Path(destination).resolve(strict=False)
    if destination.exists():
        raise CuratorError(f"refusing to overwrite runtime destination: {destination.name}")
    if not archive_path.is_file() or archive_path.is_symlink():
        raise CuratorError("CPython embed archive must be a regular file")
    if archive_path.stat().st_size <= 0 or archive_path.stat().st_size > MAX_RUNTIME_ARCHIVE_BYTES:
        raise CuratorError("CPython embed archive is empty or oversized")
    measured = hashlib.sha256(archive_path.read_bytes()).hexdigest()
    if measured != lock["python_runtime"]["sha256"]:
        raise CuratorError("CPython embed archive SHA-256 does not match the lock")

    parent = destination.parent
    parent.mkdir(parents=True, exist_ok=True)
    staging = parent / f".{destination.name}.partial-{uuid.uuid4().hex}"
    if staging.exists():
        raise CuratorError("runtime staging path unexpectedly exists")
    staging.mkdir()
    try:
        try:
            archive = zipfile.ZipFile(archive_path, "r")
        except (OSError, zipfile.BadZipFile) as error:
            raise CuratorError("CPython embed input is not a valid ZIP archive") from error
        with archive:
            entries = archive.infolist()
            if not entries or len(entries) > MAX_RUNTIME_ENTRIES:
                raise CuratorError("CPython embed archive has an invalid entry count")
            seen: set[str] = set()
            total_bytes = 0
            for entry in entries:
                if entry.is_dir():
                    continue
                name = _portable_relative(entry.filename, "CPython archive path")
                if "/" in name:
                    raise CuratorError("CPython archive path must remain flat")
                if name.lower() in seen:
                    raise CuratorError("CPython archive contains duplicate paths")
                seen.add(name.lower())
                if entry.flag_bits & 0x1:
                    raise CuratorError("CPython archive contains an encrypted entry")
                total_bytes += entry.file_size
                if total_bytes > MAX_RUNTIME_ARCHIVE_BYTES:
                    raise CuratorError("CPython archive expands beyond its byte bound")
                extension = Path(name).suffix.lower()
                allowed = name in {
                    "python.exe",
                    "python313.dll",
                    "python3.dll",
                    "python313._pth",
                    "python313.zip",
                    "python.cat",
                    "LICENSE.txt",
                } or extension in {".pyd", ".dll"}
                if not allowed:
                    # pythonw.exe is not needed by the three console worker entrypoints.
                    if name == "pythonw.exe":
                        continue
                    raise CuratorError(f"CPython archive contains unexpected entry: {name}")
                target = staging / name
                with archive.open(entry, "r") as source, target.open("xb") as output:
                    shutil.copyfileobj(source, output, length=1024 * 1024)

        required = {
            "python.exe",
            "python313.dll",
            "python313.zip",
            "python313._pth",
            lock["python_runtime"]["license_path"],
        }
        missing = sorted(name for name in required if not (staging / name).is_file())
        if missing:
            raise CuratorError(f"CPython runtime is missing required files: {', '.join(missing)}")
        _curate_stdlib_archive(
            staging / "python313.zip",
            list(lock["python_runtime"]["prune_entries"]),
        )
        (staging / "python313._pth").write_text(
            "python313.zip\n.\nLib/site-packages\n",
            encoding="utf-8",
            newline="",
        )
        (staging / "Lib" / "site-packages").mkdir(parents=True)
        staging.replace(destination)
        return destination
    except Exception:
        if (
            staging.exists()
            and staging.parent == parent
            and staging.name.startswith(f".{destination.name}.partial-")
        ):
            shutil.rmtree(staging)
        raise


def build_metadata(
    lock: dict[str, Any],
    components: list[dict[str, Any]],
    *,
    base_notice: str,
    pack_version: str,
) -> dict[str, str]:
    """Create deterministic inventory, CycloneDX 1.5 SBOM, and notice text."""

    lock = _validate_lock(lock)
    _require_string(pack_version, "pack_version", maximum=64)
    if len(base_notice.encode()) > MAX_METADATA_BYTES:
        raise CuratorError("base notice is oversized")
    if (
        ABSOLUTE_WINDOWS_PATH.search(base_notice)
        or ABSOLUTE_POSIX_USER_PATH.search(base_notice)
        or UNC_PATH.search(base_notice)
        or CREDENTIAL.search(base_notice)
    ):
        raise CuratorError("base notice contains a private path or credential")

    runtime = lock["python_runtime"]
    python_component = {
        "name": "CPython",
        "version": runtime["version"],
        "kind": "runtime",
        "source_url": runtime["source_url"],
        "license_expression": runtime["license_expression"],
        "content_sha256": runtime["sha256"],
        "license_files": [f"runtime/{runtime['license_path']}"],
    }
    normalized_components = [python_component]
    normalized_components.extend(
        sorted(components, key=lambda component: _normalize_name(component["name"]))
    )
    inventory = {
        "schema_version": 1,
        "pack_id": "org.latentdeck.h3",
        "pack_version": pack_version,
        "platform": lock["platform"],
        "curator": {"name": "latentdeck-codec-pack-curator", "schema_version": 1},
        "components": normalized_components,
    }

    sbom_components: list[dict[str, Any]] = []
    for component in normalized_components:
        normalized = _normalize_name(component["name"])
        is_python = component["kind"] == "runtime"
        package_type = "generic" if is_python else "pypi"
        sbom_component = {
            "bom-ref": f"pkg:{package_type}/{normalized}@{component['version']}",
            "type": "application" if is_python else "library",
            "name": component["name"],
            "version": component["version"],
            "hashes": [{"alg": "SHA-256", "content": component["content_sha256"]}],
            "licenses": [{"expression": component["license_expression"]}],
            "externalReferences": [{"type": "distribution", "url": component["source_url"]}],
        }
        if not is_python:
            sbom_component["purl"] = f"pkg:pypi/{normalized}@{component['version']}"
        sbom_components.append(sbom_component)
    sbom = {
        "bomFormat": "CycloneDX",
        "specVersion": "1.5",
        "version": 1,
        "metadata": {
            "component": {
                "bom-ref": f"pkg:generic/latentdeck-h3-codec-pack@{pack_version}",
                "type": "application",
                "name": "LatentDeck H3 Codec Pack",
                "version": pack_version,
            }
        },
        "components": sbom_components,
    }

    notice_lines = [base_notice.rstrip(), "", "## Runtime dependency inventory", ""]
    for component in normalized_components:
        notice_lines.extend(
            [
                f"### {component['name']} {component['version']}",
                "",
                f"- License expression/label: `{component['license_expression']}`",
                f"- Source: `{component['source_url']}`",
                f"- Curated content SHA-256: `{component['content_sha256']}`",
                "- Bundled license files:",
            ]
        )
        for path in component["license_files"]:
            portable = _portable_relative(path, "license inventory path")
            if component["kind"] == "runtime":
                display = portable
            else:
                display = f"runtime/Lib/site-packages/{portable}"
            notice_lines.append(f"  - `{display}`")
        if not component["license_files"]:
            notice_lines.append("  - See the project license text reproduced in this notice.")
        notice_lines.append("")
    notice_lines.extend(
        [
            "No model weight, cartridge, generator, or ComfyUI component is included.",
            "The decoder weight remains an explicitly selected external asset.",
            "",
        ]
    )

    result = {
        "DEPENDENCY_INVENTORY.json": json.dumps(
            inventory, ensure_ascii=False, indent=2, sort_keys=True, allow_nan=False
        )
        + "\n",
        "SBOM.cdx.json": json.dumps(
            sbom, ensure_ascii=False, indent=2, sort_keys=True, allow_nan=False
        )
        + "\n",
        "THIRD_PARTY_NOTICES.md": "\n".join(notice_lines),
    }
    for name, text in result.items():
        if len(text.encode()) > MAX_METADATA_BYTES:
            raise CuratorError(f"generated metadata is oversized: {name}")
        if (
            ABSOLUTE_WINDOWS_PATH.search(text)
            or ABSOLUTE_POSIX_USER_PATH.search(text)
            or UNC_PATH.search(text)
            or CREDENTIAL.search(text)
        ):
            raise CuratorError(f"generated metadata is not portable: {name}")
    return result


def write_metadata_atomic(output: str | Path, metadata: dict[str, str]) -> Path:
    output = Path(output).resolve(strict=False)
    if output.exists():
        raise CuratorError(f"refusing to overwrite metadata destination: {output.name}")
    if set(metadata) != METADATA_FILENAMES:
        raise CuratorError("metadata output set is not exact")
    parent = output.parent
    parent.mkdir(parents=True, exist_ok=True)
    staging = parent / f".{output.name}.partial-{uuid.uuid4().hex}"
    staging.mkdir()
    try:
        for name in sorted(metadata):
            path = staging / name
            with path.open("x", encoding="utf-8", newline="\n") as stream:
                stream.write(metadata[name])
                stream.flush()
                os.fsync(stream.fileno())
        staging.replace(output)
        return output
    except Exception:
        if (
            staging.exists()
            and staging.parent == parent
            and staging.name.startswith(f".{output.name}.partial-")
        ):
            shutil.rmtree(staging)
        raise


def _command_prepare_runtime(args: argparse.Namespace) -> int:
    lock = load_lock(args.lock)
    prepare_python_runtime(args.archive, args.destination, lock)
    return 0


def _command_curate(args: argparse.Namespace) -> int:
    lock = load_lock(args.lock)
    components = curate_site_packages(args.site_packages, lock)
    base_notice = Path(args.base_notice).read_text(encoding="utf-8", errors="strict")
    metadata = build_metadata(
        lock,
        components,
        base_notice=base_notice,
        pack_version=args.pack_version,
    )
    write_metadata_atomic(args.metadata_output, metadata)
    receipt = {
        "component_count": len(components) + 1,
        "pack_version": args.pack_version,
        "site_packages": "curated",
    }
    print(json.dumps(receipt, sort_keys=True, separators=(",", ":")))
    return 0


def _parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    subparsers = parser.add_subparsers(dest="command", required=True)
    runtime = subparsers.add_parser("prepare-runtime")
    runtime.add_argument("--lock", required=True)
    runtime.add_argument("--archive", required=True)
    runtime.add_argument("--destination", required=True)
    runtime.set_defaults(handler=_command_prepare_runtime)

    curate = subparsers.add_parser("curate")
    curate.add_argument("--lock", required=True)
    curate.add_argument("--site-packages", required=True)
    curate.add_argument("--base-notice", required=True)
    curate.add_argument("--metadata-output", required=True)
    curate.add_argument("--pack-version", required=True)
    curate.set_defaults(handler=_command_curate)
    return parser


def main(argv: list[str] | None = None) -> int:
    args = _parser().parse_args(argv)
    try:
        return int(args.handler(args))
    except CuratorError as error:
        print(f"codec-pack curator: {error}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
