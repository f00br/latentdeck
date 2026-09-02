import { describe, expect, it } from "vitest";
import rendererSource from "./DeckFaceplateRenderer.svelte?raw";
import workspaceSource from "./GenericDeckWorkspace.svelte?raw";

describe("generic embedded native-output contract", () => {
  it("owns a revisioned in-faceplate viewport without a browser fallback", () => {
    expect(rendererSource).toContain("data-native-viewport={model.exactKey}");
    expect(workspaceSource).toContain("new ResizeObserver(");
    expect(workspaceSource).toContain(
      'globalThis.addEventListener("scroll", resize, true)',
    );
    expect(workspaceSource).toContain("genericDeckClient.viewportSetBounds(");
    expect(rendererSource).not.toMatch(
      /<canvas|<video|ImageData|createImageBitmap/i,
    );
  });

  it("hides the child surface when the exact Deck is inactive or torn down", () => {
    expect(workspaceSource).toContain(
      "active && selectedSession?.foreground === true && inside",
    );
    expect(workspaceSource).toContain("hiddenEmbeddedViewportBounds(");
    expect(workspaceSource).toContain("await hideViewport();");
    expect(workspaceSource).toContain("embeddedViewportFullyInsideClient(");
  });

  it("acknowledges viewport placement for the foreground generic session", () => {
    const beginIndex = workspaceSource.indexOf(
      "await genericDeckClient.viewportSessionBegin(requested)",
    );
    const invokeIndex = workspaceSource.indexOf(
      "await genericDeckClient.viewportSetBounds(sessionId, bounds)",
    );
    const appliedIndex = workspaceSource.indexOf("viewportApplied = bounds;");
    expect(beginIndex).toBeGreaterThan(-1);
    expect(invokeIndex).toBeGreaterThan(-1);
    expect(appliedIndex).toBeGreaterThan(invokeIndex);
    expect(workspaceSource).toContain("embeddedViewportFullyInsideClient(");
    expect(rendererSource).toContain("disabled={!runtimeAvailable");
    expect(rendererSource).toContain("!sourceDraftReady");
  });

  it("makes native video the sole declarative surface in fullscreen", () => {
    expect(workspaceSource).toContain("scheduleViewportSync();");
    expect(rendererSource).toContain(
      ".fullscreen .faceplate-section:not(.monitor-section)",
    );
    expect(rendererSource).toMatch(
      /\.fullscreen \.monitor\s*\{[^}]*min-height:\s*0;/s,
    );
  });
});
