from __future__ import annotations

import types
import uuid

import latentdeck_codec_host.__main__ as cli
import pytest
from latentdeck_codec_host import TrustedCodecEntrypoint

VALID_PROTOCOL2_ARGUMENTS = [
    "--worker-protocol",
    "2",
    "--codec-pack-id",
    "org.latentdeck.h3",
    "--codec-pack-version",
    "0.2.0",
    "--codec-adapter-id",
    "org.latentdeck.h3",
    "--codec-adapter-version",
    "0.2.0",
    "--codec-entrypoint",
    "latentdeck_codec_h3.adapter:make_adapter",
]


@pytest.mark.parametrize(
    "arguments",
    [
        [*VALID_PROTOCOL2_ARGUMENTS, "--unknown", "value"],
        [*VALID_PROTOCOL2_ARGUMENTS, "--codec-pack-id", "org.example.duplicate"],
        VALID_PROTOCOL2_ARGUMENTS[:-2],
        ["--worker-protocol", "1", *VALID_PROTOCOL2_ARGUMENTS[2:]],
    ],
    ids=["unknown", "duplicate", "missing", "protocol-1"],
)
def test_protocol2_cli_rejects_open_or_ambiguous_launch_arguments(arguments) -> None:
    with pytest.raises(SystemExit) as raised:
        cli.main(arguments)

    assert raised.value.code == 2


def test_protocol2_cli_builds_one_exact_codec_and_no_startup_decks(monkeypatch) -> None:
    stdin = object()
    ring = _FakeRing()
    access_factory = object()
    captured: dict[str, object] = {}

    monkeypatch.setattr(cli.sys, "stdin", types.SimpleNamespace(buffer=stdin))
    monkeypatch.setattr(cli, "WindowsSharedRingTransport", lambda: ring)
    monkeypatch.setattr(cli, "NativeCartridgeAccessFactory", lambda: access_factory)

    def make_worker(**keywords):
        captured["worker_keywords"] = keywords
        return types.SimpleNamespace(session_id=keywords["session_id"])

    def run_service(stream, **keywords):
        captured["stream"] = stream
        captured["service_keywords"] = keywords
        captured["worker"] = keywords["worker_factory"](
            uuid.UUID("a877f311-c911-49e9-9d38-af924b55fd8e")
        )
        return 0

    monkeypatch.setattr(cli, "Protocol2Worker", make_worker)
    monkeypatch.setattr(cli, "run_protocol2_service", run_service)

    assert cli.main(VALID_PROTOCOL2_ARGUMENTS) == 0
    assert captured["stream"] is stdin
    worker_keywords = captured["worker_keywords"]
    assert worker_keywords == {
        "session_id": uuid.UUID("a877f311-c911-49e9-9d38-af924b55fd8e"),
        "codec_entrypoints": (
            TrustedCodecEntrypoint(
                pack_id="org.latentdeck.h3",
                pack_version="0.2.0",
                adapter_id="org.latentdeck.h3",
                adapter_version="0.2.0",
                entrypoint="latentdeck_codec_h3.adapter:make_adapter",
            ),
        ),
        "deck_entrypoints": (),
        "cartridge_access_factory": access_factory,
        "ring_transport": ring,
    }
    assert captured["service_keywords"]["worker_identity"] == "org.latentdeck.h3.worker"
    assert ring.closed


@pytest.mark.parametrize("failure", ["construction", "service"])
def test_protocol2_cli_closes_native_ring_on_failure(monkeypatch, failure: str) -> None:
    ring = _FakeRing()
    monkeypatch.setattr(cli.sys, "stdin", types.SimpleNamespace(buffer=object()))
    monkeypatch.setattr(cli, "WindowsSharedRingTransport", lambda: ring)
    monkeypatch.setattr(cli, "NativeCartridgeAccessFactory", object)

    if failure == "construction":
        monkeypatch.setattr(
            cli,
            "Protocol2Worker",
            lambda **_keywords: (_ for _ in ()).throw(RuntimeError("construction failed")),
        )

        def run_service(_stream, **keywords):
            keywords["worker_factory"](uuid.uuid4())
            raise AssertionError("unreachable")

    else:
        monkeypatch.setattr(
            cli,
            "Protocol2Worker",
            lambda **keywords: types.SimpleNamespace(session_id=keywords["session_id"]),
        )

        def run_service(_stream, **keywords):
            keywords["worker_factory"](uuid.uuid4())
            raise RuntimeError("service failed")

    monkeypatch.setattr(cli, "run_protocol2_service", run_service)

    with pytest.raises(RuntimeError):
        cli.main(VALID_PROTOCOL2_ARGUMENTS)

    assert ring.closed


class _FakeRing:
    def __init__(self) -> None:
        self.closed = False

    def close(self) -> None:
        self.closed = True
