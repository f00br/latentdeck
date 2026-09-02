import { describe, expect, it } from "vitest";
import rendererSource from "./DeckFaceplateRenderer.svelte?raw";
import workspaceSource from "./GenericDeckWorkspace.svelte?raw";

describe("Deck user-facing runtime status copy", () => {
  it("renders bounded generic runtime state without package-specific protocol prose", () => {
    expect(rendererSource).not.toContain("Causal state ready");
    expect(rendererSource).not.toContain("waiting for causal reset barrier");
    expect(rendererSource).not.toMatch(/D2|Q4|Protocol 1/);
    expect(workspaceSource).toContain("status.state");
    expect(workspaceSource).toContain("selectedSession.runtime.faultCode");
    expect(workspaceSource).not.toContain(
      "hostMessage = `${incoming.code}: ${incoming.detail}`",
    );
  });
});
