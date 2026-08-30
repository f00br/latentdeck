import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type {
  Q4BackendView,
  Q4CaptureView,
  Q4Controls,
  Q4ControlsAck,
  Q4ErrorEvent,
  Q4OpenRequest,
  Q4Roles,
  Q4RolesAck,
  Q4SeedAck,
  Q4Status,
  Q4Transport,
  Q4TransportAck,
} from "./q4-model";
import type { SpoutConfigure, SpoutStatus } from "./output-model";

export type StopQ4Listener = () => void;

export interface Q4HostAdapter {
  invoke<T>(command: string, args?: Record<string, unknown>): Promise<T>;
  listen<T>(
    event: string,
    handler: (payload: T) => void,
  ): Promise<StopQ4Listener>;
}

export interface Q4Client {
  backendStatusGet(): Promise<Q4BackendView>;
  selectDecoder(): Promise<Q4BackendView>;
  open(request: Q4OpenRequest): Promise<Q4Status>;
  controlsSet(controls: Q4Controls): Promise<Q4ControlsAck>;
  rolesSet(roles: Q4Roles): Promise<Q4RolesAck>;
  transportSet(transport: Q4Transport): Promise<Q4TransportAck>;
  seedSet(seed: number): Promise<Q4SeedAck>;
  restart(): Promise<Q4Status>;
  captureSnapshot(): Promise<Q4CaptureView | null>;
  captureLiveStart(): Promise<Q4CaptureView | null>;
  captureLiveStop(): Promise<Q4CaptureView>;
  captureStatusGet(): Promise<Q4CaptureView>;
  statusGet(): Promise<Q4Status>;
  spoutStatusGet(): Promise<SpoutStatus | null>;
  spoutConfigure(configure: SpoutConfigure): Promise<SpoutStatus>;
  onStatus(handler: (status: Q4Status) => void): Promise<StopQ4Listener>;
  onError(handler: (error: Q4ErrorEvent) => void): Promise<StopQ4Listener>;
  onCapture(handler: (capture: Q4CaptureView) => void): Promise<StopQ4Listener>;
  onCaptureError(handler: (error: Q4ErrorEvent) => void): Promise<StopQ4Listener>;
}

export const Q4_COMMANDS = Object.freeze({
  backendStatusGet: "deck_q4_backend_status_get",
  selectDecoder: "deck_q4_select_decoder",
  open: "deck_q4_open",
  controlsSet: "deck_q4_controls_set",
  rolesSet: "deck_q4_roles_set",
  transportSet: "deck_q4_transport_set",
  seedSet: "deck_q4_seed_set",
  restart: "deck_q4_restart",
  captureSnapshot: "deck_q4_capture_snapshot",
  captureLiveStart: "deck_q4_capture_live_start",
  captureLiveStop: "deck_q4_capture_live_stop",
  captureStatusGet: "deck_q4_capture_status_get",
  statusGet: "deck_q4_status_get",
  spoutStatusGet: "deck_q4_spout_status_get",
  spoutConfigure: "deck_q4_spout_configure",
});

export const Q4_EVENTS = Object.freeze({
  status: "deck-q4-status",
  error: "deck-q4-error",
  capture: "deck-q4-capture",
  captureError: "deck-q4-capture-error",
});

const tauriHost: Q4HostAdapter = {
  invoke: <T>(command: string, args: Record<string, unknown> = {}) =>
    invoke<T>(command, args),
  listen: <T>(event: string, handler: (payload: T) => void) =>
    listen<T>(event, ({ payload }) => handler(payload)),
};

export function createQ4Client(host: Q4HostAdapter = tauriHost): Q4Client {
  return {
    backendStatusGet: () => host.invoke<Q4BackendView>(Q4_COMMANDS.backendStatusGet, {}),
    selectDecoder: () => host.invoke<Q4BackendView>(Q4_COMMANDS.selectDecoder, {}),
    open: (request) => host.invoke<Q4Status>(Q4_COMMANDS.open, { ...request }),
    controlsSet: (controls) =>
      host.invoke<Q4ControlsAck>(Q4_COMMANDS.controlsSet, { controls }),
    rolesSet: (roles) => host.invoke<Q4RolesAck>(Q4_COMMANDS.rolesSet, { roles }),
    transportSet: (transport) =>
      host.invoke<Q4TransportAck>(Q4_COMMANDS.transportSet, { transport }),
    seedSet: (seed) => host.invoke<Q4SeedAck>(Q4_COMMANDS.seedSet, { seed }),
    restart: () => host.invoke<Q4Status>(Q4_COMMANDS.restart, {}),
    captureSnapshot: () =>
      host.invoke<Q4CaptureView | null>(Q4_COMMANDS.captureSnapshot, {}),
    captureLiveStart: () =>
      host.invoke<Q4CaptureView | null>(Q4_COMMANDS.captureLiveStart, {}),
    captureLiveStop: () => host.invoke<Q4CaptureView>(Q4_COMMANDS.captureLiveStop, {}),
    captureStatusGet: () =>
      host.invoke<Q4CaptureView>(Q4_COMMANDS.captureStatusGet, {}),
    statusGet: () => host.invoke<Q4Status>(Q4_COMMANDS.statusGet, {}),
    spoutStatusGet: () =>
      host.invoke<SpoutStatus | null>(Q4_COMMANDS.spoutStatusGet, {}),
    spoutConfigure: ({ name, enabled }) =>
      host.invoke<SpoutStatus>(Q4_COMMANDS.spoutConfigure, { name, enabled }),
    onStatus: (handler) => host.listen<Q4Status>(Q4_EVENTS.status, handler),
    onError: (handler) => host.listen<Q4ErrorEvent>(Q4_EVENTS.error, handler),
    onCapture: (handler) => host.listen<Q4CaptureView>(Q4_EVENTS.capture, handler),
    onCaptureError: (handler) =>
      host.listen<Q4ErrorEvent>(Q4_EVENTS.captureError, handler),
  };
}

export const q4Client = createQ4Client();
