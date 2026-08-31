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
  packId: string | null;
  packVersion: string | null;
  publisherName: string | null;
  publisherUrl: string | null;
  packLicenseLabel: string | null;
  decoderAssetId: string | null;
  decoderDisplayName: string | null;
  decoderVariants: DecoderVariantSummary[];
}

export interface DecoderVariantSummary {
  variantId: string;
  sha256: string;
  byteLength: number;
  sourceUrl: string;
  licenseLabel: string;
  licenseUrl: string;
  selected: boolean;
}

export interface PlayerError {
  code: string;
  message: string;
  recoverable: boolean;
}

export type PlayerOperation =
  | "open"
  | "decoder"
  | "play"
  | "pause"
  | "loop"
  | "restart"
  | "fullscreen-enter"
  | "fullscreen-exit";

export interface FullscreenStatus {
  active: boolean;
}

export interface NativeViewportSession {
  epoch: number;
}

export interface NativeViewportBounds {
  epoch: number;
  revision: number;
  xCss: number;
  yCss: number;
  widthCss: number;
  heightCss: number;
  scaleFactor: number;
  visible: boolean;
}

export interface ViewportRectLike {
  left: number;
  top: number;
  width: number;
  height: number;
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

export type DiagnosticSaveResult =
  | {
      status: "saved";
      archiveBytes: number;
      eventCount: number;
      schemaVersion: number;
    }
  | { status: "cancelled" };

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
    packId: null,
    packVersion: null,
    publisherName: null,
    publisherUrl: null,
    packLicenseLabel: null,
    decoderAssetId: null,
    decoderDisplayName: null,
    decoderVariants: [],
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
      view.codec.state !== "loading" &&
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

export function diagnosticSaveEnabled(busy: boolean): boolean {
  return !busy;
}

export function describeDiagnosticSaveResult(
  result: DiagnosticSaveResult,
): string {
  if (result.status === "cancelled") {
    return "Diagnostic save cancelled; no file was created.";
  }
  const kibibytes = result.archiveBytes / 1024;
  const size = `${kibibytes.toFixed(kibibytes >= 100 ? 0 : 1)} KiB`;
  const eventLabel = result.eventCount === 1 ? "event" : "events";
  return `Diagnostic bundle saved · ${size} · ${result.eventCount} ${eventLabel} · schema ${result.schemaVersion}`;
}

export function describePlayerOperation(operation: PlayerOperation): string {
  switch (operation) {
    case "open":
      return "Opening cartridge…";
    case "decoder":
      return "Validating decoder…";
    case "play":
      return "Starting playback…";
    case "pause":
      return "Pausing playback…";
    case "loop":
      return "Updating loop mode…";
    case "restart":
      return "Restarting decoder…";
    case "fullscreen-enter":
      return "Entering fullscreen…";
    case "fullscreen-exit":
      return "Exiting fullscreen…";
  }
}

export function describeAudioAvailability(view: PlayerView): string | null {
  if (view.cartridge === null) return null;
  return view.cartridge.audioPresent
    ? "Audio payload preserved · playback unavailable in v0.1"
    : "Visual-only cartridge · no audio payload";
}

export function selectDisplayedError(
  persistent: PlayerError | null,
  transient: PlayerError | null,
): PlayerError | null {
  return transient ?? persistent;
}

export function fullscreenActionLabel(
  status: FullscreenStatus | null,
): "Fullscreen" | "Exit fullscreen" {
  return status?.active === true ? "Exit fullscreen" : "Fullscreen";
}

export function buildNativeViewportBounds(
  epoch: number,
  revision: number,
  rect: ViewportRectLike,
  scaleFactor: number,
  visible: boolean,
): NativeViewportBounds | null {
  const values = [rect.left, rect.top, rect.width, rect.height, scaleFactor];
  if (
    !Number.isSafeInteger(epoch) ||
    epoch <= 0 ||
    !Number.isSafeInteger(revision) ||
    revision <= 0 ||
    values.some((value) => !Number.isFinite(value)) ||
    rect.left < 0 ||
    rect.top < 0 ||
    rect.width < 0 ||
    rect.height < 0 ||
    scaleFactor < 0.5 ||
    scaleFactor > 8
  ) {
    return null;
  }

  return {
    epoch,
    revision,
    xCss: rect.left,
    yCss: rect.top,
    widthCss: rect.width,
    heightCss: rect.height,
    scaleFactor,
    visible: visible && rect.width >= 1 && rect.height >= 1,
  };
}

export function hiddenNativeViewportBounds(
  epoch: number,
  revision: number,
  scaleFactor: number,
): NativeViewportBounds | null {
  return buildNativeViewportBounds(
    epoch,
    revision,
    { left: 0, top: 0, width: 0, height: 0 },
    scaleFactor,
    false,
  );
}

export function nextNativeViewportRevision(current: number): number | null {
  if (!Number.isSafeInteger(current) || current < 0) return null;
  const next = current + 1;
  return Number.isSafeInteger(next) ? next : null;
}

export function viewportRetryRequiresRemeasure(errorCode: string): boolean {
  return errorCode === "output.viewport_scale_stale";
}

export function sameNativeViewportGeometry(
  left: NativeViewportBounds | null,
  right: NativeViewportBounds,
): boolean {
  if (
    left === null ||
    left.epoch !== right.epoch ||
    left.visible !== right.visible
  )
    return false;
  const epsilon = 0.01;
  return (
    Math.abs(left.xCss - right.xCss) < epsilon &&
    Math.abs(left.yCss - right.yCss) < epsilon &&
    Math.abs(left.widthCss - right.widthCss) < epsilon &&
    Math.abs(left.heightCss - right.heightCss) < epsilon &&
    Math.abs(left.scaleFactor - right.scaleFactor) < epsilon
  );
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

export function describeRuntimeStatus(view: PlayerView): string {
  if (view.codec.detail !== null) return view.codec.detail;
  if (view.outputAvailable) return "Native output active";
  if (view.codec.state === "ready" && view.cartridge !== null) {
    return "Ready to start playback";
  }
  return "Open a cartridge to start playback";
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
