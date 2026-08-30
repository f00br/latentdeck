import { describe, expect, it } from "vitest";
import {
  deckCaptureActions,
  deckCaptureUiPolicy,
  type DeckCaptureActionInputs,
} from "./capture-policy";

const CLEAN_ACTION_INPUTS: DeckCaptureActionInputs = {
  loaded: true,
  hostBusy: false,
  captureBusy: false,
  controlsDirty: false,
  controlsDispatchRunning: false,
  controlsDispatchPending: false,
  seedDirty: false,
  rolesDirty: false,
};

describe("Deck capture UI policy", () => {
  it("leaves the Deck editable while capture is inactive", () => {
    expect(deckCaptureUiPolicy(null, "idle")).toMatchObject({
      active: false,
      load: true,
      decoder: true,
      realtimeControls: true,
      transport: true,
    });
  });

  it("locks controls and transport for a fixed Snapshot", () => {
    expect(deckCaptureUiPolicy("snapshot", "capturing")).toEqual({
      active: true,
      load: false,
      decoder: false,
      realtimeControls: false,
      transport: false,
      seed: false,
      roles: false,
    });
  });

  it("allows only provenance-recorded state controls during Live Capture", () => {
    expect(deckCaptureUiPolicy("live_capture", "capturing")).toEqual({
      active: true,
      load: false,
      decoder: false,
      realtimeControls: true,
      transport: false,
      seed: true,
      roles: true,
    });
  });

  it("locks everything once Live Capture is stopping or finalizing", () => {
    expect(
      deckCaptureUiPolicy("live_capture", "stop_armed").realtimeControls,
    ).toBe(false);
    expect(
      deckCaptureUiPolicy("live_capture", "finalizing").realtimeControls,
    ).toBe(false);
  });

  it("enables Snapshot only when every visible runtime draft is acknowledged", () => {
    expect(
      deckCaptureActions(null, "idle", CLEAN_ACTION_INPUTS).snapshotEnabled,
    ).toBe(true);

    for (const blocked of [
      "hostBusy",
      "captureBusy",
      "controlsDirty",
      "controlsDispatchRunning",
      "controlsDispatchPending",
      "seedDirty",
      "rolesDirty",
    ] as const) {
      expect(
        deckCaptureActions(null, "idle", {
          ...CLEAN_ACTION_INPUTS,
          [blocked]: true,
        }).snapshotEnabled,
        blocked,
      ).toBe(false);
    }
  });

  it("prevents capture when no Deck runtime is loaded", () => {
    expect(
      deckCaptureActions(null, "idle", {
        ...CLEAN_ACTION_INPUTS,
        loaded: false,
      }),
    ).toEqual({ snapshotEnabled: false, liveAction: null });
  });

  it("requires a clean initial Live state, then stops despite a remaining dirty draft", () => {
    const dirtyRecordedState = {
      ...CLEAN_ACTION_INPUTS,
      controlsDirty: true,
      seedDirty: true,
      rolesDirty: true,
    };
    expect(
      deckCaptureActions(null, "idle", dirtyRecordedState).liveAction,
    ).toBeNull();
    expect(
      deckCaptureActions(null, "idle", CLEAN_ACTION_INPUTS).liveAction,
    ).toBe("start");
    expect(
      deckCaptureActions("live_capture", "capturing", dirtyRecordedState)
        .liveAction,
    ).toBe("stop");
    expect(
      deckCaptureActions("live_capture", "capturing", {
        ...dirtyRecordedState,
        hostBusy: true,
      }).liveAction,
    ).toBeNull();
    expect(
      deckCaptureActions("live_capture", "capturing", {
        ...dirtyRecordedState,
        controlsDispatchPending: true,
      }).liveAction,
    ).toBeNull();
    expect(
      deckCaptureActions("live_capture", "capturing", {
        ...dirtyRecordedState,
        controlsDispatchRunning: true,
      }).liveAction,
    ).toBeNull();
  });

  it("offers no second capture action while a boundary/reset/finalize is active", () => {
    for (const state of [
      "awaiting_reset",
      "stop_armed",
      "finalizing",
    ] as const) {
      expect(
        deckCaptureActions("live_capture", state, CLEAN_ACTION_INPUTS),
      ).toEqual({ snapshotEnabled: false, liveAction: null });
    }
  });
});
