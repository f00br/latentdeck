from __future__ import annotations

import json
import subprocess
import sys


def test_importing_sdk_does_not_import_torch() -> None:
    script = (
        "import json,sys; import latentdeck_deck_sdk; print(json.dumps('torch' in sys.modules))"
    )
    completed = subprocess.run(
        [sys.executable, "-c", script],
        check=True,
        capture_output=True,
        text=True,
        timeout=30,
    )
    assert json.loads(completed.stdout) is False
