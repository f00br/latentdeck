import { describe, expect, it } from "vitest";

import {
  describeDiagnosticSaveResult,
  type DiagnosticSaveResult,
} from "./diagnostic-model";

describe("Deck diagnostic save results", () => {
  it("describes a native save receipt without exposing a destination", () => {
    const saved: DiagnosticSaveResult = {
      status: "saved",
      archiveBytes: 4_096,
      eventCount: 2,
      schemaVersion: 1,
    };

    expect(describeDiagnosticSaveResult(saved)).toBe(
      "Diagnostic bundle saved · 4.0 KiB · 2 events · schema 1",
    );
    expect(Object.keys(saved)).not.toContain("path");
    expect(Object.keys(saved)).not.toContain("destination");
  });

  it("makes native-dialog cancellation explicit", () => {
    expect(describeDiagnosticSaveResult({ status: "cancelled" })).toContain(
      "cancelled",
    );
  });
});
