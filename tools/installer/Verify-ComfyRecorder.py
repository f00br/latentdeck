"""Probe a target interpreter or verify one extracted Comfy Recorder install."""

from __future__ import annotations

import argparse
import importlib
import importlib.util
import json
import platform
import re
import struct
import sys
import tempfile
import types
from pathlib import Path


def _probe() -> dict[str, object]:
    return {
        "implementation": platform.python_implementation(),
        "major": sys.version_info.major,
        "minor": sys.version_info.minor,
        "pointer_bits": struct.calcsize("P") * 8,
        "platform": sys.platform,
        "machine": platform.machine(),
    }


def _within(path: Path, root: Path) -> bool:
    try:
        path.resolve(strict=True).relative_to(root.resolve(strict=True))
    except ValueError:
        return False
    return True


def _verify(vendor: Path) -> dict[str, object]:
    vendor = vendor.resolve(strict=True)
    sys.path.insert(0, str(vendor))

    private_safetensors = importlib.import_module(
        "latentdeck_recorder_vendor.safetensors"
    )
    cartridge = importlib.import_module("latentdeck_cartridge")
    recorder = importlib.import_module("latentdeck_comfy_cartridge")
    native = importlib.import_module("latentdeck_cartridge._native")
    safetensors_native = importlib.import_module(
        "latentdeck_recorder_vendor.safetensors._safetensors_rust"
    )

    expected = {
        "safetensors": "0.8.0",
        "latentdeck-cartridge": "0.1.0",
        "latentdeck-comfy-cartridge": "0.1.0",
    }
    actual = {
        "safetensors": getattr(private_safetensors, "__version__", None),
        "latentdeck-cartridge": getattr(cartridge, "__version__", None),
        "latentdeck-comfy-cartridge": getattr(recorder, "__version__", None),
    }
    if actual != expected:
        raise RuntimeError(f"installed package versions do not match the bundle: {actual!r}")
    mapping = getattr(recorder, "NODE_CLASS_MAPPINGS", None)
    if not isinstance(mapping, dict) or "LatentDeckSaveLatentCartridge" not in mapping:
        raise RuntimeError("Recorder node registration is unavailable")

    native_path = Path(native.__file__ or "")
    safetensors_native_path = Path(safetensors_native.__file__ or "")
    torch_source_path = (
        vendor / "latentdeck_recorder_vendor" / "safetensors" / "torch.py"
    )
    if native_path.name != "_native.pyd" or not _within(native_path, vendor):
        raise RuntimeError("Cartridge SDK did not load its bundled abi3 native module")
    if not safetensors_native_path.name.endswith(".pyd") or not _within(
        safetensors_native_path, vendor
    ):
        raise RuntimeError("Safetensors did not load its bundled Windows native module")
    torch_source = torch_source_path.read_text(encoding="utf-8")
    if re.search(r"(?m)^(?:from|import) safetensors(?:[ .]|$)", torch_source):
        raise RuntimeError("Safetensors Torch adapter retains a global package import")
    if "from . import (" not in torch_source:
        raise RuntimeError("Safetensors Torch adapter was not relocated with relative imports")

    result = _probe()
    result.update(
        {
            "packages": actual,
            "cartridge_native": native_path.name,
            "safetensors_native": safetensors_native_path.name,
            "safetensors_import": "latentdeck_recorder_vendor.safetensors",
        }
    )
    return result


def _load_shim(shim: Path, module_name: str) -> object:
    shim = shim.resolve(strict=True)
    spec = importlib.util.spec_from_file_location(
        module_name,
        shim,
        submodule_search_locations=[str(shim.parent)],
    )
    if spec is None or spec.loader is None:
        raise RuntimeError("could not load the Recorder discovery shim")
    module = importlib.util.module_from_spec(spec)
    sys.modules[module_name] = module
    spec.loader.exec_module(module)
    return module


def _verify_host_shim(shim: Path) -> dict[str, object]:
    with tempfile.TemporaryDirectory(prefix="latentdeck-safetensors-host-") as temporary:
        host_root = Path(temporary)
        package_root = host_root / "safetensors"
        package_root.mkdir()
        (package_root / "__init__.py").write_text(
            "__version__ = 'host-sentinel'\n", encoding="utf-8"
        )
        (package_root / "torch.py").write_text(
            "def save_file(*args, **kwargs):\n    return ('host-sentinel', args, kwargs)\n",
            encoding="utf-8",
        )
        sys.path.insert(0, str(host_root))
        host = importlib.import_module("safetensors")
        host_torch = importlib.import_module("safetensors.torch")
        host_path = Path(host.__file__ or "").resolve(strict=True)
        _load_shim(shim, "_latentdeck_recorder_host_test")
        recorder = importlib.import_module("latentdeck_comfy_cartridge.recorder")
        selected_save_file = recorder._resolve_safetensors_save_file()
        if sys.modules.get("safetensors") is not host:
            raise RuntimeError("Recorder shim replaced the host Safetensors module object")
        if sys.modules.get("safetensors.torch") is not host_torch:
            raise RuntimeError("Recorder shim replaced the host Safetensors Torch module object")
        if getattr(host, "__version__", None) != "host-sentinel" or not _within(
            host_path, host_root
        ):
            raise RuntimeError("Recorder shim changed the host Safetensors version or path")
        if selected_save_file is not host_torch.save_file:
            raise RuntimeError("Recorder did not preserve the host Safetensors writer")
        return {
            "safetensors": "host-sentinel",
            "path_preserved": True,
            "writer_preserved": True,
        }


def _verify_bundled_shim(shim: Path) -> dict[str, object]:
    if importlib.util.find_spec("safetensors") is not None:
        raise RuntimeError("bundled fallback test requires an interpreter without Safetensors")
    _load_shim(shim, "_latentdeck_recorder_bundled_test")
    if importlib.util.find_spec("safetensors") is not None or "safetensors" in sys.modules:
        raise RuntimeError("Recorder exposed its private Safetensors copy as a global package")
    safetensors = importlib.import_module("latentdeck_recorder_vendor.safetensors")
    native = importlib.import_module(
        "latentdeck_recorder_vendor.safetensors._safetensors_rust"
    )
    recorder = importlib.import_module("latentdeck_comfy_cartridge.recorder")
    private_torch = types.ModuleType("latentdeck_recorder_vendor.safetensors.torch")

    def private_save_file(*args: object, **kwargs: object) -> tuple[object, ...]:
        return (args, kwargs)

    private_torch.save_file = private_save_file
    sys.modules[private_torch.__name__] = private_torch
    selected_save_file = recorder._resolve_safetensors_save_file()
    vendor = shim.resolve(strict=True).parent / "vendor"
    package_path = Path(safetensors.__file__ or "")
    native_path = Path(native.__file__ or "")
    if getattr(safetensors, "__version__", None) != "0.8.0":
        raise RuntimeError("Recorder shim did not select bundled Safetensors 0.8.0")
    if not _within(package_path, vendor) or not _within(native_path, vendor):
        raise RuntimeError("Recorder shim loaded Safetensors outside its private vendor directory")
    if selected_save_file is not private_save_file:
        raise RuntimeError("Recorder did not select its uniquely namespaced Safetensors writer")
    if importlib.util.find_spec("safetensors") is not None or "safetensors" in sys.modules:
        raise RuntimeError("Recorder registered its private Safetensors copy globally")
    return {
        "safetensors": "0.8.0",
        "namespace": "latentdeck_recorder_vendor.safetensors",
        "package": package_path.name,
        "native": native_path.name,
    }


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--probe", action="store_true")
    parser.add_argument("--vendor", type=Path)
    parser.add_argument("--shim-host", type=Path)
    parser.add_argument("--shim-bundled", type=Path)
    args = parser.parse_args()
    choices = [
        args.probe,
        args.vendor is not None,
        args.shim_host is not None,
        args.shim_bundled is not None,
    ]
    if sum(bool(choice) for choice in choices) != 1:
        parser.error("choose exactly one verification mode")
    if args.probe:
        result = _probe()
    elif args.vendor is not None:
        result = _verify(args.vendor)
    elif args.shim_host is not None:
        result = _verify_host_shim(args.shim_host)
    else:
        result = _verify_bundled_shim(args.shim_bundled)
    print(json.dumps(result, sort_keys=True, separators=(",", ":")))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
