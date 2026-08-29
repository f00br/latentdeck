"""Isolated H3 LD-Q4 process entry point over Worker Protocol 1."""

from __future__ import annotations

import sys
from collections.abc import Callable
from typing import BinaryIO

from .q4_worker_state import H3Q4WorkerState
from .worker import Connector, WorkerSessionState, run_worker


def run_q4_worker(
    stdin: BinaryIO,
    *,
    connector: Connector | None = None,
    state_factory: Callable[[], WorkerSessionState] = H3Q4WorkerState,
) -> int:
    """Run one authenticated Q4 session on the supervisor-owned pipe."""

    return run_worker(stdin, connector=connector, state_factory=state_factory)


def main() -> int:
    return run_q4_worker(sys.stdin.buffer)


if __name__ == "__main__":
    raise SystemExit(main())


__all__ = ["run_q4_worker"]
