"""Trusted LD-Q4 built-in operator package."""

from .contract import (
    MAX_SPATIAL_TOKENS,
    OPERATOR_ID,
    OPERATOR_VERSION,
    Algorithm,
    ArtisticMode,
    DeckSlot,
    InfluenceMode,
    ProcessResult,
    Q4Context,
    Q4ContractError,
    Q4Controls,
    Xs5Routing,
    triangular_influence_weights,
)
from .descriptor import get_descriptor, get_descriptor_schema
from .operator import process_slot

__all__ = [
    "MAX_SPATIAL_TOKENS",
    "OPERATOR_ID",
    "OPERATOR_VERSION",
    "Algorithm",
    "ArtisticMode",
    "DeckSlot",
    "InfluenceMode",
    "ProcessResult",
    "Q4Context",
    "Q4ContractError",
    "Q4Controls",
    "Xs5Routing",
    "get_descriptor",
    "get_descriptor_schema",
    "process_slot",
    "triangular_influence_weights",
]
