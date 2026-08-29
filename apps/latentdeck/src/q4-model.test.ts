import { describe, expect, it } from "vitest";
import type { CartridgeView } from "./library-model";
import {
  DEFAULT_Q4_CONTROLS,
  DEFAULT_Q4_ROLES,
  DEFAULT_Q4_TRANSPORT,
  buildQ4OpenRequest,
  chooseQ4Sources,
  q4LiveCaptureAction,
  resolveQ4DonorWeights,
  setQ4SlotLoop,
  setQ4SlotPlaying,
  validateQ4Roles,
} from "./q4-model";

function cartridge(index: number, availability: "present" | "missing" = "present"): CartridgeView {
  return {
    cartridgeId: `00000000-0000-4000-8000-${String(index).padStart(12, "0")}`,
    archiveSha256: String(index).padStart(64, "a"),
    codecFamily: "minimax_h3",
    codecProfile: "minimax_h3/h3_av_latent@0.1.0",
    decodedWidth: 800,
    decodedHeight: 448,
    decodedFrameCount: 107,
    durationNumerator: 107,
    durationDenominator: 24,
    favorite: false,
    tags: [],
    availability,
    paths: [],
  };
}

describe("Q4 source and role model", () => {
  it("selects four distinct present cartridges while preserving valid choices", () => {
    const cartridges = [cartridge(1), cartridge(2), cartridge(3), cartridge(4), cartridge(5, "missing")];
    const selection = chooseQ4Sources(cartridges, {
      sourceAHash: cartridges[2].archiveSha256,
      sourceBHash: cartridges[2].archiveSha256,
      sourceCHash: "missing",
      sourceDHash: cartridges[0].archiveSha256,
    });
    expect(selection.sourceAHash).toBe(cartridges[2].archiveSha256);
    expect(new Set(Object.values(selection))).toEqual(
      new Set(cartridges.slice(0, 4).map((value) => value.archiveSha256)),
    );
  });

  it("leaves unavailable slots empty instead of duplicating a cartridge", () => {
    const selection = chooseQ4Sources([cartridge(1), cartridge(2), cartridge(3)], {
      sourceAHash: "",
      sourceBHash: "",
      sourceCHash: "",
      sourceDHash: "",
    });
    expect(Object.values(selection).filter(Boolean)).toHaveLength(3);
    expect(selection.sourceDHash).toBe("");
  });

  it("requires an explicit full role permutation", () => {
    expect(validateQ4Roles(DEFAULT_Q4_ROLES)).toBe(true);
    expect(validateQ4Roles({ ...DEFAULT_Q4_ROLES, donorD: "A" })).toBe(false);
  });

  it("builds a closed open request only from four distinct present sources", () => {
    const sources = [cartridge(1), cartridge(2), cartridge(3), cartridge(4)] as const;
    const request = buildQ4OpenRequest(
      sources,
      DEFAULT_Q4_ROLES,
      DEFAULT_Q4_CONTROLS,
      DEFAULT_Q4_TRANSPORT,
      42,
    );
    expect(request.roles).toEqual(DEFAULT_Q4_ROLES);
    expect(request.seed).toBe(42);
    expect(request.sourceD.archiveSha256).toBe(sources[3].archiveSha256);

    expect(() =>
      buildQ4OpenRequest(
        [sources[0], sources[1], sources[2], sources[0]],
        DEFAULT_Q4_ROLES,
        DEFAULT_Q4_CONTROLS,
        DEFAULT_Q4_TRANSPORT,
        42,
      ),
    ).toThrow("four distinct");
  });
});

describe("Q4 influence and transport", () => {
  it("normalizes manual weights beneath the global interaction macro", () => {
    expect(
      resolveQ4DonorWeights({
        ...DEFAULT_Q4_CONTROLS,
        donorWeightB: 1,
        donorWeightC: 2,
        donorWeightD: 1,
      }),
    ).toEqual([0.25, 0.5, 0.25]);
  });

  it("maps the triangle to B/C/D barycentric weights", () => {
    expect(
      resolveQ4DonorWeights({
        ...DEFAULT_Q4_CONTROLS,
        influenceMode: "TRIANGLE",
        triangleX: 0.5,
        triangleY: 1,
      }),
    ).toEqual([0, 0, 1]);
    expect(() =>
      resolveQ4DonorWeights({
        ...DEFAULT_Q4_CONTROLS,
        influenceMode: "TRIANGLE",
        triangleX: 1,
        triangleY: 1,
      }),
    ).toThrow("inside");
  });

  it("changes only the selected physical slot transport flag", () => {
    expect(setQ4SlotPlaying(DEFAULT_Q4_TRANSPORT, "C", false)).toEqual({
      ...DEFAULT_Q4_TRANSPORT,
      playingC: false,
    });
    expect(setQ4SlotLoop(DEFAULT_Q4_TRANSPORT, "D", false)).toEqual({
      ...DEFAULT_Q4_TRANSPORT,
      loopD: false,
    });
  });

  it("permits exactly one safe live-capture action for each host state", () => {
    expect(q4LiveCaptureAction({ mode: null, state: "idle" })).toBe("start");
    expect(q4LiveCaptureAction({ mode: "live_capture", state: "awaiting_reset" })).toBeNull();
    expect(q4LiveCaptureAction({ mode: "live_capture", state: "capturing" })).toBe("stop");
    expect(q4LiveCaptureAction({ mode: "live_capture", state: "stop_armed" })).toBeNull();
    expect(q4LiveCaptureAction({ mode: "live_capture", state: "finalizing" })).toBeNull();
    expect(q4LiveCaptureAction({ mode: "snapshot", state: "finalizing" })).toBeNull();
    expect(q4LiveCaptureAction({ mode: "live_capture", state: "finished" })).toBe("start");
    expect(q4LiveCaptureAction({ mode: "live_capture", state: "error" })).toBe("start");
  });
});
