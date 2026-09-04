# From research question to extension

This workflow helps a developer—or a developer working with a coding agent—turn
an idea into a result that other people can reproduce and review. It avoids
starting with a large product feature before the behavior is understood.

## 1. State the question

Begin with one sentence that can fail. Record:

- the implemented 0.1 baseline;
- the open question;
- source topology and exact compatibility assumptions;
- what will be changed;
- what will be observed;
- what result would make the direction unpromising.

Use a card from [Research Directions](../research/RESEARCH_DIRECTIONS.md) or
write one in the same format. Do not begin with a claim that a method creates a
new semantic object; begin with observable behavior.

## 2. Prototype in the Comfy Toolkit

Build the smallest explicit graph or single-file operator. Use synthetic
tensors for contract development, then a small declared external corpus for
visual evaluation when necessary.

Keep every important choice visible: device transfer, crop/alignment,
projection, decoder asset, seed, controls, topology, and report destination.
Do not repair an incompatibility invisibly.

## 3. Characterize the behavior

Establish:

- exact bypass/identity behavior;
- accepted and rejected tensor/profile shapes;
- deterministic seed and replay behavior;
- finite-value and resource bounds;
- full-clip, chunk, or streaming semantics;
- reset, loop, and causal-history behavior;
- bounded provenance fields;
- failure codes and cleanup/no-clobber behavior.

Use the Toolkit's scopes, Benchmark, Determinism Test, Streaming Compatibility
Test, and One-click Research Report. Label direct measurements as verified,
interpretations as inferred, and untested next steps as proposed.

## 4. Compare against a baseline

Run the same sources and declared context through the simplest meaningful
baseline—often identity or linear interpolation. Change one factor at a time.
Record exact package/operator/codec/decoder versions and hardware when timing or
memory is reported.

A visual difference is an observation, not automatically an improvement.
Evaluate control range, repeatability, temporal stability, failure behavior,
and cost as well as selected frames.

## 5. Choose the smallest durable surface

- Keep a **Toolkit graph** when the work is exploratory, full-tensor, or needs
  visible authoring stages.
- Create an **explicit-install operator** when the math is reusable but does not
  need a standalone realtime instrument.
- Create a **Deck** when source topology, roles, realtime controls, transport,
  faceplate, capture, and performance state form one coherent instrument.
- Create a **Codec Pack** only when supporting a latent family/profile and its
  decode/capture runtime. An algorithm that transforms compatible tensors is
  normally an operator, not a codec.
- Create a **cartridge tool** when the result is an explicit offline media
  transformation or genealogy/inspection workflow.

## 6. Package and validate

Use the public schema and scaffold/build tooling. Add unit tests, package parser
tests, deterministic rebuild checks, lifecycle tests, compatible/incompatible
matrix cases, and documentation for a fresh reader.

External Deck/Codec IDs use the author's reverse-DNS namespace. Keep weights,
cartridges, raw latents, private media, and generated reports outside the source
package. Record every redistributed dependency and license.

## 7. Validate in the real host

After CPU contract tests pass, install the exact built archive disabled, verify
it, enable it, inspect the matrix, and run it through the same generic host used
by first-party packages.

For a Deck, test source/role permutations, controls, transport, restart/loop,
Snapshot, Live Capture, output, presets, sessions, and explicit failure states.
For a Codec, test load/open/decode/reset/capture/abort/replay and runtime crash
containment. Keep hardware and visual evidence tied to the exact package hashes.

## 8. Contribute or publish independently

Open a focused pull request when the change belongs in the main project. State
the question, evidence, contract impact, package identity, tests, and
limitations. Follow [CONTRIBUTING.md](../../CONTRIBUTING.md).

An extension can also remain in its own repository and depend on LatentDeck's
public contracts. Independent distribution is a normal outcome, not a failed
contribution. Users still make an explicit trust decision before installation
and enablement.
