import { describe, expect, it } from "vitest";
import appSource from "./App.svelte?raw";
import managerSource from "./ExtensionsManager.svelte?raw";

describe("LatentDeck Extensions Manager source contract", () => {
  it("exposes Extensions as a fourth host surface", () => {
    expect(appSource).toContain('selectSurface("extensions")');
    expect(appSource).toContain("<ExtensionsManager");
  });

  it("uses only the closed local lifecycle commands", () => {
    for (const command of [
      "extensions_snapshot",
      "extensions_inspect",
      "extensions_install",
      "extensions_verify",
      "extensions_enable",
      "extensions_disable",
      "extensions_remove",
      "extensions_repair",
    ]) {
      expect(managerSource).toContain(command);
    }
    expect(managerSource).toContain('extensions: ["ld", "ldcodec"]');
    expect(managerSource).not.toMatch(/https?:\/\//);
    expect(managerSource).not.toContain("newest");
    expect(managerSource).not.toContain("auto-update");
  });

  it("requires exact SHA confirmation and labels publisher identity", () => {
    expect(managerSource).toContain("shaConfirmationMatches");
    expect(managerSource).toContain("self-declared");
    expect(managerSource).toContain("allowCorrupt");
  });
});
