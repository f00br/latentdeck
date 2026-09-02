from __future__ import annotations

import subprocess
import sys

import latentdeck_operator_d2


def test_deck_sdk_is_the_only_runtime_api_loaded_by_d2() -> None:
    assert latentdeck_operator_d2.__all__ == [
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
    probe = subprocess.run(
        [
            sys.executable,
            "-c",
            (
                "import sys\n"
                "import latentdeck_operator_d2\n"
                "assert 'latentdeck_codec_host' not in sys.modules\n"
            ),
        ],
        capture_output=True,
        check=False,
        text=True,
    )
    assert probe.returncode == 0, probe.stderr or probe.stdout
