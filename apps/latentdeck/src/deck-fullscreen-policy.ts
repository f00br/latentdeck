export interface DeckFullscreenContext {
  active: boolean;
  runtimeLoaded: boolean;
  viewportReady: boolean;
  busy: boolean;
  current: boolean | null;
}

/**
 * Entering fullscreen requires a live Deck output and an acknowledged embedded
 * viewport. Leaving fullscreen is a host recovery action and must stay
 * available even after the Deck runtime or active surface has disappeared.
 */
export function canSetDeckFullscreen(
  context: Readonly<DeckFullscreenContext>,
  enabled: boolean,
): boolean {
  if (context.busy || context.current === null) return false;
  if (!enabled) return context.current;
  return (
    !context.current &&
    context.active &&
    context.runtimeLoaded &&
    context.viewportReady
  );
}

export function shouldExitFullscreenForHiddenDeck(
  active: boolean,
  current: boolean | null,
  busy: boolean,
): boolean {
  return !active && current === true && !busy;
}
