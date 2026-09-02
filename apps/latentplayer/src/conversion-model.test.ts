import { describe, expect, it } from "vitest";

import {
  conversionCancelledCount,
  conversionControls,
  conversionProgressLabel,
  type ConversionSnapshot,
} from "./conversion-model";

function snapshot(
  phase: ConversionSnapshot["phase"],
  status: ConversionSnapshot["items"][number]["status"] = "ready",
): ConversionSnapshot {
  return {
    phase,
    selection: {
      packageId: "org.example.codec",
      packageVersion: "0.2.0",
      adapterId: "org.example.codec.adapter",
      adapterVersion: "0.2.0",
      profile: {
        codecFamily: "example_codec",
        profile: "example_latent",
        profileVersion: "0.1.0",
      },
    },
    items: [
      {
        sourceName: "clip.safetensors",
        relativeOutput: "clip.lc",
        status,
        metadata: null,
        error: null,
        archiveSha256: null,
      },
    ],
    completed: 0,
    failed: 0,
    activeIndex: null,
    stopRequested: phase === "stopping",
  };
}

describe("conversion controls", () => {
  it("moves a prepared batch through start and stop-after-current states", () => {
    expect(conversionControls(null, 0, false, false, false)).toEqual({
      preflight: false,
      start: false,
      stopAfterCurrent: false,
      changeSelection: true,
    });
    expect(conversionControls(null, 2, true, true, false).preflight).toBe(true);
    expect(conversionControls(null, 2, true, false, false).preflight).toBe(
      false,
    );
    expect(
      conversionControls(snapshot("planned"), 2, true, true, false),
    ).toEqual({
      preflight: true,
      start: true,
      stopAfterCurrent: false,
      changeSelection: true,
    });
    expect(
      conversionControls(snapshot("planned", "failed"), 2, true, true, false)
        .start,
    ).toBe(false);
    expect(
      conversionControls(snapshot("running"), 2, true, true, false),
    ).toEqual({
      preflight: false,
      start: false,
      stopAfterCurrent: true,
      changeSelection: false,
    });
    expect(
      conversionControls(snapshot("stopping"), 2, true, true, false),
    ).toEqual({
      preflight: false,
      start: false,
      stopAfterCurrent: false,
      changeSelection: false,
    });
  });

  it("reports only actually ready files in a mixed preflight", () => {
    const mixed = snapshot("planned");
    mixed.items.push({
      ...mixed.items[0],
      sourceName: "bad.safetensors",
      status: "failed",
    });
    mixed.failed = 1;

    expect(conversionProgressLabel(mixed)).toBe(
      "1 of 2 files ready · 1 failed",
    );
  });

  it("counts cancelled items as terminal after stop-after-current", () => {
    const stopped = snapshot("stopped", "complete");
    stopped.items.push({
      ...stopped.items[0],
      sourceName: "queued.safetensors",
      status: "cancelled",
    });
    stopped.completed = 1;

    expect(conversionCancelledCount(stopped)).toBe(1);
    expect(conversionProgressLabel(stopped)).toBe(
      "Stopped · 2 / 2 settled · 1 cancelled",
    );
  });
});
