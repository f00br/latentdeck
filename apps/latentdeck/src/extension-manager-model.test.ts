import { describe, expect, it } from "vitest";
import {
  extensionPackageKey,
  inspectionMatchesPackage,
  isExactLowercaseSha256,
  shaConfirmationMatches,
  type InspectedExtension,
} from "./extension-manager-model";

const inspection: InspectedExtension = {
  package: {
    kind: "deck_pack",
    packageId: "org.example.deck",
    packageVersion: "1.2.3",
  },
  displayName: "Example Deck",
  publisherName: "Example",
  publisherIdentityClaim: "self_declared",
  archiveSha256: "a".repeat(64),
  archiveByteLength: 100,
  fileCount: 5,
  extractedByteLength: 200,
};

describe("LatentDeck Extensions Manager model", () => {
  it("requires the exact lowercase measured SHA-256", () => {
    expect(isExactLowercaseSha256("a".repeat(64))).toBe(true);
    expect(isExactLowercaseSha256("A".repeat(64))).toBe(false);
    expect(
      shaConfirmationMatches("a".repeat(64), inspection.archiveSha256),
    ).toBe(true);
    expect(
      shaConfirmationMatches("b".repeat(64), inspection.archiveSha256),
    ).toBe(false);
  });

  it("binds repair and actions to one exact package kind, id, and version", () => {
    expect(inspectionMatchesPackage(inspection, inspection.package)).toBe(true);
    expect(
      inspectionMatchesPackage(inspection, {
        ...inspection.package,
        packageVersion: "1.2.4",
      }),
    ).toBe(false);
    expect(extensionPackageKey(inspection.package)).toContain(
      "org.example.deck",
    );
  });
});
