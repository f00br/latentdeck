from __future__ import annotations

class CartridgeError(Exception):
    code: str
    detail: str
    entry: str | None
    tensor: str | None
    json_pointer: str | None

    def __init__(
        self,
        code: str,
        detail: str,
        entry: str | None = None,
        tensor: str | None = None,
        json_pointer: str | None = None,
    ) -> None: ...

BINDING_ABI_VERSION: str

def inspect_json(path: str) -> str: ...
def validate_json(path: str) -> str: ...
def hash_json(path: str) -> str: ...
def pack_json(
    manifest_json: str,
    payload_path: str,
    output_path: str,
    preview_path: str | None = None,
    overwrite: bool = False,
) -> str: ...
def pack_raw_h3_json(
    payload_path: str,
    output_path: str,
    preview_path: str | None = None,
    cartridge_id: str | None = None,
    provenance_json: str | None = None,
    overwrite: bool = False,
) -> str: ...
