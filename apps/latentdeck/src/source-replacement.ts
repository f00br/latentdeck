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
