"""Authoritative Deck SDK implementation of the bundled LD-Q4 Deck."""

from .contract import (
    DECK_ID,
    DECK_VERSION,
    MAX_SPATIAL_TOKENS,
    OPERATOR_ID,
    OPERATOR_VERSION,
    Algorithm,
    ArtisticMode,
    DeckSlot,
    InfluenceMode,
    Q4ContractError,
    Q4Controls,
    Xs5Routing,
    triangular_influence_weights,
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
    "DeckSlot",
    "InfluenceMode",
    "Q4ContractError",
    "Q4Controls",
    "Xs5Routing",
    "process_sources",
    "triangular_influence_weights",
]
