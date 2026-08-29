import type { CartridgeView } from "./library-model";

export type D2Algorithm = "LINEAR" | "XS1" | "XS2" | "XS3" | "XS4" | "XS5";
export type D2Mode = "HYBRIDIZE" | "INTERACT";
export type D2Routing = "A" | "B";
export type D2Xs5Routing = "TOPK" | "SINKHORN";
export type D2Slot = "A" | "B";

export interface D2Controls {
  algorithm: D2Algorithm;
  mix: number;
  mode: D2Mode;
  routing: D2Routing;
  interaction: number;
  preserve: number;
  chaos: number;
  xs1ChannelA: number;
  xs1ChannelB: number;
  xs1AngleDegrees: number;
  xs2Radius: number;
  xs3HighGain: number;
  xs4Epsilon: number;
  xs5Routing: D2Xs5Routing;
  temperature: number;
  topK: number;
  sinkhornIterations: number;
}

export interface D2Transport {
  playingA: boolean;
  playingB: boolean;
  loopA: boolean;
  loopB: boolean;
}

export interface D2SourceIdentity {
  cartridgeId: string;
  archiveSha256: string;
}

export interface D2OpenRequest {
  sourceA: D2SourceIdentity;
  sourceB: D2SourceIdentity;
  controls: D2Controls;
  transport: D2Transport;
  seed: number;
}

export interface D2ControlsAck {
  controls: D2Controls;
  requiresCausalReset: false;
}

export interface D2TransportAck {
  transport: D2Transport;
  requiresCausalReset: false;
}

export interface D2SeedAck {
  seed: number;
  requiresCausalReset: false;
}

export interface D2Status {
  loaded: boolean;
  streamGeneration: string;
  streamSequence: string;
  playheadA: number;
  playheadB: number;
  transport: D2Transport;
  controls: D2Controls;
  seed: number;
  pendingReset: boolean;
  pendingResetReasons: string[];
}

export interface D2ErrorEvent {
  code: string;
  detail: string;
}

export type D2CaptureMode = "snapshot" | "live_capture";
export type D2CaptureState =
  | "idle"
  | "awaiting_reset"
  | "capturing"
  | "stop_armed"
  | "finalizing"
  | "finished"
  | "aborted"
  | "error";

export interface D2CaptureView {
  captureId: string | null;
  mode: D2CaptureMode | null;
  state: D2CaptureState;
  latentSlots: string;
  targetLatentSlots: string | null;
  cartridgeId: string | null;
  archiveSha256: string | null;
  detail: string | null;
}

export type D2BackendState =
  "missing" | "incompatible" | "decoder_missing" | "ready" | "error";

export interface D2DecoderView {
  assetId: string;
  variantId: string;
  sha256: string;
  byteLength: number;
  sourceUrl: string;
  licenseLabel: string;
  licenseUrl: string;
}

export interface D2BackendView {
  state: D2BackendState;
  packId: string | null;
  packVersion: string | null;
  displayName: string | null;
  d2EntrypointAvailable: boolean;
  decoder: D2DecoderView | null;
  detail: string | null;
}

export const MAX_SAFE_D2_SEED = Number.MAX_SAFE_INTEGER;

export const DEFAULT_D2_CONTROLS: D2Controls = Object.freeze({
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

export const DEFAULT_D2_TRANSPORT: D2Transport = Object.freeze({
  playingA: true,
  playingB: true,
  loopA: true,
  loopB: true,
});

export const DEFAULT_D2_STATUS: D2Status = Object.freeze({
  loaded: false,
  streamGeneration: "0",
  streamSequence: "0",
  playheadA: 0,
  playheadB: 0,
  transport: DEFAULT_D2_TRANSPORT,
  controls: DEFAULT_D2_CONTROLS,
  seed: 0,
  pendingReset: false,
  pendingResetReasons: [],
});

export const DEFAULT_D2_BACKEND: D2BackendView = Object.freeze({
  state: "missing",
  packId: null,
  packVersion: null,
  displayName: null,
  d2EntrypointAvailable: false,
  decoder: null,
  detail: "Install a compatible H3 Codec Pack.",
});

export const DEFAULT_D2_CAPTURE: D2CaptureView = Object.freeze({
  captureId: null,
  mode: null,
  state: "idle",
  latentSlots: "0",
  targetLatentSlots: null,
  cartridgeId: null,
  archiveSha256: null,
  detail: null,
});

export function isD2CaptureActive(state: D2CaptureState): boolean {
  return (
    state === "awaiting_reset" ||
    state === "capturing" ||
    state === "stop_armed" ||
    state === "finalizing"
  );
}

export function copyD2Controls(controls: D2Controls): D2Controls {
  return { ...controls };
}

export function copyD2Transport(transport: D2Transport): D2Transport {
  return { ...transport };
}

export function chooseD2Sources(
  cartridges: readonly CartridgeView[],
  currentA: string,
  currentB: string,
): { sourceAHash: string; sourceBHash: string } {
  const hashes = cartridges
    .filter((cartridge) => cartridge.availability === "present")
    .map((cartridge) => cartridge.archiveSha256);
  const sourceAHash = hashes.includes(currentA) ? currentA : (hashes[0] ?? "");
  const sourceBHash = hashes.includes(currentB)
    ? currentB
    : (hashes.find((hash) => hash !== sourceAHash) ?? hashes[0] ?? "");
  return { sourceAHash, sourceBHash };
}

export function buildD2OpenRequest(
  sourceA: CartridgeView,
  sourceB: CartridgeView,
  controls: D2Controls,
  transport: D2Transport,
  seed: number,
): D2OpenRequest {
  if (
    sourceA.availability !== "present" ||
    sourceB.availability !== "present"
  ) {
    throw new Error("D2 sources must be present before they can be opened.");
  }
  if (parseD2Seed(seed) === null) {
    throw new Error("D2 seed must be a non-negative u53 integer.");
  }
  return {
    sourceA: sourceIdentity(sourceA),
    sourceB: sourceIdentity(sourceB),
    controls: copyD2Controls(controls),
    transport: copyD2Transport(transport),
    seed,
  };
}

export function parseD2Seed(value: string | number): number | null {
  if (typeof value === "string" && value.trim() === "") return null;
  const parsed = typeof value === "number" ? value : Number(value);
  return Number.isSafeInteger(parsed) &&
    parsed >= 0 &&
    parsed <= MAX_SAFE_D2_SEED
    ? parsed
    : null;
}

export function setSlotPlaying(
  transport: D2Transport,
  slot: D2Slot,
  playing: boolean,
): D2Transport {
  return slot === "A"
    ? { ...transport, playingA: playing }
    : { ...transport, playingB: playing };
}

export function setSlotLoop(
  transport: D2Transport,
  slot: D2Slot,
  loop: boolean,
): D2Transport {
  return slot === "A"
    ? { ...transport, loopA: loop }
    : { ...transport, loopB: loop };
}

function sourceIdentity(cartridge: CartridgeView): D2SourceIdentity {
  return {
    cartridgeId: cartridge.cartridgeId,
    archiveSha256: cartridge.archiveSha256,
  };
}
