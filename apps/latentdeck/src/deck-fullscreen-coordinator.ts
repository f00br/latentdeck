export type DeckSurface = "library" | "deck" | "extensions";

export interface DeckSurfaceTransition {
  target: DeckSurface;
  current(): DeckSurface;
  leave(): Promise<void>;
  commit(surface: DeckSurface): void;
}

export interface DeckFullscreenCoordinator {
  run<T>(operation: () => Promise<T>): Promise<T>;
  transition(request: DeckSurfaceTransition): Promise<void>;
}

/**
 * Own one FIFO for every host-fullscreen read, write, and Deck-surface switch.
 *
 * Every declarative Deck shares one top-level HWND. Keeping all host output
 * commands on this queue prevents a newly selected exact package from
 * observing fullscreen before the outgoing session has exited.
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
        if (outgoing === "deck") await request.leave();
        request.commit(request.target);
      }),
  };
}
