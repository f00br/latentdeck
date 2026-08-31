import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";
import tauriConfigSource from "../src-tauri/tauri.conf.json?raw";
import appSource from "./App.svelte?raw";

const stylesSource = readFileSync(resolve("src/styles.css"), "utf8");

const tauriConfig = JSON.parse(tauriConfigSource) as {
  app: { windows: Array<{ minWidth: number; minHeight: number }> };
};

describe("LatentPlayer single-window layout contract", () => {
  it("keeps the audited 720 by 480 minimum window", () => {
    expect(tauriConfig.app.windows[0]).toMatchObject({
      minWidth: 720,
      minHeight: 480,
    });
  });

  it("reserves and revision-syncs a native viewport", () => {
    expect(appSource).toContain("data-native-viewport");
    expect(appSource).toContain("new ResizeObserver(scheduleViewportSync)");
    expect(appSource).toContain("requestAnimationFrame");
    expect(appSource).toContain(
      'invoke("player_viewport_set_bounds", { bounds })',
    );
  });

  it("retries rejected desired bounds without marking them applied", () => {
    const invokeIndex = appSource.indexOf(
      'await invoke("player_viewport_set_bounds", { bounds });',
    );
    const appliedIndex = appSource.indexOf("viewportApplied = bounds;");

    expect(invokeIndex).toBeGreaterThan(-1);
    expect(appliedIndex).toBeGreaterThan(invokeIndex);
    expect(appSource).toContain(
      "const VIEWPORT_RETRY_DELAYS_MS = [250, 1000, 2500] as const;",
    );
    expect(appSource).toContain(
      "sameNativeViewportGeometry(viewportDesired, bounds)",
    );
    expect(appSource).toContain(
      "viewportDesired?.revision !== bounds.revision",
    );
    expect(appSource).toContain("viewportQueued = bounds;");
    expect(appSource).toContain("globalThis.setTimeout(() => {");
    expect(appSource).not.toContain("viewportLast");
  });

  it("keeps decoded presentation outside browser media fallbacks", () => {
    expect(appSource).not.toMatch(
      /<canvas|<video|ImageData|createImageBitmap/i,
    );
    expect(appSource).toContain('class="native-viewport-anchor"');
  });

  it("contains page scrolling and reflows details at the compact width", () => {
    expect(stylesSource).toContain("height: 100dvh;");
    expect(stylesSource).toContain("overflow: hidden;");
    expect(stylesSource).toContain(".utility-rail {");
    expect(stylesSource).toContain("overflow-y: auto;");
    expect(stylesSource).toContain("@media (max-width: 800px)");
  });

  it("keeps the native viewport on the only fullscreen monitor row", () => {
    expect(stylesSource).toMatch(
      /\.fullscreen-shell \.output-monitor\s*\{[^}]*grid-template-rows:\s*minmax\(0, 1fr\);/s,
    );
    expect(appSource).toContain("await tick();");
    expect(appSource).toContain('code.startsWith("output.viewport_")');
  });

  it("offers a complete PLAY and PREPARE workflow without codec dependency", () => {
    expect(appSource).toContain('type WorkspaceMode = "play" | "prepare"');
    expect(appSource).toContain("Add raw files");
    expect(appSource).toContain("Add folder");
    expect(appSource).toContain("Include nested folders");
    expect(appSource).toContain("Choose output folder");
    expect(appSource).toContain("Stop after current");
    expect(appSource).toContain("Open in Player");
    expect(appSource).toContain(
      'invoke<ConversionSnapshot>("player_conversion_plan"',
    );
    expect(appSource).toContain('invoke("player_conversion_start")');
    expect(stylesSource).toContain(".prepare-workspace {");
    expect(stylesSource).toContain(".conversion-queue {");
  });

  it("renders every preflight identity and latent-shape field promised by UAT", () => {
    expect(appSource).toContain("item.metadata.latentSlots");
    expect(appSource).toContain("item.metadata.latentHeight");
    expect(appSource).toContain("item.metadata.latentWidth");
    expect(appSource).toContain("item.metadata.payloadSha256");
    expect(appSource).toContain('class="item-fingerprint"');
    expect(appSource).toContain("conversionCancelledCount(conversion)");
  });
});
