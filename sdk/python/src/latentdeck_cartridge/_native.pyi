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
INTEGRITY_HANDLE_ABI_VERSION: str

class ValidatedCartridgeHandle:
    def manifest_json(self) -> str: ...
    def validation_json(self) -> str: ...
    def integrity_access_receipt_json(self) -> str: ...
    def tensor_names(self) -> list[str]: ...
    def tensor_descriptors_json(self) -> str: ...
    def read_tensor(self, name: str, max_tensor_bytes: int | None = None) -> bytes: ...
    def read_tensor_range(
        self,
        name: str,
        offset: int,
        byte_length: int,
        max_read_bytes: int | None = None,
    ) -> bytes: ...

def open_integrity_handle(path: str) -> ValidatedCartridgeHandle: ...
def open_integrity_handle_from_raw(
    raw_handle: int, integrity_access_receipt: str
) -> ValidatedCartridgeHandle: ...
def inspect_json(path: str) -> str: ...
def validate_json(path: str) -> str: ...
def hash_json(path: str) -> str: ...
def inspect_raw_h3_json(path: str) -> str: ...
def read_h3(
    path: str,
    max_visual_values: int | None = None,
    max_tensor_bytes: int | None = None,
) -> tuple[str, bytes, bytes | None]: ...
def read_raw_h3(
    path: str,
    max_visual_values: int | None = None,
    max_tensor_bytes: int | None = None,
) -> tuple[str, bytes, bytes | None]: ...
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
