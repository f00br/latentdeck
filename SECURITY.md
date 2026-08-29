# Security

LatentDeck has no supported public release yet. Early workspace targets must not
be used with untrusted files until the cartridge validation gates are complete.

The 0.1 contract treats every `.lc` cartridge as untrusted, data-only media. The
loader must reject malformed manifests and payloads before runtime use,
validate tensor schema, dtype, shapes, sizes, hashes, codec compatibility, and
memory limits, and must never execute cartridge-supplied code.

Private vulnerability reporting is not configured because no public GitHub
repository exists yet. Enabling a private reporting channel is a mandatory
release gate. Until then, do not publish suspected secrets or vulnerabilities
in public artifacts.
