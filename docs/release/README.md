# Local release engineering

These runbooks define the local Windows `0.1.0` release-candidate path. They do
not authorize publication, Git tags, remote changes, signing, or uploads.

- [Current UAT and release handoff](continue.md)
- [Master-user test guide](MASTER_USER_TEST.md)
- [Verified and open 0.1.0 acceptance gates](ACCEPTANCE_STATUS.md)
- [Windows application release candidate](WINDOWS_LOCAL_RC.md)
- [H3 Codec Pack packaging and lifecycle](H3_CODEC_PACK.md)
- [Isolated ComfyUI user-test environment](ISOLATED_COMFY_TEST_ENVIRONMENT.md)

Application installers and Codec Packs are independent artifacts with
independent install, update, and removal lifecycles. Every application RC set
also carries the validated CycloneDX 1.5 SBOM at
`metadata/latentdeck-0.1.0-sbom.cdx.json`; its length and SHA-256 are bound by
the RC receipt and `SHA256SUMS.txt`. The SBOM explicitly inventories the
separately prepared upstream Spout2 native source, and the same RC metadata
directory carries the hash-bound `THIRD_PARTY_NOTICES.md` with its
BSD-2-Clause terms. Receiver executables are local QA tools and are not release
artifacts.
