# MiniMax H3 codec profile 0.1

Status: normative for H3 Codec Profile <code>0.1.0</code>.

This profile specializes [LC Spec 0.1](../latent-cartridge/README.md) for
MiniMax H3 audiovisual latent cartridges. It defines enough information for an
independent implementation to validate, compare, load, stream, and resample H3
latents without making H3 the definition of <code>.lc</code>.

The key words **MUST**, **MUST NOT**, **REQUIRED**, **SHOULD**, **SHOULD NOT**,
and **MAY** are interpreted as described by
[BCP 14](https://www.rfc-editor.org/info/bcp14) when capitalized.

## 1. Profile identity

An H3 0.1 manifest uses exactly:

| Field                                | Value                                    |
| ------------------------------------ | ---------------------------------------- |
| <code>spec_version</code>            | <code>0.1.0</code>                       |
| <code>codec.family</code>            | <code>minimax_h3</code>                  |
| <code>codec.profile</code>           | <code>h3_av_latent</code>                |
| <code>codec.profile_version</code>   | <code>0.1.0</code>                       |
| <code>timing.contract</code>         | <code>minimax_h3_causal</code>           |
| <code>timing.contract_version</code> | <code>0.1.0</code>                       |
| Payload path                         | <code>payloads/h3.safetensors</code>     |
| Payload media type                   | <code>application/vnd.safetensors</code> |

The profile describes stored latents. It does not bundle or license an H3
Transformer, native HQ VAE, TAEHV/taeh3 weight, or other model asset. Codec
runtime and weight discovery are separate installation concerns.

## 2. Safetensors payload

The payload contains exactly one tensor named <code>video</code> and MAY
contain one tensor named <code>audio</code>. No other tensor name is valid.

The reserved Safetensors <code>**metadata**</code> string map MAY be present.
It is non-authoritative descriptive data, is covered by the payload hash and
header limit, and MUST NOT override the LC manifest or trigger behavior.

A profile validator MUST independently verify:

- bounded and valid Safetensors JSON with duplicate-key rejection;
- exact tensor names and count;
- manifest/header agreement for dtype, shape, and payload;
- checked element and byte counts;
- in-range, non-overlapping, contiguous data offsets that cover the tensor data
  region exactly;
- finite F16 or F32 values in every tensor;
- exact payload byte length and SHA-256.

An imported F32 Safetensors payload is copied byte-for-byte into the cartridge.
The packer MUST NOT rewrite it merely to normalize header formatting.

## 3. Visual tensor

The required visual descriptor and tensor are:

| Property                              | Rule                                                               |
| ------------------------------------- | ------------------------------------------------------------------ |
| <code>stream</code>                   | <code>visual</code>                                                |
| <code>name</code>                     | <code>video</code>                                                 |
| Rank and layout                       | <code>[1, 24, T, H, W]</code>                                      |
| Storage dtype                         | <code>F16</code> or <code>F32</code>                               |
| Runtime dtype                         | exactly <code>F16</code>                                           |
| Batch                                 | exactly <code>1</code>                                             |
| Channels                              | exactly <code>24</code>                                            |
| Temporal axis <code>T</code>          | <code>2..1048576</code>, additionally valid under the cadence rule |
| Latent <code>H</code>, <code>W</code> | positive and bounded by decoded geometry                           |

An F32 import remains F32 in storage and is explicitly cast to F16 only when
the H3 runtime loads it. An F16 import is loaded unchanged. This F32-to-F16
visual runtime cast is the only profile-authorized implicit dtype conversion.

A post-operator resample records the actual post-operator visual state before
decode. The 0.1 operator runtime state is F16, so a resampled H3 visual tensor
MUST use both <code>storage_dtype = F16</code> and
<code>runtime_dtype = F16</code>; it MUST NOT upcast to F32 for presentation.

## 4. Decoded geometry

H3 0.1 has spatial expansion factor 16 on each axis:

    decoded_video.height = H * 16
    decoded_video.width  = W * 16

Both multiplications use checked arithmetic. Each decoded axis MUST be at most
4096, and decoded width times height MUST be at most 16,777,216 pixels. The
manifest values MUST equal the derived values exactly.

The profile does not permit a loader to crop, pad, transpose, resize, or
reinterpret H and W to make a cartridge fit another cartridge or output.

## 5. Full-clip cadence

A self-contained H3 0.1 clip has a valid visual temporal length only when:

    T = 2 + 5n, where n is an integer and n >= 0

Its decoded frame count is:

    decoded_frames = 5 + 17n

Equivalent checked validation:

    (T - 2) mod 5 == 0
    n = (T - 2) / 5
    decoded_frames == 5 + 17 * n

The initial two latent slots establish five decoded frames. Each subsequent
complete block of five latent slots contributes seventeen decoded frames.

Normative full-clip cases:

| Visual <code>T</code> |  <code>n</code> |   Decoded frames |
| --------------------: | --------------: | ---------------: |
|       <code>32</code> |  <code>6</code> | <code>107</code> |
|       <code>72</code> | <code>14</code> | <code>243</code> |

These mappings are not <code>4 * T</code> and MUST NOT be approximated. A
mismatch produces <code>timing_mismatch</code>.

## 6. Streaming cadence and causal state

The streaming contract is distinct from interpreting an arbitrary complete
file with <code>T = 5</code>:

    5 newly processed latent slots -> 17 usable decoded frames

The H3 codec adapter owns priming, causal overlap/trim, and the association
between a five-slot processing block and its seventeen newly usable frames.
Deck, Player, and UI code MUST query the adapter and MUST NOT reproduce cadence
math independently.

Play and Pause retain the current causal state. Loop and Restart MUST reset the
causal decoder state before decoding again from the clip start. Worker recovery
also clears it. H3 Profile 0.1 does not define arbitrary seek or scratch.

A Live Capture request begins and finishes only on boundaries accepted by the
codec adapter. A capture is finalized only if its resulting self-contained
visual tensor satisfies the full-clip rule <code>T = 2 + 5n</code>. The
implementation MUST NOT silently discard slots to force validity.

## 7. Frame rate and duration

H3 Profile 0.1 fixes <code>frame_rate</code> to numerator <code>24</code> and
denominator <code>1</code>.

The exact reduced duration is:

    duration = decoded_frames / 24 seconds

For example, a 107-frame clip records <code>107/24</code>; a 243-frame clip
records <code>81/8</code>. Floating-point duration is not authoritative.

## 8. Optional audio tensor

When audio exists, its descriptor and tensor are:

| Property                           | Rule                                                           |
| ---------------------------------- | -------------------------------------------------------------- |
| <code>stream</code>                | <code>audio</code>                                             |
| <code>name</code>                  | <code>audio</code>                                             |
| Rank and layout                    | <code>[1, 32, 2, T_audio]</code>                               |
| Storage dtype                      | <code>F16</code> or <code>F32</code>                           |
| Runtime dtype                      | exactly equal to storage dtype                                 |
| Batch                              | exactly <code>1</code>                                         |
| Channels                           | exactly <code>32</code>                                        |
| Plane count                        | exactly <code>2</code>                                         |
| Temporal axis <code>T_audio</code> | <code>1..1048576</code> and equal to the cadence-derived value |

For a source H3 AV latent:

    T_audio = floor(decoded_frames * 5 / 3)

All multiplication and division use checked integer arithmetic. Normative
cases:

|   Decoded frames | <code>T_audio</code> |
| ---------------: | -------------------: |
| <code>107</code> |     <code>178</code> |
| <code>243</code> |     <code>405</code> |

LatentDeck 0.1 preserves audio data but does not play or synthesize it. Audio is
not uploaded to the 0.1 operator path. A validator still verifies its shape,
dtype, bytes, hash, and finite values.

### 8.1 Audio policies during authoring and resampling

- A visual-only source uses <code>source_absent</code>.
- A direct AV import uses <code>preserved_source</code> and retains the
  original audio bytes and actual dtype.
- Snapshot MAY use <code>copied_from_carrier_exact</code> only when output
  duration, frame cadence, temporal mapping, and the full carrier cycle match
  exactly.
- Live Capture MAY use <code>copied_from_carrier_exact</code> only when its
  complete output has exactly the source carrier duration and temporal mapping.
- When upstream audio exists but either exact condition fails, the output MUST
  omit the audio tensor and use <code>omitted_timing_mismatch</code> with the
  precise mismatch reason.

No mode may crop, stretch, resample, synthesize, or silently discard audio to
make it fit.

## 9. Synthesis compatibility

H3 cartridges can share one D2 or Q4 synthesis session only when their visual
compatibility keys match exactly:

    codec.family
    codec.profile
    codec.profile_version
    video.runtime_dtype
    video.batch
    video.channels
    video.H
    video.W
    timing.contract
    timing.contract_version
    decoded_video.frame_rate

Because slots have independent playheads, <code>T</code>, decoded frame count,
and duration are deliberately not part of this key. Thus valid
<code>T = 32</code> and <code>T = 72</code> cartridges can synthesize together
when the listed fields match.

F16- and F32-storage visual cartridges are compatible because both explicitly
declare F16 runtime dtype. Audio presence, storage dtype, and policy do not
block visual synthesis in 0.1 because audio is not processed. They do affect
whether a later resample may copy audio.

An implementation MUST NOT make incompatible cartridges appear compatible by
hidden crop, resize, channel selection, dtype conversion beyond the declared
visual cast, frame-rate conversion, or re-encoding. It returns a specific
compatibility error instead.

## 10. Resampling contract

H3 resampling writes the post-operator visual latent state before decode:

    source cartridges
      -> H3 operators
      -> resampled H3 Safetensors payload
      -> H3 decode

It never records decoded RGB as the latent payload.

A Snapshot contains one complete structural-carrier cycle with controls held
fixed. A Live Capture records changing post-operator slots through a bounded
temporary spool and stops on a codec-valid boundary. Both modes:

- write F16 visual storage matching actual runtime state;
- record all parent cartridge UUIDs and archive hashes with their roles;
- append ordered operation records with operator version, controls, and
  deterministic seed, including seed zero;
- select an explicit audio policy under section 8.1;
- write to <code>.partial</code>, fully validate, and atomically rename only on
  success.

Changing output resolution, latent geometry, cadence, or algorithm in order to
meet a performance target is not part of resampling and MUST NOT happen
silently.

## 11. Profile validation

After generic LC validation, the H3 validator applies these checks in order:

1. exact family, profile, profile version, timing contract, payload path, and
   media type;
2. exactly one <code>video</code> and at most one <code>audio</code> tensor,
   with no unknown tensor;
3. storage/runtime dtype combinations and exact layouts;
4. checked Safetensors byte counts and descriptor agreement;
5. full finite-value scan;
6. cadence-valid <code>T</code> and exact decoded frame count;
7. exact 24/1 frame rate and reduced duration;
8. exact spatial expansion and decoded bounds;
9. cadence-derived <code>T_audio</code> when audio exists;
10. consistency between audio policy, audio tensor presence, and required
    source-cartridge fields.

The generic stable errors are used as follows:

| Failure                                                         | Error code                                                    |
| --------------------------------------------------------------- | ------------------------------------------------------------- |
| Family is not implemented                                       | <code>unsupported_codec</code>                                |
| Profile or timing-contract version is not implemented           | <code>unsupported_profile_version</code>                      |
| Missing or extra H3 tensor                                      | <code>tensor_missing</code> or <code>tensor_unexpected</code> |
| Forbidden dtype combination                                     | <code>tensor_dtype_forbidden</code>                           |
| Rank, fixed axes, temporal congruence, or audio length is wrong | <code>tensor_shape_invalid</code>                             |
| Tensor/manifest header mismatch                                 | <code>tensor_descriptor_mismatch</code>                       |
| Non-finite tensor data                                          | <code>tensor_non_finite</code>                                |
| Frame cadence, FPS, or duration mismatch                        | <code>timing_mismatch</code>                                  |
| Spatial expansion mismatch                                      | <code>decoded_geometry_mismatch</code>                        |
| A declared or estimated resource exceeds a limit                | <code>runtime_limit_exceeded</code>                           |

## 12. Codec adapter requirements

The codec adapter, not the LC reader or UI, owns H3-specific runtime behavior:

- report the profile and compatibility key;
- report full-clip and streaming timing;
- load F16 directly or perform the declared F32-to-F16 visual cast;
- retain independent slot playheads;
- process five-slot streaming blocks and expose seventeen usable frames;
- reset causal state on loop, restart, unload, and worker recovery;
- decode through an explicitly installed compatible decoder;
- expose missing or incompatible codec assets as errors;
- produce post-operator latents for Snapshot and Live Capture.

Codec assets are external to the application installer and repository. A Codec
Manager must present their source, license, hash, and compatibility before use.
No manifest field authorizes a silent download.

## 13. Conformance tests and evidence status

The cadence and schema contracts above are normative and have verified
reference cases for:

- full visual <code>T = 32 -> 107 frames</code>;
- full visual <code>T = 72 -> 243 frames</code>;
- a five-slot streaming block producing seventeen usable frames;
- AV mapping <code>107 frames -> T_audio = 178</code>;
- AV mapping <code>243 frames -> T_audio = 405</code>.

These public values are sufficient for deterministic synthetic tests. The
private source latents, their media, paths, hashes, prompts, and model assets
are opt-in E2E evidence and MUST NOT become committed fixtures or hidden
requirements.

A minimum public H3 test matrix generates temporary synthetic tensors and
checks:

- valid F16 visual-only <code>T = 32</code> and F32 AV
  <code>T = 72</code> round trips;
- byte-preserving F32 import and F16 resample;
- 32/72 visual cartridges passing compatibility despite different durations;
- exact streaming five-to-seventeen accounting and state reset;
- invalid temporal congruence, decoded count, FPS, duration, geometry, batch,
  channels, plane count, dtype, audio length, and audio-policy contradiction;
- positive and negative infinity plus quiet and signaling NaN in F16 and F32
  visual and audio values;
- Snapshot audio copy on exact timing and explicit omission on mismatch.

Visual judgement and private GPU playback remain separate acceptance evidence.
They do not replace contract tests or alter the normative formulas.
