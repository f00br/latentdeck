"""Isolated Codec Pack v2 runtime probe for the official H3 pack.

This file is executed by the pack-owned CPython runtime with isolated-mode
flags.  It intentionally stops at ``codec.descriptor``: no cartridge, decoder
asset, CUDA tensor, or model is needed to prove the Protocol 2 pre-load path.
"""

from __future__ import annotations

import base64
import importlib.metadata
import importlib.util
import json
import platform
import sys
import uuid
from collections.abc import Mapping, Sequence

EXPECTED_PACKAGES = {
    "latentdeck-cartridge": "0.1.0",
    "latentdeck-codec-h3": "0.2.0",
    "latentdeck-codec-host": "0.1.0",
    "latentdeck-codec-sdk": "0.2.0",
    "latentdeck-deck-sdk": "0.2.0",
    "latentdeck-rgb-ring": "0.1.0",
    "msgpack": "1.2.2",
    "numpy": "2.5.2",
    "safetensors": "0.8.0",
    "torch": "2.13.0+cu130",
    "tqdm": "4.70.0",
}
REQUIRED_CAPABILITIES = (
    "player",
    "realtime",
    "resample",
    "snapshot_capture",
    "live_capture",
    "raw_import",
)
H3_PROFILE = {
    "codec_family": "minimax_h3",
    "profile": "h3_av_latent",
    "profile_version": "0.1.0",
}
FORBIDDEN_MODULES = ("comfy", "comfyui", "diffusers", "transformers")


def main(arguments: Sequence[str] | None = None) -> int:
    values = list(sys.argv[1:] if arguments is None else arguments)
    if len(values) != 3:
        raise RuntimeError(
            "expected require_cuda, exact_python_version, and base64 codec-pack.json"
        )
    if values[0] not in {"true", "false"}:
        raise RuntimeError("require_cuda must be true or false")
    require_cuda = values[0] == "true"
    expected_python = values[1]
    manifest = _decode_manifest(values[2])
    _validate_manifest_contract(manifest)

    versions = {name: importlib.metadata.version(name) for name in sorted(EXPECTED_PACKAGES)}
    if versions != EXPECTED_PACKAGES:
        raise RuntimeError(f"runtime version drift: {versions!r}")
    if platform.python_version() != expected_python:
        raise RuntimeError(
            f"runtime CPython drift: expected {expected_python!r}, "
            f"found {platform.python_version()!r}"
        )
    if sys.version_info[:2] != (3, 13) or platform.architecture()[0] != "64bit":
        raise RuntimeError("runtime is not CPython 3.13 x64")
    if "torch" in sys.modules:
        raise RuntimeError("Torch was imported before the Codec Pack probe began")

    external_decoder_accesses: list[str] = []

    def audit(event: str, args: tuple[object, ...]) -> None:
        if event != "open" or not args:
            return
        candidate = str(args[0]).replace("\\", "/").lower()
        if candidate.endswith("/taeh3.safetensors") or candidate == "taeh3.safetensors":
            external_decoder_accesses.append(candidate)

    sys.addaudithook(audit)

    from latentdeck_codec_h3.adapter import make_adapter
    from latentdeck_codec_host.native_cartridge import NativeCartridgeAccessFactory
    from latentdeck_codec_host.runtime_v2 import Protocol2Worker, TrustedCodecEntrypoint
    from latentdeck_rgb_ring import (
        BINDING_ABI_VERSION,
        PROTOCOL2_BINDING_ABI_VERSION,
        WindowsSharedRingTransport,
    )

    if "torch" in sys.modules:
        raise RuntimeError("H3 Protocol 2 modules imported Torch before codec.load")
    if BINDING_ABI_VERSION != "1" or PROTOCOL2_BINDING_ABI_VERSION != "2":
        raise RuntimeError("native RGB ring ABI drift")

    direct_descriptor = make_adapter().descriptor()
    expected_descriptor = _expected_descriptor(manifest)
    if _descriptor_wire(direct_descriptor) != expected_descriptor:
        raise RuntimeError("H3 make_adapter descriptor does not match codec-pack.json")
    if "torch" in sys.modules or external_decoder_accesses:
        raise RuntimeError("H3 descriptor path touched Torch or the external decoder")

    manifest_adapter = _mapping(manifest["adapter"], "adapter")
    trusted = TrustedCodecEntrypoint(
        pack_id=str(manifest["pack_id"]),
        pack_version=str(manifest["pack_version"]),
        adapter_id=str(manifest_adapter["adapter_id"]),
        adapter_version=str(manifest_adapter["adapter_version"]),
        entrypoint=str(manifest_adapter["entrypoint"]),
    )
    trusted.validate()
    ring = WindowsSharedRingTransport()
    worker = Protocol2Worker(
        session_id=uuid.uuid4(),
        codec_entrypoints=(trusted,),
        deck_entrypoints=(),
        cartridge_access_factory=NativeCartridgeAccessFactory(),
        ring_transport=ring,
    )
    commands: list[str] = []
    sequence = 1
    try:
        configure = _command(
            worker.session_id,
            sequence,
            "session.configure",
            {
                "selected_protocol_version": 2,
                "app_version": "0.2.0",
                "heartbeat_interval_ms": 1_000,
                "heartbeat_hard_timeout_ms": 5_000,
                "max_frame_bytes": 262_144,
                "max_inflight_batches": 1,
                "requested_capabilities": list(REQUIRED_CAPABILITIES),
            },
        )
        configure_reply = worker.handle_envelope(configure)
        _assert_ack(configure_reply, "session.configure")
        commands.append("session.configure")
        sequence += 1

        descriptor_command = _command(
            worker.session_id,
            sequence,
            "codec.descriptor",
            {
                "pack_id": str(manifest["pack_id"]),
                "pack_version": str(manifest["pack_version"]),
                "adapter_id": str(manifest_adapter["adapter_id"]),
            },
        )
        descriptor_reply = worker.handle_envelope(descriptor_command)
        descriptor_payload = _assert_ack(descriptor_reply, "codec.descriptor")
        commands.append("codec.descriptor")
        if descriptor_payload != expected_descriptor:
            raise RuntimeError("Protocol 2 codec.descriptor does not match codec-pack.json")
        if "torch" in sys.modules or external_decoder_accesses:
            raise RuntimeError("Protocol 2 pre-load path touched Torch or the external decoder")
    finally:
        worker.abort_transport()
        ring.close()

    for forbidden in FORBIDDEN_MODULES:
        if importlib.util.find_spec(forbidden) is not None:
            raise RuntimeError(f"forbidden module is importable: {forbidden}")

    torch_imported_before_preload = "torch" in sys.modules
    if torch_imported_before_preload:
        raise RuntimeError("Torch was imported before all pre-load checks completed")

    import torch

    if str(torch.__version__) != EXPECTED_PACKAGES["torch"]:
        raise RuntimeError(f"unexpected Torch build: {torch.__version__!r}")
    if torch.version.cuda != "13.0":
        raise RuntimeError(f"unexpected PyTorch CUDA runtime: {torch.version.cuda!r}")
    cuda_available = bool(torch.cuda.is_available())
    device_name = None
    if require_cuda and not cuda_available:
        raise RuntimeError("CUDA was required but is unavailable")
    if require_cuda:
        value = torch.tensor([2.0], device="cuda") * 3.0
        torch.cuda.synchronize()
        if value.cpu().item() != 6.0:
            raise RuntimeError("CUDA arithmetic smoke failed")
        device_name = torch.cuda.get_device_name(0)

    print(
        json.dumps(
            {
                "adapter": expected_descriptor,
                "cuda": {
                    "available": cuda_available,
                    "compiled_runtime": torch.version.cuda,
                    "device_name": device_name,
                    "required": require_cuda,
                },
                "forbidden_modules": "absent",
                "packages": versions,
                "preload_guards": {
                    "external_decoder_accesses": len(external_decoder_accesses),
                    "torch_imported": torch_imported_before_preload,
                },
                "protocol": {
                    "commands": commands,
                    "selected_version": 2,
                    "worker_protocol": int(
                        _mapping(manifest["compatibility"], "compatibility")["worker_protocol"]
                    ),
                },
                "python": platform.python_version(),
                "rgb_ring_abi": {
                    "protocol1": BINDING_ABI_VERSION,
                    "protocol2": PROTOCOL2_BINDING_ABI_VERSION,
                },
            },
            sort_keys=True,
            separators=(",", ":"),
            allow_nan=False,
        )
    )
    return 0


def _decode_manifest(encoded: str) -> dict[str, object]:
    try:
        raw = base64.b64decode(encoded, validate=True)
        manifest = json.loads(raw.decode("utf-8"), parse_constant=_reject_json_constant)
    except (UnicodeError, ValueError, json.JSONDecodeError) as error:
        raise RuntimeError("codec-pack.json probe input is invalid") from error
    if not isinstance(manifest, dict) or any(not isinstance(key, str) for key in manifest):
        raise RuntimeError("codec-pack.json probe input must be an object")
    return manifest


def _validate_manifest_contract(manifest: Mapping[str, object]) -> None:
    compatibility = _mapping(manifest.get("compatibility"), "compatibility")
    adapter = _mapping(manifest.get("adapter"), "adapter")
    python = _mapping(compatibility.get("python"), "compatibility.python")
    profiles = _array(compatibility.get("profiles"), "compatibility.profiles")
    capabilities = _array(manifest.get("capabilities"), "capabilities")
    external_assets = _array(manifest.get("external_assets"), "external_assets")
    if (
        manifest.get("manifest_version") != "2.0.0"
        or manifest.get("kind") != "codec_pack"
        or manifest.get("pack_id") != "org.latentdeck.h3"
        or manifest.get("pack_version") != "0.2.0"
        or compatibility.get("worker_protocol") != 2
        or compatibility.get("codec_adapter_api") != 1
        or compatibility.get("tensor_abi") != "latentdeck.tensor.v1"
        or compatibility.get("torch_exact_build") != EXPECTED_PACKAGES["torch"]
        or python
        != {
            "implementation": "cpython",
            "version": "3.13",
            "platform_tag": "win_amd64",
        }
        or profiles != [H3_PROFILE]
        or capabilities != list(REQUIRED_CAPABILITIES)
        or adapter
        != {
            "adapter_id": "org.latentdeck.h3",
            "adapter_version": "0.2.0",
            "entrypoint": "latentdeck_codec_h3.adapter:make_adapter",
        }
    ):
        raise RuntimeError("codec-pack.json does not match the exact H3 v2 runtime contract")
    if len(external_assets) != 1:
        raise RuntimeError("H3 v2 must declare exactly one external decoder")
    asset = _mapping(external_assets[0], "external_assets[0]")
    if (
        asset.get("asset_id") != "taeh3"
        or asset.get("byte_length") != 22_709_752
        or asset.get("sha256") != "4fd022bfcab08772fe0536b17ea1a3bbb5625be11e397868d1c5d891863d4c13"
    ):
        raise RuntimeError("H3 external decoder identity drift")


def _expected_descriptor(manifest: Mapping[str, object]) -> dict[str, object]:
    adapter = _mapping(manifest["adapter"], "adapter")
    compatibility = _mapping(manifest["compatibility"], "compatibility")
    return {
        "pack_id": manifest["pack_id"],
        "pack_version": manifest["pack_version"],
        "adapter_id": adapter["adapter_id"],
        "adapter_version": adapter["adapter_version"],
        "host_api_version": "2.0",
        "capabilities": list(REQUIRED_CAPABILITIES),
        "profiles": list(_array(compatibility["profiles"], "compatibility.profiles")),
    }


def _descriptor_wire(descriptor: object) -> dict[str, object]:
    return {
        "pack_id": descriptor.pack_id,
        "pack_version": descriptor.pack_version,
        "adapter_id": descriptor.adapter_id,
        "adapter_version": descriptor.adapter_version,
        "host_api_version": descriptor.host_api_version,
        "capabilities": [item.value for item in descriptor.capabilities],
        "profiles": [
            {
                "codec_family": item.codec_family,
                "profile": item.profile,
                "profile_version": item.profile_version,
            }
            for item in descriptor.profiles
        ],
    }


def _command(
    session_id: uuid.UUID,
    sequence: int,
    name: str,
    payload: Mapping[str, object],
) -> dict[str, object]:
    return {
        "protocol": "latentdeck.worker",
        "protocol_version": 2,
        "session_id": str(session_id),
        "sequence": sequence,
        "message_id": str(uuid.uuid4()),
        "sender_uptime_ns": sequence,
        "message": {
            "kind": "command",
            "body": {"name": name, "payload": dict(payload)},
        },
    }


def _assert_ack(reply: Mapping[str, object], command: str) -> dict[str, object]:
    message = _mapping(reply.get("message"), "reply.message")
    if message.get("kind") != "ack":
        raise RuntimeError(f"Protocol 2 {command} failed: {message!r}")
    body = _mapping(message.get("body"), "reply.message.body")
    ack = _mapping(body.get("ack"), "reply.message.body.ack")
    if ack.get("name") != command:
        raise RuntimeError(f"Protocol 2 ack mismatch for {command}")
    return dict(_mapping(ack.get("payload"), "reply.message.body.ack.payload"))


def _mapping(value: object, label: str) -> dict[str, object]:
    if not isinstance(value, dict) or any(not isinstance(key, str) for key in value):
        raise RuntimeError(f"{label} must be an object")
    return value


def _array(value: object, label: str) -> list[object]:
    if not isinstance(value, list):
        raise RuntimeError(f"{label} must be an array")
    return value


def _reject_json_constant(value: str) -> object:
    raise ValueError(f"non-finite JSON constant {value}")


if __name__ == "__main__":
    raise SystemExit(main())
