# Synthetic Codec source

This data-free CPU example implements the complete Codec SDK surface for a
small synthetic latent profile. It demonstrates descriptor/profile binding,
retained cartridge access, source lifecycle, slot reads, RGBA decode, decoder
reset, capture finish, capture abort, and the in-process Worker Protocol 2
command path.
It is a teaching and conformance adapter, not a media codec.

The tests keep those boundaries explicit. One direct SDK test exercises CPU
capture finish, no-clobber, and abort. A second test calls the Protocol 2 worker
command surface directly for descriptor/profile binding, snapshot capture,
decode, reset/replay, and a Live Capture finalization fault. A third test copies
the current CPython executable into a temporary package runtime, launches that
copy, performs the authenticated service bootstrap over an in-memory framed
stream, and exercises decode, snapshot capture, reset/replay, and shutdown abort
through `run_protocol2_service`. This proves the service/bootstrap contract; it
does not claim Windows named-pipe or installed-process-supervisor coverage.

Protocol 2 has no standalone `capture.abort` command. A failed `capture.stop`
and `session.shutdown` are the normative paths that invoke the adapter writer's
`abort()` boundary. The tests observe both while proving that an existing output
is not overwritten.

The repository deliberately does not contain an embedded Python runtime. To
turn the source into `.ldcodec`, supply an isolated CPython 3.13 runtime at
`runtime/python.exe` together with its adjacent `runtime/python313.dll`, install
the Codec Host, Codec SDK, matching Torch build, and this `adapter.py` in that
runtime, then replace `runtime/runtime.lock` with the exact lock and update its
SHA-256 in `codec-pack.json`.

Run the CPU-only contract test without packaging a runtime:

```powershell
uv run --no-sync pytest codec-host/python/tests/test_public_synthetic_codec.py
```

After supplying the runtime, build and inspect without editing or cataloguing
the source tree manually:

```powershell
cargo run -p latentdeck-extension-manager -- build --source examples/extensions/synthetic-codec --output synthetic-codec.ldcodec
cargo run -p latentdeck-extension-manager -- inspect --archive synthetic-codec.ldcodec
```

`build` will fail closed while the declared runtime executable is absent or
when the runtime lock hash is stale. It never copies a local environment into
the repository for you.
