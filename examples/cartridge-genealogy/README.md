# Runtime-generated `A.lc -> B.lc` genealogy

This CPU-only example starts from a caller-owned visual-only H3 cartridge,
applies an explicit tensor transformation, and writes a new cartridge with a
new UUID, exact parent archive hash, operator identity/version, controls, seed,
and `source_absent` audio disposition. Cartridge I/O uses the supported Toolkit
package-root functions `load_lc`, `parent_cartridge_ref`, and
`save_resampled_lc`, plus the authoritative Rust-backed Cartridge SDK. The
example imports `annotate_operation` from
`latentdeck_comfy_toolkit.workflow_metadata` only to attach the operation
record; that helper is not part of the supported root-level 0.1 LC I/O surface
(`import_raw_h3` is the fourth supported root export).

The function refuses AV input rather than silently dropping audio and never
overwrites an existing output. After writing, it fully validates `B.lc` and
checks that the resulting manifest binds the intended parent and new identity.

```python
from transform import transform_cartridge


def invert_visual(samples, controls, seed):
    del seed
    return (samples * -float(controls["amount"])).contiguous()


receipt = transform_cartridge(
    "A.lc",
    "B.lc",
    operator=invert_visual,
    operator_id="org.example.invert",
    operator_version="0.1.0",
    controls={"amount": 1.0},
    seed=17,
)
print(receipt["archive_sha256"])
```

Run the repository-owned, runtime-generated round trip:

```powershell
uv run --no-sync pytest comfy/toolkit/tests/test_public_genealogy_example.py
```

The test creates both the raw Safetensors input and `.lc` files under a
temporary directory. No latent payload or generated cartridge is stored in
Git.
