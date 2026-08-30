# Channel Roll external operator example

This package is a small, separately installed example for the LatentDeck
explicit-install Operator API 0.1. It demonstrates a deterministic community
operator without becoming a builtin and without weakening the cartridge
data-only boundary.

The operator blends a structural carrier with a donor whose channels are
rotated by a bounded control plus the deterministic context seed. It preserves
the F16 `[1,24,1,H,W]` slot shape, processes the full grid, and returns bounded
JSON provenance.

Importing the package has no registry side effect. A trusted host must create a
`TrustedOperatorRegistry` and call `install_into(registry)` explicitly. The
registry receives the callable directly and verifies that its exported identity
matches the closed packaged descriptor. It does not dynamically import the
entrypoint string.

This example is executable Python and therefore must never be embedded in or
installed by a `.lc` cartridge. Cartridges remain untrusted data-only media.
Installation means the user or application has separately obtained and trusted
this Python distribution.

Local checks from the repository root:

```powershell
uv run --no-sync pytest operators/examples/channel-roll/tests
uv run --no-sync ruff check operators/examples/channel-roll
```

The tests create finite synthetic tensors in memory. The package contains no
weights, media, workflows, prompts, network access, or generated output.
