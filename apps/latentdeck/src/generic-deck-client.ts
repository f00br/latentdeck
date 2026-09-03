import { invoke } from "@tauri-apps/api/core";

import type {
  EmbeddedViewportBounds,
  EmbeddedViewportSession,
} from "./embedded-viewport";
import type { ExtensionCompatibilityReason } from "./extension-manager-model";
import type {
  GenericControlBinding,
  GenericDeckSourceIdentity,
  GenericRoleBinding,
  GenericSourceTransportBinding,
} from "./generic-deck-model";
import type { SpoutConfigure, SpoutStatus } from "./output-model";
import type { DecodedRecordingState } from "./recording-model";

export type GenericDevice = "cpu" | "cuda";
export type GenericCaptureMode = "snapshot" | "live_capture";
export type GenericProtocolCaptureState =
  | "idle"
  | "starting"
  | "capturing"
  | "finalizing"
  | "completed"
  | "aborted"
  | "faulted";
export type GenericCaptureState =
  | "idle"
  | "starting"
  | "capturing"
  | "finalizing"
  | "finished"
  | "aborted"
  | "error";

export interface GenericProfileKey {
  codecFamily: string;
  profile: string;
  profileVersion: string;
}

export interface GenericExactPackage {
  packageId: string;
  packageVersion: string;
}

export interface GenericRuntimeOptionsRequest {
  deckId: string;
  deckVersion: string;
  codecId: string;
  codecVersion: string;
  profileKey: GenericProfileKey | null;
  device: GenericDevice;
  deviceOrdinal: number;
  sources: GenericDeckSourceIdentity[];
}

export interface GenericRuntimeExternalAsset {
  assetId: string;
  displayName: string;
  requiredSha256: string;
  byteLength: number;
  required: boolean;
  bound: boolean;
  boundSha256: string | null;
}

export interface GenericRuntimeSourceOption extends GenericDeckSourceIdentity {
  reason: ExtensionCompatibilityReason;
}

export interface GenericRuntimeOptions {
  deck: GenericExactPackage;
  codec: GenericExactPackage;
  reason: ExtensionCompatibilityReason;
  profiles: GenericProfileKey[];
  device: GenericDevice;
  slots: number;
  externalAssets: GenericRuntimeExternalAsset[];
  sources: GenericRuntimeSourceOption[];
}

export interface GenericDeckOpenRequest extends GenericRuntimeOptionsRequest {
  profileKey: GenericProfileKey;
  roles: GenericRoleBinding[];
  controls: GenericControlBinding[];
  sourceTransport: GenericSourceTransportBinding[];
  seed: number;
}

export interface GenericPlayheadSnapshot {
  physical_slot: number;
  latent_slot: number;
  loop_enabled: boolean;
  end_of_stream: boolean;
}

export interface GenericDeckStatusSnapshot {
  deck_session_id: string;
  state:
    | "empty"
    | "loading"
    | "ready"
    | "playing"
    | "paused"
    | "capturing"
    | "faulted";
  deck_revision: number;
  stream_generation: number;
  stream_sequence: number;
  playheads: GenericPlayheadSnapshot[];
  roles: GenericRoleBinding[];
  controls: GenericControlBinding[];
  source_transport: GenericSourceTransportBinding[];
  seed: number;
  capture_state: GenericProtocolCaptureState;
}

export interface GenericDeckRuntimeView {
  status: GenericDeckStatusSnapshot;
  outputVisible: boolean;
  faultCode: string | null;
}

export interface GenericDeckSessionView {
  sessionId: string;
  workerId: string;
  deck: GenericExactPackage;
  codec: GenericExactPackage;
  profileKey: GenericProfileKey;
  device: GenericDevice;
  deviceOrdinal: number;
  externalAssets: GenericSessionExternalAssetReceipt[];
  sources: GenericDeckSourceIdentity[];
  runtime: GenericDeckRuntimeView;
  foreground: boolean;
}

export interface GenericSessionExternalAssetReceipt {
  assetId: string;
  sha256: string;
  byteLength: number;
}

export interface GenericForegroundLease {
  sessionId: string;
  generation: number;
}

export interface GenericOutputPin {
  session_id: string;
  lease_generation: number;
  pin_generation: number;
  kind: "capture" | "mp4";
}

export interface GenericDeckFault {
  sessionId: string;
  workerId: string;
  code: string;
}

export interface GenericDeckSessionsView {
  sessions: GenericDeckSessionView[];
  foregroundOutput: GenericForegroundLease | null;
  outputPin: GenericOutputPin | null;
  recentFaults: GenericDeckFault[];
}

export interface GenericExternalAssetView {
  codecId: string;
  codecVersion: string;
  assetId: string;
  bound: boolean;
  sha256: string | null;
  byteLength: number | null;
}

export interface GenericCaptureView {
  sessionId: string;
  captureId: string | null;
  mode: GenericCaptureMode | null;
  state: GenericCaptureState;
  latentSlots: string;
  resetEvents: number;
  cartridgeId: string | null;
  archiveSha256: string | null;
  detail: string | null;
}

export interface GenericRecordingView {
  sessionId: string;
  state: DecodedRecordingState;
  framesAccepted: number;
  framesWritten: number;
  width: number | null;
  height: number | null;
  errorCode: string | null;
}

export const GENERIC_DECK_COMMANDS = Object.freeze({
  runtimeOptions: "deck_generic_runtime_options",
  externalAssetSelect: "deck_generic_external_asset_select",
  externalAssetClear: "deck_generic_external_asset_clear",
  open: "deck_generic_open",
  sessionsGet: "deck_generic_sessions_get",
  statusGet: "deck_generic_status_get",
  processOnce: "deck_generic_process_once",
  controlsSet: "deck_generic_controls_set",
  rolesSet: "deck_generic_roles_set",
  transportSet: "deck_generic_transport_set",
  seedSet: "deck_generic_seed_set",
  reset: "deck_generic_reset",
  foregroundSet: "deck_generic_foreground_set",
  foregroundClear: "deck_generic_foreground_clear",
  close: "deck_generic_close",
  viewportSessionBegin: "deck_generic_viewport_session_begin",
  viewportSetBounds: "deck_generic_viewport_set_bounds",
  fullscreenStatusGet: "deck_generic_fullscreen_status_get",
  fullscreenSet: "deck_generic_fullscreen_set",
  spoutStatusGet: "deck_generic_spout_status_get",
  spoutConfigure: "deck_generic_spout_configure",
  captureStart: "deck_generic_capture_start",
  captureStop: "deck_generic_capture_stop",
  captureStatusGet: "deck_generic_capture_status_get",
  recordingStart: "deck_generic_recording_start",
  recordingStop: "deck_generic_recording_stop",
  recordingStatusGet: "deck_generic_recording_status_get",
});

export interface GenericDeckHostAdapter {
  invoke<T>(command: string, args?: Record<string, unknown>): Promise<T>;
}

export interface GenericDeckClient {
  runtimeOptions(
    request: GenericRuntimeOptionsRequest,
  ): Promise<GenericRuntimeOptions>;
  externalAssetSelect(
    codecId: string,
    codecVersion: string,
    assetId: string,
  ): Promise<GenericExternalAssetView | null>;
  externalAssetClear(
    codecId: string,
    codecVersion: string,
    assetId: string,
  ): Promise<GenericExternalAssetView>;
  open(request: GenericDeckOpenRequest): Promise<GenericDeckSessionView>;
  sessionsGet(): Promise<GenericDeckSessionsView>;
  statusGet(sessionId: string): Promise<GenericDeckSessionView>;
  processOnce(sessionId: string): Promise<GenericDeckRuntimeView>;
  controlsSet(
    sessionId: string,
    controls: GenericControlBinding[],
  ): Promise<GenericDeckRuntimeView>;
  rolesSet(
    sessionId: string,
    roles: GenericRoleBinding[],
  ): Promise<GenericDeckRuntimeView>;
  transportSet(
    sessionId: string,
    sourceTransport: GenericSourceTransportBinding[],
  ): Promise<GenericDeckRuntimeView>;
  seedSet(sessionId: string, seed: number): Promise<GenericDeckRuntimeView>;
  reset(
    sessionId: string,
    preservePlayheads: boolean,
  ): Promise<GenericDeckRuntimeView>;
  foregroundSet(sessionId: string): Promise<GenericDeckSessionsView>;
  foregroundClear(): Promise<GenericDeckSessionsView>;
  close(sessionId: string): Promise<void>;
  viewportSessionBegin(): Promise<EmbeddedViewportSession>;
  viewportSetBounds(bounds: EmbeddedViewportBounds): Promise<void>;
  fullscreenStatusGet(sessionId: string): Promise<boolean | null>;
  fullscreenSet(sessionId: string, enabled: boolean): Promise<boolean>;
  spoutStatusGet(sessionId: string): Promise<SpoutStatus | null>;
  spoutConfigure(
    sessionId: string,
    configure: SpoutConfigure,
  ): Promise<SpoutStatus>;
  captureStart(
    sessionId: string,
    mode: GenericCaptureMode,
  ): Promise<GenericCaptureView | null>;
  captureStop(sessionId: string): Promise<GenericCaptureView>;
  captureStatusGet(sessionId: string): Promise<GenericCaptureView>;
  recordingStart(sessionId: string): Promise<GenericRecordingView | null>;
  recordingStop(sessionId: string): Promise<GenericRecordingView>;
  recordingStatusGet(sessionId: string): Promise<GenericRecordingView>;
}

const tauriHost: GenericDeckHostAdapter = {
  invoke: <T>(command: string, args: Record<string, unknown> = {}) =>
    invoke<T>(command, args),
};

export function createGenericDeckClient(
  host: GenericDeckHostAdapter = tauriHost,
): GenericDeckClient {
  const session = <T>(command: string, sessionId: string) =>
    host.invoke<T>(command, { sessionId });
  return {
    runtimeOptions: (request) =>
      host.invoke(GENERIC_DECK_COMMANDS.runtimeOptions, { request }),
    externalAssetSelect: (codecId, codecVersion, assetId) =>
      host.invoke(GENERIC_DECK_COMMANDS.externalAssetSelect, {
        codecId,
        codecVersion,
        assetId,
      }),
    externalAssetClear: (codecId, codecVersion, assetId) =>
      host.invoke(GENERIC_DECK_COMMANDS.externalAssetClear, {
        codecId,
        codecVersion,
        assetId,
      }),
    open: (request) => host.invoke(GENERIC_DECK_COMMANDS.open, { request }),
    sessionsGet: () => host.invoke(GENERIC_DECK_COMMANDS.sessionsGet, {}),
    statusGet: (sessionId) =>
      session(GENERIC_DECK_COMMANDS.statusGet, sessionId),
    processOnce: (sessionId) =>
      session(GENERIC_DECK_COMMANDS.processOnce, sessionId),
    controlsSet: (sessionId, controls) =>
      host.invoke(GENERIC_DECK_COMMANDS.controlsSet, { sessionId, controls }),
    rolesSet: (sessionId, roles) =>
      host.invoke(GENERIC_DECK_COMMANDS.rolesSet, { sessionId, roles }),
    transportSet: (sessionId, sourceTransport) =>
      host.invoke(GENERIC_DECK_COMMANDS.transportSet, {
        sessionId,
        sourceTransport,
      }),
    seedSet: (sessionId, seed) =>
      host.invoke(GENERIC_DECK_COMMANDS.seedSet, { sessionId, seed }),
    reset: (sessionId, preservePlayheads) =>
      host.invoke(GENERIC_DECK_COMMANDS.reset, {
        sessionId,
        preservePlayheads,
      }),
    foregroundSet: (sessionId) =>
      session(GENERIC_DECK_COMMANDS.foregroundSet, sessionId),
    foregroundClear: () =>
      host.invoke(GENERIC_DECK_COMMANDS.foregroundClear, {}),
    close: (sessionId) => session(GENERIC_DECK_COMMANDS.close, sessionId),
    viewportSessionBegin: () =>
      host.invoke(GENERIC_DECK_COMMANDS.viewportSessionBegin, {}),
    viewportSetBounds: (bounds) =>
      host.invoke(GENERIC_DECK_COMMANDS.viewportSetBounds, { bounds }),
    fullscreenStatusGet: (sessionId) =>
      session(GENERIC_DECK_COMMANDS.fullscreenStatusGet, sessionId),
    fullscreenSet: (sessionId, enabled) =>
      host.invoke(GENERIC_DECK_COMMANDS.fullscreenSet, {
        sessionId,
        enabled,
      }),
    spoutStatusGet: (sessionId) =>
      session(GENERIC_DECK_COMMANDS.spoutStatusGet, sessionId),
    spoutConfigure: (sessionId, { name, enabled }) =>
      host.invoke(GENERIC_DECK_COMMANDS.spoutConfigure, {
        sessionId,
        name,
        enabled,
      }),
    captureStart: async (sessionId, mode) => {
      const value = await host.invoke<unknown>(
        GENERIC_DECK_COMMANDS.captureStart,
        {
          sessionId,
          mode,
        },
      );
      return value === null ? null : parseGenericCaptureView(value);
    },
    captureStop: async (sessionId) =>
      parseGenericCaptureView(
        await session<unknown>(GENERIC_DECK_COMMANDS.captureStop, sessionId),
      ),
    captureStatusGet: async (sessionId) =>
      parseGenericCaptureView(
        await session<unknown>(
          GENERIC_DECK_COMMANDS.captureStatusGet,
          sessionId,
        ),
      ),
    recordingStart: async (sessionId) => {
      const value = await session<unknown>(
        GENERIC_DECK_COMMANDS.recordingStart,
        sessionId,
      );
      return value === null ? null : parseGenericRecordingView(value);
    },
    recordingStop: async (sessionId) =>
      parseGenericRecordingView(
        await session<unknown>(GENERIC_DECK_COMMANDS.recordingStop, sessionId),
      ),
    recordingStatusGet: async (sessionId) =>
      parseGenericRecordingView(
        await session<unknown>(
          GENERIC_DECK_COMMANDS.recordingStatusGet,
          sessionId,
        ),
      ),
  };
}

export const genericDeckClient = createGenericDeckClient();

const GENERIC_CAPTURE_STATES = new Set<GenericCaptureState>([
  "idle",
  "starting",
  "capturing",
  "finalizing",
  "finished",
  "aborted",
  "error",
]);

/** Strict boundary for the path-free host capture wrapper. */
export function parseGenericCaptureView(value: unknown): GenericCaptureView {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    throw new Error("Generic capture status is not an object.");
  }
  const capture = value as Record<string, unknown>;
  const exactKeys = [
    "sessionId",
    "captureId",
    "mode",
    "state",
    "latentSlots",
    "resetEvents",
    "cartridgeId",
    "archiveSha256",
    "detail",
  ];
  if (
    Object.keys(capture).length !== exactKeys.length ||
    Object.keys(capture).some((key) => !exactKeys.includes(key)) ||
    typeof capture.sessionId !== "string" ||
    (capture.captureId !== null && typeof capture.captureId !== "string") ||
    (capture.mode !== null &&
      capture.mode !== "snapshot" &&
      capture.mode !== "live_capture") ||
    typeof capture.state !== "string" ||
    !GENERIC_CAPTURE_STATES.has(capture.state as GenericCaptureState) ||
    typeof capture.latentSlots !== "string" ||
    !/^(?:0|[1-9][0-9]*)$/.test(capture.latentSlots) ||
    !Number.isSafeInteger(capture.resetEvents) ||
    (capture.resetEvents as number) < 0 ||
    (capture.cartridgeId !== null && typeof capture.cartridgeId !== "string") ||
    (capture.archiveSha256 !== null &&
      typeof capture.archiveSha256 !== "string") ||
    (capture.detail !== null && typeof capture.detail !== "string")
  ) {
    throw new Error("Generic capture status violates the exact wire contract.");
  }
  return capture as unknown as GenericCaptureView;
}

const RECORDING_STATES = new Set<DecodedRecordingState>([
  "idle",
  "armed",
  "recording",
  "finalizing",
  "finished",
  "cancelled",
  "failed",
]);

/** Strict boundary for bounded, path-free decoded MP4 status. */
export function parseGenericRecordingView(
  value: unknown,
): GenericRecordingView {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    throw new Error("Generic recording status is not an object.");
  }
  const recording = value as Record<string, unknown>;
  const exactKeys = [
    "sessionId",
    "state",
    "framesAccepted",
    "framesWritten",
    "width",
    "height",
    "errorCode",
  ];
  if (
    Object.keys(recording).length !== exactKeys.length ||
    Object.keys(recording).some((key) => !exactKeys.includes(key)) ||
    typeof recording.sessionId !== "string" ||
    typeof recording.state !== "string" ||
    !RECORDING_STATES.has(recording.state as DecodedRecordingState) ||
    !boundedCounter(recording.framesAccepted) ||
    !boundedCounter(recording.framesWritten) ||
    !nullablePositiveInteger(recording.width) ||
    !nullablePositiveInteger(recording.height) ||
    (recording.errorCode !== null && typeof recording.errorCode !== "string")
  ) {
    throw new Error(
      "Generic recording status violates the exact wire contract.",
    );
  }
  return recording as unknown as GenericRecordingView;
}

function boundedCounter(value: unknown): value is number {
  return Number.isSafeInteger(value) && (value as number) >= 0;
}

function nullablePositiveInteger(value: unknown): value is number | null {
  return (
    value === null || (Number.isSafeInteger(value) && (value as number) > 0)
  );
}
