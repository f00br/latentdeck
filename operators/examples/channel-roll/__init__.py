"""ComfyUI discovery shim for the copyable Channel Roll example."""

from __future__ import annotations

import sys
from importlib import import_module
from pathlib import Path

_SOURCE = str(Path(__file__).resolve().parent / "src")
_ADDED_SOURCE = _SOURCE not in sys.path
if _ADDED_SOURCE:
    sys.path.insert(0, _SOURCE)
try:
    _package = import_module("latentdeck_example_channel_roll")
finally:
    if _ADDED_SOURCE:
        sys.path.remove(_SOURCE)

NODE_CLASS_MAPPINGS = _package.NODE_CLASS_MAPPINGS
NODE_DISPLAY_NAME_MAPPINGS = _package.NODE_DISPLAY_NAME_MAPPINGS

__all__ = ["NODE_CLASS_MAPPINGS", "NODE_DISPLAY_NAME_MAPPINGS"]
