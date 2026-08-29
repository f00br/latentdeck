"""LatentDeck's isolated codec-host package."""

from sys import version_info

from .operator_api import (
    BuiltinOperatorRegistry,
    LoadedOperator,
    OperatorDescriptor,
    OperatorLoadError,
    validate_descriptor,
)

__version__ = "0.1.0"
COMPONENT_NAME = "codec-host"


def runtime_descriptor() -> dict[str, str]:
    """Return dependency-free identity data for process smoke checks."""

    return {
        "component": COMPONENT_NAME,
        "package_version": __version__,
        "python": f"{version_info.major}.{version_info.minor}",
    }


__all__ = [
    "COMPONENT_NAME",
    "BuiltinOperatorRegistry",
    "LoadedOperator",
    "OperatorDescriptor",
    "OperatorLoadError",
    "__version__",
    "runtime_descriptor",
    "validate_descriptor",
]
