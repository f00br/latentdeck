# Channel Roll external operator example

This package is the copyable `MyLatentOperator`-style example for the
LatentDeck explicit-install Operator API 0.1. The complete PyTorch operator is
50 lines in `operator.py`: copy the package, change the ID, descriptor, and
`process_sources`, then explicitly install it into a trusted research host.

The descriptor declares a `dual_source` topology with two ordered inputs,
full-clip/streaming/chunk support, deterministic behavior, controls, and an
exact runtime-owned bypass at `amount = 0`. The operator blends a structural
carrier with a donor whose channels are rotated by a bounded control plus the
deterministic context seed. It preserves the F16 `[1,24,1,H,W]` slot shape,
processes the full grid, and returns bounded JSON provenance.

Importing the package has no registry side effect. A trusted host must create a
`TrustedOperatorRegistry` and call `install_into(registry)` explicitly. The
registry receives the callable directly and verifies that its exported identity
matches the closed packaged descriptor. It does not dynamically import the
entrypoint string.

The primary callable is:

```python
process_sources(
    sources: tuple[torch.Tensor, ...],
    controls: dict[str, object],
    context: OperatorContext,
) -> ToolkitOperatorResult
```

For this example, `sources == (carrier, donor)`. Hosts can call
`loaded.process_dual(...)`; the original `loaded.process_slot(...)` spelling is
retained as a dual-source compatibility wrapper.

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
