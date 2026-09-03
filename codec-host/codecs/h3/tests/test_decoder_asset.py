from __future__ import annotations

import hashlib
from pathlib import Path

import pytest

import latentdeck_codec_h3.decoder as decoder_module
from latentdeck_codec_h3.decoder import (
    CodecRuntimeError,
    configure_torch_cpu_threads,
    host_validated_decoder_path,
    validate_decoder_asset,
)


def test_explicit_decoder_asset_is_revalidated_by_size_and_hash(tmp_path: Path) -> None:
    asset = tmp_path / "taeh3.safetensors"
    asset.write_bytes(b"synthetic external asset")
    digest = hashlib.sha256(asset.read_bytes()).hexdigest()

    assert validate_decoder_asset(asset, digest, asset.stat().st_size) == asset

    with pytest.raises(CodecRuntimeError, match="byte length changed"):
        validate_decoder_asset(asset, digest, asset.stat().st_size + 1)
    with pytest.raises(CodecRuntimeError, match="hash changed"):
        validate_decoder_asset(asset, "0" * 64, asset.stat().st_size)


def test_decoder_asset_rejects_noncanonical_expectations(tmp_path: Path) -> None:
    asset = tmp_path / "weight.safetensors"
    asset.write_bytes(b"x")

    with pytest.raises(CodecRuntimeError, match="must be positive"):
        validate_decoder_asset(asset, "0" * 64, 0)
    with pytest.raises(CodecRuntimeError, match="not canonical"):
        validate_decoder_asset(asset, "A" * 64, 1)


def test_host_validated_decoder_path_does_not_repeat_the_payload_hash(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    asset = tmp_path / "taeh3.safetensors"
    asset.write_bytes(b"host-retained exact bytes")
    digest = hashlib.sha256(asset.read_bytes()).hexdigest()

    def unexpected_hash() -> object:
        raise AssertionError("Protocol 2 worker repeated the host payload hash")

    monkeypatch.setattr(decoder_module.hashlib, "sha256", unexpected_hash)
    assert host_validated_decoder_path(asset, digest, asset.stat().st_size) == asset


def test_pack_local_torch_thread_limits_are_exact_and_idempotent(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    class FakeTorch:
        intra = 20
        inter = 8
        calls: list[tuple[str, int]] = []

        @classmethod
        def get_num_threads(cls) -> int:
            return cls.intra

        @classmethod
        def get_num_interop_threads(cls) -> int:
            return cls.inter

        @classmethod
        def set_num_threads(cls, value: int) -> None:
            cls.calls.append(("intra", value))
            cls.intra = value

        @classmethod
        def set_num_interop_threads(cls, value: int) -> None:
            cls.calls.append(("inter", value))
            cls.inter = value

    for name in decoder_module.TORCH_ENVIRONMENT:
        monkeypatch.setenv(name, "inherited")

    assert configure_torch_cpu_threads(FakeTorch) == 1
    assert configure_torch_cpu_threads(FakeTorch) == 1
    assert FakeTorch.calls == [("inter", 1), ("intra", 1)]
    assert {
        name: decoder_module.os.environ[name] for name in decoder_module.TORCH_ENVIRONMENT
    } == decoder_module.TORCH_ENVIRONMENT
