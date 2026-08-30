export const ALL_CARTRIDGES_ID = "latentdeck.virtual.all";
export const UNASSIGNED_ID = "latentdeck.virtual.unassigned";

export type Availability = "present" | "warning" | "missing";
export type PathState = "present" | "missing" | "invalid" | "content_changed";

export interface SlotAssignmentView {
  deckType: "d2" | "q4";
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

export type SignalOrientation = "portrait" | "landscape" | "square";
export type SignalCompatibilityPolicy =
  | "playback"
  | "spatial_synthesis"
  | "full_tensor_synthesis";

export interface SignalGeometry {
  codec_family: string;
  profile: string;
  profile_version: string;
  runtime_dtype: "F16" | "F32";
  batch: number;
  latent_channels: number;
  latent_slots: number;
  latent_height: number;
  latent_width: number;
  decoded_frame_count: number;
  decoded_height: number;
  decoded_width: number;
  timing_contract: string;
  timing_contract_version: string;
  frame_rate: { numerator: number; denominator: number };
}

export interface SignalPresentation {
  orientation: SignalOrientation;
  aspect_ratio: { width: number; height: number };
  decoded_width: number;
  decoded_height: number;
}

export interface SignalWorkload {
  latent_sites_per_slot: number | null;
  latent_values_per_slot: number | null;
  latent_values_per_clip: number | null;
  decoded_pixels_per_frame: number | null;
}

export type SignalGeometryMismatchCode =
  | "codec_family"
  | "profile"
  | "profile_version"
  | "runtime_dtype"
  | "batch"
  | "latent_channels"
  | "latent_height"
  | "latent_width"
  | "latent_slots"
  | "decoded_height"
  | "decoded_width"
  | "decoded_frame_count"
  | "timing_contract"
  | "timing_contract_version"
  | "frame_rate";

export interface SignalGeometryMismatch {
  candidate_index: number;
  code: SignalGeometryMismatchCode;
  expected: string;
  actual: string;
}

export interface SignalCompatibilityReport {
  policy: SignalCompatibilityPolicy;
  compatible: boolean;
  mismatches: SignalGeometryMismatch[];
}

export interface CartridgeView {
  archiveSha256: string;
  cartridgeId: string;
  codecFamily: string;
  codecProfile: string;
  codecProfileVersion: string;
  timingContract: string;
  timingContractVersion: string;
  frameRateNumerator: number;
  frameRateDenominator: number;
  decodedWidth: number;
  decodedHeight: number;
  decodedFrameCount: number;
  durationNumerator: number;
  durationDenominator: number;
  signalGeometry: SignalGeometry;
  signalPresentation: SignalPresentation;
  signalWorkload: SignalWorkload;
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

export function describeLoadedSlots(slots: SlotAssignmentView[]): string {
  const decks = (["d2", "q4"] as const).flatMap((deckType) => {
    const deckSlots = slots
      .filter((slot) => slot.deckType === deckType)
      .map((slot) => slot.slot)
      .sort((left, right) => left.localeCompare(right));
    return deckSlots.length === 0
      ? []
      : [`${deckType.toUpperCase()} ${deckSlots.join("/")}`];
  });
  return decks.length === 0
    ? "No retained slots"
    : `${decks.join(" · ")} retained`;
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

export interface IntrinsicFormatDescription {
  aspectLabel: string;
  decodedGeometry: string;
  latentGrid: string | null;
}

export function describeIntrinsicFormat(
  cartridge: CartridgeView,
): IntrinsicFormatDescription {
  const presentation = cartridge.signalPresentation;
  const ratio = presentation.aspect_ratio;
  const aspectLabel = `${presentation.orientation.toUpperCase()} · ${ratio.width}:${ratio.height}`;
  return {
    aspectLabel,
    decodedGeometry: `${cartridge.decodedWidth}×${cartridge.decodedHeight}`,
    latentGrid: `${cartridge.signalGeometry.latent_width}×${cartridge.signalGeometry.latent_height}`,
  };
}

export function compatibilityReasonsByHash(
  candidateHashes: readonly string[],
  report: SignalCompatibilityReport,
): ReadonlyMap<string, readonly string[]> {
  const reasons = new Map<string, string[]>();
  for (const mismatch of report.mismatches) {
    const hash = candidateHashes[mismatch.candidate_index];
    if (hash === undefined) continue;
    const existing = reasons.get(hash) ?? [];
    existing.push(describeSignalMismatch(mismatch));
    reasons.set(hash, existing);
  }
  return reasons;
}

export function describeSignalMismatch(
  mismatch: SignalGeometryMismatch,
): string {
  const label: Record<SignalGeometryMismatchCode, string> = {
    codec_family: "codec",
    profile: "profile",
    profile_version: "profile version",
    runtime_dtype: "runtime dtype",
    batch: "batch",
    latent_channels: "latent channels",
    latent_height: "latent height",
    latent_width: "latent width",
    latent_slots: "latent T",
    decoded_height: "decoded height",
    decoded_width: "decoded width",
    decoded_frame_count: "decoded frames",
    timing_contract: "timing contract",
    timing_contract_version: "timing version",
    frame_rate: "frame rate",
  };
  return `${label[mismatch.code]} ${mismatch.actual} (needs ${mismatch.expected})`;
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
