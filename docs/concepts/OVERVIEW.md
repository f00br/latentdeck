# Concept overview

LatentDeck separates generative creation from realtime performance. A
generation system can produce a latent representation once; the representation
can then be saved as a cartridge, transported, played, processed before decode,
and recorded into another cartridge.

This documentation uses three labels deliberately:

- **Implemented in 0.1** describes behavior available in the current code and
  contracts.
- **Design principle** describes a boundary that guides the architecture.
- **Research direction** describes a question worth testing, not an announced
  feature or scientific conclusion.

## The playable object

**Implemented in 0.1.** A Latent Cartridge (`.lc`) is a canonical,
codec-neutral, data-only archive. It contains one validated Safetensors payload,
a strict manifest, and an optional preview. The manifest binds codec/profile,
tensor layout, decoded geometry, timing, audio disposition, provenance, parent
cartridges, and operation history.

A cartridge is media, not a plugin. Opening it cannot import Python, install an
operator, select a remote resource, or execute embedded code. The
[LC specification](../../spec/latent-cartridge/README.md) is the normative
definition.

**Design principle.** H3 is the first implemented profile, but the cartridge is
not an H3 container by definition. Codec-specific geometry and timing live in a
separate profile so a future latent family can use `.lc` without weakening the
stable format or pretending incompatible tensors are interchangeable.

## Before pixels

Conventional video performance tools receive decoded image frames. LatentDeck
places an additional performance stage earlier in the path:

```text
generation → saved latent → latent operation → decoder → image
```

**Implemented in 0.1.** LatentPlayer decodes one cartridge. LatentDeck runs one
or more cartridges through an installable Deck's operator before decode. D2
provides a two-source topology with a linear baseline and XS1–XS5 operations.
Q4 provides a carrier-plus-three-donors topology with linear and XS5 modes.
These are reference instruments, not a claim that one algorithm is the single
correct form of latent synthesis.

**Design principle.** A Deck is part of the transformation. Source topology,
role assignment, operator math, temporal behavior, and control mapping can all
change the result. That is why a Deck is a versioned extension rather than one
ever-growing application panel.

LatentDeck complements RGB video tools rather than replacing them. Decoded
output can be displayed directly, shared through Spout2, or recorded as MP4 and
then used by an existing visual-performance system.

## Performance instead of prompting

**Implemented in 0.1.** Normal playback and Deck performance do not run the
original generative model. Cartridges already exist; the selected Codec Pack
reads them, the operator processes their latent slots, and the decoder reveals
the output. Controls change the current stream rather than starting a new file
render.

This does not make every latent inexpensive to process or portable to every
GPU. Cost and compatibility depend on the codec, decoder, tensor geometry,
algorithm, hardware, driver, and runtime. A performance statement is valid only
for the exact measured configuration recorded by its receipt.

## Resampling and genealogy

**Implemented in 0.1.** Snapshot and Live Capture write the post-operator
latent state before decode. The finished cartridge records its source parents,
operator identity/version, normalized controls, seed, and explicit audio
disposition. It is validated and can be used as new material:

```text
A.lc + B.lc → Deck/operator → C.lc → another performance
```

This makes transformation history inspectable without making it executable.
Genealogy records describe what produced a cartridge; they do not authorize a
host to fetch or run the named operator.

## The ecosystem

**Implemented in 0.1.** The project has separable roles:

- LatentPlayer plays cartridges and can prepare raw latent data through a
  selected codec capability.
- LatentDeck manages cartridges and runs realtime Decks.
- The ComfyUI recorder writes a generation's latent output as `.lc`.
- The Comfy Toolkit is an offline laboratory for inspection, explicit
  alignment, operators, decoder comparison, measurement, and resampling.
- Cartridge, Deck, and Codec SDKs expose typed authoring surfaces.
- `.ld` Deck and `.ldcodec` Codec packages use exact-hash installation,
  compatibility resolution, and explicit enablement.

**Design principle.** Cartridges and the realtime signal contract form the
stable center. User interfaces, Decks, codecs, workers, and output adapters can
evolve independently around them.

## Deliberate 0.1 limits

LatentDeck 0.1 does not provide audio playback or synthesis, prompting,
generation, a production timeline, projection mapping, or hidden media repair.
It supports at most four warm Deck sessions and one foreground output lease.
Direct synthesis requires exact signal compatibility; a visible authoring step
must create a new cartridge when crop or alignment is intentional.

The next practical step is the [artist workflow](../guides/ARTIST_WORKFLOW.md)
or the [developer hub](../developers/README.md). Questions beyond the current
contracts belong in [research directions](../research/RESEARCH_DIRECTIONS.md).
