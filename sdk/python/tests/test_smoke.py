from latentdeck_cartridge import BINDING_ABI_VERSION, NATIVE_MODULE_NAME, __version__


def test_python_sdk_reserves_a_versioned_native_binding_boundary() -> None:
    assert __version__ == "0.1.0"
    assert BINDING_ABI_VERSION == "0.1"
    assert NATIVE_MODULE_NAME == "latentdeck_cartridge._native"
