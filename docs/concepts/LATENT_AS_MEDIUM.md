# Latent as a medium

> **Project perspective.** This essay explains the artistic premise behind
> LatentDeck. It is not a novelty claim, a scientific survey, or a normative
> specification. Verified product behavior is identified separately in the
> [concept overview](OVERVIEW.md).

Generative images and videos are usually treated as finished when pixels or an
MP4 appear. The internal representation that preceded those pixels is then
discarded as implementation detail. LatentDeck begins with a different working
choice: save that representation and ask what kind of material it can become
after generation.

A latent representation is not a conventional image, and its structure depends
on the model family that produced it. It should not be mystified as hidden
meaning or a complete visual object. It is learned numerical data from which a
compatible decoder can form an image. Some spatial, temporal, channel, and
feature relationships are available there in a form that is no longer the same
after RGB decode.

That difference creates an artistic interval between generation and image.
Instead of asking the generator for another finished result, an artist can
retain the generated state, play it later, combine it with other compatible
states, vary a latent-domain operation in realtime, and save the transformed
state again. Prompting produces source material; performance begins when the
artist takes direct control of that material.

The word *cartridge* names this shift in use. A `.lc` file is a portable,
validated media object with an explicit codec, timing contract, provenance,
and genealogy. It is not executable and does not contain a universal image.
The visible result still depends on a decoder and, during synthesis, on the
Deck, operator, source roles, controls, and performance decisions around it.
One cartridge can therefore participate in more than one performance without
claiming that every decoder or transformation is equivalent.

There is a useful analogy with recorded sound. A recording can be evidence of
an earlier performance, but it can also be looped, cut, layered, filtered, or
resampled into material for another performance. The analogy does not prove
that latent video will develop in the same way. It simply suggests a productive
question: what changes when a saved representation is treated as material
rather than residue?

LatentDeck explores that question before pixels. A linear interpolation is a
necessary baseline, but the domain also permits experiments with feature
correspondence, channel structure, temporal state, statistics, frequency, and
multi-source routing. None of those techniques guarantees a semantically
meaningful new object. Their value must be judged through controllability,
repeatability, failure behavior, performance, and the visual distinctions they
actually produce.

Resampling keeps the inquiry cumulative. A transformed post-operator latent can
be written as a new cartridge, with the original cartridges and operation
recorded as data. The new cartridge can then become an input to another Deck or
offline experiment. Generation is no longer required at every step, and the
performance does not have to terminate only as decoded video.

This approach is called *post-generative* here because creative work continues
after the generation stage. It does not replace generators, MP4, editing,
Resolume, VDMX, TouchDesigner, or other visual systems. LatentDeck can instead
act as a source whose decoded output enters those environments, while preserving
a separate path in which the latent result becomes another cartridge.

The durable ambition is modest in wording but broad in consequence: give
artists and developers a precise, interoperable way to handle the state between
generation and image. Whether that grows into a larger instrumental language
depends on the experiments people can reproduce, critique, extend, and perform.

Continue with the bounded questions in [Research
Directions](../research/RESEARCH_DIRECTIONS.md) or follow the practical
[research-to-extension workflow](../developers/RESEARCH_TO_EXTENSION.md).
