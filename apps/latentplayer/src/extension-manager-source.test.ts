import { describe, expect, it } from "vitest";
import appSource from "./App.svelte?raw";

describe("LatentPlayer Extensions Manager source contract", () => {
  it("uses only the closed local lifecycle command surface", () => {
    for (const command of [
      "extensions_snapshot",
      "extensions_inspect",
      "extensions_install",
      "extensions_verify",
      "extensions_enable",
      "extensions_disable",
      "extensions_remove",
      "extensions_repair",
      "player_select_codec_exact",
    ]) {
      expect(appSource).toContain(`"${command}"`);
    }

    expect(appSource).not.toContain('"extensions_update"');
    expect(appSource).not.toContain("Install from URL");
    expect(appSource).not.toContain("newestVersion");
  });

  it("opens only local .ld and .ldcodec archives and confirms exact SHA bytes", () => {
    expect(appSource).toContain('extensions: ["ld", "ldcodec"]');
    expect(appSource).toContain("expectedSha256: installExpectedSha256");
    expect(appSource).toContain("expectedSha256: repairExpectedSha256");
    expect(appSource).toContain("Exact lowercase SHA-256");
    expect(appSource).toContain("shaConfirmationMatches(");
  });

  it("keeps exact package actions and repair inspection explicit", () => {
    expect(appSource).toContain("{ package: summary.package }");
    expect(appSource).toContain(
      "inspectionMatchesPackage(repairInspection, repairTarget.package)",
    );
    expect(appSource).toContain("allowCorrupt:");
    expect(appSource).toContain('inspectExtensionArchive("repair")');
    expect(appSource).toContain("Publisher identity is self-declared");
    expect(appSource).toContain("compatibilityReasonLabel(pair.reason)");
  });

  it("selects one healthy enabled codec version and an explicit device for Player", () => {
    expect(appSource).toContain("Use in Player");
    expect(appSource).toContain('<option value="cpu">CPU</option>');
    expect(appSource).toContain('<option value="cuda">CUDA</option>');
    expect(appSource).toContain("packageId: summary.package.packageId");
    expect(appSource).toContain(
      "packageVersion: summary.package.packageVersion",
    );
    expect(appSource).toContain("device,");
    expect(appSource).toContain('await invoke<PlayerView>("player_snapshot")');
    expect(appSource).toContain(
      "player = acceptTrustedSnapshot(player, snapshot)",
    );
  });
});
