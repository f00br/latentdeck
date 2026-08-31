import { describe, expect, it } from "vitest";
import {
  createDeckFullscreenCoordinator,
  type DeckSurface,
} from "./deck-fullscreen-coordinator";

describe("Deck fullscreen coordinator", () => {
  it("finishes outgoing fullscreen work before exposing the incoming Deck", async () => {
    const coordinator = createDeckFullscreenCoordinator();
    const enterGate = deferred<void>();
    const exitGate = deferred<void>();
    const events: string[] = [];
    let activeSurface: DeckSurface = "d2";
    let hostFullscreen = false;

    const pendingEnter = coordinator.run(async () => {
      events.push("enter:start");
      await enterGate.promise;
      hostFullscreen = true;
      events.push("enter:done");
      return true;
    });
    const transition = coordinator.transition({
      target: "q4",
      current: () => activeSurface,
      leave: async (surface) => {
        events.push(`exit:${surface}:start`);
        await exitGate.promise;
        hostFullscreen = false;
        events.push(`exit:${surface}:done`);
      },
      commit: (surface) => {
        activeSurface = surface;
        events.push(`commit:${surface}`);
      },
    });
    const incomingStatus = coordinator.run(async () => {
      events.push("incoming:status");
      return hostFullscreen;
    });

    await Promise.resolve();
    expect(activeSurface).toBe("d2");
    expect(events).toEqual(["enter:start"]);

    enterGate.resolve();
    await pendingEnter;
    await waitUntil(() => events.includes("exit:d2:start"));
    expect(activeSurface).toBe("d2");
    expect(events).not.toContain("incoming:status");

    exitGate.resolve();
    await transition;
    expect(await incomingStatus).toBe(false);
    expect(activeSurface).toBe("q4");
    expect(events).toEqual([
      "enter:start",
      "enter:done",
      "exit:d2:start",
      "exit:d2:done",
      "commit:q4",
      "incoming:status",
    ]);
  });
});

function deferred<T>(): {
  promise: Promise<T>;
  resolve: (value: T | PromiseLike<T>) => void;
} {
  let resolve!: (value: T | PromiseLike<T>) => void;
  const promise = new Promise<T>((fulfill) => {
    resolve = fulfill;
  });
  return { promise, resolve };
}

async function waitUntil(predicate: () => boolean): Promise<void> {
  for (let attempt = 0; attempt < 10; attempt += 1) {
    if (predicate()) return;
    await Promise.resolve();
  }
  throw new Error("condition did not become true");
}
