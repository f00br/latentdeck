"""Protocol 2-only command-line entry point for the isolated codec host."""

from __future__ import annotations

import argparse
import sys
from collections.abc import Sequence

from latentdeck_rgb_ring import WindowsSharedRingTransport

from . import __version__
from .native_cartridge import NativeCartridgeAccessFactory
from .runtime_v2 import (
    Protocol2Worker,
    TrustedCodecEntrypoint,
    run_protocol2_service,
)

_PROTOCOL2_FIELDS = (
    "worker_protocol",
    "codec_pack_id",
    "codec_pack_version",
    "codec_adapter_id",
    "codec_adapter_version",
    "codec_entrypoint",
)


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(prog="latentdeck-codec-host")
    parser.add_argument("--version", action="store_true", help="print the package version")
    parser.add_argument("--worker-protocol", type=int, choices=(2,))
    parser.add_argument("--codec-pack-id")
    parser.add_argument("--codec-pack-version")
    parser.add_argument("--codec-adapter-id")
    parser.add_argument("--codec-adapter-version")
    parser.add_argument("--codec-entrypoint")

    raw_arguments = list(sys.argv[1:] if argv is None else argv)
    _reject_duplicate_options(parser, raw_arguments)
    arguments = parser.parse_args(raw_arguments)

    if arguments.version:
        if len(raw_arguments) != 1:
            parser.error("--version cannot be combined with a worker launch")
        print(f"latentdeck-codec-host {__version__}")
        return 0

    missing = [
        field.replace("_", "-") for field in _PROTOCOL2_FIELDS if getattr(arguments, field) is None
    ]
    if missing:
        parser.error(f"Protocol 2 launch is missing required options: {', '.join(missing)}")

    trusted_codec = TrustedCodecEntrypoint(
        pack_id=arguments.codec_pack_id,
        pack_version=arguments.codec_pack_version,
        adapter_id=arguments.codec_adapter_id,
        adapter_version=arguments.codec_adapter_version,
        entrypoint=arguments.codec_entrypoint,
    )
    try:
        trusted_codec.validate()
    except ValueError as error:
        parser.error(str(error))

    ring_transport: WindowsSharedRingTransport | None = None
    worker_factory_called = False

    def worker_factory(session_id):
        nonlocal ring_transport, worker_factory_called
        if worker_factory_called:
            raise RuntimeError("Protocol 2 worker factory is single-use")
        worker_factory_called = True
        access_factory = NativeCartridgeAccessFactory()
        ring_transport = WindowsSharedRingTransport()
        try:
            return Protocol2Worker(
                session_id=session_id,
                codec_entrypoints=(trusted_codec,),
                deck_entrypoints=(),
                cartridge_access_factory=access_factory,
                ring_transport=ring_transport,
            )
        except Exception:
            ring_transport.close()
            ring_transport = None
            raise

    # The hello contract carries one bounded identifier, not a package
    # coordinate. Exact pack/adapter versions are negotiated by the following
    # codec.descriptor exchange, so keep this process identity valid for every
    # semver accepted by TrustedCodecEntrypoint.
    worker_identity = f"{trusted_codec.pack_id}.worker"
    try:
        return run_protocol2_service(
            sys.stdin.buffer,
            worker_factory=worker_factory,
            worker_identity=worker_identity,
        )
    finally:
        if ring_transport is not None:
            ring_transport.close()


def _reject_duplicate_options(parser: argparse.ArgumentParser, arguments: Sequence[str]) -> None:
    seen: set[str] = set()
    for token in arguments:
        if not token.startswith("--"):
            continue
        option = token.split("=", maxsplit=1)[0]
        if option in seen:
            parser.error(f"option {option} may be supplied only once")
        seen.add(option)


if __name__ == "__main__":
    raise SystemExit(main())
