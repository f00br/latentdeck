"""Separately installed LatentDeck community operator example."""

from latentdeck_comfy_toolkit import TrustedOperatorRegistry

from .comfy_node import NODE_CLASS_MAPPINGS, NODE_DISPLAY_NAME_MAPPINGS
from .descriptor import get_descriptor
from .operator import OPERATOR_ID, OPERATOR_VERSION, process_sources


def install_into(registry: TrustedOperatorRegistry) -> None:
    """Perform the explicit host-side install; importing this package is inert."""

    registry.install(
        get_descriptor(),
        process_sources,
        exported_entrypoint="latentdeck_example_channel_roll:process_sources",
    )


__all__ = [
    "NODE_CLASS_MAPPINGS",
    "NODE_DISPLAY_NAME_MAPPINGS",
    "OPERATOR_ID",
    "OPERATOR_VERSION",
    "get_descriptor",
    "install_into",
    "process_sources",
]
