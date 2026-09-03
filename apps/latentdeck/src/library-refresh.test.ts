import { describe, expect, it } from "vitest";
import appSource from "./App.svelte?raw";
import workspaceSource from "./GenericDeckWorkspace.svelte?raw";
import {
  createLibraryRefreshController,
  createLatestRequestRunner,
  notifyLibraryInvalidated,
  onLibraryInvalidated,
} from "./library-refresh";

describe("latest Library snapshot requests", () => {
  it("marks an older response stale when a newer refresh finishes first", async () => {
    let resolveOlder!: (value: string) => void;
    let resolveNewer!: (value: string) => void;
    const older = new Promise<string>((resolve) => {
      resolveOlder = resolve;
    });
    const newer = new Promise<string>((resolve) => {
      resolveNewer = resolve;
    });
    const runner = createLatestRequestRunner<string>();

    const olderResult = runner.run(() => older);
    const newerResult = runner.run(() => newer);
    resolveNewer("newest");
    resolveOlder("stale");

    await expect(newerResult).resolves.toEqual({
      state: "current",
      value: "newest",
    });
    await expect(olderResult).resolves.toEqual({ state: "stale" });
  });

  it("notifies every mounted Library consumer until it unsubscribes", () => {
    const target = new EventTarget();
    let notifications = 0;
    const stop = onLibraryInvalidated(() => {
      notifications += 1;
    }, target);

    notifyLibraryInvalidated(target);
    notifyLibraryInvalidated(target);
    stop();
    notifyLibraryInvalidated(target);

    expect(notifications).toBe(2);
  });

  it("refreshes on activation and invalidation without applying a stale snapshot", async () => {
    const target = new EventTarget();
    const pending: Array<(value: string) => void> = [];
    const applied: string[] = [];
    let loads = 0;
    const controller = createLibraryRefreshController({
      load: () => {
        loads += 1;
        return new Promise<string>((resolve) => pending.push(resolve));
      },
      apply: (value) => {
        applied.push(value);
      },
      onError: (error) => {
        throw error;
      },
      target,
    });

    controller.setActive(false);
    controller.setActive(true);
    notifyLibraryInvalidated(target);
    expect(loads).toBe(2);

    pending[1]?.("newest");
    pending[0]?.("stale");
    await controller.settled();
    expect(applied).toEqual(["newest"]);

    controller.setActive(true);
    expect(loads).toBe(2);
    controller.setActive(false);
    controller.setActive(true);
    expect(loads).toBe(3);
    pending[2]?.("reactivated");
    await controller.settled();
    expect(applied).toEqual(["newest", "reactivated"]);
    controller.dispose();
  });

  it("wires generic captures and preset loads back into the shared Library", () => {
    expect(appSource).toContain("notifyLibraryInvalidated();");
    expect(appSource).toContain("onLibraryChanged={acceptDeckLibrary}");
    expect(workspaceSource).toContain("publishCompletedCapture(capture)");
    expect(workspaceSource).toContain('"library_resolve_preset_sources"');
    expect(workspaceSource).toContain("acceptLibrarySnapshot(incoming");
    expect(workspaceSource).toContain("onLibraryChanged(next)");
  });
});
