"""MiniMax H3 adapter package for LatentDeck."""

from .cartridge import CartridgeLoadError, H3Cycle, H3VideoSource, load_video_source
from .presentation import H3CadenceError, H3PresentationCadence, StreamingDecoder

__version__ = "0.2.0"
CODEC_FAMILY = "minimax_h3"
PROFILE_VERSION = "0.1.0"


def descriptor() -> dict[str, str]:
    """Describe the adapter without importing the optional ML runtime."""

    return {
        "codec_family": CODEC_FAMILY,
        "profile_version": PROFILE_VERSION,
        "runtime_extra": "cu130",
    }


__all__ = [
    "CODEC_FAMILY",
    "CartridgeLoadError",
    "H3Cycle",
    "PROFILE_VERSION",
    "H3CadenceError",
    "H3PresentationCadence",
    "H3VideoSource",
    "StreamingDecoder",
    "__version__",
    "descriptor",
    "load_video_source",
]
