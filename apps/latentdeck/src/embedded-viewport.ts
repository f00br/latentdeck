/** Host-issued identity for one mounted WebView viewport client. */
export interface EmbeddedViewportSession {
  epoch: number;
}

/** Revisioned CSS measurement for a native decoded-video child surface. */
export interface EmbeddedViewportBounds {
  epoch: number;
  revision: number;
  xCss: number;
  yCss: number;
  widthCss: number;
  heightCss: number;
  scaleFactor: number;
  visible: boolean;
}

/**
 * Deck-facing host capability for one native output surface.
 *
 * A faceplate owns only its layout anchor and fullscreen intent. The host owns
 * validation, the child HWND, native rendering, and professional outputs.
 * Keeping this interface free of D2/Q4 types lets another Deck reuse the same
 * presentation boundary without gaining raw-window access.
 */
export interface DeckEmbeddedOutputHost {
  viewportSessionBegin(): Promise<EmbeddedViewportSession>;
  viewportSetBounds(bounds: EmbeddedViewportBounds): Promise<void>;
  fullscreenStatusGet(): Promise<boolean | null>;
  fullscreenSet(enabled: boolean): Promise<boolean>;
}

export interface ViewportRectLike {
  left: number;
  top: number;
  width: number;
  height: number;
}

export interface ViewportClientRectLike extends ViewportRectLike {
  right: number;
  bottom: number;
}

/** Rust accepts the same physical rounding noise at the trusted client edge. */
export const EMBEDDED_VIEWPORT_CLIENT_EDGE_TOLERANCE_PHYSICAL_PX = 2;

/**
 * Allocate the next revision inside one host-issued epoch.
 *
 * Revisions deliberately restart at one for a new session; the Rust host maps
 * them to its own monotonic revisions before sending placement to the actor.
 */
export function nextEmbeddedViewportRevision(current: number): number | null {
  if (!Number.isSafeInteger(current) || current < 0) return null;
  const next = current + 1;
  return Number.isSafeInteger(next) ? next : null;
}

export function embeddedViewportFullyInsideClient(
  rect: ViewportClientRectLike,
  clientWidth: number,
  clientHeight: number,
  scaleFactor: number,
): boolean {
  const values = [
    rect.left,
    rect.top,
    rect.right,
    rect.bottom,
    clientWidth,
    clientHeight,
    scaleFactor,
  ];
  if (
    values.some((value) => !Number.isFinite(value)) ||
    clientWidth <= 0 ||
    clientHeight <= 0 ||
    scaleFactor < 0.5 ||
    scaleFactor > 8
  ) {
    return false;
  }
  const cssTolerance =
    EMBEDDED_VIEWPORT_CLIENT_EDGE_TOLERANCE_PHYSICAL_PX / scaleFactor;
  return (
    rect.left >= 0 &&
    rect.top >= 0 &&
    rect.right <= clientWidth + cssTolerance &&
    rect.bottom <= clientHeight + cssTolerance
  );
}

export function buildEmbeddedViewportBounds(
  epoch: number,
  revision: number,
  rect: ViewportRectLike,
  scaleFactor: number,
  visible: boolean,
): EmbeddedViewportBounds | null {
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

export function hiddenEmbeddedViewportBounds(
  epoch: number,
  revision: number,
  scaleFactor: number,
): EmbeddedViewportBounds | null {
  return buildEmbeddedViewportBounds(
    epoch,
    revision,
    { left: 0, top: 0, width: 0, height: 0 },
    scaleFactor,
    false,
  );
}

export function sameEmbeddedViewportGeometry(
  left: EmbeddedViewportBounds | null,
  right: EmbeddedViewportBounds,
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

export const EMBEDDED_VIEWPORT_RETRY_DELAYS_MS = [250, 1_000, 2_500] as const;

/**
 * Re-measure a native child viewport when conditional DOM around its anchor is
 * inserted or removed. `ResizeObserver` cannot see a position-only move when
 * the anchor keeps the same dimensions, which is common for transient status
 * rows such as Restart/Reset feedback.
 */
export function observeEmbeddedViewportReflow(
  root: Node,
  schedule: () => void,
): () => void {
  const observer = new MutationObserver(() => schedule());
  observer.observe(root, { childList: true, subtree: true });
  return () => observer.disconnect();
}
