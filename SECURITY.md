# Security

LatentDeck has no supported public release yet. The owner has accepted the local
0.1 functional surface, but clean-machine, signing, security-contact, and
publication gates remain open. The candidate includes the cartridge validation
boundary, but every external cartridge, raw latent, Codec Pack, decoder asset,
and explicitly installed operator must still be treated according to its own
trust boundary.

The 0.1 loader treats every `.lc` cartridge as untrusted, data-only media. It
rejects malformed manifests and payloads before runtime use, validates tensor
schema, dtype, shapes, sizes, hashes, codec compatibility, finite values, and
memory limits, and never executes cartridge-supplied code. External operators
are trusted Python code and require a separate explicit installation.

Private vulnerability reporting is not configured because no public GitHub
repository exists yet. Enabling a private reporting channel is a mandatory
release gate. Until then, do not publish suspected secrets or vulnerabilities
in public artifacts.
