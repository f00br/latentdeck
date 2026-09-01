# LatentDeck H3 Codec Pack installer notices

These notices cover the native lifecycle helper and installer runtime shipped
by `LatentDeck-H3-CodecPack-<version>-setup.exe`. The H3 runtime payload has its
own dependency inventory, SBOM, and notices inside the adjacent Codec Pack ZIP.

## Nullsoft Scriptable Install System (NSIS)

- Source: <https://nsis.sourceforge.io/>
- Version: `3.11`
- Pinned build-tool archive SHA-256:
  `c7d27f780ddb6cffb4730138cd1591e841f4b7edb155856901cdf5f214394fa1`
- Licenses: see the exact pinned upstream `INSTALLER_NSIS_COPYING.txt` delivered
  beside setup and embedded in the version-scoped maintenance directory.

NSIS supplies the setup and generated uninstaller runtime. The build invokes
the same separately authorized signing command for both executables when a
signed publication candidate is requested.

## Native Rust lifecycle helper

The helper is original LatentDeck Apache-2.0 code. It is embedded in setup and
in the generated uninstaller; it is never added to the integrity-closed Codec
Pack directory. Its exact release dependency closure is recorded in
`installer-SBOM.cdx.json`. Complete dependency license texts are delivered in
`INSTALLER_RUST_LICENSES.txt`.
