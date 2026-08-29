import { describe, expect, it } from "vitest";
import {
  EMPTY_PLAYER_VIEW,
  acceptTrustedSnapshot,
  controlsFor,
  formatFrameRate,
  formatFramePosition,
  progressPercent,
  type PlayerView,
} from "./player-model";

const READY: PlayerView = {
  revision: 4,
  phase: "ready",
  cartridge: {
    cartridgeId: "019d0000-0000-7000-8000-000000000001",
    archiveSha256: "a".repeat(64),
    fileName: "synthetic.lc",
    width: 800,
    height: 448,
    frameCount: 107,
    frameRateNumerator: 24,
    frameRateDenominator: 1,
    audioPresent: true,
  },
  codec: { state: "ready", displayName: "H3", detail: null },
  positionFrame: 0,
  loopEnabled: false,
  outputAvailable: true,
  error: null,
};

describe("LatentPlayer presentation state", () => {
  it("never exposes playback controls without a ready codec", () => {
    expect(controlsFor(EMPTY_PLAYER_VIEW, false)).toEqual({
      open: true,
      configureCodec: false,
      play: false,
      pause: false,
      loop: false,
      restart: false,
      fullscreen: false,
    });
  });

  it("distinguishes play and pause without offering arbitrary seek", () => {
    expect(controlsFor(READY, false).play).toBe(true);
    expect(controlsFor(READY, false).pause).toBe(false);

    const playing = { ...READY, phase: "playing" as const };
    expect(controlsFor(playing, false).play).toBe(false);
    expect(controlsFor(playing, false).pause).toBe(true);
    expect(Object.keys(controlsFor(playing, false))).not.toContain("seek");
  });

  it("clamps read-only progress to the decoded frame range", () => {
    expect(progressPercent({ ...READY, positionFrame: 53 })).toBe(50);
    expect(progressPercent({ ...READY, positionFrame: 999 })).toBe(100);
    expect(formatFramePosition({ ...READY, positionFrame: 106 })).toBe(
      "107 / 107",
    );
  });

  it("disables every mutating control while a command is in flight", () => {
    expect(controlsFor(READY, true)).toEqual({
      open: false,
      configureCodec: false,
      play: false,
      pause: false,
      loop: false,
      restart: false,
      fullscreen: false,
    });
  });

  it("does not let a delayed trusted snapshot roll back live state", () => {
    const playing = {
      ...READY,
      revision: 12,
      phase: "playing" as const,
      positionFrame: 31,
    };
    const delayed = { ...READY, revision: 11, positionFrame: 29 };
    const advanced = { ...playing, revision: 13, positionFrame: 32 };

    expect(acceptTrustedSnapshot(playing, delayed)).toBe(playing);
    expect(acceptTrustedSnapshot(playing, advanced)).toBe(advanced);
  });

  it("formats the exact manifest frame-rate instead of assuming 24 fps", () => {
    expect(formatFrameRate(READY)).toBe("24 fps");
    expect(
      formatFrameRate({
        ...READY,
        cartridge: {
          ...READY.cartridge!,
          frameRateNumerator: 24_000,
          frameRateDenominator: 1_001,
        },
      }),
    ).toBe("23.976 fps");
  });
});
