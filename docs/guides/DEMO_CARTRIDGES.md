# LatentDeck Demo LC Pack

The demo pack is a separately hosted set of eight data-only H3 cartridges for
first playback, D2/Q4 experiments, and genealogy inspection. It contains seven
source imports and one D2 live-capture resample. It is not part of the source
repository, the GitHub release, the H3 Codec Pack, or the decoder asset.

## Pinned distribution

- Friendly page: [LatentDeck Demo LC Pack](https://huggingface.co/datasets/f00br/latentdeck-demo-lc-pack)
- Reviewed revision: [`67402c05f9155fa3af7d2d89a1bd0477a358f05f`](https://huggingface.co/datasets/f00br/latentdeck-demo-lc-pack/tree/67402c05f9155fa3af7d2d89a1bd0477a358f05f)
- Published checksums: [`SHA256SUMS.txt`](https://huggingface.co/datasets/f00br/latentdeck-demo-lc-pack/blob/67402c05f9155fa3af7d2d89a1bd0477a358f05f/SHA256SUMS.txt)

Use the pinned revision for a reproducible first-run test. A later change to
the friendly page does not change the files reviewed here.

| File | Bytes | SHA-256 |
| --- | ---: | --- |
| `abyssal-light-city.lc` | 41,572,941 | `d24888be9b1827f50b5ac4f261c5c93a8ac400d43b989ecb53bf664005886cf8` |
| `anim-hud.lc` | 41,572,941 | `895498aa2e38dbe40dc58b01773fedf906015047d20041773e8557c9691a45a5` |
| `d2-live-capture-resample.lc` | 29,419,923 | `29a4c36c1117da07778d44be5e050595c1cbcbb0a3d19172ea3bc226bc95a082` |
| `dust.lc` | 41,572,941 | `6d37619c11d094661121d40eb5e665fd3e91cc4ed6247b6bc3b05610849e239f` |
| `knight.lc` | 41,572,941 | `c6e0fec6dea237ed3eba9bc8d413afccf16998b58e6e516316f9ef7f4405c105` |
| `landscape.lc` | 41,572,941 | `78f59bf510861724d0308d2646b710621c3416969df38ac9db5817210c3d3f62` |
| `liquid-chrome-bloom.lc` | 41,572,941 | `d45414330b47469580d2bc6e1db93f0bc5245404343346c22d281851188b6f3f` |
| `neon-silk-vortex.lc` | 41,572,941 | `f926006634b5635cef70b20ccd2613741629ef649b878e13481b249b543f5d77` |

The revision and file listing are public and ungated. Verify the downloaded
files locally rather than trusting a filename or a shortened digest.

For this review, all eight complete archives were checked against the public
pinned revision, matched the published hashes, and passed the authoritative
Rust validator at `validation_level=full`, including the finite-value and H3
profile checks.

## Technical identity

Each reviewed archive contains only `manifest.json` and
`payloads/h3.safetensors`. All eight declare:

- LC specification `0.1.0`;
- codec profile `minimax_h3 / h3_av_latent / 0.1.0`;
- timing contract `minimax_h3_causal / 0.1.0`;
- decoded geometry `1344 × 768` at 24 fps;
- the same 0.1 synthesis compatibility key.

The seven source imports use F32 visual storage with F16 runtime shape
`[1, 24, 107, 48, 84]`, decode to 362 frames, and preserve F32 audio shape
`[1, 32, 2, 603]`. The derived resample uses F16 visual shape
`[1, 24, 152, 48, 84]`, decodes to 515 frames, and records
`omitted_timing_mismatch` because its captured duration and mapping do not
match either parent audio stream.

The resample manifest identifies exact `knight.lc` donor and `landscape.lc`
carrier cartridge IDs and hashes, bundled D2 operator `0.2.0`, XS5 controls,
and capture disposition. All eight can share a D2 or Q4 session; this is a
contract-level compatibility statement, not a performance claim. Seven
distinct source imports provide enough material for Q4 without reusing a file.

The cartridges contain no decoder or model weight. Install and enable exact H3
Codec Pack `0.2.1`, select its declared TAEH3 decoder file, and select CUDA
before playback. LatentDeck 0.1 preserves the audio latent but does not play,
synthesize, mix, or export audio.

## Verify and open

Compare each result from:

```powershell
Get-FileHash -Algorithm SHA256 .\*.lc
```

Opening a cartridge in LatentPlayer performs full validation before runtime
allocation. Developers can run the same authoritative validator directly from
a source checkout:

```powershell
cargo run -p latentdeck-cartridge -- validate .\anim-hud.lc
cargo run -p latentdeck-cartridge -- validate .\d2-live-capture-resample.lc
```

Then follow the [Windows quick start](../../README.md#quick-start-play-the-demo-pack)
or the complete [artist workflow](ARTIST_WORKFLOW.md).

## Media terms and provenance status

The eight `.lc` cartridge media files and their dataset documentation are
licensed under the [Creative Commons Attribution 4.0 International
license](https://creativecommons.org/licenses/by/4.0/) (CC BY 4.0).

When sharing or adapting this material, credit **f00br** and **LatentDeck Demo
LC Pack**, link both the [dataset](https://huggingface.co/datasets/f00br/latentdeck-demo-lc-pack)
and the license, and indicate whether changes were made. The CC BY 4.0 terms
apply only to media and documentation in that Hugging Face dataset. LatentDeck
source code and documentation use the repository license; no decoder, model
weight, or third-party software is included or relicensed by the demo pack.
