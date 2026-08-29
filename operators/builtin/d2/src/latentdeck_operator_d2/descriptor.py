"""Machine-readable descriptor access without mutable shared state."""

from __future__ import annotations

import json
from copy import deepcopy
from importlib.resources import files
from typing import Any


def _read_json(name: str) -> dict[str, Any]:
    resource = files(__package__).joinpath(name)
    payload = json.loads(resource.read_text(encoding="utf-8"))
    if not isinstance(payload, dict):
        raise RuntimeError(f"{name} must contain a JSON object")
    return payload


_DESCRIPTOR = _read_json("descriptor.json")
_SCHEMA = _read_json("descriptor.schema.json")


def get_descriptor() -> dict[str, Any]:
    """Return a defensive copy of the trusted operator descriptor."""

    return deepcopy(_DESCRIPTOR)


def get_descriptor_schema() -> dict[str, Any]:
    """Return a defensive copy of the descriptor JSON Schema."""

    return deepcopy(_SCHEMA)
