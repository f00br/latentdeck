export type DeckCaptureMode = "snapshot" | "live_capture" | null;
export type DeckCaptureState =
  | "idle"
  | "awaiting_reset"
  | "capturing"
  | "stop_armed"
  | "finalizing"
  | "finished"
  | "aborted"
  | "error";

export interface DeckCaptureUiPolicy {
  active: boolean;
  load: boolean;
  decoder: boolean;
  realtimeControls: boolean;
  transport: boolean;
  seed: boolean;
  roles: boolean;
}

export type DeckLiveCaptureAction = "start" | "stop" | null;

export interface DeckCaptureActionInputs {
  loaded: boolean;
  hostBusy: boolean;
  captureBusy: boolean;
  controlsDirty: boolean;
  controlsDispatchRunning: boolean;
  controlsDispatchPending: boolean;
  seedDirty: boolean;
  rolesDirty: boolean;
}

export interface DeckCaptureActions {
  snapshotEnabled: boolean;
  liveAction: DeckLiveCaptureAction;
}

/** The shared UI safety contract for Snapshot and bounded Live Capture. */
export function deckCaptureUiPolicy(
  mode: DeckCaptureMode,
  state: DeckCaptureState,
): DeckCaptureUiPolicy {
  const active =
    state === "awaiting_reset" ||
    state === "capturing" ||
    state === "stop_armed" ||
    state === "finalizing";
  if (!active) {
    return {
      active: false,
      load: true,
      decoder: true,
      realtimeControls: true,
      transport: true,
      seed: true,
      roles: true,
    };
  }
  const liveControls =
    mode === "live_capture" &&
    (state === "awaiting_reset" || state === "capturing");
  return {
    active: true,
    load: false,
    decoder: false,
    realtimeControls: liveControls,
    transport: false,
    seed: liveControls,
    roles: liveControls,
  };
}

/**
 * Resolve capture button availability from authoritative runtime/capture state
 * plus every unapplied Deck draft. Snapshot must never serialize a runtime
 * state different from the fixed values currently shown by the UI. Live
 * Capture starts from the same clean initial state, then deliberately permits
 * provenance-recorded changes. Stop waits for in-flight control/host work but
 * never traps the user merely because a draft remains dirty.
 */
export function deckCaptureActions(
  mode: DeckCaptureMode,
  state: DeckCaptureState,
  inputs: Readonly<DeckCaptureActionInputs>,
): DeckCaptureActions {
  const captureUi = deckCaptureUiPolicy(mode, state);
  const hostReady = inputs.loaded && !inputs.hostBusy && !inputs.captureBusy;
  const initialDraftClean =
    !inputs.controlsDirty &&
    !inputs.controlsDispatchRunning &&
    !inputs.controlsDispatchPending &&
    !inputs.seedDirty &&
    !inputs.rolesDirty;

  let liveAction: DeckLiveCaptureAction = null;
  if (hostReady) {
    if (
      mode === "live_capture" &&
      state === "capturing" &&
      !inputs.controlsDispatchRunning &&
      !inputs.controlsDispatchPending
    ) {
      liveAction = "stop";
    } else if (!captureUi.active && initialDraftClean) {
      liveAction = "start";
    }
  }

  return {
    snapshotEnabled: hostReady && !captureUi.active && initialDraftClean,
    liveAction,
  };
}
