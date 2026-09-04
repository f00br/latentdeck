"""Minimal realtime Deck operator that preserves the selected latent."""

from __future__ import annotations

from latentdeck_deck_sdk import DeckContractError, DeckOperatorResult


def process_sources_host(sources, controls, context):
    """Return a distinct contiguous tensor and bounded data-only provenance."""

    del context
    if controls != {"mode": "identity"}:
        raise DeckContractError("control.mode", "mode must be identity")
    return DeckOperatorResult(
        output=sources[0].clone().contiguous(),
        provenance={
            "operator_id": "org.example.latentdeck.identity",
            "operator_version": "0.1.0",
        },
    )
