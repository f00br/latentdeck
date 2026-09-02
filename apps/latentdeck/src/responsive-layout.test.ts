import { describe, expect, it } from "vitest";
import tauriConfigSource from "../src-tauri/tauri.conf.json?raw";
import rendererSource from "./DeckFaceplateRenderer.svelte?raw";
import workspaceSource from "./GenericDeckWorkspace.svelte?raw";

const tauriConfig = JSON.parse(tauriConfigSource) as {
  app: { windows: Array<{ minWidth: number }> };
};

function mediaBlock(source: string, maximumWidth: number): string {
  const marker = `@media (max-width: ${maximumWidth}px)`;
  const start = source.indexOf(marker);
  if (start < 0) return "";
  return source.slice(start);
}

describe("LatentDeck minimum-window layout contract", () => {
  it("keeps the desktop minimum at the audited 960px contract", () => {
    expect(tauriConfig.app.windows[0]?.minWidth).toBe(960);
  });

  it("collapses generic host tools before the 960px app minimum", () => {
    const responsive = mediaBlock(workspaceSource, 1180);
    expect(responsive).toContain(".host-tools");
    expect(responsive).toContain("grid-template-columns: 1fr 1fr;");
    expect(responsive).toContain(".preset-tools");
    expect(responsive).toContain(".spout-tools");
    expect(responsive).toContain(".recording-tools");
  });

  it("keeps every Deck output inside one revisioned native viewport", () => {
    expect(rendererSource).toContain("data-native-viewport={model.exactKey}");
    expect(workspaceSource).toContain("new ResizeObserver(");
    expect(workspaceSource).toContain(
      'globalThis.addEventListener("scroll", resize, true)',
    );
    expect(workspaceSource).toContain("genericDeckClient.viewportSetBounds(");
    expect(workspaceSource).toContain("hiddenEmbeddedViewportBounds(");
    expect(rendererSource).toMatch(
      /\.monitor-section\s*\{[^}]*position:\s*sticky;[^}]*top:\s*0;/s,
    );
    expect(rendererSource).not.toMatch(
      /<canvas|<video|ImageData|createImageBitmap/i,
    );
  });

  it("gives the declarative monitor the only fullscreen faceplate row", () => {
    expect(rendererSource).toContain(
      "class:fullscreen={active && outputFullscreen === true && runtimeLoaded}",
    );
    expect(rendererSource).toContain(
      ".fullscreen .faceplate-section:not(.monitor-section)",
    );
    expect(rendererSource).toContain(".fullscreen .monitor-section");
    expect(workspaceSource).toContain("scheduleViewportSync();");
  });

  it("keeps generic output visible while declarative controls scroll", () => {
    expect(rendererSource).toMatch(
      /\.monitor-section\s*\{[^}]*position:\s*sticky;[^}]*top:\s*0;/s,
    );
    expect(workspaceSource).toContain("viewportEpoch");
    expect(workspaceSource).toContain("viewportApplied = bounds");
  });
});
