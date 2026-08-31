import type { CartridgeView } from "./library-model";

export interface DeckSourceTruthState {
  loadedArchiveSha256s: readonly string[] | null;
  draftEditedAfterLoad: boolean;
}

export interface DeckSourceTruthReconciliation<
  THashes extends readonly string[],
> {
  state: DeckSourceTruthState;
  draftArchiveSha256s: THashes;
  synchronized: boolean;
}

export function createDeckSourceTruthState(): DeckSourceTruthState {
  return {
    loadedArchiveSha256s: null,
    draftEditedAfterLoad: false,
  };
}

/**
 * Reconcile the editable next-load draft with the host-owned runtime truth.
 *
 * An arriving runtime identity may update an untouched draft. Once the user
 * deliberately edits that draft, repeated status events and direct host loads
 * cannot silently overwrite it. A successful Load naturally clears the edit
 * marker because its acknowledged runtime hashes equal the submitted draft.
 */
export function reconcileDeckSourceTruth<THashes extends readonly string[]>(
  current: Readonly<DeckSourceTruthState>,
  draftArchiveSha256s: THashes,
  incomingLoadedArchiveSha256s: THashes | null,
): DeckSourceTruthReconciliation<THashes> {
  if (incomingLoadedArchiveSha256s === null) {
    return {
      state: {
        loadedArchiveSha256s: null,
        draftEditedAfterLoad: current.draftEditedAfterLoad,
      },
      draftArchiveSha256s,
      synchronized: false,
    };
  }

  const draftMatchesRuntime = sameHashes(
    draftArchiveSha256s,
    incomingLoadedArchiveSha256s,
  );
  const shouldSynchronize =
    !current.draftEditedAfterLoad || draftMatchesRuntime;
  const nextDraft = shouldSynchronize
    ? ([...incomingLoadedArchiveSha256s] as unknown as THashes)
    : draftArchiveSha256s;

  return {
    state: {
      loadedArchiveSha256s: [...incomingLoadedArchiveSha256s],
      draftEditedAfterLoad: !shouldSynchronize,
    },
    draftArchiveSha256s: nextDraft,
    synchronized:
      shouldSynchronize && !sameHashes(draftArchiveSha256s, nextDraft),
  };
}

export function markDeckSourceDraftEdited<THashes extends readonly string[]>(
  current: Readonly<DeckSourceTruthState>,
  draftArchiveSha256s: THashes,
): DeckSourceTruthState {
  return {
    loadedArchiveSha256s:
      current.loadedArchiveSha256s === null
        ? null
        : [...current.loadedArchiveSha256s],
    draftEditedAfterLoad:
      current.loadedArchiveSha256s === null ||
      !sameHashes(draftArchiveSha256s, current.loadedArchiveSha256s),
  };
}

export function deckSourceDraftDiffers(
  draftArchiveSha256s: readonly string[],
  loadedArchiveSha256s: readonly string[] | null,
): boolean {
  return (
    loadedArchiveSha256s !== null &&
    !sameHashes(draftArchiveSha256s, loadedArchiveSha256s)
  );
}

export interface PlayingSourceStatus {
  cartridgeId: string;
  archiveSha256: string;
  latentSlotCount: string;
}

export interface CurrentlyPlayingReadout {
  codecLabel: string;
  geometryLabel: string;
  frameLabel: string;
  latentLabel: string;
}

export const DECK_SOURCE_RESOLUTION_RETRY_DELAYS_MS = [
  150, 500, 1_500,
] as const;

export function resolvePlayingSourceView(
  source: Pick<PlayingSourceStatus, "cartridgeId" | "archiveSha256">,
  candidates: readonly (CartridgeView | null)[],
): CartridgeView | undefined {
  return candidates.find(
    (candidate): candidate is CartridgeView =>
      candidate !== null &&
      candidate.archiveSha256 === source.archiveSha256 &&
      candidate.cartridgeId === source.cartridgeId,
  );
}

export function describeCurrentlyPlayingSource(
  source: PlayingSourceStatus,
  resolved: CartridgeView | undefined,
): string {
  const name =
    resolved?.paths[0]?.fileName ?? resolved?.cartridgeId ?? source.cartridgeId;
  const hash =
    source.archiveSha256.length > 12
      ? `${source.archiveSha256.slice(0, 12)}…`
      : source.archiveSha256;
  const media =
    resolved === undefined
      ? "LIBRARY DETAILS UNAVAILABLE"
      : `${resolved.decodedWidth}×${resolved.decodedHeight} · ${resolved.decodedFrameCount} FRAMES`;
  return `${name} · ${hash} · ${media} · ${source.latentSlotCount} LATENT SLOTS`;
}

/**
 * Build the prominent readout from runtime truth, never from the editable
 * next-load draft. Runtime identity and latent-slot count remain useful even
 * while the friendly Library record is temporarily unavailable.
 */
export function currentlyPlayingReadout(
  source: PlayingSourceStatus,
  resolved: CartridgeView | undefined,
): CurrentlyPlayingReadout {
  return {
    codecLabel: resolved?.codecProfile ?? "RUNTIME SOURCE VERIFIED",
    geometryLabel:
      resolved === undefined
        ? "RUNTIME SOURCE"
        : `${resolved.decodedWidth}×${resolved.decodedHeight}`,
    frameLabel:
      resolved !== undefined && resolved.decodedFrameCount > 0
        ? `${resolved.decodedFrameCount} DECODED FRAMES`
        : `${source.latentSlotCount} LATENT SLOTS`,
    latentLabel: `${source.latentSlotCount} LATENT SLOTS`,
  };
}

export function shouldShowNextLoadDraftReadout(
  source: PlayingSourceStatus,
  draftArchiveSha256: string,
  resolvedDraft: CartridgeView | undefined,
): boolean {
  return (
    source.archiveSha256 !== draftArchiveSha256 || resolvedDraft === undefined
  );
}

export function deckSourceResolutionRetryDelay(attempt: number): number | null {
  if (!Number.isSafeInteger(attempt) || attempt < 0) return null;
  return DECK_SOURCE_RESOLUTION_RETRY_DELAYS_MS[attempt] ?? null;
}

function sameHashes(
  left: readonly string[] | null,
  right: readonly string[] | null,
): boolean {
  return (
    left !== null &&
    right !== null &&
    left.length === right.length &&
    left.every((hash, index) => hash === right[index])
  );
}
