"""Authoritative Deck SDK implementation of the bundled LD-D2 Deck."""

from .contract import (
    DECK_ID,
    DECK_VERSION,
    MAX_SPATIAL_TOKENS,
    OPERATOR_ID,
    OPERATOR_VERSION,
    Algorithm,
    ArtisticMode,
    D2ContractError,
    D2Controls,
    Routing,
    Xs5Routing,
)
from .operator import process_sources

__all__ = [
    "DECK_ID",
    "DECK_VERSION",
    "MAX_SPATIAL_TOKENS",
    "OPERATOR_ID",
    "OPERATOR_VERSION",
    "Algorithm",
    "ArtisticMode",
    "D2ContractError",
    "D2Controls",
    "Routing",
    "Xs5Routing",
    "process_sources",
]
