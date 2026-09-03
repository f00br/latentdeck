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
      "const visible = active && !viewportSuspended && inside",
    );
    expect(workspaceSource).toContain("hiddenEmbeddedViewportBounds(");
    expect(workspaceSource).toContain("await hideViewport();");
    expect(workspaceSource).toContain("embeddedViewportFullyInsideClient(");
  });

  it("acknowledges global viewport placement before a first generic session", () => {
    const beginIndex = workspaceSource.indexOf(
      "await genericDeckClient.viewportSessionBegin()",
    );
    const syncIndex = workspaceSource.indexOf("async function syncViewport()");
    const invokeIndex = workspaceSource.indexOf(
      "await genericDeckClient.viewportSetBounds(bounds)",
      syncIndex,
    );
    const appliedIndex = workspaceSource.indexOf(
      "confirmViewportBounds(bounds)",
      invokeIndex,
    );
    expect(beginIndex).toBeGreaterThan(-1);
    expect(syncIndex).toBeGreaterThan(beginIndex);
    expect(invokeIndex).toBeGreaterThan(-1);
    expect(appliedIndex).toBeGreaterThan(invokeIndex);
    expect(workspaceSource).toContain("viewportApplied = bounds;");
    expect(workspaceSource).toContain("embeddedViewportFullyInsideClient(");
    expect(rendererSource).toContain("disabled={!runtimeAvailable");
    expect(rendererSource).toContain("!sourceDraftReady");
    expect(workspaceSource).toContain(
      "sessionCapacityAvailable && viewportApplied?.visible === true",
    );
    expect(workspaceSource).toContain(
      'errorCode = "output.viewport_not_ready"',
    );
  });

  it("bounds bootstrap retries and exposes a fresh recovery trigger", () => {
    expect(workspaceSource).toContain(
      "const VIEWPORT_RETRY_DELAYS_MS = [100, 250, 500] as const",
    );
    expect(workspaceSource).toContain("viewportRetryExhausted = true");
    expect(workspaceSource).toContain(
      "const resize = () => requestViewportRecovery()",
    );
    expect(workspaceSource).toContain("resetViewportBootstrap();");
  });

  it("makes native video the sole declarative surface in fullscreen", () => {
    expect(workspaceSource).toContain("scheduleViewportSync();");
    expect(rendererSource).toContain(".fullscreen .control-column");
    expect(rendererSource).toContain(".fullscreen .output-actions");
    expect(rendererSource).toContain(":global(body.deck-output-fullscreen)");
    expect(rendererSource).toMatch(
      /\.fullscreen \.monitor\s*\{[^}]*min-height:\s*0;/s,
    );
  });
});
