from latentdeck_codec_host import COMPONENT_NAME, __version__, runtime_descriptor
from latentdeck_codec_host.__main__ import main


def test_runtime_descriptor_identifies_python_313_codec_host() -> None:
    descriptor = runtime_descriptor()

    assert descriptor == {
        "component": COMPONENT_NAME,
        "package_version": __version__,
        "python": "3.13",
    }


def test_version_command_is_a_minimal_process_smoke_target(capsys) -> None:
    assert main(["--version"]) == 0
    assert capsys.readouterr().out.strip() == "latentdeck-codec-host 0.1.0"
