# LatentDeck Demo LC Pack

The demo pack is a separately hosted set of three data-only H3 cartridges for
first playback and D2/Q4 experiments. It is not part of the source repository,
the GitHub release, the H3 Codec Pack, or the decoder asset.

## Pinned distribution

- Friendly page: [LatentDeck Demo LC Pack](https://huggingface.co/datasets/f00br/latentdeck-demo-lc-pack)
- Reviewed revision: [`0e7b98f7152607c2d1709a896f9173859886ad79`](https://huggingface.co/datasets/f00br/latentdeck-demo-lc-pack/tree/0e7b98f7152607c2d1709a896f9173859886ad79)
- Published checksums: [`SHA256SUMS.txt`](https://huggingface.co/datasets/f00br/latentdeck-demo-lc-pack/blob/0e7b98f7152607c2d1709a896f9173859886ad79/SHA256SUMS.txt)

Use the pinned revision for a reproducible first-run test. A later change to
the friendly page does not change the files reviewed here.

| File | Bytes | SHA-256 |
| --- | ---: | --- |
| `anim-hud.lc` | 41,572,941 | `895498aa2e38dbe40dc58b01773fedf906015047d20041773e8557c9691a45a5` |
| `knight.lc` | 41,572,941 | `c6e0fec6dea237ed3eba9bc8d413afccf16998b58e6e516316f9ef7f4405c105` |
| `landscape.lc` | 41,572,941 | `78f59bf510861724d0308d2646b710621c3416969df38ac9db5817210c3d3f62` |

The revision and file listing are public and ungated. Verify the downloaded
files locally rather than trusting a filename or a shortened digest.

For this review, all three complete archives were downloaded anonymously from
the pinned revision, matched the published hashes, and passed the authoritative
Rust validator at `validation_level=full`, including the finite-value and H3
profile checks.

## Technical identity

Each reviewed archive contains only `manifest.json` and
`payloads/h3.safetensors` and declares:

- LC specification `0.1.0`;
- codec profile `minimax_h3 / h3_av_latent / 0.1.0`;
- timing contract `minimax_h3_causal / 0.1.0`;
- F32 visual storage with F16 runtime shape `[1, 24, 107, 48, 84]`;
- decoded video `1344 × 768`, 362 frames at 24 fps;
- F32 audio shape `[1, 32, 2, 603]` with `preserved_source` disposition.

All three manifests have the same 0.1 synthesis compatibility key, so they can
share a D2 or Q4 session. This is a contract-level compatibility statement,
not a performance claim. Q4 requires four physical slots; reusing one of the
three cartridges is valid but does not demonstrate four-source diversity.

The cartridges contain no decoder or model weight. Install and enable exact H3
Codec Pack `0.2.1`, select its declared TAEH3 decoder file, and select CUDA
before playback. LatentDeck 0.1 preserves the audio latent but does not play,
synthesize, mix, or export audio.

## Verify and open

Compare each result from:

```powershell
Get-FileHash -Algorithm SHA256 .\anim-hud.lc
Get-FileHash -Algorithm SHA256 .\knight.lc
Get-FileHash -Algorithm SHA256 .\landscape.lc
```

Opening a cartridge in LatentPlayer performs full validation before runtime
allocation. Developers can run the same authoritative validator directly from
a source checkout:

```powershell
cargo run -p latentdeck-cartridge -- validate .\anim-hud.lc
cargo run -p latentdeck-cartridge -- validate .\knight.lc
cargo run -p latentdeck-cartridge -- validate .\landscape.lc
```

Then follow the [Windows quick start](../../README.md#quick-start-play-the-demo-pack)
or the complete [artist workflow](ARTIST_WORKFLOW.md).

## Media terms and provenance status

The three `.lc` cartridge media files and their dataset documentation are
licensed under the [Creative Commons Attribution 4.0 International
license](https://creativecommons.org/licenses/by/4.0/) (CC BY 4.0).

When sharing or adapting this material, credit **f00br** and **LatentDeck Demo
LC Pack**, link both the [dataset](https://huggingface.co/datasets/f00br/latentdeck-demo-lc-pack)
and the license, and indicate whether changes were made. The CC BY 4.0 terms
apply only to media and documentation in that Hugging Face dataset. LatentDeck
source code and documentation use the repository license; no decoder, model
weight, or third-party software is included or relicensed by the demo pack.
