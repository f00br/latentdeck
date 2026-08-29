"""MiniMax H3 adapter package for LatentDeck."""

__version__ = "0.1.0"
CODEC_FAMILY = "minimax_h3"
PROFILE_VERSION = "0.1.0"


def descriptor() -> dict[str, str]:
    """Describe the adapter without importing the optional ML runtime."""

    return {
        "codec_family": CODEC_FAMILY,
        "profile_version": PROFILE_VERSION,
        "runtime_extra": "cu130",
    }


__all__ = ["CODEC_FAMILY", "PROFILE_VERSION", "__version__", "descriptor"]
