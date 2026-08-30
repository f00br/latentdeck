export interface LatestValueDispatchState {
  running: boolean;
  pending: boolean;
}

export interface LatestValueDispatcherOptions<T> {
  throttleMs: number;
  apply(value: T): Promise<void>;
  onError(error: unknown): void;
  onStateChange?(state: LatestValueDispatchState): void;
}

/**
 * A bounded realtime command lane: one request may be in flight and only the
 * newest waiting value is retained. This prevents slider input from building
 * an unbounded IPC queue while keeping the last physical control position.
 */
export class LatestValueDispatcher<T> {
  readonly #options: LatestValueDispatcherOptions<T>;
  #pending: T | undefined;
  #running = false;
  #timer: ReturnType<typeof globalThis.setTimeout> | null = null;
  #lastStartedAt = Number.NEGATIVE_INFINITY;
  #disposed = false;

  constructor(options: LatestValueDispatcherOptions<T>) {
    if (!Number.isFinite(options.throttleMs) || options.throttleMs < 0) {
      throw new Error("Realtime control throttle must be non-negative.");
    }
    this.#options = options;
  }

  push(value: T, immediate = false): void {
    if (this.#disposed) return;
    this.#pending = value;
    if (immediate && this.#timer !== null) {
      globalThis.clearTimeout(this.#timer);
      this.#timer = null;
    }
    this.#notify();
    this.#schedule(immediate);
  }

  cancelPending(): void {
    this.#pending = undefined;
    if (this.#timer !== null) {
      globalThis.clearTimeout(this.#timer);
      this.#timer = null;
    }
    this.#notify();
  }

  dispose(): void {
    this.#disposed = true;
    this.cancelPending();
  }

  #schedule(immediate: boolean): void {
    if (
      this.#disposed ||
      this.#running ||
      this.#timer !== null ||
      this.#pending === undefined
    ) {
      return;
    }
    const elapsed = performance.now() - this.#lastStartedAt;
    const delay = immediate
      ? 0
      : Math.max(0, this.#options.throttleMs - elapsed);
    this.#timer = globalThis.setTimeout(() => {
      this.#timer = null;
      void this.#dispatch();
    }, delay);
  }

  async #dispatch(): Promise<void> {
    if (this.#disposed || this.#running || this.#pending === undefined) return;
    const value = this.#pending;
    this.#pending = undefined;
    this.#running = true;
    this.#lastStartedAt = performance.now();
    this.#notify();
    try {
      await this.#options.apply(value);
    } catch (error) {
      this.#options.onError(error);
    } finally {
      this.#running = false;
      this.#notify();
      this.#schedule(false);
    }
  }

  #notify(): void {
    this.#options.onStateChange?.({
      running: this.#running,
      pending: this.#pending !== undefined || this.#timer !== null,
    });
  }
}

export function sameControlSnapshot<T extends object>(
  left: T,
  right: T,
): boolean {
  const keys = Object.keys(left) as Array<keyof T>;
  const rightKeys = Object.keys(right);
  return (
    keys.length === rightKeys.length &&
    keys.every((key) => Object.is(left[key], right[key]))
  );
}
