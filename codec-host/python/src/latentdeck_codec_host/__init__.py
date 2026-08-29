"""LatentDeck's isolated codec-host package."""

from sys import version_info

__version__ = "0.1.0"
COMPONENT_NAME = "codec-host"


def runtime_descriptor() -> dict[str, str]:
    """Return dependency-free identity data for process smoke checks."""

    return {
        "component": COMPONENT_NAME,
        "package_version": __version__,
        "python": f"{version_info.major}.{version_info.minor}",
    }


__all__ = ["COMPONENT_NAME", "__version__", "runtime_descriptor"]
