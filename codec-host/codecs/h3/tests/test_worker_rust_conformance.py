from __future__ import annotations

import io
import struct
import subprocess
from pathlib import Path

from latentdeck_codec_h3.worker import StreamConnection, run_worker

WORKSPACE_ROOT = Path(__file__).resolve().parents[4]


class MemoryConnector:
    def __init__(self, inbound: bytes) -> None:
        self.outbound = io.BytesIO()
        self._connection = StreamConnection(io.BytesIO(inbound), self.outbound)

    def connect(self, _pipe_name: str) -> StreamConnection:
        return self._connection


def _rust(mode: str, payload: bytes | None = None) -> bytes:
    completed = subprocess.run(
        [
            "cargo",
            "run",
            "--quiet",
            "-p",
            "latentdeck-control",
            "--example",
            "worker_protocol_conformance",
            "--",
            mode,
        ],
        cwd=WORKSPACE_ROOT,
        input=payload,
        capture_output=True,
        check=False,
        timeout=120,
    )
    if completed.returncode != 0:
        raise AssertionError(completed.stderr.decode(errors="replace"))
    return completed.stdout


def _split_bootstrap(generated: bytes) -> tuple[bytes, bytes]:
    assert len(generated) >= 4
    bootstrap_length = struct.unpack("<I", generated[:4])[0]
    boundary = 4 + bootstrap_length
    assert boundary <= len(generated)
    return generated[:boundary], generated[boundary:]


def _run_generated_session(mode: str) -> tuple[int, bytes]:
    bootstrap, commands = _split_bootstrap(_rust(mode))
    connector = MemoryConnector(commands)
    exit_code = run_worker(io.BytesIO(bootstrap), connector=connector)
    return exit_code, connector.outbound.getvalue()


def test_rust_commands_and_python_responses_share_one_typed_session() -> None:
    exit_code, responses = _run_generated_session("emit-session")
    assert exit_code == 0
    _rust("validate-session", responses)


def test_rust_sequence_gap_becomes_a_python_fatal_event_rust_accepts() -> None:
    exit_code, responses = _run_generated_session("emit-sequence-gap")
    assert exit_code == 2
    _rust("validate-sequence-gap", responses)
