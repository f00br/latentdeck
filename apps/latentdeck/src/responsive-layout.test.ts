import { describe, expect, it } from "vitest";
import tauriConfigSource from "../src-tauri/tauri.conf.json?raw";
import d2Faceplate from "./D2Faceplate.svelte?raw";
import q4Faceplate from "./Q4Faceplate.svelte?raw";

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

  it("collapses D2 Bank content before the 960px app minimum", () => {
    const responsive = mediaBlock(d2Faceplate, 1120);
    expect(responsive).toContain(".d2-bank-strip {");
    expect(responsive).toContain(
      "grid-template-columns: minmax(0, 1fr) minmax(0, 1fr);",
    );
    expect(responsive).toContain(".d2-bank-strip > * {");
    expect(responsive).toContain(".d2-bank-strip select {");
    expect(responsive).toContain(".d2-preset-controls {");
    expect(responsive).toContain("grid-column: 1 / -1;");
  });

  it("keeps D2 decoded output inside a revisioned native viewport", () => {
    expect(d2Faceplate).toContain('data-native-viewport="d2"');
    expect(d2Faceplate).toContain("new ResizeObserver(scheduleViewportSync)");
    expect(d2Faceplate).toContain(
      'globalThis.addEventListener("scroll", scheduleViewportSync, true)',
    );
    expect(d2Faceplate).toContain("d2Client.viewportSetBounds(bounds)");
    expect(d2Faceplate).toContain("hiddenEmbeddedViewportBounds(");
    expect(d2Faceplate).toContain("!viewportReady ||");
    expect(d2Faceplate).toMatch(
      /\.d2-output-monitor\s*\{[^}]*position:\s*sticky;[^}]*top:\s*0;/s,
    );
    expect(d2Faceplate).not.toMatch(
      /<canvas|<video|ImageData|createImageBitmap/i,
    );
  });

  it("gives the D2 viewport the only fullscreen faceplate row", () => {
    expect(d2Faceplate).toContain("class:fullscreen-faceplate={active &&");
    expect(d2Faceplate).toMatch(
      /\.fullscreen-faceplate \.d2-output-monitor\s*\{[^}]*grid-template-rows:\s*minmax\(0, 1fr\);/s,
    );
    expect(d2Faceplate).toContain("await tick();");
    expect(d2Faceplate).toContain("scheduleViewportSync();");
  });

  it("collapses Q4 Bank content before the 960px app minimum", () => {
    const responsive = mediaBlock(q4Faceplate, 1180);
    expect(responsive).toContain(".bank-strip {");
    expect(responsive).toContain(
      "grid-template-columns: minmax(0, 1fr) minmax(0, 1fr);",
    );
    expect(responsive).toContain(".bank-strip > * {");
    expect(responsive).toContain(".bank-strip select {");
    expect(responsive).toContain(".preset-controls {");
    expect(responsive).toContain("grid-column: 1 / -1;");
  });

  it("keeps Q4 output visible while its controls scroll", () => {
    expect(q4Faceplate).toMatch(
      /\.q4-output-monitor\s*\{[^}]*position:\s*sticky;[^}]*top:\s*0;/s,
    );
    expect(q4Faceplate).toContain("viewportEpoch !== null");
    expect(q4Faceplate).toContain("viewportApplied?.visible === true");
  });
});
