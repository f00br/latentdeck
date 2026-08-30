import { afterEach, describe, expect, it, vi } from "vitest";
import {
  LatestValueDispatcher,
  sameControlSnapshot,
} from "./realtime-controls";

afterEach(() => {
  vi.useRealTimers();
});

describe("bounded latest-value realtime dispatch", () => {
  it("throttles a burst down to its newest value", async () => {
    vi.useFakeTimers();
    const applied: number[] = [];
    const dispatcher = new LatestValueDispatcher<number>({
      throttleMs: 75,
      apply: async (value) => {
        applied.push(value);
      },
      onError: () => undefined,
    });

    dispatcher.push(1);
    dispatcher.push(2);
    dispatcher.push(3);
    await vi.runAllTimersAsync();

    expect(applied).toEqual([3]);
    dispatcher.dispose();
  });

  it("keeps at most the latest value while one request is in flight", async () => {
    vi.useFakeTimers();
    let releaseFirst: (() => void) | undefined;
    const first = new Promise<void>((resolve) => {
      releaseFirst = resolve;
    });
    const applied: number[] = [];
    const states: Array<{ running: boolean; pending: boolean }> = [];
    const dispatcher = new LatestValueDispatcher<number>({
      throttleMs: 75,
      apply: async (value) => {
        applied.push(value);
        if (value === 1) await first;
      },
      onError: () => undefined,
      onStateChange: (state) => states.push(state),
    });

    dispatcher.push(1, true);
    await vi.advanceTimersByTimeAsync(0);
    dispatcher.push(2);
    dispatcher.push(3);
    expect(applied).toEqual([1]);

    releaseFirst?.();
    await Promise.resolve();
    await vi.runAllTimersAsync();

    expect(applied).toEqual([1, 3]);
    expect(states.at(-1)).toEqual({ running: false, pending: false });
    dispatcher.dispose();
  });

  it("reports a failed apply and accepts an explicit retry", async () => {
    vi.useFakeTimers();
    const errors: unknown[] = [];
    let attempts = 0;
    const dispatcher = new LatestValueDispatcher<number>({
      throttleMs: 0,
      apply: async () => {
        attempts += 1;
        if (attempts === 1) throw new Error("worker rejected controls");
      },
      onError: (error) => errors.push(error),
    });

    dispatcher.push(5, true);
    await vi.runAllTimersAsync();
    dispatcher.push(5, true);
    await vi.runAllTimersAsync();

    expect(attempts).toBe(2);
    expect(errors).toHaveLength(1);
    expect(String(errors[0])).toContain("worker rejected controls");
    dispatcher.dispose();
  });

  it("compares acknowledged control snapshots without serialization", () => {
    expect(
      sameControlSnapshot({ mix: 0.5, mode: "A" }, { mix: 0.5, mode: "A" }),
    ).toBe(true);
    expect(
      sameControlSnapshot({ mix: 0.5, mode: "A" }, { mix: 0.6, mode: "A" }),
    ).toBe(false);
  });
});
