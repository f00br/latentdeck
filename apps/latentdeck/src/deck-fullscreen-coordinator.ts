export type DeckSurface = "library" | "d2" | "q4";
export type FullscreenDeckSurface = Exclude<DeckSurface, "library">;

export interface DeckSurfaceTransition {
  target: DeckSurface;
  current(): DeckSurface;
  leave(surface: FullscreenDeckSurface): Promise<void>;
  commit(surface: DeckSurface): void;
}

export interface DeckFullscreenCoordinator {
  run<T>(operation: () => Promise<T>): Promise<T>;
  transition(request: DeckSurfaceTransition): Promise<void>;
}

/**
 * Own one FIFO for every host-fullscreen read, write, and Deck-surface switch.
 *
 * D2 and Q4 share one top-level HWND. Keeping their commands on this queue
 * prevents a new faceplate from observing fullscreen before the outgoing
 * faceplate's idempotent exit has completed.
 */
export function createDeckFullscreenCoordinator(): DeckFullscreenCoordinator {
  let tail: Promise<void> = Promise.resolve();

  function run<T>(operation: () => Promise<T>): Promise<T> {
    const result = tail.then(operation);
    tail = result.then(
      () => undefined,
      () => undefined,
    );
    return result;
  }

  return {
    run,
    transition: (request) =>
      run(async () => {
        const outgoing = request.current();
        if (outgoing === request.target) return;
        if (outgoing !== "library") await request.leave(outgoing);
        request.commit(request.target);
      }),
  };
}
