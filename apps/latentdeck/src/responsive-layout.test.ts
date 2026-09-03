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

  it("collapses the compact technical rack before the app minimum", () => {
    const responsive = mediaBlock(workspaceSource, 1180);
    expect(responsive).toContain(".runtime-config > header");
    expect(responsive).toContain(".runtime-message");
    expect(responsive).toContain(".config-grid");
    expect(responsive).toContain("grid-template-columns: 1fr 1fr;");
  });

  it("keeps every Deck output inside one revisioned native viewport", () => {
    expect(rendererSource).toContain("data-native-viewport={model.exactKey}");
    expect(workspaceSource).toContain("new ResizeObserver(");
    expect(workspaceSource).toContain(
      'globalThis.addEventListener("scroll", resize, true)',
    );
    expect(workspaceSource).toContain("genericDeckClient.viewportSetBounds(");
    expect(workspaceSource).toContain("hiddenEmbeddedViewportBounds(");
    expect(rendererSource).toContain('data-workbench-region="output"');
    expect(rendererSource).toMatch(
      /\.output-column\s*\{[^}]*position:\s*sticky;[^}]*top:\s*var\(--output-sticky-top\);/s,
    );
    expect(rendererSource).toContain('data-output-persistence="sticky"');
    expect(rendererSource).not.toMatch(
      /<canvas|<video|ImageData|createImageBitmap/i,
    );
  });

  it("keeps the professional faceplate dense across both generic workbench columns", () => {
    expect(rendererSource).toContain('data-layout-density="compact"');
    expect(rendererSource).toMatch(
      /\.declarative-deck\s*\{[^}]*--control-height:\s*26px;[^}]*--control-font-size:\s*0\.64rem;/s,
    );
    expect(rendererSource).toMatch(
      /\.widget-slider label\s*\{[^}]*grid-template-columns:/s,
    );
    expect(rendererSource).toMatch(
      /\.widget-seed > label\s*\{[^}]*grid-template-columns:/s,
    );
    expect(rendererSource).toMatch(
      /\.transport-grid\s*\{[^}]*minmax\(78px, 1fr\)/s,
    );
    expect(rendererSource).toMatch(
      /\.capture-status\s*\{[^}]*grid-template-columns:\s*auto minmax\(0, 1fr\);[^}]*min-height:\s*1rem;/s,
    );
  });

  it("gives the declarative monitor the only fullscreen faceplate row", () => {
    expect(rendererSource).toContain("class:fullscreen={fullscreenActive}");
    expect(rendererSource).toContain("deck-output-fullscreen");
    expect(rendererSource).toContain(".fullscreen .control-column");
    expect(rendererSource).toContain(".fullscreen .output-actions");
    expect(rendererSource).toContain(":global(html.deck-output-fullscreen)");
    expect(rendererSource).toContain(":global(body.deck-output-fullscreen)");
    expect(workspaceSource).toContain("scheduleViewportSync();");
  });

  it("uses a two-region workbench and keeps transient capture copy out of output geometry", () => {
    expect(rendererSource).toContain('data-workbench-region="output-actions"');
    expect(rendererSource).toContain('data-workbench-region="controls"');
    expect(rendererSource).toMatch(
      /\.deck-workbench\s*\{[^}]*grid-template-columns:/s,
    );
    expect(rendererSource).toContain('class="capture-reason"');
    expect(rendererSource).toMatch(/\.capture-reason\s*\{[^}]*min-height:/s);
    const responsive = mediaBlock(rendererSource, 820);
    expect(responsive).toContain(".deck-workbench");
    expect(responsive).toContain("grid-template-columns: minmax(0, 1fr);");
    expect(rendererSource).not.toContain("@media (max-width: 1120px)");
    expect(workspaceSource).toContain("viewportEpoch");
    expect(workspaceSource).toContain("viewportApplied = bounds");
  });
});
