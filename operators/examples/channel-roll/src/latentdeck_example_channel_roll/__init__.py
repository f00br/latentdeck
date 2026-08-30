"""Separately installed LatentDeck community operator example."""

from latentdeck_comfy_toolkit import TrustedOperatorRegistry

from .descriptor import get_descriptor
from .operator import OPERATOR_ID, OPERATOR_VERSION, process_slot


def install_into(registry: TrustedOperatorRegistry) -> None:
    """Perform the explicit host-side install; importing this package is inert."""

    registry.install(
        get_descriptor(),
        process_slot,
        exported_entrypoint="latentdeck_example_channel_roll:process_slot",
    )


__all__ = [
    "OPERATOR_ID",
    "OPERATOR_VERSION",
    "get_descriptor",
    "install_into",
    "process_slot",
]
