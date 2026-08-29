"""Lightweight ComfyUI recorder for Latent Cartridge files."""

from .nodes import NODE_CLASS_MAPPINGS, NODE_DISPLAY_NAME_MAPPINGS, SaveLatentCartridge

__version__ = "0.1.0"

__all__ = [
    "NODE_CLASS_MAPPINGS",
    "NODE_DISPLAY_NAME_MAPPINGS",
    "SaveLatentCartridge",
    "__version__",
]
