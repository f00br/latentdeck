export type PlayerPhase =
  "empty" | "loading" | "ready" | "playing" | "paused" | "error";

export type CodecState =
  "missing" | "loading" | "ready" | "incompatible" | "error";

export interface CartridgeSummary {
  cartridgeId: string;
  archiveSha256: string;
  fileName: string;
  width: number;
  height: number;
  frameCount: number;
  frameRateNumerator: number;
  frameRateDenominator: number;
  audioPresent: boolean;
}

export interface CodecSummary {
  state: CodecState;
  displayName: string | null;
  detail: string | null;
}

export interface PlayerError {
  code: string;
  message: string;
  recoverable: boolean;
}

export interface PlayerView {
  revision: number;
  phase: PlayerPhase;
  cartridge: CartridgeSummary | null;
  codec: CodecSummary;
  positionFrame: number;
  loopEnabled: boolean;
  outputAvailable: boolean;
  error: PlayerError | null;
}

export interface SpoutStatus {
  sdkBuilt: boolean;
  ready: boolean;
  enabled: boolean;
  published: boolean;
  requestedName: string;
  activeName: string;
  width: number;
  height: number;
  format: "rgba8_unorm";
  submittedFrames: number;
  lastSequence: number | null;
  spoutFrame: number | null;
  lastErrorCode: string | null;
}

export interface SpoutControls {
  rename: boolean;
  toggle: boolean;
}

export interface PlayerControls {
  open: boolean;
  configureCodec: boolean;
  play: boolean;
  pause: boolean;
  loop: boolean;
  restart: boolean;
  fullscreen: boolean;
}

export const EMPTY_PLAYER_VIEW = Object.freeze({
  revision: 0,
  phase: "empty",
  cartridge: null,
  codec: {
    state: "missing",
    displayName: null,
    detail: "Install and select a compatible H3 Codec Pack.",
  },
  positionFrame: 0,
  loopEnabled: false,
  outputAvailable: false,
  error: null,
} satisfies PlayerView);

export function controlsFor(view: PlayerView, busy: boolean): PlayerControls {
  const playable =
    !busy &&
    view.cartridge !== null &&
    view.codec.state === "ready" &&
    (view.phase === "ready" || view.phase === "paused");
  const hasPlayableCartridge =
    !busy && view.cartridge !== null && view.codec.state === "ready";

  return {
    open: !busy,
    configureCodec:
      !busy &&
      view.codec.state === "missing" &&
      view.codec.displayName !== null,
    play: playable,
    pause: !busy && view.phase === "playing",
    loop: !busy && view.cartridge !== null && view.phase !== "loading",
    restart: hasPlayableCartridge && view.phase !== "loading",
    fullscreen: !busy && view.outputAvailable,
  };
}

export function spoutControlsFor(
  status: SpoutStatus | null,
  busy: boolean,
): SpoutControls {
  const ready = !busy && status?.sdkBuilt === true && status.ready;
  return { rename: ready, toggle: ready };
}

export function acceptTrustedSnapshot(
  current: PlayerView,
  incoming: PlayerView,
): PlayerView {
  return incoming.revision >= current.revision ? incoming : current;
}

export function formatFrameRate(view: PlayerView): string {
  const cartridge = view.cartridge;
  if (
    cartridge === null ||
    cartridge.frameRateNumerator <= 0 ||
    cartridge.frameRateDenominator <= 0
  ) {
    return "— fps";
  }

  const framesPerSecond =
    cartridge.frameRateNumerator / cartridge.frameRateDenominator;
  return `${Number.isInteger(framesPerSecond) ? framesPerSecond : framesPerSecond.toFixed(3)} fps`;
}

export function progressPercent(view: PlayerView): number {
  const frameCount = view.cartridge?.frameCount ?? 0;
  if (frameCount <= 1) {
    return 0;
  }
  const boundedFrame = Math.min(
    Math.max(view.positionFrame, 0),
    frameCount - 1,
  );
  return (boundedFrame / (frameCount - 1)) * 100;
}

export function formatFramePosition(view: PlayerView): string {
  if (view.cartridge === null) {
    return "— / —";
  }
  const visibleFrame = Math.min(
    Math.max(view.positionFrame + 1, 1),
    view.cartridge.frameCount,
  );
  return `${visibleFrame} / ${view.cartridge.frameCount}`;
}
