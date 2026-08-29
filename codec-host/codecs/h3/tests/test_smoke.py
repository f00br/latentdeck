import subprocess
import sys

from latentdeck_codec_h3 import (
    CODEC_FAMILY,
    PROFILE_VERSION,
    H3PresentationCadence,
    descriptor,
)


def test_h3_descriptor_is_available_without_importing_torch() -> None:
    isolated = subprocess.run(
        [
            sys.executable,
            "-I",
            "-c",
            (
                "import sys; "
                "assert 'torch' not in sys.modules; "
                "import latentdeck_codec_h3 as h3; "
                "assert 'torch' not in sys.modules; "
                "assert h3.descriptor()['runtime_extra'] == 'cu130'"
            ),
        ],
        check=False,
        capture_output=True,
        text=True,
    )
    assert isolated.returncode == 0, isolated.stderr
    assert descriptor() == {
        "codec_family": CODEC_FAMILY,
        "profile_version": PROFILE_VERSION,
        "runtime_extra": "cu130",
    }
    assert PROFILE_VERSION == "0.1.0"
    assert H3PresentationCadence.__module__ == "latentdeck_codec_h3.presentation"
