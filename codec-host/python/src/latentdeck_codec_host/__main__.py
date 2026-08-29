"""Minimal command-line entry point for codec-host process checks."""

from __future__ import annotations

import argparse
from collections.abc import Sequence

from . import __version__


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(prog="latentdeck-codec-host")
    parser.add_argument("--version", action="store_true", help="print the package version")
    arguments = parser.parse_args(argv)

    if arguments.version:
        print(f"latentdeck-codec-host {__version__}")
        return 0

    parser.print_help()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
