# Local release engineering

These runbooks define the local Windows `0.1.0` application release-candidate
path and the independently versioned extension lifecycle. They do not
authorize publication, Git tags, remote changes, signing, or uploads.

- [Current release-preparation handoff](continue.md)
- [Master-user test guide](MASTER_USER_TEST.md)
- [Owner acceptance and open publication gates](ACCEPTANCE_STATUS.md)
- [Windows application release candidate](WINDOWS_LOCAL_RC.md)
- [H3 Codec Pack packaging and lifecycle](H3_CODEC_PACK.md)
- [Isolated ComfyUI user-test environment](ISOLATED_COMFY_TEST_ENVIRONMENT.md)

Application installers and Codec Packs are independent artifacts with
independent install, update, and removal lifecycles. The public Windows H3
artifact set contains a small
`LatentDeck-H3-CodecPack-<version>-setup.exe` and its exact required adjacent
`LatentDeck-H3-CodecPack-<version>-windows-x64.ldcodec`. The user runs setup; it is
offline, current-user only, fixed-path, registered for exact-version removal in
Windows Installed Apps, and requires no elevation, PowerShell, system Python,
decoder, or model. Setup and maintenance receipts remain outside the
integrity-closed installed pack directory.

Every application RC set also carries the validated CycloneDX 1.5 SBOM at
`metadata/latentdeck-0.1.0-sbom.cdx.json`; its length and SHA-256 are bound by
the RC receipt and `SHA256SUMS.txt`. The SBOM explicitly inventories the
separately prepared upstream Spout2 native source, and the same RC metadata
directory carries the hash-bound `THIRD_PARTY_NOTICES.md` with its
BSD-2-Clause terms. Receiver executables are local QA tools and are not release
artifacts.

Local unsigned setup proof does not satisfy publisher trust or clean-machine
acceptance. Authenticated signing and the offline clean Windows lifecycle remain
release gates and do not authorize publication by themselves.

The owner accepted the completed Protocol 2 modular runtime and final local
functional pass on 2026-09-03 at implementation commit `3648e7c`. Clean `main`
documentation checkpoint `0fd1303` then produced the unsigned first-install UAT
sets, with exact identities retained in their generated receipts and
`SHA256SUMS.txt`. Fresh-install UAT remains open. This post-build documentation
update makes those artifacts an older UAT snapshot, so rebuild from the final
accepted commit before publication review.
