from __future__ import annotations

import importlib.util
import sys
from pathlib import Path

import pytest
import torch

from latentdeck_deck_sdk import (
    DeckContractError,
    DeckOperatorContext,
    RoleBinding,
    process_sources_checked,
)

REPOSITORY_ROOT = Path(__file__).resolve().parents[3]
OPERATOR_PATH = (
    REPOSITORY_ROOT
    / "examples/extensions/starter-deck/python/latentdeck_example_identity_deck/operator.py"
)


def _load_operator():
    module_name = "latentdeck_public_starter_deck"
    specification = importlib.util.spec_from_file_location(module_name, OPERATOR_PATH)
    assert specification is not None and specification.loader is not None
    module = importlib.util.module_from_spec(specification)
    sys.modules[module_name] = module
    previous = sys.dont_write_bytecode
    sys.dont_write_bytecode = True
    try:
        specification.loader.exec_module(module)
    finally:
        sys.dont_write_bytecode = previous
    return module.process_sources_host


def _context() -> DeckOperatorContext:
    return DeckOperatorContext(
        codec_family="synthetic",
        profile="example_latent",
        profile_version="0.1.0",
        timing_contract="synthetic_ticks",
        timing_contract_version="0.1.0",
        frame_rate_numerator=24,
        frame_rate_denominator=1,
        generation=1,
        sequence=1,
        seed=17,
        playheads=(0,),
        physical_slots=(1,),
        roles=(RoleBinding("source", 1),),
        previous_sources=(None,),
    )


def test_public_starter_deck_runs_as_a_cpu_identity_operator() -> None:
    source = torch.arange(24, dtype=torch.float32).reshape(1, 4, 1, 2, 3)
    result = process_sources_checked(
        _load_operator(),
        (source,),
        {"mode": "identity"},
        _context(),
        torch_module=torch,
    )

    assert torch.equal(result.output, source)
    assert result.output.data_ptr() != source.data_ptr()
    assert result.output.is_contiguous()
    assert result.provenance == {
        "operator_id": "org.example.latentdeck.identity",
        "operator_version": "0.1.0",
    }


def test_public_starter_deck_rejects_an_undeclared_mode() -> None:
    source = torch.zeros((1, 4, 1, 2, 3), dtype=torch.float32)
    with pytest.raises(DeckContractError) as failure:
        process_sources_checked(
            _load_operator(),
            (source,),
            {"mode": "other"},
            _context(),
            torch_module=torch,
        )
    assert failure.value.code == "control.mode"
