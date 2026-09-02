import { describe, expect, it } from "vitest";

import appSource from "./App.svelte?raw";
import rendererSource from "./DeckFaceplateRenderer.svelte?raw";
import workspaceSource from "./GenericDeckWorkspace.svelte?raw";
import clientSource from "./generic-deck-client.ts?raw";

describe("generic Deck production surface", () => {
  it("routes bundled D2/Q4 and installed external Decks through one declarative renderer", () => {
    expect(appSource).toContain("deckCatalog.decks");
    expect(appSource).toContain("<GenericDeckWorkspace");
    expect(appSource).not.toMatch(/D2Faceplate|Q4Faceplate/);
    expect(appSource).not.toMatch(/d2Client|q4Client/);
    expect(appSource).not.toMatch(
      /activeSurface\s*===\s*["'](?:d2|q4|external)["']/,
    );
    expect(appSource).not.toMatch(/BUNDLED_(?:D2|Q4)_EXACT_KEY/);
  });

  it("uses only generic Tauri namespaces with exact versions and no fallback", () => {
    expect(clientSource).toContain('open: "deck_generic_open"');
    expect(clientSource).toContain(
      'runtimeOptions: "deck_generic_runtime_options"',
    );
    expect(clientSource).toContain(
      'externalAssetSelect: "deck_generic_external_asset_select"',
    );
    expect(clientSource).not.toMatch(/deck_d2_|deck_q4_/);
    expect(clientSource).not.toMatch(/fallback|newest|unique/i);
    expect(workspaceSource).toContain("deckVersion: model.deckVersion");
    expect(workspaceSource).toContain(
      "codecVersion: selectedCodec.codecVersion",
    );
    expect(workspaceSource).toContain("profileKey: selectedProfile");
    expect(workspaceSource).toContain(
      "if (extensionsRefreshPending !== null) return extensionsRefreshPending",
    );
  });

  it("exposes explicit four-session foreground and close operations", () => {
    expect(workspaceSource).toContain("MAX_WARM_DECK_SESSIONS");
    expect(workspaceSource).toContain("genericDeckClient.foregroundSet");
    expect(workspaceSource).toContain("genericDeckClient.close");
    expect(workspaceSource).toContain("session.capacity_exceeded");
    expect(workspaceSource).toContain("session.output_lease_pinned");
  });

  it("rehydrates the selected worker snapshot and shows its immutable negotiated identity", () => {
    expect(workspaceSource).toContain("genericDeckDraftFromSessionSnapshot");
    expect(workspaceSource).toContain("session.profileKey");
    expect(workspaceSource).toContain("session.deviceOrdinal");
    expect(workspaceSource).toContain("session.externalAssets");
    expect(workspaceSource).toContain("sessionSnapshotValid");
    expect(workspaceSource).toContain("New warm-session negotiation");
  });

  it("keeps host-rendered controls, barycentric3, capture and monitor boundaries", () => {
    expect(rendererSource).toContain('widget.kind === "barycentric3"');
    expect(rendererSource).toContain('widget.kind === "capture"');
    expect(rendererSource).toContain('widget.kind === "monitor"');
    expect(workspaceSource).toContain("viewportSetBounds");
    expect(workspaceSource).toContain("spoutConfigure");
    expect(workspaceSource).toContain("captureStart");
    expect(rendererSource).toContain("liveCaptureActive");
    expect(workspaceSource).toContain("recordingStart");
    expect(workspaceSource).toContain("Record MP4");
    expect(workspaceSource).toContain("deck_generic_preset_save");
    expect(workspaceSource).toContain("deck_generic_preset_load");
  });

  it("never gives the web UI local filesystem path authority", () => {
    expect(workspaceSource).not.toMatch(
      /assetPath|localPath|sourcePath|stagingPath/,
    );
    expect(clientSource).not.toMatch(
      /assetPath|localPath|sourcePath|stagingPath/,
    );
    expect(workspaceSource).not.toContain("@tauri-apps/plugin-dialog");
  });
});
