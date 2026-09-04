# Security policy

LatentDeck processes large, untrusted media and can also run explicitly
installed extension code. Please report security problems privately so they
can be investigated before public disclosure.

## Supported versions

| Version | Supported |
| --- | --- |
| Latest `0.1.0-preview.*` source and release | Yes |
| Older previews, local development builds, and unmaintained forks | No |

Until a preview asset is published, the current `main` branch is the only
supported source target for coordinated security fixes.

## Report a vulnerability

When the public repository exposes **Security → Report a vulnerability**, use
it to open a private vulnerability report. GitHub Private Vulnerability
Reporting is enabled and verified as part of the public-visibility transition.
Do not include exploit details, credentials, private cartridges, model assets,
or personal data in a public Issue or Discussion.

Include:

- the affected version or source revision;
- the input/package type and trust boundary involved;
- reproduction steps using the smallest safe synthetic input possible;
- expected and observed behavior;
- impact and whether code execution, data exposure, denial of service, or
  package-trust bypass is involved;
- any proposed disclosure timeline.

If that GitHub control is not yet visible, including during private prerelease
review, send a private Discord message to `@wde` containing only a request for
a secure reporting channel. Do not send secrets or exploit material until the
channel is confirmed.

The maintainers will acknowledge a complete report, investigate it, coordinate
a fix and advisory when appropriate, and credit reporters who wish to be
credited. Response time may vary while the project has a single maintainer.

## Trust boundaries

- An `.lc` cartridge is untrusted, data-only media. It must never execute code,
  import modules, install extensions, or resolve network resources.
- Raw latent files and all archive inputs are untrusted. Validation covers
  archive structure, strict schemas, tensor layout, sizes, hashes, finite
  values, compatibility, and bounded memory before GPU allocation.
- `.ld` Decks, `.ldcodec` Codec Packs, and explicitly installed research
  operators may contain executable Python or native code. Exact hashes,
  integrity catalogs, explicit enable actions, retained handles, process
  supervision, and Job Objects are integrity and lifecycle controls—not a
  security sandbox.
- Publisher metadata is self-declared in the preview. It is not an
  authenticated publisher identity.
- External decoder/model assets are selected locally and verified against an
  explicit byte identity. The project does not grant rights to those assets.
- Diagnostic bundles deliberately exclude payloads, paths, credentials, and
  raw exception details, but users should still inspect a bundle before
  sharing it.

Security controls and expected behavior are specified in the [Latent Cartridge
contract](spec/latent-cartridge/README.md), [extension package
contracts](spec/codec-pack/README.md), [Worker Protocol](spec/worker-protocol/README.md),
and [diagnostics guide](docs/repository/DIAGNOSTICS.md).

## Out of scope

The following are not vulnerabilities by themselves:

- Windows warning that the preview installers are not Authenticode-signed;
- failure to run on an undocumented GPU, driver, codec, model, or operating
  system combination;
- visible refusal of an incompatible or malformed cartridge/package;
- malicious behavior by extension code the user deliberately installed and
  enabled, unless it bypassed a documented trust decision or containment
  boundary;
- availability or licensing changes in an external model-asset source.

Do not test against systems, accounts, or data you do not own or have explicit
permission to use.
