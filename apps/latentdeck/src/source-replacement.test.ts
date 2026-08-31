import { describe, expect, it } from "vitest";
import d2Faceplate from "./D2Faceplate.svelte?raw";
import q4Faceplate from "./Q4Faceplate.svelte?raw";
import {
  createExclusiveOperationGate,
  replaceDraftSource,
  retainDraftSourceOptions,
} from "./source-replacement";

describe("explicit captured-source replacement", () => {
  it("changes only the requested draft slot", () => {
    expect(replaceDraftSource(["a", "b"] as const, 0, "capture")).toEqual([
      "capture",
      "b",
    ]);
    expect(
      replaceDraftSource(["a", "b", "c", "d"] as const, 2, "capture"),
    ).toEqual(["a", "b", "capture", "d"]);
  });

  it("retains selected resolved sources outside the active collection", () => {
    const bank = [{ archiveSha256: "bank" }];
    const outside = { archiveSha256: "outside" };

    expect(
      retainDraftSourceOptions(bank, [outside, null], ["bank", "outside"]),
    ).toEqual([bank[0], outside]);
    expect(retainDraftSourceOptions(bank, [outside], ["bank"])).toEqual([
      bank[0],
    ]);
  });

  it("coalesces overlapping source replacement requests and always releases", async () => {
    const transitions: boolean[] = [];
    const gate = createExclusiveOperationGate((active) =>
      transitions.push(active),
    );
    let release!: () => void;
    let executions = 0;
    const first = gate.run(async () => {
      executions += 1;
      await new Promise<void>((resolve) => {
        release = resolve;
      });
    });
    await expect(gate.run(async () => void (executions += 1))).resolves.toBe(
      "ignored",
    );
    release();
    await expect(first).resolves.toBe("completed");
    await expect(gate.run(async () => void (executions += 1))).resolves.toBe(
      "completed",
    );

    expect(executions).toBe(2);
    expect(transitions).toEqual([true, false, true, false]);
  });

  it("offers an explicit per-slot capture action with a bounded restart notice", () => {
    expect(d2Faceplate).toContain(
      "async function useCapturedSource(slot: D2Slot)",
    );
    expect(d2Faceplate).toContain('useCapturedSource("A")');
    expect(d2Faceplate).toContain('useCapturedSource("B")');
    expect(d2Faceplate).toContain("Use capture in A");

    expect(q4Faceplate).toContain(
      "async function useCapturedSource(slot: Q4Slot)",
    );
    expect(q4Faceplate).toContain("{#each Q4_SLOTS as slot (slot)}");
    expect(q4Faceplate).toContain("useCapturedSource(slot)");
    expect(q4Faceplate).toContain("Use capture in {slot}");

    for (const faceplate of [d2Faceplate, q4Faceplate]) {
      expect(faceplate).toContain("bounded worker restart");
      expect(faceplate).toContain("causal state restarts");
      expect(faceplate).toContain("createExclusiveOperationGate");
      expect(faceplate).toContain("retainDraftSourceOptions");
      expect(faceplate).toContain("sourceReplaceBusy");
    }
  });
});
