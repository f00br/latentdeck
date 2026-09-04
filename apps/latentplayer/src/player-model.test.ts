import { describe, expect, it } from "vitest";
import {
  EMPTY_PLAYER_VIEW,
  acceptTrustedSnapshot,
  buildNativeViewportBounds,
  controlsFor,
  describeAudioAvailability,
  describeDiagnosticSaveResult,
  describePlayerOperation,
  describeRuntimeStatus,
  diagnosticSaveEnabled,
  formatFrameRate,
  formatFramePosition,
  fullscreenActionLabel,
  hiddenNativeViewportBounds,
  nextNativeViewportRevision,
  progressPercent,
  selectDisplayedError,
  sameNativeViewportGeometry,
  spoutControlsFor,
  viewportRetryRequiresRemeasure,
  type DiagnosticSaveResult,
  type PlayerError,
  type PlayerView,
  type SpoutStatus,
} from "./player-model";

const SPOUT_READY: SpoutStatus = {
  sdkBuilt: true,
  ready: true,
  enabled: false,
  published: false,
  requestedName: "LatentPlayer Output",
  activeName: "LatentPlayer Output",
  width: 800,
  height: 448,
  format: "rgba8_unorm",
  submittedFrames: 0,
  lastSequence: null,
  spoutFrame: null,
  lastErrorCode: null,
};

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
  codec: {
    state: "ready",
    displayName: "H3",
    detail: null,
    packId: "org.latentdeck.h3",
    packVersion: "0.1.0",
    publisherName: "LatentDeck",
    publisherUrl: "https://github.com/f00br/latentdeck",
    packLicenseLabel: "Apache-2.0",
    decoderAssetId: "taeh3",
    decoderDisplayName: "TAEH3",
    decoderVariants: [
      {
        variantId: "taeh3-official",
        sha256: "b".repeat(64),
        byteLength: 1_024,
        sourceUrl: "https://example.invalid/taeh3",
        licenseLabel: "Apache-2.0",
        licenseUrl: "https://example.invalid/license",
        selected: true,
      },
    ],
  },
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

  it("allows retrying decoder selection after an incompatible weight", () => {
    const incompatible = {
      ...READY,
      codec: { ...READY.codec, state: "incompatible" as const },
    };

    expect(controlsFor(incompatible, false).configureCodec).toBe(true);
    expect(controlsFor(incompatible, false).play).toBe(false);
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

  it("does not claim that the lazy runtime is connected before native output exists", () => {
    expect(describeRuntimeStatus({ ...READY, outputAvailable: false })).toBe(
      "Ready to start playback",
    );
    expect(describeRuntimeStatus(READY)).toBe("Native output active");
  });

  it("enables Spout controls only after the real SDK opens on native output", () => {
    expect(spoutControlsFor(null, false)).toEqual({
      rename: false,
      toggle: false,
    });
    expect(
      spoutControlsFor({ ...SPOUT_READY, sdkBuilt: false }, false),
    ).toEqual({ rename: false, toggle: false });
    expect(spoutControlsFor(SPOUT_READY, false)).toEqual({
      rename: true,
      toggle: true,
    });
    expect(spoutControlsFor(SPOUT_READY, true)).toEqual({
      rename: false,
      toggle: false,
    });
  });

  it("keeps lifecycle-only diagnostics available without a cartridge or codec", () => {
    expect(diagnosticSaveEnabled(false)).toBe(true);
    expect(diagnosticSaveEnabled(true)).toBe(false);
    expect(controlsFor(EMPTY_PLAYER_VIEW, false).play).toBe(false);
  });

  it("describes saved and cancelled native diagnostic results without a path", () => {
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
    expect(describeDiagnosticSaveResult({ status: "cancelled" })).toContain(
      "cancelled",
    );
  });

  it("gives every long-running master-user action a visible operation label", () => {
    expect(describePlayerOperation("open")).toBe("Opening cartridge…");
    expect(describePlayerOperation("decoder")).toBe("Validating decoder…");
    expect(describePlayerOperation("play")).toBe("Starting playback…");
    expect(describePlayerOperation("restart")).toBe("Restarting decoder…");
  });

  it("states the v0.1 audio boundary for both AV and visual-only cartridges", () => {
    expect(describeAudioAvailability(READY)).toBe(
      "Audio payload preserved · playback unavailable in v0.1",
    );
    expect(
      describeAudioAvailability({
        ...READY,
        cartridge: { ...READY.cartridge!, audioPresent: false },
      }),
    ).toBe("Visual-only cartridge · no audio payload");
    expect(describeAudioAvailability(EMPTY_PLAYER_VIEW)).toBeNull();
  });

  it("shows a fresh command failure ahead of an older coordinator failure", () => {
    const persistent: PlayerError = {
      code: "worker.crashed",
      message: "The decoder worker stopped.",
      recoverable: true,
    };
    const transient: PlayerError = {
      code: "codec.asset_validation_failed",
      message: "The selected decoder is incompatible.",
      recoverable: true,
    };

    expect(selectDisplayedError(persistent, transient)).toBe(transient);
    expect(selectDisplayedError(persistent, null)).toBe(persistent);
  });

  it("labels fullscreen from confirmed native state", () => {
    expect(fullscreenActionLabel(null)).toBe("Fullscreen");
    expect(fullscreenActionLabel({ active: false })).toBe("Fullscreen");
    expect(fullscreenActionLabel({ active: true })).toBe("Exit fullscreen");
  });

  it("builds revisioned CSS viewport bounds without converting media pixels", () => {
    expect(
      buildNativeViewportBounds(
        3,
        7,
        { left: 12.5, top: 48, width: 611.25, height: 344 },
        1.25,
        true,
      ),
    ).toEqual({
      epoch: 3,
      revision: 7,
      xCss: 12.5,
      yCss: 48,
      widthCss: 611.25,
      heightCss: 344,
      scaleFactor: 1.25,
      visible: true,
    });
  });

  it("suspends a zero-sized native viewport and rejects unsafe measurements", () => {
    expect(
      buildNativeViewportBounds(
        1,
        8,
        { left: 0, top: 0, width: 0, height: 200 },
        1,
        true,
      ),
    ).toMatchObject({ epoch: 1, revision: 8, visible: false });
    expect(
      buildNativeViewportBounds(
        1,
        9,
        { left: -1, top: 0, width: 100, height: 100 },
        1,
        true,
      ),
    ).toBeNull();
    expect(
      buildNativeViewportBounds(
        1,
        10,
        { left: 0, top: 0, width: Number.NaN, height: 100 },
        1,
        true,
      ),
    ).toBeNull();
    expect(
      buildNativeViewportBounds(
        1,
        11,
        { left: 0, top: 0, width: 100, height: 100 },
        9,
        true,
      ),
    ).toBeNull();
  });

  it("coalesces unchanged viewport geometry while preserving revisions", () => {
    const first = buildNativeViewportBounds(
      1,
      12,
      { left: 10, top: 20, width: 600, height: 320 },
      1.5,
      true,
    )!;
    const unchanged = { ...first, revision: 13, xCss: 10.005 };
    const resized = { ...first, revision: 14, widthCss: 601 };

    expect(sameNativeViewportGeometry(first, unchanged)).toBe(true);
    expect(sameNativeViewportGeometry(first, resized)).toBe(false);
    expect(sameNativeViewportGeometry(first, { ...unchanged, epoch: 2 })).toBe(
      false,
    );
  });

  it("allocates client revisions inside one host-issued epoch", () => {
    expect(nextNativeViewportRevision(0)).toBe(1);
    expect(nextNativeViewportRevision(Number.MAX_SAFE_INTEGER)).toBeNull();
    expect(hiddenNativeViewportBounds(4, 1, 1.5)).toMatchObject({
      epoch: 4,
      revision: 1,
      visible: false,
      widthCss: 0,
    });
    expect(hiddenNativeViewportBounds(0, 1, 1.5)).toBeNull();
  });

  it("re-measures instead of replaying stale DPI geometry", () => {
    expect(viewportRetryRequiresRemeasure("output.viewport_scale_stale")).toBe(
      true,
    );
    expect(
      viewportRetryRequiresRemeasure("output.window_placement_failed"),
    ).toBe(false);
  });
});
