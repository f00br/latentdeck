import type { CartridgeView } from "./library-model";

export type Q4Algorithm = "LINEAR" | "XS5";
export type Q4Mode = "HYBRIDIZE" | "INTERACT";
export type Q4InfluenceMode = "MANUAL" | "TRIANGLE";
export type Q4Xs5Routing = "TOPK" | "SINKHORN";
export type Q4Slot = "A" | "B" | "C" | "D";

export interface Q4Roles {
  carrier: Q4Slot;
  donorB: Q4Slot;
  donorC: Q4Slot;
  donorD: Q4Slot;
}

export interface Q4Controls {
  algorithm: Q4Algorithm;
  interaction: number;
  mode: Q4Mode;
  preserve: number;
  influenceMode: Q4InfluenceMode;
  donorWeightB: number;
  donorWeightC: number;
  donorWeightD: number;
  triangleX: number;
  triangleY: number;
  xs5Routing: Q4Xs5Routing;
  temperature: number;
  topK: number;
  sinkhornIterations: number;
  chaos: number;
}

export interface Q4Transport {
  playingA: boolean;
  playingB: boolean;
  playingC: boolean;
  playingD: boolean;
  loopA: boolean;
  loopB: boolean;
  loopC: boolean;
  loopD: boolean;
}

export interface Q4SourceIdentity {
  cartridgeId: string;
  archiveSha256: string;
}

export interface Q4OpenRequest {
  sourceA: Q4SourceIdentity;
  sourceB: Q4SourceIdentity;
  sourceC: Q4SourceIdentity;
  sourceD: Q4SourceIdentity;
  roles: Q4Roles;
  controls: Q4Controls;
  transport: Q4Transport;
  seed: number;
}

export interface Q4ControlsAck {
  controls: Q4Controls;
  requiresCausalReset: false;
}

export interface Q4RolesAck {
  roles: Q4Roles;
  requiresCausalReset: false;
}

export interface Q4TransportAck {
  transport: Q4Transport;
  requiresCausalReset: false;
}

export interface Q4SeedAck {
  seed: number;
  requiresCausalReset: false;
}

export interface Q4Status {
  loaded: boolean;
  streamGeneration: string;
  streamSequence: string;
  playheadA: number;
  playheadB: number;
  playheadC: number;
  playheadD: number;
  roles: Q4Roles;
  transport: Q4Transport;
  controls: Q4Controls;
  seed: number;
  pendingReset: boolean;
  pendingResetReasons: string[];
}

export interface Q4ErrorEvent {
  code: string;
  detail: string;
}

export type Q4CaptureMode = "snapshot" | "live_capture";
export type Q4CaptureState =
  | "idle"
  | "awaiting_reset"
  | "capturing"
  | "stop_armed"
  | "finalizing"
  | "finished"
  | "aborted"
  | "error";

export interface Q4CaptureView {
  captureId: string | null;
  mode: Q4CaptureMode | null;
  state: Q4CaptureState;
  latentSlots: string;
  targetLatentSlots: string | null;
  cartridgeId: string | null;
  archiveSha256: string | null;
  detail: string | null;
}

export type Q4BackendState =
  | "missing"
  | "incompatible"
  | "decoder_missing"
  | "ready"
  | "error";

export interface Q4DecoderView {
  assetId: string;
  variantId: string;
  sha256: string;
  byteLength: number;
  sourceUrl: string;
  licenseLabel: string;
  licenseUrl: string;
}

export interface Q4BackendView {
  state: Q4BackendState;
  packId: string | null;
  packVersion: string | null;
  displayName: string | null;
  q4EntrypointAvailable: boolean;
  decoder: Q4DecoderView | null;
  detail: string | null;
}

export interface Q4SourceSelection {
  sourceAHash: string;
  sourceBHash: string;
  sourceCHash: string;
  sourceDHash: string;
}

export const Q4_SLOTS: readonly Q4Slot[] = Object.freeze(["A", "B", "C", "D"]);
export const MAX_SAFE_Q4_SEED = Number.MAX_SAFE_INTEGER;

export const DEFAULT_Q4_ROLES: Q4Roles = Object.freeze({
  carrier: "A",
  donorB: "B",
  donorC: "C",
  donorD: "D",
});

export const DEFAULT_Q4_CONTROLS: Q4Controls = Object.freeze({
  algorithm: "LINEAR",
  interaction: 0,
  mode: "HYBRIDIZE",
  preserve: 0.55,
  influenceMode: "MANUAL",
  donorWeightB: 1,
  donorWeightC: 1,
  donorWeightD: 1,
  triangleX: 0.5,
  triangleY: 1 / 3,
  xs5Routing: "TOPK",
  temperature: 0.12,
  topK: 8,
  sinkhornIterations: 5,
  chaos: 0,
});

export const DEFAULT_Q4_TRANSPORT: Q4Transport = Object.freeze({
  playingA: true,
  playingB: true,
  playingC: true,
  playingD: true,
  loopA: true,
  loopB: true,
  loopC: true,
  loopD: true,
});

export const DEFAULT_Q4_STATUS: Q4Status = Object.freeze({
  loaded: false,
  streamGeneration: "0",
  streamSequence: "0",
  playheadA: 0,
  playheadB: 0,
  playheadC: 0,
  playheadD: 0,
  roles: DEFAULT_Q4_ROLES,
  transport: DEFAULT_Q4_TRANSPORT,
  controls: DEFAULT_Q4_CONTROLS,
  seed: 0,
  pendingReset: false,
  pendingResetReasons: [],
});

export const DEFAULT_Q4_BACKEND: Q4BackendView = Object.freeze({
  state: "missing",
  packId: null,
  packVersion: null,
  displayName: null,
  q4EntrypointAvailable: false,
  decoder: null,
  detail: "Install a compatible H3 Codec Pack with an LD-Q4 entrypoint.",
});

export const DEFAULT_Q4_CAPTURE: Q4CaptureView = Object.freeze({
  captureId: null,
  mode: null,
  state: "idle",
  latentSlots: "0",
  targetLatentSlots: null,
  cartridgeId: null,
  archiveSha256: null,
  detail: null,
});

export function copyQ4Controls(controls: Q4Controls): Q4Controls {
  return { ...controls };
}

export function copyQ4Roles(roles: Q4Roles): Q4Roles {
  return { ...roles };
}

export function copyQ4Transport(transport: Q4Transport): Q4Transport {
  return { ...transport };
}

export function isQ4CaptureActive(state: Q4CaptureState): boolean {
  return (
    state === "awaiting_reset" ||
    state === "capturing" ||
    state === "stop_armed" ||
    state === "finalizing"
  );
}

export type Q4LiveCaptureAction = "start" | "stop" | null;

export function q4LiveCaptureAction(
  capture: Pick<Q4CaptureView, "mode" | "state">,
): Q4LiveCaptureAction {
  if (capture.mode === "live_capture" && capture.state === "capturing") return "stop";
  return isQ4CaptureActive(capture.state) ? null : "start";
}

export function chooseQ4Sources(
  cartridges: readonly CartridgeView[],
  current: Readonly<Q4SourceSelection>,
): Q4SourceSelection {
  const present = cartridges
    .filter((cartridge) => cartridge.availability === "present")
    .map((cartridge) => cartridge.archiveSha256)
    .filter((hash, index, hashes) => hashes.indexOf(hash) === index);
  const used = new Set<string>();
  const choose = (candidate: string): string => {
    if (present.includes(candidate) && !used.has(candidate)) {
      used.add(candidate);
      return candidate;
    }
    const next = present.find((hash) => !used.has(hash)) ?? "";
    if (next !== "") used.add(next);
    return next;
  };
  return {
    sourceAHash: choose(current.sourceAHash),
    sourceBHash: choose(current.sourceBHash),
    sourceCHash: choose(current.sourceCHash),
    sourceDHash: choose(current.sourceDHash),
  };
}

export function validateQ4Roles(roles: Q4Roles): boolean {
  return new Set([roles.carrier, roles.donorB, roles.donorC, roles.donorD]).size === 4;
}

export function resolveQ4DonorWeights(
  controls: Q4Controls,
): readonly [number, number, number] {
  if (controls.influenceMode === "TRIANGLE") {
    const b = 1 - controls.triangleX - 0.5 * controls.triangleY;
    const c = controls.triangleX - 0.5 * controls.triangleY;
    const d = controls.triangleY;
    if (![b, c, d].every(Number.isFinite) || Math.min(b, c, d) < -1e-12) {
      throw new Error("Q4 triangle point must lie inside the B/C/D influence field.");
    }
    const total = b + c + d;
    return [Math.max(0, b) / total, Math.max(0, c) / total, Math.max(0, d) / total];
  }
  const values = [controls.donorWeightB, controls.donorWeightC, controls.donorWeightD];
  const total = values.reduce((sum, value) => sum + value, 0);
  if (!values.every((value) => Number.isFinite(value) && value >= 0) || total <= 0) {
    throw new Error("At least one finite non-negative Q4 donor weight is required.");
  }
  return [values[0] / total, values[1] / total, values[2] / total];
}

export function buildQ4OpenRequest(
  sources: readonly [CartridgeView, CartridgeView, CartridgeView, CartridgeView],
  roles: Q4Roles,
  controls: Q4Controls,
  transport: Q4Transport,
  seed: number,
): Q4OpenRequest {
  if (sources.some((source) => source.availability !== "present")) {
    throw new Error("All four Q4 sources must be present before loading the Deck.");
  }
  if (new Set(sources.map((source) => source.archiveSha256)).size !== 4) {
    throw new Error("Q4 requires four distinct cartridges.");
  }
  if (!validateQ4Roles(roles)) {
    throw new Error("Q4 carrier and donor roles must be an A/B/C/D permutation.");
  }
  if (parseQ4Seed(seed) === null) {
    throw new Error("Q4 seed must be a non-negative u53 integer.");
  }
  resolveQ4DonorWeights(controls);
  const [sourceA, sourceB, sourceC, sourceD] = sources;
  return {
    sourceA: sourceIdentity(sourceA),
    sourceB: sourceIdentity(sourceB),
    sourceC: sourceIdentity(sourceC),
    sourceD: sourceIdentity(sourceD),
    roles: copyQ4Roles(roles),
    controls: copyQ4Controls(controls),
    transport: copyQ4Transport(transport),
    seed,
  };
}

export function parseQ4Seed(value: string | number): number | null {
  if (typeof value === "string" && value.trim() === "") return null;
  const parsed = typeof value === "number" ? value : Number(value);
  return Number.isSafeInteger(parsed) && parsed >= 0 ? parsed : null;
}

export function setQ4SlotPlaying(
  transport: Q4Transport,
  slot: Q4Slot,
  playing: boolean,
): Q4Transport {
  return { ...transport, [`playing${slot}`]: playing } as Q4Transport;
}

export function setQ4SlotLoop(
  transport: Q4Transport,
  slot: Q4Slot,
  loop: boolean,
): Q4Transport {
  return { ...transport, [`loop${slot}`]: loop } as Q4Transport;
}

function sourceIdentity(cartridge: CartridgeView): Q4SourceIdentity {
  return {
    cartridgeId: cartridge.cartridgeId,
    archiveSha256: cartridge.archiveSha256,
  };
}
