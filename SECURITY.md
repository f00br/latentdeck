# Security

LatentDeck has no supported public release yet. The owner has accepted the local
0.1 application surface and Protocol 2 modular runtime, but clean-machine,
signing, security-contact, and publication gates remain open. Every external
cartridge, raw latent, Codec Pack, Deck package, decoder asset, and explicitly
installed research operator must be treated according to its own trust
boundary.

The 0.1 loader treats every `.lc` cartridge as untrusted, data-only media. It
rejects malformed manifests and payloads before runtime use, validates tensor
schema, dtype, shapes, sizes, hashes, codec compatibility, finite values, and
memory limits, and never executes cartridge-supplied code. `.ld` Deck and
`.ldcodec` Codec packages may contain trusted Python or native runtime code;
they require a local exact-hash installation and explicit enable action.
Publisher metadata is self-declared until an authenticated distribution path
exists. Extension processes run with the user's permissions; process isolation
and Job Objects are containment mechanisms, not a security sandbox. Research
operators are a separate explicit-install trust boundary.

Private vulnerability reporting is not configured because no public GitHub
repository exists yet. Enabling a private reporting channel is a mandatory
release gate. Until then, do not publish suspected secrets or vulnerabilities
in public artifacts.
