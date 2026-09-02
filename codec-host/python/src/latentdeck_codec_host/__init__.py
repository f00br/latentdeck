"""LatentDeck's isolated codec-host package."""

from importlib import import_module
from sys import version_info
from typing import Any

__version__ = "0.1.0"
COMPONENT_NAME = "codec-host"

_LAZY_EXPORTS = {
    "NativeCartridgeAccess": (".native_cartridge", "NativeCartridgeAccess"),
    "NativeCartridgeAccessFactory": (".native_cartridge", "NativeCartridgeAccessFactory"),
    "CartridgeAccessFactory": (".runtime_v2", "CartridgeAccessFactory"),
    "CommandResult": (".runtime_v2", "CommandResult"),
    "ProcessReceipt": (".runtime_v2", "ProcessReceipt"),
    "Protocol2Bootstrap": (".runtime_v2", "Protocol2Bootstrap"),
    "Protocol2Worker": (".runtime_v2", "Protocol2Worker"),
    "SharedRingTransport": (".runtime_v2", "SharedRingTransport"),
    "StreamConnection": (".runtime_v2", "StreamConnection"),
    "TrustedCodecEntrypoint": (".runtime_v2", "TrustedCodecEntrypoint"),
    "TrustedDeckEntrypoint": (".runtime_v2", "TrustedDeckEntrypoint"),
    "WindowsNamedPipeConnector": (".runtime_v2", "WindowsNamedPipeConnector"),
    "WorkerPipeConnector": (".runtime_v2", "WorkerPipeConnector"),
    "WorkerRuntimeError": (".runtime_v2", "WorkerRuntimeError"),
    "read_protocol2_bootstrap": (".runtime_v2", "read_protocol2_bootstrap"),
    "run_protocol2_service": (".runtime_v2", "run_protocol2_service"),
}


def __getattr__(name: str) -> Any:
    """Load Protocol 2 dependencies only when a runtime surface is requested."""

    target = _LAZY_EXPORTS.get(name)
    if target is None:
        raise AttributeError(name)
    module_name, attribute_name = target
    value = getattr(import_module(module_name, __name__), attribute_name)
    globals()[name] = value
    return value


def runtime_descriptor() -> dict[str, str]:
    """Return dependency-free identity data for process smoke checks."""

    return {
        "component": COMPONENT_NAME,
        "package_version": __version__,
        "python": f"{version_info.major}.{version_info.minor}",
    }


__all__ = [
    "COMPONENT_NAME",
    "CartridgeAccessFactory",
    "CommandResult",
    "NativeCartridgeAccess",
    "NativeCartridgeAccessFactory",
    "ProcessReceipt",
    "Protocol2Bootstrap",
    "Protocol2Worker",
    "SharedRingTransport",
    "StreamConnection",
    "TrustedCodecEntrypoint",
    "TrustedDeckEntrypoint",
    "WindowsNamedPipeConnector",
    "WorkerPipeConnector",
    "WorkerRuntimeError",
    "__version__",
    "runtime_descriptor",
    "read_protocol2_bootstrap",
    "run_protocol2_service",
]
