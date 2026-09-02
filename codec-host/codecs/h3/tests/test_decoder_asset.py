from __future__ import annotations

import hashlib
from pathlib import Path

import pytest

import latentdeck_codec_h3.decoder as decoder_module
from latentdeck_codec_h3.decoder import (
    CodecRuntimeError,
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
