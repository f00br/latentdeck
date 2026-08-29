from __future__ import annotations

import hashlib
from pathlib import Path

import pytest

from latentdeck_codec_h3.decoder import CodecRuntimeError, validate_decoder_asset


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
