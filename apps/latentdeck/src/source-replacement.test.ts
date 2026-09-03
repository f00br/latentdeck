import { describe, expect, it } from "vitest";
import rendererSource from "./DeckFaceplateRenderer.svelte?raw";
import workspaceSource from "./GenericDeckWorkspace.svelte?raw";
import { setSlotPlaying, type D2Transport } from "./d2-model";
import { setQ4SlotPlaying, type Q4Transport } from "./q4-model";
import {
  createExclusiveOperationGate,
  playDraftAwareSlot,
  replaceDraftSource,
  retainDraftSourceOptions,
  selectedSourceAspectWarning,
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

  it("keeps generic source edits as a next-load draft until exact Load", () => {
    expect(rendererSource).toContain(
      "draft.sourceArchiveSha256s[slotIndex] = archiveSha256",
    );
    expect(rendererSource).toContain("Load exact Deck draft");
    expect(workspaceSource).toContain("buildGenericDeckOpenDraft(");
    expect(workspaceSource).toContain("genericDeckClient.open({");
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

  it("describes an exact cross-slot aspect mismatch without proposing hidden conversion", () => {
    const sources = [
      {
        archiveSha256: "a",
        decodedWidth: 448,
        decodedHeight: 800,
        signalPresentation: { aspect_ratio: { width: 9, height: 16 } },
      },
      {
        archiveSha256: "b",
        decodedWidth: 1344,
        decodedHeight: 768,
        signalPresentation: { aspect_ratio: { width: 16, height: 9 } },
      },
    ];

    expect(selectedSourceAspectWarning(["a", "b"], sources)).toBe(
      "Aspect mismatch · A 9:16 (448×800) · B 16:9 (1344×768). No hidden resize or crop; align sources in the Toolkit first.",
    );
    expect(selectedSourceAspectWarning(["a", "a"], sources)).toBe("");
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

  it("publishes completed captures back into the generic Library source bank", () => {
    expect(workspaceSource).toContain("publishCompletedCapture(capture)");
    expect(workspaceSource).toContain('"library_resolve_preset_sources"');
    expect(workspaceSource).toContain("acceptLibrarySnapshot(incoming");
    expect(workspaceSource).toContain("genericDeckClient.replaceSources(");
    expect(rendererSource).toContain("{#each sourceOptions as option");
    expect(rendererSource).toContain("Use capture in {slotLabel(slotIndex)}");
  });
});
