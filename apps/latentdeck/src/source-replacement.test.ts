import { describe, expect, it } from "vitest";
import d2Faceplate from "./D2Faceplate.svelte?raw";
import q4Faceplate from "./Q4Faceplate.svelte?raw";
import { setSlotPlaying, type D2Transport } from "./d2-model";
import { setQ4SlotPlaying, type Q4Transport } from "./q4-model";
import {
  createExclusiveOperationGate,
  playDraftAwareSlot,
  replaceDraftSource,
  retainDraftSourceOptions,
  transportForDraftLoad,
} from "./source-replacement";

describe("explicit captured-source replacement", () => {
  it("loads a changed next-load draft before playing the slot", async () => {
    const actions: string[] = [];

    await expect(
      playDraftAwareSlot({
        loadedArchiveSha256: "currently-playing",
        draftArchiveSha256: "new-capture",
        loadDraftAndPlay: async () => void actions.push("load-and-play"),
        toggleCurrent: async () => void actions.push("toggle-current"),
      }),
    ).resolves.toBe("loaded_draft");
    expect(actions).toEqual(["load-and-play"]);
  });

  it("keeps Play and Pause as transport-only actions for the loaded source", async () => {
    const actions: string[] = [];

    await expect(
      playDraftAwareSlot({
        loadedArchiveSha256: "currently-playing",
        draftArchiveSha256: "currently-playing",
        loadDraftAndPlay: async () => void actions.push("load-and-play"),
        toggleCurrent: async () => void actions.push("toggle-current"),
      }),
    ).resolves.toBe("toggled_current");
    expect(actions).toEqual(["toggle-current"]);
  });

  it("starts only the requested slot while retaining the rest of D2 and Q4 transport", () => {
    const d2Transport: D2Transport = {
      playingA: false,
      playingB: true,
      loopA: false,
      loopB: true,
    };
    expect(transportForDraftLoad(d2Transport, "A", setSlotPlaying)).toEqual({
      playingA: true,
      playingB: true,
      loopA: false,
      loopB: true,
    });

    const q4Transport: Q4Transport = {
      playingA: true,
      playingB: false,
      playingC: false,
      playingD: true,
      loopA: false,
      loopB: true,
      loopC: false,
      loopD: true,
    };
    expect(transportForDraftLoad(q4Transport, "C", setQ4SlotPlaying)).toEqual({
      ...q4Transport,
      playingC: true,
    });
  });

  it("wires changed drafts to an explicit Load + Play action in both Decks", () => {
    for (const faceplate of [d2Faceplate, q4Faceplate]) {
      expect(faceplate).toContain("playDraftAwareSlot");
      expect(faceplate).toContain("transportForDraftLoad");
      expect(faceplate).toContain("Load + Play");
      expect(faceplate).toContain("openDeck(slot)");
    }
  });

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
