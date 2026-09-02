export type ConversionPhase =
  "planned" | "running" | "stopping" | "complete" | "stopped";

export type ConversionStatus =
  "ready" | "converting" | "complete" | "failed" | "cancelled";

export interface ConversionError {
  code: string;
  message: string;
  recoverable: boolean;
}

export interface ConversionMetadata {
  sourceBytes: number;
  sourceSha256: string;
  storageDtype: "F16" | "F32";
  latentSlots: number;
  latentHeight: number;
  latentWidth: number;
  decodedWidth: number;
  decodedHeight: number;
  decodedFrames: number;
  audioPresent: boolean;
}

export interface RawImportProfile {
  codecFamily: string;
  profile: string;
  profileVersion: string;
}

export interface RawImportCodecOptions {
  packageId: string;
  packageVersion: string;
  adapterId: string;
  adapterVersion: string;
  displayName: string;
  profiles: RawImportProfile[];
}

export interface RawImportSelection {
  packageId: string;
  packageVersion: string;
  adapterId: string;
  adapterVersion: string;
  profile: RawImportProfile;
}

export interface ConversionItem {
  sourceName: string;
  relativeOutput: string;
  status: ConversionStatus;
  metadata: ConversionMetadata | null;
  error: ConversionError | null;
  archiveSha256: string | null;
}

export interface ConversionSnapshot {
  phase: ConversionPhase;
  selection: Omit<RawImportSelection, "displayName" | "profiles">;
  items: ConversionItem[];
  completed: number;
  failed: number;
  activeIndex: number | null;
  stopRequested: boolean;
}

export interface ConversionControls {
  preflight: boolean;
  start: boolean;
  stopAfterCurrent: boolean;
  changeSelection: boolean;
}

export function conversionControls(
  snapshot: ConversionSnapshot | null,
  inputCount: number,
  outputSelected: boolean,
  profileSelected: boolean,
  busy: boolean,
): ConversionControls {
  const active =
    snapshot?.phase === "running" || snapshot?.phase === "stopping";
  const hasReadyItem =
    snapshot?.items.some((item) => item.status === "ready") === true;
  return {
    preflight:
      !busy && !active && inputCount > 0 && outputSelected && profileSelected,
    start: !busy && snapshot?.phase === "planned" && hasReadyItem,
    stopAfterCurrent: !busy && snapshot?.phase === "running",
    changeSelection: !busy && !active,
  };
}

export function rawImportProfileKey(profile: RawImportProfile): string {
  return `${profile.codecFamily}\u0000${profile.profile}\u0000${profile.profileVersion}`;
}

export function conversionIsActive(
  snapshot: ConversionSnapshot | null,
): boolean {
  return snapshot?.phase === "running" || snapshot?.phase === "stopping";
}

export function conversionProgressLabel(
  snapshot: ConversionSnapshot | null,
): string {
  if (snapshot === null) return "No conversion prepared";
  const cancelled = conversionCancelledCount(snapshot);
  const settled = snapshot.completed + snapshot.failed + cancelled;
  const total = snapshot.items.length;
  if (snapshot.phase === "stopping") {
    return `Stopping after current file · ${settled} / ${total} settled`;
  }
  if (snapshot.phase === "stopped") {
    return `Stopped · ${settled} / ${total} settled · ${cancelled} cancelled`;
  }
  if (snapshot.phase === "complete") {
    return `Finished · ${snapshot.completed} converted · ${snapshot.failed} failed`;
  }
  if (snapshot.phase === "running" && snapshot.activeIndex !== null) {
    const active = snapshot.items[snapshot.activeIndex];
    return `Converting ${active?.sourceName ?? "current file"} · ${settled} / ${total} settled`;
  }
  const ready = snapshot.items.filter((item) => item.status === "ready").length;
  const noun = total === 1 ? "file" : "files";
  const failed = snapshot.failed > 0 ? ` · ${snapshot.failed} failed` : "";
  return `${ready} of ${total} ${noun} ready${failed}`;
}

export function conversionCancelledCount(
  snapshot: ConversionSnapshot | null,
): number {
  return (
    snapshot?.items.filter((item) => item.status === "cancelled").length ?? 0
  );
}
