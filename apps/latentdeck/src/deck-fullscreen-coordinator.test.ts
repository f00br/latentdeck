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
    let activeSurface: DeckSurface = "deck";
    let hostFullscreen = false;

    const pendingEnter = coordinator.run(async () => {
      events.push("enter:start");
      await enterGate.promise;
      hostFullscreen = true;
      events.push("enter:done");
      return true;
    });
    const transition = coordinator.transition({
      target: "library",
      current: () => activeSurface,
      leave: async () => {
        events.push("exit:deck:start");
        await exitGate.promise;
        hostFullscreen = false;
        events.push("exit:deck:done");
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
    expect(activeSurface).toBe("deck");
    expect(events).toEqual(["enter:start"]);

    enterGate.resolve();
    await pendingEnter;
    await waitUntil(() => events.includes("exit:deck:start"));
    expect(activeSurface).toBe("deck");
    expect(events).not.toContain("incoming:status");

    exitGate.resolve();
    await transition;
    expect(await incomingStatus).toBe(false);
    expect(activeSurface).toBe("library");
    expect(events).toEqual([
      "enter:start",
      "enter:done",
      "exit:deck:start",
      "exit:deck:done",
      "commit:library",
      "incoming:status",
    ]);
  });

  it("treats Extensions as a host surface without inventing a fullscreen Deck", async () => {
    const coordinator = createDeckFullscreenCoordinator();
    const left: string[] = [];
    let activeSurface: DeckSurface = "deck";

    await coordinator.transition({
      target: "extensions",
      current: () => activeSurface,
      leave: async () => {
        left.push("deck");
      },
      commit: (surface) => {
        activeSurface = surface;
      },
    });
    await coordinator.transition({
      target: "library",
      current: () => activeSurface,
      leave: async () => {
        left.push("deck");
      },
      commit: (surface) => {
        activeSurface = surface;
      },
    });

    expect(left).toEqual(["deck"]);
    expect(activeSurface).toBe("library");
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
