"""Packaged descriptor for the channel-roll example operator."""

from __future__ import annotations

import json
from importlib.resources import files
from typing import Any


def get_descriptor() -> dict[str, Any]:
    """Return an independent descriptor object for explicit installation."""

    resource = files(__package__).joinpath("descriptor.json")
    value = json.loads(resource.read_text(encoding="utf-8"))
    if not isinstance(value, dict):
        raise RuntimeError("packaged operator descriptor is not an object")
    return value


__all__ = ["get_descriptor"]
