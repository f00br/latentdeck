from __future__ import annotations

import subprocess
import sys

import latentdeck_operator_q4


def test_deck_sdk_is_the_only_runtime_api_loaded_by_q4() -> None:
    assert latentdeck_operator_q4.__all__ == [
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
    probe = subprocess.run(
        [
            sys.executable,
            "-c",
            (
                "import sys\n"
                "import latentdeck_operator_q4\n"
                "assert 'latentdeck_codec_host' not in sys.modules\n"
            ),
        ],
        capture_output=True,
        check=False,
        text=True,
    )
    assert probe.returncode == 0, probe.stderr or probe.stdout
