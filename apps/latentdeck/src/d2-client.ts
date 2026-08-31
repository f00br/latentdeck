import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type {
  D2BackendView,
  D2Controls,
  D2ControlsAck,
  D2ErrorEvent,
  D2OpenRequest,
  D2SeedAck,
  D2Status,
  D2Transport,
  D2TransportAck,
  D2CaptureView,
} from "./d2-model";
import type { SpoutConfigure, SpoutStatus } from "./output-model";
import type {
  DeckEmbeddedOutputHost,
  EmbeddedViewportSession,
} from "./embedded-viewport";

export type StopD2Listener = () => void;

export interface D2HostAdapter {
  invoke<T>(command: string, args?: Record<string, unknown>): Promise<T>;
  listen<T>(
    event: string,
    handler: (payload: T) => void,
  ): Promise<StopD2Listener>;
}

export interface D2Client extends DeckEmbeddedOutputHost {
  backendStatusGet(): Promise<D2BackendView>;
  backendRediscover(): Promise<D2BackendView>;
  selectDecoder(): Promise<D2BackendView>;
  open(request: D2OpenRequest): Promise<D2Status>;
  controlsSet(controls: D2Controls): Promise<D2ControlsAck>;
  transportSet(transport: D2Transport): Promise<D2TransportAck>;
  seedSet(seed: number): Promise<D2SeedAck>;
  restart(): Promise<D2Status>;
  captureSnapshot(): Promise<D2CaptureView | null>;
  captureLiveStart(): Promise<D2CaptureView | null>;
  captureLiveStop(): Promise<D2CaptureView>;
  captureStatusGet(): Promise<D2CaptureView>;
  statusGet(): Promise<D2Status>;
  spoutStatusGet(): Promise<SpoutStatus | null>;
  spoutConfigure(configure: SpoutConfigure): Promise<SpoutStatus>;
  onStatus(handler: (status: D2Status) => void): Promise<StopD2Listener>;
  onError(handler: (error: D2ErrorEvent) => void): Promise<StopD2Listener>;
  onCapture(handler: (capture: D2CaptureView) => void): Promise<StopD2Listener>;
  onCaptureError(
    handler: (error: D2ErrorEvent) => void,
  ): Promise<StopD2Listener>;
}

export const D2_COMMANDS = Object.freeze({
  backendStatusGet: "deck_d2_backend_status_get",
  backendRediscover: "deck_d2_backend_rediscover",
  selectDecoder: "deck_d2_select_decoder",
  open: "deck_d2_open",
  controlsSet: "deck_d2_controls_set",
  transportSet: "deck_d2_transport_set",
  seedSet: "deck_d2_seed_set",
  restart: "deck_d2_restart",
  viewportSessionBegin: "deck_d2_viewport_session_begin",
  viewportSetBounds: "deck_d2_viewport_set_bounds",
  fullscreenStatusGet: "deck_d2_fullscreen_status_get",
  fullscreenSet: "deck_d2_fullscreen_set",
  captureSnapshot: "deck_d2_capture_snapshot",
  captureLiveStart: "deck_d2_capture_live_start",
  captureLiveStop: "deck_d2_capture_live_stop",
  captureStatusGet: "deck_d2_capture_status_get",
  statusGet: "deck_d2_status_get",
  spoutStatusGet: "deck_d2_spout_status_get",
  spoutConfigure: "deck_d2_spout_configure",
});

export const D2_EVENTS = Object.freeze({
  status: "deck-d2-status",
  error: "deck-d2-error",
  capture: "deck-d2-capture",
  captureError: "deck-d2-capture-error",
});

const tauriHost: D2HostAdapter = {
  invoke: <T>(command: string, args: Record<string, unknown> = {}) =>
    invoke<T>(command, args),
  listen: <T>(event: string, handler: (payload: T) => void) =>
    listen<T>(event, ({ payload }) => handler(payload)),
};

export function createD2Client(host: D2HostAdapter = tauriHost): D2Client {
  return {
    backendStatusGet: () =>
      host.invoke<D2BackendView>(D2_COMMANDS.backendStatusGet, {}),
    backendRediscover: () =>
      host.invoke<D2BackendView>(D2_COMMANDS.backendRediscover, {}),
    selectDecoder: () =>
      host.invoke<D2BackendView>(D2_COMMANDS.selectDecoder, {}),
    open: (request) => host.invoke<D2Status>(D2_COMMANDS.open, { ...request }),
    controlsSet: (controls) =>
      host.invoke<D2ControlsAck>(D2_COMMANDS.controlsSet, { controls }),
    transportSet: (transport) =>
      host.invoke<D2TransportAck>(D2_COMMANDS.transportSet, { transport }),
    seedSet: (seed) => host.invoke<D2SeedAck>(D2_COMMANDS.seedSet, { seed }),
    restart: () => host.invoke<D2Status>(D2_COMMANDS.restart, {}),
    viewportSessionBegin: () =>
      host.invoke<EmbeddedViewportSession>(
        D2_COMMANDS.viewportSessionBegin,
        {},
      ),
    viewportSetBounds: (bounds) =>
      host.invoke<void>(D2_COMMANDS.viewportSetBounds, { bounds }),
    fullscreenStatusGet: () =>
      host.invoke<boolean | null>(D2_COMMANDS.fullscreenStatusGet, {}),
    fullscreenSet: (enabled) =>
      host.invoke<boolean>(D2_COMMANDS.fullscreenSet, { enabled }),
    captureSnapshot: () =>
      host.invoke<D2CaptureView | null>(D2_COMMANDS.captureSnapshot, {}),
    captureLiveStart: () =>
      host.invoke<D2CaptureView | null>(D2_COMMANDS.captureLiveStart, {}),
    captureLiveStop: () =>
      host.invoke<D2CaptureView>(D2_COMMANDS.captureLiveStop, {}),
    captureStatusGet: () =>
      host.invoke<D2CaptureView>(D2_COMMANDS.captureStatusGet, {}),
    statusGet: () => host.invoke<D2Status>(D2_COMMANDS.statusGet, {}),
    spoutStatusGet: () =>
      host.invoke<SpoutStatus | null>(D2_COMMANDS.spoutStatusGet, {}),
    spoutConfigure: ({ name, enabled }) =>
      host.invoke<SpoutStatus>(D2_COMMANDS.spoutConfigure, { name, enabled }),
    onStatus: (handler) => host.listen<D2Status>(D2_EVENTS.status, handler),
    onError: (handler) => host.listen<D2ErrorEvent>(D2_EVENTS.error, handler),
    onCapture: (handler) =>
      host.listen<D2CaptureView>(D2_EVENTS.capture, handler),
    onCaptureError: (handler) =>
      host.listen<D2ErrorEvent>(D2_EVENTS.captureError, handler),
  };
}

export interface D2DecoderSelectionResult {
  backend: D2BackendView;
  status: D2Status;
}

/**
 * The native picker intentionally returns the current backend on cancel. Read
 * the backend-owned runtime status afterwards so cancel preserves a loaded
 * session while a real decoder replacement reflects its required unload.
 */
export async function selectD2DecoderAndStatus(
  client: Pick<D2Client, "selectDecoder" | "statusGet">,
): Promise<D2DecoderSelectionResult> {
  const backend = await client.selectDecoder();
  const status = await client.statusGet();
  return { backend, status };
}

export const d2Client = createD2Client();
