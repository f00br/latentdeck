import { describe, expect, it } from "vitest";
import d2Faceplate from "./D2Faceplate.svelte?raw";
import q4Faceplate from "./Q4Faceplate.svelte?raw";
import {
  canSetDeckFullscreen,
  shouldExitFullscreenForHiddenDeck,
} from "./deck-fullscreen-policy";

describe("Deck host fullscreen recovery policy", () => {
  it("requires a loaded visible Deck and acknowledged viewport to enter", () => {
    const ready = {
      active: true,
      runtimeLoaded: true,
      viewportReady: true,
      busy: false,
      current: false,
    } as const;

    expect(canSetDeckFullscreen(ready, true)).toBe(true);
    expect(canSetDeckFullscreen({ ...ready, runtimeLoaded: false }, true)).toBe(
      false,
    );
    expect(canSetDeckFullscreen({ ...ready, active: false }, true)).toBe(false);
    expect(canSetDeckFullscreen({ ...ready, viewportReady: false }, true)).toBe(
      false,
    );
  });

  it("keeps host exit available after runtime loss or surface deactivation", () => {
    const stranded = {
      active: false,
      runtimeLoaded: false,
      viewportReady: false,
      busy: false,
      current: true,
    } as const;

    expect(canSetDeckFullscreen(stranded, false)).toBe(true);
    expect(shouldExitFullscreenForHiddenDeck(false, true, false)).toBe(true);
    expect(shouldExitFullscreenForHiddenDeck(true, true, false)).toBe(false);
    expect(shouldExitFullscreenForHiddenDeck(false, true, true)).toBe(false);
  });

  it("rejects actions while host state is unknown or a transition is pending", () => {
    expect(
      canSetDeckFullscreen(
        {
          active: true,
          runtimeLoaded: true,
          viewportReady: true,
          busy: false,
          current: null,
        },
        false,
      ),
    ).toBe(false);
    expect(
      canSetDeckFullscreen(
        {
          active: true,
          runtimeLoaded: true,
          viewportReady: true,
          busy: true,
          current: true,
        },
        false,
      ),
    ).toBe(false);
  });

  it("keeps status refresh separate from fullscreen transitions and activation-driven", () => {
    for (const faceplate of [d2Faceplate, q4Faceplate]) {
      const refreshStatus = functionSlice(
        faceplate,
        "async function refreshFullscreenStatus()",
        "async function toggleFullscreen()",
      );

      expect(faceplate).toContain("let fullscreenStatusPending = false;");
      expect(faceplate).toContain(
        "$: if (viewportMounted) void syncViewportAfterSurfaceChange(active);",
      );
      expect(refreshStatus).toContain(
        "if (fullscreenBusy || fullscreenStatusPending) return;",
      );
      expect(refreshStatus).toContain("fullscreenStatusPending = true;");
      expect(refreshStatus).toContain("fullscreenStatusPending = false;");
      expect(refreshStatus).not.toContain("fullscreenBusy = true;");
      expect(refreshStatus).not.toContain("fullscreenBusy = false;");
    }
    const q4SpoutPoll = q4Faceplate.match(
      /spoutPoll = setInterval\(\(\) => \{([\s\S]*?)\}, 250\);/,
    )?.[1];
    expect(q4SpoutPoll).toBeDefined();
    expect(q4SpoutPoll).not.toContain("refreshFullscreenStatus()");
  });
});

function functionSlice(source: string, start: string, end: string): string {
  const startIndex = source.indexOf(start);
  const endIndex = source.indexOf(end, startIndex);
  expect(startIndex).toBeGreaterThanOrEqual(0);
  expect(endIndex).toBeGreaterThan(startIndex);
  return source.slice(startIndex, endIndex);
}
