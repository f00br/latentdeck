import { describe, expect, it } from "vitest";
import d2Faceplate from "./D2Faceplate.svelte?raw";
import q4Faceplate from "./Q4Faceplate.svelte?raw";

describe("Deck user-facing runtime status copy", () => {
  it("keeps causal generations and protocol wording out of the normal UI", () => {
    for (const faceplate of [d2Faceplate, q4Faceplate]) {
      expect(faceplate).not.toContain("Causal state ready");
      expect(faceplate).not.toContain("waiting for causal reset barrier");
      expect(faceplate).not.toContain("status.streamGeneration");
      expect(faceplate).not.toContain("status.streamSequence");
      expect(faceplate).toContain("Restarting playback…");
    }

    expect(d2Faceplate).not.toContain(
      "hostMessage = `${incoming.code}: ${incoming.detail}`",
    );
    expect(q4Faceplate).not.toContain(
      "hostMessage = `${incoming.code}: ${incoming.detail}`",
    );
    for (const faceplate of [d2Faceplate, q4Faceplate]) {
      expect(faceplate).not.toContain(
        "detail: `${incoming.code}: ${incoming.detail}`",
      );
    }
  });
});
