import { describe, expect, it } from "vitest";
import {
  EMPTY_EXTENSIONS_SNAPSHOT,
  compatibilityReasonLabel,
  extensionPackageKey,
  inspectionMatchesPackage,
  isExactLowercaseSha256,
  publisherIdentityNotice,
  replaceVerifiedSummary,
  shaConfirmationMatches,
  type ExtensionPackageReference,
  type ExtensionPackageSummary,
  type ExtensionCodecDevice,
  type InspectedExtension,
} from "./extension-manager-model";

const DECK_010: ExtensionPackageReference = {
  kind: "deck_pack",
  packageId: "net.example.deck",
  packageVersion: "0.1.0",
};

const DECK_020: ExtensionPackageReference = {
  ...DECK_010,
  packageVersion: "0.2.0",
};

const INSPECTION: InspectedExtension = {
  package: DECK_010,
  displayName: "Example Deck",
  publisherName: "Example Publisher",
  publisherIdentityClaim: "self_declared",
  archiveSha256: "a".repeat(64),
  archiveByteLength: 512,
  fileCount: 5,
  extractedByteLength: 1_024,
};

const SUMMARY: ExtensionPackageSummary = {
  package: DECK_010,
  displayName: "Example Deck",
  publisherName: "Example Publisher",
  enabled: false,
  health: "healthy",
  errorCode: null,
  errorDetail: null,
};

describe("LatentPlayer Extensions Manager model", () => {
  it("keeps Player codec device selection closed to CPU or CUDA", () => {
    const devices: ExtensionCodecDevice[] = ["cpu", "cuda"];
    expect(devices).toEqual(["cpu", "cuda"]);
  });

  it("requires the exact user-entered lowercase SHA-256", () => {
    const sha256 = "a".repeat(64);

    expect(isExactLowercaseSha256(sha256)).toBe(true);
    expect(shaConfirmationMatches(sha256, sha256)).toBe(true);
    expect(isExactLowercaseSha256(sha256.toUpperCase())).toBe(false);
    expect(shaConfirmationMatches(` ${sha256}`, sha256)).toBe(false);
    expect(shaConfirmationMatches("b".repeat(64), sha256)).toBe(false);
  });

  it("keys immutable versions independently instead of selecting newest", () => {
    expect(extensionPackageKey(DECK_010)).toBe(
      "deck_pack\u0000net.example.deck\u00000.1.0",
    );
    expect(extensionPackageKey(DECK_020)).not.toBe(
      extensionPackageKey(DECK_010),
    );
    expect(EMPTY_EXTENSIONS_SNAPSHOT).toEqual({ packages: [], matrix: [] });
  });

  it("allows repair only when the separately inspected archive has exact identity", () => {
    expect(inspectionMatchesPackage(INSPECTION, DECK_010)).toBe(true);
    expect(inspectionMatchesPackage(INSPECTION, DECK_020)).toBe(false);
    expect(
      inspectionMatchesPackage(
        {
          ...INSPECTION,
          package: { ...DECK_010, kind: "codec_pack" },
        },
        DECK_010,
      ),
    ).toBe(false);
  });

  it("labels publisher metadata as self-declared without implying trust", () => {
    expect(publisherIdentityNotice(INSPECTION)).toBe(
      "Example Publisher · self-declared metadata; SHA-256 confirms bytes, not publisher identity.",
    );
  });

  it("renders stable compatibility refusal reasons without fallback language", () => {
    expect(compatibilityReasonLabel("compatible")).toBe("Compatible");
    expect(compatibilityReasonLabel("unsupported_tensor_abi")).toBe(
      "Unsupported tensor ABI",
    );
    expect(compatibilityReasonLabel("missing_asset")).toBe(
      "Required external asset missing",
    );
  });

  it("replaces only the exact verified summary", () => {
    const newer = { ...SUMMARY, package: DECK_020 };
    const verified = { ...SUMMARY, enabled: true };
    const snapshot = replaceVerifiedSummary(
      { packages: [SUMMARY, newer], matrix: [] },
      verified,
    );

    expect(snapshot.packages).toEqual([verified, newer]);
  });
});
