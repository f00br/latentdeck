# Research directions

This page offers starting points for reproducible latent-media experiments. It
is not a roadmap and does not promise that any candidate technique will become
a product feature.

Each card follows the same discipline:

`0.1 baseline → open question → candidate experiment → observables → possible extension surface`

Use synthetic tensors while developing contracts. When visual media is
necessary, keep it outside the source repository unless its provenance,
license, size, and publication safety have been approved.

## A + B → C: correspondence transport

**0.1 baseline:** D2 provides linear mixing and bounded cosine-affinity
transport; Q4 applies one carrier to three donors in a fixed accumulation
order.

**Open question:** Which correspondence rules produce a controllable state that
is distinguishable from interpolation without collapsing the carrier or
copying the donor?

**Candidate experiment:** Compare linear, TOPK, Sinkhorn, mutual-nearest,
region-constrained, or temporally regularized transport on the same compatible
source pair. Sweep one declared parameter at a time and preserve the seed.

**Observables:** bypass identity, determinism, finite output, channel/statistical
drift, temporal discontinuity, execution time, memory delta, and blinded visual
comparison at matched controls.

**Possible extension surface:** Toolkit operator first; a realtime Deck mode
only after bounded streaming behavior and a clear control model are proven.

## Temporal feedback and history

**0.1 baseline:** The Deck SDK supplies the previous latent slot for each
physical source and keeps that history attached when logical roles change. The
Toolkit Feedback and Temporal Labs provide bounded offline operations.

**Open question:** Can short, explicit latent history create repeatable motion
memory without unbounded recursion or stale state across restart/loop?

**Candidate experiment:** Test one-step feedback, delayed residuals, temporal
frequency separation, and decay envelopes with generation/reset boundaries
made explicit.

**Observables:** exact reset behavior, long-run finite values, latency, drift,
loop-boundary continuity, memory bound, and deterministic replay.

**Possible extension surface:** research operator, then a Deck whose state and
reset semantics are part of its package contract.

## Channel, statistics, and frequency structure

**0.1 baseline:** XS1–XS4 and the Toolkit expose channel rotation, spatial
grafting, frequency exchange, and bounded statistics transfer.

**Open question:** Which low-dimensional controls correspond to stable visual
changes across more than one cartridge while preserving the exact tensor ABI?

**Candidate experiment:** Measure individual channels, channel groups,
orthogonal transforms, spatial frequency bands, and normalized statistics
against a linear baseline.

**Observables:** identity at bypass, invertibility where claimed, energy
distribution, clipping/non-finite behavior, temporal stability, and perceptual
repeatability across a small declared corpus.

**Possible extension surface:** Toolkit node or installable operator; promote
only controls whose meaning survives outside one test clip.

## Multi-source topology

**0.1 baseline:** D2 uses carrier/donor roles. Q4 uses one carrier and three
donors with manual or triangular weights. Physical playheads remain independent
from logical role assignments.

**Open question:** What new behavior comes from topology rather than simply
adding more weighted layers?

**Candidate experiment:** Compare chains, rings, pairwise graphs, staged
carrier selection, winner-take-most routing, and sparse donor activation while
keeping source order and provenance explicit.

**Observables:** permutation sensitivity, role clarity, accumulation-order
effects, control dimensionality, compatibility failures, and realtime cost as
source count increases.

**Possible extension surface:** a new `.ld` Deck with a topology-specific
faceplate, not additional hidden controls in D2 or Q4.

## Decoder sensitivity and projection

**0.1 baseline:** The Toolkit can compare explicitly selected FAST and HQ H3
decoders and can perform a visible native decode→encode projection experiment.
The realtime product does not repair a latent through projection.

**Open question:** Which operator outputs remain useful across compatible
decoder choices, and when does explicit manifold projection improve or erase
the intended transformation?

**Candidate experiment:** Decode the same raw and projected latent through the
same declared decoder pair, preserving exact asset hashes and parameters.

**Observables:** MAE/RMSE/PSNR where applicable, finite data, latent drift,
visual continuity, encode/decode cost, and whether a claimed improvement is
decoder-specific.

**Possible extension surface:** offline Toolkit workflow and research report;
not a hidden realtime fallback.

## Explicit cross-codec mapping

**0.1 baseline:** `.lc` is codec-neutral, while direct playback and synthesis
require an exact supported profile. No cross-codec conversion is implied.

**Open question:** Can a declared transformation map between two codec
representations with enough fidelity and provenance to create a new valid
cartridge?

**Candidate experiment:** Define one source profile, one target profile, a
bounded mapping, and an independent target-profile validator. Treat the result
as a new authoring operation rather than compatibility.

**Observables:** target conformance, reconstruction comparison, information
loss, determinism, resource cost, failure boundaries, and complete genealogy.

**Possible extension surface:** external conversion tool or Toolkit node. A
Codec Pack should not silently map an unsupported source.

## Resampling genealogy

**0.1 baseline:** New cartridges can record ordered parents and operation
history, including operator identity/version, controls, and seed.

**Open question:** How can multi-generation work remain understandable without
turning descriptive provenance into executable workflow code?

**Candidate experiment:** Generate a chain and a branch of derived cartridges,
then build a data-only visualizer that resolves only explicitly supplied local
parents by UUID and archive hash.

**Observables:** missing-parent behavior, identity collisions, ordering,
bounded depth, privacy of source metadata, and reproducibility when an operator
implementation is unavailable.

**Possible extension surface:** cartridge inspection tool or Library view; no
network fetch or operator installation from cartridge metadata.

## Future audiovisual control

**0.1 baseline:** LC 0.1 may preserve an H3 audio latent and explicit audio
disposition, but LatentDeck does not play, synthesize, or route audio.

**Open question:** Which timing and control contracts would let audio influence
visual latent processing without claiming that unlike latent representations
are directly interchangeable?

**Candidate experiment:** Begin with decoded or measured control features that
are explicitly time-aligned to visual slots. Keep audio playback, control
extraction, and latent transformation as separate declared stages.

**Observables:** clock ownership, drift, latency, reset/loop behavior,
determinism, audio preservation policy, and failure when alignment is invalid.

**Possible extension surface:** offline research node first. Any realtime AV
Deck requires a versioned signal/timing contract beyond 0.1.

## Turning a result into a contribution

A useful result includes the baseline, exact versions, test inputs or synthetic
generator, parameters, failures, measurements, and a distinction between what
was observed and what was inferred. Follow [Research to
Extension](../developers/RESEARCH_TO_EXTENSION.md) before proposing a core or
package change.
