from __future__ import annotations

import hashlib
import json
import struct
import tempfile
import unittest
import uuid
from pathlib import Path
from typing import BinaryIO

import torch

from latentdeck_codec_h3.resample_spool import (
    H3ResampleSpool,
    ResampleAudioSource,
    ResampleSpoolError,
)


def _slot(value: float = 0.0, *, height: int = 2, width: int = 3) -> torch.Tensor:
    return torch.full((1, 24, 1, height, width), value, dtype=torch.float16)


def _read_safetensors(path: Path) -> tuple[dict[str, object], bytes]:
    encoded = path.read_bytes()
    header_length = struct.unpack("<Q", encoded[:8])[0]
    header = json.loads(encoded[8 : 8 + header_length])
    return header, encoded[8 + header_length :]


class H3ResampleSpoolTests(unittest.TestCase):
    def test_streams_one_complete_h3_visual_tensor_to_a_bounded_partial(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            capture_id = str(uuid.uuid4())
            spool = H3ResampleSpool(root, capture_id, latent_height=2, latent_width=3)

            for value in range(7):
                spool.append_slot(_slot(float(value)))
            receipt = spool.finish()

            self.assertEqual(receipt.capture_id, capture_id)
            self.assertEqual(receipt.shape, (1, 24, 7, 2, 3))
            self.assertEqual(receipt.decoded_frame_count, 22)
            self.assertEqual(receipt.storage_dtype, "F16")
            self.assertTrue(receipt.payload_path.name.endswith(".safetensors.partial"))
            self.assertFalse((root / f"{capture_id}.visual.f16.partial").exists())
            header, payload = _read_safetensors(receipt.payload_path)
            self.assertEqual(
                header,
                {
                    "video": {
                        "data_offsets": [0, 1 * 24 * 7 * 2 * 3 * 2],
                        "dtype": "F16",
                        "shape": [1, 24, 7, 2, 3],
                    }
                },
            )
            expected = torch.cat([_slot(float(value)) for value in range(7)], dim=2)
            self.assertEqual(payload, expected.contiguous().numpy().tobytes())
            self.assertEqual(receipt.byte_length, receipt.payload_path.stat().st_size)
            self.assertEqual(
                receipt.sha256,
                hashlib.sha256(receipt.payload_path.read_bytes()).hexdigest(),
            )

    def test_header_and_payload_are_deterministic(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            receipts = []
            for _ in range(2):
                spool = H3ResampleSpool(
                    root,
                    str(uuid.uuid4()),
                    latent_height=1,
                    latent_width=1,
                )
                spool.append_slot(_slot(0.25, height=1, width=1))
                spool.append_slot(_slot(-0.5, height=1, width=1))
                receipts.append(spool.finish())
            self.assertEqual(
                receipts[0].payload_path.read_bytes(),
                receipts[1].payload_path.read_bytes(),
            )

    def test_streams_exact_source_audio_after_visual_without_buffering_it(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            audio_tensor = torch.arange(1 * 32 * 2 * 8, dtype=torch.float32).reshape(1, 32, 2, 8)
            audio_bytes = audio_tensor.numpy().tobytes()
            copy_calls = 0

            def copy_audio(destination: BinaryIO) -> int:
                nonlocal copy_calls
                copy_calls += 1
                write = destination.write
                halfway = len(audio_bytes) // 2
                return write(audio_bytes[:halfway]) + write(audio_bytes[halfway:])

            audio = ResampleAudioSource(
                storage_dtype="F32",
                shape=(1, 32, 2, 8),
                byte_length=len(audio_bytes),
                copy_to=copy_audio,
            )
            spool = H3ResampleSpool(root, str(uuid.uuid4()), 1, 1)
            spool.append_slot(_slot(0.25, height=1, width=1))
            spool.append_slot(_slot(-0.5, height=1, width=1))

            receipt = spool.finish(audio=audio)

            self.assertEqual(copy_calls, 1)
            self.assertIsNotNone(receipt.audio)
            assert receipt.audio is not None
            self.assertEqual(receipt.audio.storage_dtype, "F32")
            header, payload = _read_safetensors(receipt.payload_path)
            visual_bytes = 1 * 24 * 2 * 1 * 1 * 2
            self.assertEqual(
                header["audio"],
                {
                    "data_offsets": [visual_bytes, visual_bytes + len(audio_bytes)],
                    "dtype": "F32",
                    "shape": [1, 32, 2, 8],
                },
            )
            expected_visual = (
                torch.cat(
                    [_slot(0.25, height=1, width=1), _slot(-0.5, height=1, width=1)],
                    dim=2,
                )
                .numpy()
                .tobytes()
            )
            self.assertEqual(payload, expected_visual + audio_bytes)

    def test_rejects_audio_descriptor_byte_mismatch_before_finalization(self) -> None:
        with self.assertRaisesRegex(ResampleSpoolError, "audio byte length"):
            ResampleAudioSource(
                storage_dtype="F16",
                shape=(1, 32, 2, 8),
                byte_length=1,
                copy_to=lambda _: 1,
            )

    def test_rejects_invalid_slot_before_writing_it(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            spool = H3ResampleSpool(root, str(uuid.uuid4()), 2, 3)
            invalid_cases = [
                torch.zeros((1, 23, 1, 2, 3), dtype=torch.float16),
                torch.zeros((1, 24, 2, 2, 3), dtype=torch.float16),
                torch.zeros((1, 24, 1, 2, 3), dtype=torch.float32),
                torch.zeros((1, 24, 1, 3, 2), dtype=torch.float16),
            ]
            for invalid in invalid_cases:
                with (
                    self.subTest(shape=tuple(invalid.shape), dtype=str(invalid.dtype)),
                    self.assertRaisesRegex(ResampleSpoolError, "slot"),
                ):
                    spool.append_slot(invalid)
            self.assertEqual(spool.latent_slots, 0)
            spool.abort()

    def test_rejects_non_finite_slot_and_invalid_final_cadence(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            spool = H3ResampleSpool(root, str(uuid.uuid4()), 2, 3)
            non_finite = _slot()
            non_finite[0, 0, 0, 0, 0] = torch.inf
            with self.assertRaisesRegex(ResampleSpoolError, "finite"):
                spool.append_slot(non_finite)
            for _ in range(3):
                spool.append_slot(_slot())
            with self.assertRaisesRegex(ResampleSpoolError, r"2 \+ 5n"):
                spool.finish()
            self.assertTrue(spool.raw_path.exists())
            spool.abort()

    def test_enforces_slot_and_byte_limits_without_partial_append(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            slot_bytes = _slot(height=1, width=1).numel() * 2
            spool = H3ResampleSpool(
                root,
                str(uuid.uuid4()),
                latent_height=1,
                latent_width=1,
                max_latent_slots=2,
                max_visual_bytes=slot_bytes * 2,
            )
            spool.append_slot(_slot(height=1, width=1))
            spool.append_slot(_slot(height=1, width=1))
            before = spool.raw_path.stat().st_size
            with self.assertRaisesRegex(ResampleSpoolError, "limit"):
                spool.append_slot(_slot(height=1, width=1))
            self.assertEqual(spool.raw_path.stat().st_size, before)
            receipt = spool.finish()
            self.assertEqual(receipt.shape[2], 2)

    def test_abort_removes_only_capture_owned_partial_files(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            unrelated = root / "keep.txt"
            unrelated.write_text("owned elsewhere", encoding="utf-8")
            capture_id = str(uuid.uuid4())
            spool = H3ResampleSpool(root, capture_id, 1, 1)
            spool.append_slot(_slot(height=1, width=1))
            raw_path = spool.raw_path

            spool.abort()

            self.assertFalse(raw_path.exists())
            self.assertFalse((root / f"{capture_id}.safetensors.partial").exists())
            self.assertEqual(unrelated.read_text(encoding="utf-8"), "owned elsewhere")

    def test_rejects_noncanonical_capture_id_and_impossible_geometry(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            with self.assertRaises(ResampleSpoolError):
                H3ResampleSpool(root, "../escape", 1, 1)
            with self.assertRaises(ResampleSpoolError):
                H3ResampleSpool(root, str(uuid.uuid4()).upper(), 1, 1)
            with self.assertRaises(ResampleSpoolError):
                H3ResampleSpool(root, str(uuid.uuid4()), 0, 1)


if __name__ == "__main__":
    unittest.main()
