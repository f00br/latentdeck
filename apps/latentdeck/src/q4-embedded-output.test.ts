import { describe, expect, it } from "vitest";
import faceplateSource from "./Q4Faceplate.svelte?raw";

describe("Q4 embedded native-output contract", () => {
  it("owns a revisioned in-faceplate viewport without a browser fallback", () => {
    expect(faceplateSource).toContain('data-native-viewport="q4"');
    expect(faceplateSource).toContain(
      "new ResizeObserver(scheduleViewportSync)",
    );
    expect(faceplateSource).toContain(
      "new IntersectionObserver(scheduleViewportSync)",
    );
    expect(faceplateSource).toContain(
      'globalThis.addEventListener("scroll", scheduleViewportSync, true)',
    );
    expect(faceplateSource).toContain("q4Client.viewportSetBounds(bounds)");
    expect(faceplateSource).not.toMatch(
      /<canvas|<video|ImageData|createImageBitmap/i,
    );
  });

  it("hides the child surface when Q4 is inactive or torn down", () => {
    expect(faceplateSource).toContain("active &&");
    expect(faceplateSource).toContain("hiddenEmbeddedViewportBounds(");
    expect(faceplateSource).toContain(
      "void q4Client.viewportSetBounds(hidden).catch(() => undefined)",
    );
    expect(faceplateSource).toContain("anchor.offsetParent !== null");
    expect(faceplateSource).toContain("fullyInsideClient");
  });

  it("acknowledges viewport placement before enabling Q4 Load", () => {
    const beginIndex = faceplateSource.indexOf(
      "await q4Client.viewportSessionBegin();",
    );
    const invokeIndex = faceplateSource.indexOf(
      "await q4Client.viewportSetBounds(bounds);",
    );
    const appliedIndex = faceplateSource.indexOf("viewportApplied = bounds;");
    expect(beginIndex).toBeGreaterThan(-1);
    expect(invokeIndex).toBeGreaterThan(-1);
    expect(appliedIndex).toBeGreaterThan(invokeIndex);
    expect(faceplateSource).toContain(": !viewportReady");
    expect(faceplateSource).toContain(
      "sameEmbeddedViewportGeometry(viewportApplied, bounds)",
    );
    expect(faceplateSource).toContain("embeddedViewportFullyInsideClient(");
    expect(faceplateSource).toContain("data-viewport-ready={viewportReady}");
    expect(faceplateSource).toContain(
      "data-all-sources-ready={allSourcesReady}",
    );
    expect(faceplateSource).toContain("disabled={loadGateReason !== null}");
    expect(faceplateSource).toContain("Embedded Q4 output is not ready");
  });

  it("makes native video the sole faceplate surface in main-window fullscreen", () => {
    expect(faceplateSource).toContain("await tick();");
    expect(faceplateSource).toContain("scheduleViewportSync();");
    expect(faceplateSource).toMatch(
      /\.q4-faceplate\.output-fullscreen\s*\{[^}]*grid-template-rows:\s*minmax\(0, 1fr\);/s,
    );
    expect(faceplateSource).toMatch(
      /\.output-fullscreen \.q4-output-monitor\s*\{[^}]*grid-template-rows:\s*minmax\(0, 1fr\);/s,
    );
  });
});
