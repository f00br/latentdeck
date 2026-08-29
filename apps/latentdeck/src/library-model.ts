export const ALL_CARTRIDGES_ID = "latentdeck.virtual.all";
export const UNASSIGNED_ID = "latentdeck.virtual.unassigned";

export type Availability = "present" | "warning" | "missing";
export type PathState = "present" | "missing" | "invalid" | "content_changed";

export interface SlotAssignmentView {
  slot: string;
  archiveSha256: string;
}

export interface DeckSessionView {
  activeCollectionId: string;
  loadedSlots: SlotAssignmentView[];
}

export interface CollectionView {
  id: string;
  name: string;
  position: number | null;
  isVirtual: boolean;
  memberCount: number;
}

export interface CartridgePathView {
  path: string;
  fileName: string;
  state: PathState;
  warningCode: string | null;
}

export interface CartridgeView {
  archiveSha256: string;
  cartridgeId: string;
  codecFamily: string;
  codecProfile: string;
  decodedWidth: number;
  decodedHeight: number;
  decodedFrameCount: number;
  durationNumerator: number;
  durationDenominator: number;
  favorite: boolean;
  tags: string[];
  availability: Availability;
  paths: CartridgePathView[];
}

export interface LibraryView {
  deckSession: DeckSessionView;
  collections: CollectionView[];
  cartridges: CartridgeView[];
  recent: CartridgeView[];
  search: string;
  totalIndexed: number;
  activeMemberCount: number;
}

export interface ImportFailureView {
  path: string;
  code: string;
  message: string;
}

export interface ImportSummary {
  accepted: number;
  rejected: ImportFailureView[];
  ignoredNonCartridges: number;
}

export interface ReindexSummary {
  unchanged: number;
  present: number;
  missing: number;
  invalid: number;
  contentChanged: number;
}

export const EMPTY_LIBRARY_VIEW: LibraryView = Object.freeze({
  deckSession: {
    activeCollectionId: ALL_CARTRIDGES_ID,
    loadedSlots: [],
  },
  collections: [],
  cartridges: [],
  recent: [],
  search: "",
  totalIndexed: 0,
  activeMemberCount: 0,
});

export function selectActiveCollection(
  session: DeckSessionView,
  activeCollectionId: string,
): DeckSessionView {
  return {
    activeCollectionId,
    loadedSlots: session.loadedSlots,
  };
}

export function realCollections(
  collections: CollectionView[],
): CollectionView[] {
  return collections.filter((collection) => !collection.isVirtual);
}

export function moveItem<T>(
  items: readonly T[],
  key: string,
  direction: -1 | 1,
  keyOf: (item: T) => string,
): T[] {
  const current = items.findIndex((item) => keyOf(item) === key);
  const target = current + direction;
  if (current < 0 || target < 0 || target >= items.length) {
    return [...items];
  }
  const reordered = [...items];
  [reordered[current], reordered[target]] = [
    reordered[target],
    reordered[current],
  ];
  return reordered;
}

export function canReorderActiveMembers(
  view: LibraryView,
  search: string,
): boolean {
  const active = view.collections.find(
    (collection) => collection.id === view.deckSession.activeCollectionId,
  );
  return (
    active !== undefined &&
    !active.isVirtual &&
    search.trim() === "" &&
    view.cartridges.length === view.activeMemberCount
  );
}

export function parseTags(value: string): string[] {
  const seen = new Set<string>();
  const tags: string[] = [];
  for (const candidate of value.split(",")) {
    const tag = candidate.trim();
    const normalized = tag.toLocaleLowerCase();
    if (tag !== "" && !seen.has(normalized)) {
      seen.add(normalized);
      tags.push(tag);
    }
  }
  return tags;
}

export function shortHash(hash: string): string {
  return hash.length > 12 ? `${hash.slice(0, 12)}…` : hash;
}

export function formatDuration(numerator: number, denominator: number): string {
  if (denominator <= 0) {
    return "—";
  }
  return `${(numerator / denominator).toFixed(2)}s`;
}

export function describeCommandError(error: unknown): string {
  if (
    typeof error === "object" &&
    error !== null &&
    "message" in error &&
    typeof error.message === "string"
  ) {
    return error.message;
  }
  return error instanceof Error ? error.message : String(error);
}
