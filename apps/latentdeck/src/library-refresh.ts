export type LatestRequestResult<T> =
  { state: "current"; value: T } | { state: "stale" };

export interface LatestRequestRunner<T> {
  run(request: () => Promise<T>): Promise<LatestRequestResult<T>>;
}

export interface LibraryRefreshController<T> {
  setActive(active: boolean): void;
  refresh(): Promise<void>;
  settled(): Promise<void>;
  dispose(): void;
}

export interface LibraryRefreshControllerOptions<T> {
  load(): Promise<T>;
  apply(value: T): void | Promise<void>;
  onError(error: unknown): void;
  target?: EventTarget;
}

const LIBRARY_INVALIDATED_EVENT = "latentdeck-library-invalidated";

export function createLatestRequestRunner<T>(): LatestRequestRunner<T> {
  let latestRevision = 0;
  return {
    async run(request) {
      const revision = ++latestRevision;
      try {
        const value = await request();
        return revision === latestRevision
          ? { state: "current", value }
          : { state: "stale" };
      } catch (error) {
        if (revision !== latestRevision) return { state: "stale" };
        throw error;
      }
    },
  };
}

export function notifyLibraryInvalidated(
  target: EventTarget = globalThis as unknown as EventTarget,
): void {
  target.dispatchEvent(new Event(LIBRARY_INVALIDATED_EVENT));
}

export function onLibraryInvalidated(
  handler: () => void,
  target: EventTarget = globalThis as unknown as EventTarget,
): () => void {
  target.addEventListener(LIBRARY_INVALIDATED_EVENT, handler);
  return () => target.removeEventListener(LIBRARY_INVALIDATED_EVENT, handler);
}

export function createLibraryRefreshController<T>(
  options: LibraryRefreshControllerOptions<T>,
): LibraryRefreshController<T> {
  const runner = createLatestRequestRunner<T>();
  const pending = new Set<Promise<void>>();
  let active = false;

  const controller: LibraryRefreshController<T> = {
    setActive(nextActive) {
      const activated = nextActive && !active;
      active = nextActive;
      if (activated) void controller.refresh();
    },
    refresh() {
      const refresh = runner
        .run(options.load)
        .then(async (result) => {
          if (result.state === "current") await options.apply(result.value);
        })
        .catch(options.onError);
      pending.add(refresh);
      void refresh.finally(() => pending.delete(refresh));
      return refresh;
    },
    async settled() {
      while (pending.size > 0) await Promise.all([...pending]);
    },
    dispose() {
      stopInvalidation();
    },
  };
  const stopInvalidation = onLibraryInvalidated(
    () => void controller.refresh(),
    options.target,
  );
  return controller;
}
