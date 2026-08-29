"""Native-only Python surface for the worker-side RGB Ring ABI 1 producer."""

from ._native import BINDING_ABI_VERSION, RingError, WindowsRgbRingProducer

__version__ = "0.1.0"

__all__ = [
    "BINDING_ABI_VERSION",
    "RingError",
    "WindowsRgbRingProducer",
    "__version__",
]
