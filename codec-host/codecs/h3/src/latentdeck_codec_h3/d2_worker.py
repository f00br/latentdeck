"""Isolated H3 LD-D2 process entry point over Worker Protocol 1."""

from __future__ import annotations

import sys
from collections.abc import Callable
from typing import BinaryIO

from .d2_worker_state import H3D2WorkerState
from .worker import Connector, WorkerSessionState, run_worker


def run_d2_worker(
    stdin: BinaryIO,
    *,
    connector: Connector | None = None,
    state_factory: Callable[[], WorkerSessionState] = H3D2WorkerState,
) -> int:
    """Run one authenticated D2 session on the supervisor-owned pipe."""

    return run_worker(stdin, connector=connector, state_factory=state_factory)


def main() -> int:
    return run_d2_worker(sys.stdin.buffer)


if __name__ == "__main__":
    raise SystemExit(main())


__all__ = ["run_d2_worker"]
