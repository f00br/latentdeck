# Develop a latent operator

An operator is the smallest place to test latent-domain behavior. Begin in the
Comfy Toolkit when the question is mathematical or visual; package a realtime
Deck only after the behavior, controls, topology, and streaming semantics are
clear.

## Select the operator path

### Comfy Toolkit node or graph

Use existing Toolkit nodes when an experiment can be expressed as an explicit
graph. This gives you LC validation, compatibility checks, device transfer,
scopes, benchmarks, determinism tests, streaming comparison, resampling, and a
research report without designing a package.

The [Toolkit workflow guide](../../comfy/toolkit/workflows/README.md) includes
dual-source, quad-source, projection/resample, explicit align/crop, and operator
development graphs.

### Single-file research operator

Copy [`MyLatentOperator.py`](../../comfy/toolkit/templates/MyLatentOperator.py)
into a new ComfyUI custom-node directory. Follow its [developer
template](../../comfy/toolkit/docs/OPERATOR_DEVELOPER_TEMPLATE.md) to choose
`single_source`, `dual_source`, or `carrier_donors`; declare controls,
compatibility, bypass, determinism, and streaming capabilities; then implement
the callable.

Copying the file is an explicit decision to trust executable Python. A
cartridge never discovers or installs it.

### Packaged research operator

Use the [Channel Roll example](../../operators/examples/channel-roll/README.md)
when the operator needs an independent wheel, descriptor resource, tests, and
license metadata. The Toolkit's trusted registry receives the callable from
installed code and verifies its exported identity; it never imports an
entrypoint supplied by cartridge data.

### Realtime Deck operator

Use the [Deck SDK](../../sdk/deck-python/README.md) and [Deck authoring
guide](DECKS.md) when the operator must run in LatentDeck with source pickers,
roles, transport, a faceplate, output, capture, and presets. This is a different
context/result/descriptor contract from the Toolkit API.

## Define the experiment before the function

Record:

- the implemented 0.1 baseline used for comparison;
- the open question;
- source topology and logical roles;
- exact compatibility assumptions;
- identity/bypass state;
- deterministic seed behavior;
- controls and their bounded ranges;
- full-clip, chunked, or streaming expectations;
- observables and failure conditions.

The [research directions](../research/RESEARCH_DIRECTIONS.md) provide candidate
questions without prescribing an answer.

## Preserve the tensor boundary

Operator inputs are finite latent tensors already admitted by the host. An
operator must:

- treat sources and previous-source history as read-only;
- preserve the negotiated output shape, dtype, device, contiguity, and finite
  values;
- use the supplied deterministic context seed;
- keep role assignment separate from physical source order and playhead
  history;
- return bounded data-only provenance;
- fail explicitly instead of casting, resizing, cropping, padding, aligning,
  moving devices, dropping donors, changing an algorithm, or decoding.

Visible authoring nodes may deliberately crop or align and then write a new
cartridge with provenance. That is not permission for a realtime operator to
repair incompatible inputs.

## Test the behavior

At minimum, test:

- exact bypass identity;
- determinism for equal inputs/controls/context;
- a changed result for controls that claim to affect output;
- every source role and supported topology;
- finite, contiguous, shape/dtype/device-preserving output;
- invalid controls, NaN/Inf, wrong tensor ABI, and excessive resource bounds;
- full-clip versus chunk/stream equivalence when claimed;
- reset/loop/generation boundaries for stateful behavior;
- bounded provenance with no paths, tensors, bytes, or arbitrary objects.

Use synthetic tensors for automated tests. Visual evidence can inform an
experiment, but it does not replace contract tests or justify a universal
claim.

## Promote only when the surface is clear

An operator is ready for a Deck when its inputs, roles, controls, compatibility,
bypass, determinism, performance bound, and state/reset behavior can be
declared without hidden context from a research graph. Follow [Research to
Extension](RESEARCH_TO_EXTENSION.md) and then scaffold a `.ld` package.
