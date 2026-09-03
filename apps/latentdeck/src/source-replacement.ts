export function replaceDraftSource<THashes extends readonly string[]>(
  draft: THashes,
  slotIndex: number,
  archiveSha256: string,
): THashes {
  if (
    !Number.isSafeInteger(slotIndex) ||
    slotIndex < 0 ||
    slotIndex >= draft.length
  ) {
    throw new RangeError("Deck source slot is out of range.");
  }
  const next = [...draft];
  next[slotIndex] = archiveSha256;
  return next as unknown as THashes;
}

export function retainDraftSourceOptions<
  TSource extends { archiveSha256: string },
>(
  bankSources: readonly TSource[],
  resolvedSources: readonly (TSource | null)[],
  draftArchiveSha256s: readonly string[],
): TSource[] {
  const selected = new Set(draftArchiveSha256s);
  const retained = new Map(
    bankSources.map((source) => [source.archiveSha256, source]),
  );
  for (const source of resolvedSources) {
    if (
      source !== null &&
      selected.has(source.archiveSha256) &&
      !retained.has(source.archiveSha256)
    ) {
      retained.set(source.archiveSha256, source);
    }
  }
  return [...retained.values()];
}

export function selectedSourceAspectWarning<
  TSource extends {
    archiveSha256: string;
    decodedWidth: number;
    decodedHeight: number;
    signalPresentation: {
      aspect_ratio: { width: number; height: number };
    };
  },
>(
  selectedArchiveSha256s: readonly string[],
  sources: readonly TSource[],
): string {
  const selected = selectedArchiveSha256s.map((archiveSha256) =>
    sources.find((source) => source.archiveSha256 === archiveSha256),
  );
  const reference = selected[0];
  if (reference === undefined) return "";
  const expected = reference.signalPresentation.aspect_ratio;
  const mismatchIndex = selected.findIndex((source, index) => {
    if (index === 0 || source === undefined) return false;
    const actual = source.signalPresentation.aspect_ratio;
    return expected.width * actual.height !== actual.width * expected.height;
  });
  const mismatch = selected[mismatchIndex];
  if (mismatchIndex < 1 || mismatch === undefined) return "";
  const actual = mismatch.signalPresentation.aspect_ratio;
  return [
    "Aspect mismatch",
    `A ${expected.width}:${expected.height} (${reference.decodedWidth}×${reference.decodedHeight})`,
    `${String.fromCharCode(65 + mismatchIndex)} ${actual.width}:${actual.height} (${mismatch.decodedWidth}×${mismatch.decodedHeight}). No hidden resize or crop; align sources in the Toolkit first.`,
  ].join(" · ");
}

export interface DraftAwareSlotPlayOptions {
  loadedArchiveSha256: string;
  draftArchiveSha256: string;
  loadDraftAndPlay: () => Promise<void>;
  toggleCurrent: () => Promise<void>;
}

export async function playDraftAwareSlot(
  options: DraftAwareSlotPlayOptions,
): Promise<"loaded_draft" | "toggled_current"> {
  if (options.loadedArchiveSha256 !== options.draftArchiveSha256) {
    await options.loadDraftAndPlay();
    return "loaded_draft";
  }
  await options.toggleCurrent();
  return "toggled_current";
}

export function transportForDraftLoad<TTransport, TSlot>(
  retainedTransport: TTransport,
  playSlot: TSlot | null,
  setPlaying: (
    transport: TTransport,
    slot: TSlot,
    playing: boolean,
  ) => TTransport,
): TTransport {
  return playSlot === null
    ? retainedTransport
    : setPlaying(retainedTransport, playSlot, true);
}

export interface ExclusiveOperationGate {
  run(operation: () => Promise<void>): Promise<"completed" | "ignored">;
}

export function createExclusiveOperationGate(
  onActiveChange: (active: boolean) => void,
): ExclusiveOperationGate {
  let active = false;
  return {
    async run(operation) {
      if (active) return "ignored";
      active = true;
      try {
        onActiveChange(true);
        await operation();
        return "completed";
      } finally {
        active = false;
        onActiveChange(false);
      }
    },
  };
}
