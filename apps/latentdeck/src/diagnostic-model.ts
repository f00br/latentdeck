export type DiagnosticSaveResult =
  | {
      status: "saved";
      archiveBytes: number;
      eventCount: number;
      schemaVersion: number;
    }
  | { status: "cancelled" };

export function describeDiagnosticSaveResult(
  result: DiagnosticSaveResult,
): string {
  if (result.status === "cancelled") {
    return "Diagnostic save cancelled; no file was created.";
  }
  const kibibytes = result.archiveBytes / 1024;
  const size = `${kibibytes.toFixed(kibibytes >= 100 ? 0 : 1)} KiB`;
  const eventLabel = result.eventCount === 1 ? "event" : "events";
  return `Diagnostic bundle saved · ${size} · ${result.eventCount} ${eventLabel} · schema ${result.schemaVersion}`;
}
