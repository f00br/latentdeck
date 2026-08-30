import { describe, expect, it } from "vitest";
import {
  ALL_CARTRIDGES_ID,
  EMPTY_LIBRARY_VIEW,
  canReorderActiveMembers,
  describeLoadedSlots,
  moveItem,
  parseTags,
  selectActiveCollection,
  type CollectionView,
  type DeckSessionView,
  type LibraryView,
} from "./library-model";

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
