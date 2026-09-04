# Implemented application screenshots

These PNG files are original LatentDeck project screenshots captured by project
owner `@f00br` on 2026-08-30 from the running Windows Tauri applications at
version `0.1.0`. Their origin is the project applications themselves; they
contain no third-party artwork or user media.

Capture conditions were deliberately public-safe:

- isolated, empty local application-data root;
- isolated codec root with no Codec Pack or model asset installed;
- no browser command mocks;
- no `.lc` cartridge, raw latent, preview, weight, prompt, workflow, or user
  media.

The D2 and Q4 images document the supported missing-codec state as well as the
implemented faceplates. The Library image documents virtual `All Cartridges`
and `Unassigned` banks. The Player image documents its empty playback and Spout
surface.

## File provenance and disposition

| File | SHA-256 | Intended use |
| --- | --- | --- |
| [`latentdeck-d2-missing-codec.png`](latentdeck-d2-missing-codec.png) | `c3487f5d0086d70cdb4e0c02177387eb3e95fa33071d8f3827c26d83f0c55862` | Public documentation of the implemented D2 faceplate and missing-codec state. |
| [`latentdeck-library-empty.png`](latentdeck-library-empty.png) | `220d44d21768b29abd671aa73b97bd6922d5a25d5bc7cfec2081babab59e69d6` | Public README reference for the empty Library and virtual collection banks. |
| [`latentdeck-q4-missing-codec.png`](latentdeck-q4-missing-codec.png) | `2b94081e45e1a67e1d667395656c221c82ec87b570ca7b549ed64f41b05e86c9` | Public documentation of the implemented Q4 faceplate and missing-codec state. |
| [`latentplayer-empty.png`](latentplayer-empty.png) | `fb8f1b0d58ad030fb94da954f0ece142223c14c65a4faff5925f23624b2f0d1f` | Public documentation of the empty Player playback and Spout surface. |

Author and rights holder: `@f00br`. These screenshots are original project
documentation redistributed under the repository's Apache-2.0 license. They
may be replaced when a later implemented UI state is a more useful release
reference, but any replacement must update this provenance record and its
hash.
