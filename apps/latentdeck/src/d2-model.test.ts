import { describe, expect, it } from "vitest";
import type { CartridgeView } from "./library-model";
import {
  DEFAULT_D2_BACKEND,
  DEFAULT_D2_CAPTURE,
  DEFAULT_D2_CONTROLS,
  DEFAULT_D2_TRANSPORT,
  MAX_SAFE_D2_SEED,
  buildD2OpenRequest,
  chooseD2Sources,
  parseD2Seed,
  isD2CaptureActive,
  setSlotPlaying,
  setSlotLoop,
} from "./d2-model";

function cartridge(
  hashByte: string,
  availability: CartridgeView["availability"] = "present",
): CartridgeView {
  return {
    archiveSha256: hashByte.repeat(64),
    cartridgeId: `cartridge-${hashByte}`,
    codecFamily: "minimax_h3",
    codecProfile: "h3_av_latent",
    decodedWidth: 800,
    decodedHeight: 448,
    decodedFrameCount: 107,
    durationNumerator: 107,
    durationDenominator: 24,
    favorite: false,
    tags: [],
    availability,
    paths: [
      {
        path: `private-${hashByte}.lc`,
        fileName: `${hashByte}.lc`,
        state: availability === "present" ? "present" : "missing",
        warningCode: availability === "present" ? null : "library.path_missing",
      },
    ],
  };
}

describe("LD-D2 operator draft", () => {
  it("starts with an explicit missing Codec Pack state", () => {
    expect(DEFAULT_D2_BACKEND).toEqual({
      state: "missing",
      packId: null,
      packVersion: null,
      displayName: null,
      d2EntrypointAvailable: false,
      decoder: null,
      detail: "Install a compatible H3 Codec Pack.",
    });
  });

  it("matches the closed 0.1 control surface and keeps chaos zero deterministic", () => {
    expect(DEFAULT_D2_CONTROLS).toEqual({
      algorithm: "LINEAR",
      mix: 0.5,
      mode: "HYBRIDIZE",
      routing: "A",
      interaction: 0,
      preserve: 0.55,
      chaos: 0,
      xs1ChannelA: 0,
      xs1ChannelB: 1,
      xs1AngleDegrees: 30,
      xs2Radius: 1,
      xs3HighGain: 0.5,
      xs4Epsilon: 0.000001,
      xs5Routing: "TOPK",
      temperature: 0.12,
      topK: 8,
      sinkhornIterations: 5,
    });
    expect(DEFAULT_D2_CONTROLS.xs1ChannelA).not.toBe(
      DEFAULT_D2_CONTROLS.xs1ChannelB,
    );
  });

  it("builds an open request from immutable identities without leaking local paths", () => {
    const request = buildD2OpenRequest(
      cartridge("a"),
      cartridge("b"),
      DEFAULT_D2_CONTROLS,
      DEFAULT_D2_TRANSPORT,
      44,
    );
    expect(request.sourceA).toEqual({
      cartridgeId: "cartridge-a",
      archiveSha256: "a".repeat(64),
    });
    expect(request.sourceB).toEqual({
      cartridgeId: "cartridge-b",
      archiveSha256: "b".repeat(64),
    });
    expect(JSON.stringify(request)).not.toContain("private-");
  });

  it("chooses deterministic present sources from the active Bank only", () => {
    const choices = chooseD2Sources(
      [cartridge("a", "missing"), cartridge("b"), cartridge("c")],
      "",
      "",
    );
    expect(choices).toEqual({
      sourceAHash: "b".repeat(64),
      sourceBHash: "c".repeat(64),
    });
  });
});

describe("host-owned transport and seed inputs", () => {
  it("changes only the requested play or loop flag", () => {
    expect(setSlotPlaying(DEFAULT_D2_TRANSPORT, "B", false)).toEqual({
      playingA: true,
      playingB: false,
      loopA: true,
      loopB: true,
    });
    expect(setSlotLoop(DEFAULT_D2_TRANSPORT, "A", false)).toEqual({
      playingA: true,
      playingB: true,
      loopA: false,
      loopB: true,
    });
  });

  it("accepts only finite non-negative u53 integer seeds", () => {
    expect(parseD2Seed("0")).toBe(0);
    expect(parseD2Seed(String(MAX_SAFE_D2_SEED))).toBe(MAX_SAFE_D2_SEED);
    expect(parseD2Seed("1.5")).toBeNull();
    expect(parseD2Seed("-1")).toBeNull();
    expect(parseD2Seed(String(MAX_SAFE_D2_SEED + 1))).toBeNull();
  });
});

describe("path-free capture model", () => {
  it("has a bounded idle default and recognizes only active phases", () => {
    expect(DEFAULT_D2_CAPTURE).toEqual({
      captureId: null,
      mode: null,
      state: "idle",
      latentSlots: "0",
      targetLatentSlots: null,
      cartridgeId: null,
      archiveSha256: null,
      detail: null,
    });
    expect(isD2CaptureActive("awaiting_reset")).toBe(true);
    expect(isD2CaptureActive("capturing")).toBe(true);
    expect(isD2CaptureActive("stop_armed")).toBe(true);
    expect(isD2CaptureActive("finalizing")).toBe(true);
    expect(isD2CaptureActive("finished")).toBe(false);
    expect(JSON.stringify(DEFAULT_D2_CAPTURE)).not.toContain("path");
  });
});
