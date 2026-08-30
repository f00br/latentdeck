"""Safe Python surface for the single Rust Latent Cartridge implementation."""

from __future__ import annotations

import json
import os
from collections.abc import Mapping

from . import _native

__version__ = "0.1.0"
BINDING_ABI_VERSION = _native.BINDING_ABI_VERSION
NATIVE_MODULE_NAME = "latentdeck_cartridge._native"

CartridgeError = _native.CartridgeError
type StrPath = str | os.PathLike[str]
type ResultDict = dict[str, object]


def _path_text(path: StrPath) -> str:
    value = os.fspath(path)
    if not isinstance(value, str):
        raise TypeError("LatentDeck paths must resolve to text")
    return value


def _result(encoded: str) -> ResultDict:
    value = json.loads(encoded)
    if not isinstance(value, dict):
        raise CartridgeError(
            "manifest_json_invalid",
            "native binding returned a non-object JSON result",
        )
    return value


def inspect(path: StrPath) -> ResultDict:
    """Inspect bounded cartridge structure without granting tensor access."""

    return _result(_native.inspect_json(_path_text(path)))


def validate(path: StrPath) -> ResultDict:
    """Fully validate archive hashes and every finite tensor value."""

    return _result(_native.validate_json(_path_text(path)))


def hash(path: StrPath) -> ResultDict:
    """Stream the complete file SHA-256 without loading it into memory."""

    return _result(_native.hash_json(_path_text(path)))


def inspect_raw_h3(path: StrPath) -> ResultDict:
    """Fully validate one raw H3 Safetensors payload without writing a cartridge."""

    return _result(_native.inspect_raw_h3_json(_path_text(path)))


def read_h3(
    path: StrPath,
    *,
    max_visual_values: int | None = None,
    max_tensor_bytes: int | None = None,
) -> ResultDict:
    """Read validated H3 tensors with optional pre-allocation admission bounds."""

    encoded, video, audio = _native.read_h3(
        _path_text(path), max_visual_values, max_tensor_bytes
    )
    result = _result(encoded)
    tensors = result.get("tensors")
    if not isinstance(tensors, dict) or not isinstance(tensors.get("video"), dict):
        raise CartridgeError("manifest_json_invalid", "native binding omitted H3 tensor metadata")
    tensors["video"]["data"] = video
    if audio is not None:
        audio_metadata = tensors.get("audio")
        if not isinstance(audio_metadata, dict):
            raise CartridgeError(
                "manifest_json_invalid", "native binding omitted H3 audio metadata"
            )
        audio_metadata["data"] = audio
    return result


def read_raw_h3(
    path: StrPath,
    *,
    max_visual_values: int | None = None,
    max_tensor_bytes: int | None = None,
) -> ResultDict:
    """Read validated raw H3 tensors with optional pre-allocation admission bounds."""

    encoded, video, audio = _native.read_raw_h3(
        _path_text(path), max_visual_values, max_tensor_bytes
    )
    result = _result(encoded)
    tensors = result.get("tensors")
    if not isinstance(tensors, dict) or not isinstance(tensors.get("video"), dict):
        raise CartridgeError("manifest_json_invalid", "native binding omitted H3 tensor metadata")
    tensors["video"]["data"] = video
    if audio is not None:
        audio_metadata = tensors.get("audio")
        if not isinstance(audio_metadata, dict):
            raise CartridgeError(
                "manifest_json_invalid", "native binding omitted H3 audio metadata"
            )
        audio_metadata["data"] = audio
    return result


def pack(
    manifest: dict[str, object],
    payload_path: StrPath,
    output_path: StrPath,
    preview_path: StrPath | None = None,
    *,
    overwrite: bool = False,
) -> ResultDict:
    """Write, validate, and atomically commit one finalized H3 cartridge."""

    try:
        manifest_json = json.dumps(
            manifest,
            ensure_ascii=False,
            allow_nan=False,
            separators=(",", ":"),
        )
    except (TypeError, ValueError) as error:
        raise CartridgeError("manifest_json_invalid", str(error)) from error
    return _result(
        _native.pack_json(
            manifest_json,
            _path_text(payload_path),
            _path_text(output_path),
            None if preview_path is None else _path_text(preview_path),
            overwrite,
        )
    )


def pack_raw_h3(
    payload_path: StrPath,
    output_path: StrPath,
    preview_path: StrPath | None = None,
    *,
    cartridge_id: str | None = None,
    provenance: Mapping[str, object] | None = None,
    overwrite: bool = False,
) -> ResultDict:
    """Build an H3 manifest in Rust, then atomically pack the raw payload."""

    try:
        provenance_json = (
            None
            if provenance is None
            else json.dumps(
                dict(provenance),
                ensure_ascii=False,
                allow_nan=False,
                separators=(",", ":"),
            )
        )
    except (TypeError, ValueError) as error:
        raise CartridgeError("manifest_json_invalid", str(error)) from error
    return _result(
        _native.pack_raw_h3_json(
            _path_text(payload_path),
            _path_text(output_path),
            None if preview_path is None else _path_text(preview_path),
            cartridge_id,
            provenance_json,
            overwrite,
        )
    )


__all__ = [
    "BINDING_ABI_VERSION",
    "CartridgeError",
    "NATIVE_MODULE_NAME",
    "hash",
    "inspect",
    "inspect_raw_h3",
    "pack",
    "pack_raw_h3",
    "read_h3",
    "read_raw_h3",
    "validate",
    "__version__",
]
