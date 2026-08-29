"""Dependency-free command-line adapter for the Python Cartridge binding."""

from __future__ import annotations

import argparse
import json
import sys
from collections.abc import Sequence
from pathlib import Path

from . import CartridgeError, inspect, pack, pack_raw_h3, validate
from . import hash as hash_cartridge

MAX_MANIFEST_BYTES = 1024 * 1024


def _parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        prog="python -m latentdeck_cartridge",
        description="Pack, inspect, validate, and hash data-only Latent Cartridges.",
    )
    subcommands = parser.add_subparsers(dest="command", required=True)

    pack_parser = subcommands.add_parser("pack")
    pack_parser.add_argument("--manifest", type=Path, required=True)
    pack_parser.add_argument("--payload", type=Path, required=True)
    pack_parser.add_argument("--preview", type=Path)
    pack_parser.add_argument("--output", type=Path, required=True)
    pack_parser.add_argument("--overwrite", action="store_true")

    for name in ("inspect", "validate", "hash"):
        command = subcommands.add_parser(name)
        command.add_argument("path", type=Path)
    return parser


def _read_manifest(path: Path) -> dict[str, object]:
    with path.open("rb") as stream:
        encoded = stream.read(MAX_MANIFEST_BYTES + 1)
    if len(encoded) > MAX_MANIFEST_BYTES:
        raise CartridgeError("manifest_too_large", "manifest exceeds the 1 MiB limit")
    try:
        value = json.loads(encoded)
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        raise CartridgeError("manifest_json_invalid", str(error)) from error
    if not isinstance(value, dict):
        raise CartridgeError("manifest_json_invalid", "manifest root must be an object")
    return value


def _exit_status(error: CartridgeError) -> int:
    if error.code in {
        "unsupported_spec_version",
        "unsupported_codec",
        "unsupported_profile_version",
    }:
        return 4
    if error.code in {"io_open", "io_read", "io_write", "target_exists", "atomic_commit_failed"}:
        return 5
    if error.code == "postwrite_validation_failed":
        return 6
    return 3


def _emit_result(result: dict[str, object]) -> None:
    json.dump(result, sys.stdout, indent=2)
    sys.stdout.write("\n")


def _emit_error(error: CartridgeError) -> int:
    json.dump(
        {
            "status": "error",
            "code": error.code,
            "detail": error.detail,
            "location": {
                "entry": error.entry,
                "tensor": error.tensor,
                "json_pointer": error.json_pointer,
            },
        },
        sys.stderr,
        indent=2,
    )
    sys.stderr.write("\n")
    return _exit_status(error)


def pack_main(argv: Sequence[str] | None = None) -> int:
    """Execute ``latentdeck-pack INPUT --profile h3 -o OUTPUT``."""

    parser = argparse.ArgumentParser(
        prog="latentdeck-pack",
        description="Build a validated LC 0.1 cartridge from raw H3 Safetensors.",
    )
    parser.add_argument("input", type=Path)
    parser.add_argument("--profile", required=True, choices=("h3",))
    parser.add_argument("-o", "--output", type=Path, required=True)
    parser.add_argument("--preview", type=Path)
    parser.add_argument("--cartridge-id")
    parser.add_argument("--overwrite", action="store_true")
    arguments = parser.parse_args(argv)
    try:
        result = pack_raw_h3(
            arguments.input,
            arguments.output,
            arguments.preview,
            cartridge_id=arguments.cartridge_id,
            overwrite=arguments.overwrite,
        )
    except CartridgeError as error:
        return _emit_error(error)
    _emit_result(result)
    return 0


def main(argv: Sequence[str] | None = None) -> int:
    """Execute the Python CLI and return the stable process status."""

    arguments = _parser().parse_args(argv)
    try:
        if arguments.command == "pack":
            result = pack(
                _read_manifest(arguments.manifest),
                arguments.payload,
                arguments.output,
                arguments.preview,
                overwrite=arguments.overwrite,
            )
        elif arguments.command == "inspect":
            result = inspect(arguments.path)
        elif arguments.command == "validate":
            result = validate(arguments.path)
        else:
            result = hash_cartridge(arguments.path)
    except CartridgeError as error:
        return _emit_error(error)

    _emit_result(result)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
