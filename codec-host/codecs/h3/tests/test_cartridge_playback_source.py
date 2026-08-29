from __future__ import annotations

import hashlib
import json
import struct
from pathlib import Path

import pytest
from latentdeck_cartridge import hash as hash_cartridge
from latentdeck_cartridge import pack_raw_h3

from latentdeck_codec_h3.cartridge import CartridgeLoadError, load_video_source


def safetensors_payload(dtype: str = "F16") -> bytes:
    byte_width = 2 if dtype == "F16" else 4
    tensor_bytes = bytes(1 * 24 * 32 * byte_width)
    header = json.dumps(
        {
            "video": {
                "dtype": dtype,
                "shape": [1, 24, 32, 1, 1],
                "data_offsets": [0, len(tensor_bytes)],
            }
        },
        separators=(",", ":"),
    ).encode()
    header += b" " * (-len(header) % 8)
    return struct.pack("<Q", len(header)) + header + tensor_bytes


def packed_cartridge(tmp_path: Path, dtype: str = "F16") -> tuple[Path, str]:
    payload = tmp_path / f"{dtype}.safetensors"
    cartridge = tmp_path / f"{dtype}.lc"
    payload.write_bytes(safetensors_payload(dtype))
    pack_raw_h3(payload, cartridge)
    receipt = hash_cartridge(cartridge)
    return cartridge, str(receipt["sha256"])


@pytest.mark.parametrize("dtype", ["F16", "F32"])
def test_reads_only_the_visual_tensor_and_exact_h3_cycle_contract(
    tmp_path: Path,
    dtype: str,
) -> None:
    cartridge, archive_hash = packed_cartridge(tmp_path, dtype)

    source = load_video_source(cartridge, archive_hash)

    assert source.storage_dtype == dtype
    assert source.shape == (1, 24, 32, 1, 1)
    assert len(source.video_bytes) == 1 * 24 * 32 * (2 if dtype == "F16" else 4)
    assert (source.width, source.height, source.frame_count) == (16, 16, 107)
    assert source.cycle_count == 7
    assert source.cycle(0).latent_count == 2
    assert source.cycle(0).decoded_frame_count == 5
    assert source.cycle(6).latent_start == 27
    assert source.cycle(6).decoded_start_frame == 90
    assert source.cycle(6).decoded_frame_count == 17
    assert source.cycle(6).end_of_stream is True


def test_rejects_path_bytes_that_changed_after_rust_validation(tmp_path: Path) -> None:
    cartridge, archive_hash = packed_cartridge(tmp_path)
    changed = bytearray(cartridge.read_bytes())
    changed[-1] ^= 0x01
    cartridge.write_bytes(changed)

    with pytest.raises(CartridgeLoadError, match="hash changed"):
        load_video_source(cartridge, archive_hash)


def test_rejects_noncanonical_hash_and_cycle_seek(tmp_path: Path) -> None:
    cartridge, archive_hash = packed_cartridge(tmp_path)
    with pytest.raises(CartridgeLoadError, match="not canonical"):
        load_video_source(cartridge, archive_hash.upper())

    source = load_video_source(cartridge, archive_hash)
    with pytest.raises(CartridgeLoadError, match="outside"):
        source.cycle(source.cycle_count)


def test_fixture_hash_is_bound_to_the_actual_archive(tmp_path: Path) -> None:
    cartridge, archive_hash = packed_cartridge(tmp_path)
    assert hashlib.sha256(cartridge.read_bytes()).hexdigest() == archive_hash
