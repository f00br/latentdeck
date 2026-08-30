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
});
