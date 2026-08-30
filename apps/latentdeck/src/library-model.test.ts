import { describe, expect, it } from "vitest";
import {
  ALL_CARTRIDGES_ID,
  EMPTY_LIBRARY_VIEW,
  canReorderActiveMembers,
  compatibilityReasonsByHash,
  describeIntrinsicFormat,
  describeLoadedSlots,
  moveItem,
  parseTags,
  selectActiveCollection,
  type CollectionView,
  type CartridgeView,
  type DeckSessionView,
  type LibraryView,
} from "./library-model";

function cartridge(decodedWidth: number, decodedHeight: number): CartridgeView {
  const divisor = greatestCommonDivisor(decodedWidth, decodedHeight);
  return {
    archiveSha256: "a".repeat(64),
    cartridgeId: "11111111-1111-4111-8111-111111111111",
    codecFamily: "minimax_h3",
    codecProfile: "h3_av_latent",
    codecProfileVersion: "0.1.0",
    timingContract: "minimax_h3_causal",
    timingContractVersion: "0.1.0",
    frameRateNumerator: 24,
    frameRateDenominator: 1,
    decodedWidth,
    decodedHeight,
    decodedFrameCount: 107,
    durationNumerator: 107,
    durationDenominator: 24,
    signalGeometry: {
      codec_family: "minimax_h3",
      profile: "h3_av_latent",
      profile_version: "0.1.0",
      runtime_dtype: "F16",
      batch: 1,
      latent_channels: 24,
      latent_slots: 32,
      latent_height: decodedHeight / 16,
      latent_width: decodedWidth / 16,
      decoded_frame_count: 107,
      decoded_height: decodedHeight,
      decoded_width: decodedWidth,
      timing_contract: "minimax_h3_causal",
      timing_contract_version: "0.1.0",
      frame_rate: { numerator: 24, denominator: 1 },
    },
    signalPresentation: {
      orientation:
        decodedWidth === decodedHeight
          ? "square"
          : decodedWidth < decodedHeight
            ? "portrait"
            : "landscape",
      aspect_ratio: {
        width: decodedWidth / divisor,
        height: decodedHeight / divisor,
      },
      decoded_width: decodedWidth,
      decoded_height: decodedHeight,
    },
    signalWorkload: {
      latent_sites_per_slot: (decodedWidth / 16) * (decodedHeight / 16),
      latent_values_per_slot: null,
      latent_values_per_clip: null,
      decoded_pixels_per_frame: decodedWidth * decodedHeight,
    },
    favorite: false,
    tags: [],
    availability: "present",
    paths: [],
  };
}

function greatestCommonDivisor(left: number, right: number): number {
  while (right !== 0) {
    [left, right] = [right, left % right];
  }
  return left || 1;
}

describe("active Collection / Bank contract", () => {
  it("changes the browser selection without unloading playing slots", () => {
    const slots = [
      { deckType: "d2" as const, slot: "A", archiveSha256: "a".repeat(64) },
      { deckType: "d2" as const, slot: "B", archiveSha256: "b".repeat(64) },
    ];
    const session: DeckSessionView = {
      activeCollectionId: ALL_CARTRIDGES_ID,
      loadedSlots: slots,
    };
    const selected = selectActiveCollection(session, "collection-id");
    expect(selected.activeCollectionId).toBe("collection-id");
    expect(selected.loadedSlots).toBe(slots);
  });

  it("reports the retained slots per running Deck", () => {
    expect(describeLoadedSlots([])).toBe("No retained slots");
    expect(
      describeLoadedSlots([
        { deckType: "d2", slot: "A", archiveSha256: "a".repeat(64) },
        { deckType: "d2", slot: "B", archiveSha256: "b".repeat(64) },
        { deckType: "q4", slot: "A", archiveSha256: "c".repeat(64) },
        { deckType: "q4", slot: "D", archiveSha256: "d".repeat(64) },
      ]),
    ).toBe("D2 A/B · Q4 A/D retained");
  });
});

describe("manual ordering", () => {
  const collections: CollectionView[] = [
    { id: "a", name: "A", position: 0, isVirtual: false, memberCount: 0 },
    { id: "b", name: "B", position: 1, isVirtual: false, memberCount: 0 },
    { id: "c", name: "C", position: 2, isVirtual: false, memberCount: 0 },
  ];

  it("moves exactly one item and leaves boundary moves unchanged", () => {
    expect(
      moveItem(collections, "b", -1, (item) => item.id).map((item) => item.id),
    ).toEqual(["b", "a", "c"]);
    expect(moveItem(collections, "a", -1, (item) => item.id)).toEqual(
      collections,
    );
  });

  it("only enables member reorder for a complete unfiltered real collection", () => {
    const view: LibraryView = {
      ...EMPTY_LIBRARY_VIEW,
      deckSession: { activeCollectionId: "a", loadedSlots: [] },
      collections,
      activeMemberCount: 0,
    };
    expect(canReorderActiveMembers(view, "")).toBe(true);
    expect(canReorderActiveMembers(view, "filtered")).toBe(false);
    expect(canReorderActiveMembers({ ...view, activeMemberCount: 3 }, "")).toBe(
      false,
    );
  });
});

describe("tag input", () => {
  it("trims and removes case-insensitive duplicates while preserving display case", () => {
    expect(parseTags("Ambient, warm, AMBIENT, , Cold")).toEqual([
      "Ambient",
      "warm",
      "Cold",
    ]);
  });
});

describe("intrinsic cartridge format", () => {
  it("shows a human aspect badge, exact decoded geometry, and derived H3 grid", () => {
    expect(describeIntrinsicFormat(cartridge(448, 800))).toEqual({
      aspectLabel: "PORTRAIT · 14:25",
      decodedGeometry: "448×800",
      latentGrid: "28×50",
    });
    expect(describeIntrinsicFormat(cartridge(1344, 768))).toEqual({
      aspectLabel: "LANDSCAPE · 7:4",
      decodedGeometry: "1344×768",
      latentGrid: "84×48",
    });
    expect(describeIntrinsicFormat(cartridge(1024, 768)).aspectLabel).toBe(
      "LANDSCAPE · 4:3",
    );
  });

  it("indexes typed Core mismatch reasons by the exact candidate order", () => {
    const hashes = ["a".repeat(64), "b".repeat(64)];
    const reasons = compatibilityReasonsByHash(hashes, {
      policy: "spatial_synthesis",
      compatible: false,
      mismatches: [
        {
          candidate_index: 1,
          code: "latent_width",
          expected: "28",
          actual: "84",
        },
        {
          candidate_index: 1,
          code: "decoded_width",
          expected: "448",
          actual: "1344",
        },
      ],
    });

    expect(reasons.has(hashes[0])).toBe(false);
    expect(reasons.get(hashes[1])).toEqual([
      "latent width 84 (needs 28)",
      "decoded width 1344 (needs 448)",
    ]);
  });
});
