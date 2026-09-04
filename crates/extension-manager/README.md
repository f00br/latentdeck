# LatentDeck Extension Manager

This crate implements the shared, hash-bound lifecycle for `.ld` Deck and
`.ldcodec` Codec packages. It is both a Rust library used by the applications
and a structured-JSON command-line tool for extension authors.

## Authoring commands

Create a new no-clobber source directory with a publisher-owned reverse-DNS
identity:

```powershell
cargo run -p latentdeck-extension-manager -- scaffold --kind deck --id org.example.my-deck --version 0.1.0 --output my-deck
cargo run -p latentdeck-extension-manager -- scaffold --kind codec --id org.example.my-codec --version 0.1.0 --output my-codec
```

A Deck scaffold is immediately buildable. A Codec scaffold deliberately does
not copy or invent an isolated Python runtime: add the exact declared
`runtime/python.exe`, Codec Host, SDK, adapter dependencies, licenses, and
runtime lock before building.

Build from author-friendly source without editing it:

```powershell
cargo run -p latentdeck-extension-manager -- build --source my-deck --output my-deck.ld
cargo run -p latentdeck-extension-manager -- inspect --archive my-deck.ld
```

`build` inventories and bounds the complete source before copying, rejects
reparse points, repository metadata, and credential-like paths, and detects
additions, removals, replacements, or byte changes before publication. In an
isolated staging tree it evaluates the embedded public JSON Schemas, applies
the normative Rust parsers and Deck cross-file checks, generates a sorted
`integrity.json`, binds the catalog SHA-256 into a temporary manifest, and
delegates to the existing deterministic `pack` and post-pack inspection path.
Its JSON receipt includes the exact sorted archive-path catalog for review.
Both `scaffold` and `build` refuse existing destinations.

The lower-level `pack --source <catalogued-tree> --output <archive>` command is
retained for build systems that already own a complete `integrity.json` and
bound manifest.

## Lifecycle commands

`inspect`, `install`, `verify`, `enable`, `disable`, `repair`, `remove`,
`list`, and `matrix` expose the same validation and immutable exact-version
lifecycle used by LatentDeck. All successful commands write one JSON value to
standard output; failures use stable error codes and exit classes.

See the normative [Deck package](../../spec/deck-package/README.md) and
[Codec package](../../spec/codec-pack/README.md) specifications before
distributing an extension.
