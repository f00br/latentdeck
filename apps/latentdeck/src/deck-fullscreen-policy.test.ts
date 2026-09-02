import { describe, expect, it, vi } from "vitest";
import genericWorkspace from "./GenericDeckWorkspace.svelte?raw";
import {
  canSetDeckFullscreen,
  handleDeckFullscreenKeydown,
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

  it("keeps generic status polling separate from explicit fullscreen transitions", () => {
    const poll = functionSlice(
      genericWorkspace,
      "async function pollForegroundState()",
      "async function captureAction(",
    );
    const toggle = functionSlice(
      genericWorkspace,
      "async function toggleFullscreen()",
      "async function run(",
    );

    expect(genericWorkspace).toContain(
      "$: if (active && selectedSession?.sessionId !== viewportSessionId)",
    );
    expect(poll).toContain("fullscreenStatusGet(sessionId)");
    expect(poll).not.toContain("fullscreenSet(");
    expect(toggle).toContain("genericDeckClient.fullscreenSet(");
    expect(genericWorkspace).toContain("await hideViewport();");
  });

  it("routes Escape through the serialized exit path even after output loss", () => {
    const exit = vi.fn();
    const event = new KeyboardEvent("keydown", {
      key: "Escape",
      cancelable: true,
    });

    expect(
      handleDeckFullscreenKeydown(
        event,
        {
          active: false,
          runtimeLoaded: false,
          viewportReady: false,
          busy: false,
          current: true,
        },
        exit,
      ),
    ).toBe(true);
    expect(event.defaultPrevented).toBe(true);
    expect(exit).toHaveBeenCalledOnce();

    const handler = functionSlice(
      genericWorkspace,
      "function handleWindowKeydown(event: KeyboardEvent)",
      "async function run(",
    );
    expect(genericWorkspace).toContain(
      "<svelte:window onkeydown={handleWindowKeydown} />",
    );
    expect(handler).toContain("handleDeckFullscreenKeydown(");
    expect(handler).toContain("void toggleFullscreen()");
  });
});

function functionSlice(source: string, start: string, end: string): string {
  const startIndex = source.indexOf(start);
  const endIndex = source.indexOf(end, startIndex);
  expect(startIndex).toBeGreaterThanOrEqual(0);
  expect(endIndex).toBeGreaterThan(startIndex);
  return source.slice(startIndex, endIndex);
}
