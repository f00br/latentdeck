from __future__ import annotations

import json
import os

from latentdeck_codec_h3 import worker
from latentdeck_codec_h3.worker_state import WorkerCommandError


def test_worker_diagnostics_are_path_free_bounded_and_retained(tmp_path, monkeypatch) -> None:
    monkeypatch.setattr(worker.tempfile, "gettempdir", lambda: str(tmp_path))
    monkeypatch.setattr(worker.os, "getpid", lambda: 4242)
    directory = tmp_path / "LatentDeck" / "worker-diagnostics"
    directory.mkdir(parents=True)

    for index in range(20):
        path = directory / f"worker-{1000 + index}.jsonl"
        path.write_text("{}\n", encoding="utf-8")
        os.utime(path, ns=(index + 1, index + 1))

    error = WorkerCommandError(
        "worker.decode_failed",
        r"decoder failed at X:\private\private.lc",
        diagnostic_code="ring_map_failed",
        diagnostic_detail=r"MapViewOfFile failed at Y:\private\payload.safetensors secret-token",
    )
    worker._record_diagnostic("worker.decode_failed", code=error.code, error=error)

    files = list(directory.glob("worker-*.jsonl"))
    assert len(files) <= worker.MAX_DIAGNOSTIC_FILES
    current = directory / "worker-4242.jsonl"
    record = json.loads(current.read_text(encoding="utf-8"))
    assert set(record) == {
        "schema_version",
        "timestamp_ns",
        "event",
        "code",
        "error_type",
        "cause_code",
    }
    assert record["event"] == "worker.decode_failed"
    assert record["code"] == "worker.decode_failed"
    assert record["cause_code"] == "ring_map_failed"

    serialized = current.read_text(encoding="utf-8")
    for forbidden in (
        "X:\\private",
        "Y:\\private",
        "private.lc",
        "payload.safetensors",
        "secret-token",
        "detail",
        '"pid"',
    ):
        assert forbidden.casefold() not in serialized.casefold()


def test_worker_diagnostics_drop_invalid_tokens_and_whole_records_at_budget(
    tmp_path, monkeypatch
) -> None:
    monkeypatch.setattr(worker.tempfile, "gettempdir", lambda: str(tmp_path))
    monkeypatch.setattr(worker.os, "getpid", lambda: 99)
    monkeypatch.setattr(worker, "MAX_DIAGNOSTIC_BYTES", 256)

    worker._record_diagnostic(r"X:\private\escape")
    path = tmp_path / "LatentDeck" / "worker-diagnostics" / "worker-99.jsonl"
    assert not path.exists()

    worker._record_diagnostic("worker.started", code=r"X:\private\escape")
    before = path.read_bytes()
    assert b'"code"' not in before
    while path.stat().st_size < worker.MAX_DIAGNOSTIC_BYTES - 8:
        with path.open("ab") as stream:
            stream.write(b"x")
    size_before = path.stat().st_size
    worker._record_diagnostic("worker.finished")
    assert path.stat().st_size == size_before
    assert path.read_bytes().startswith(before)
