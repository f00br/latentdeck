import { describe, expect, it } from "vitest";
import rendererSource from "./DeckFaceplateRenderer.svelte?raw";
import workspaceSource from "./GenericDeckWorkspace.svelte?raw";
import {
  currentlyPlayingReadout,
  createDeckSourceTruthState,
  deckSourceResolutionRetryDelay,
  deckSourceDraftDiffers,
  describeCurrentlyPlayingSource,
  markDeckSourceDraftEdited,
  reconcileDeckSourceTruth,
  resolvePlayingSourceView,
  shouldShowNextLoadDraftReadout,
} from "./deck-source-truth";
import type { CartridgeView } from "./library-model";

const A = "a".repeat(64);
const B = "b".repeat(64);
const C = "c".repeat(64);
const D = "d".repeat(64);
const E = "e".repeat(64);

describe("Deck runtime source truth", () => {
  it("synchronizes an untouched D2 draft when a direct host load arrives", () => {
    const incoming = [A, B] as const;
    const result = reconcileDeckSourceTruth(
      createDeckSourceTruthState(),
      [C, D] as const,
      incoming,
    );

    expect(result.draftArchiveSha256s).toEqual(incoming);
    expect(result.synchronized).toBe(true);
    expect(result.state).toEqual({
      loadedArchiveSha256s: incoming,
      draftEditedAfterLoad: false,
    });
  });

  it("keeps a deliberate next-load draft across repeated and replacement Q4 status events", () => {
    const loaded = [A, B, C, D] as const;
    const initial = reconcileDeckSourceTruth(
      createDeckSourceTruthState(),
      ["", "", "", ""] as const,
      loaded,
    );
    const editedDraft = [A, B, C, E] as const;
    const edited = markDeckSourceDraftEdited(initial.state, editedDraft);

    const repeated = reconcileDeckSourceTruth(edited, editedDraft, loaded);
    expect(repeated.draftArchiveSha256s).toEqual(editedDraft);
    expect(repeated.synchronized).toBe(false);
    expect(repeated.state.draftEditedAfterLoad).toBe(true);

    const directReplacement = reconcileDeckSourceTruth(
      repeated.state,
      editedDraft,
      [B, C, D, A] as const,
    );
    expect(directReplacement.draftArchiveSha256s).toEqual(editedDraft);
    expect(directReplacement.state.loadedArchiveSha256s).toEqual([B, C, D, A]);
    expect(
      deckSourceDraftDiffers(
        directReplacement.draftArchiveSha256s,
        directReplacement.state.loadedArchiveSha256s,
      ),
    ).toBe(true);
  });

  it("clears divergence when a successful Load acknowledges the draft hashes", () => {
    const loaded = [A, B] as const;
    const initial = reconcileDeckSourceTruth(
      createDeckSourceTruthState(),
      [A, B] as const,
      loaded,
    );
    const draft = [C, D] as const;
    const edited = markDeckSourceDraftEdited(initial.state, draft);

    const acknowledged = reconcileDeckSourceTruth(edited, draft, draft);
    expect(acknowledged.draftArchiveSha256s).toEqual(draft);
    expect(acknowledged.state.draftEditedAfterLoad).toBe(false);
    expect(
      deckSourceDraftDiffers(
        acknowledged.draftArchiveSha256s,
        acknowledged.state.loadedArchiveSha256s,
      ),
    ).toBe(false);
  });

  it("resolves and describes the exact playing Library identity", () => {
    const wrong = cartridge("stale-draft.lc", "stale", B, 448, 800, 107);
    const expected = cartridge(
      "playing-source.lc",
      "playing",
      A,
      1344,
      768,
      243,
    );
    const status = {
      cartridgeId: "playing",
      archiveSha256: A,
      latentSlotCount: "72",
    };

    const resolved = resolvePlayingSourceView(status, [wrong, expected]);
    expect(resolved).toBe(expected);
    expect(describeCurrentlyPlayingSource(status, resolved)).toBe(
      `playing-source.lc · ${A.slice(0, 12)}… · 1344×768 · 243 FRAMES · 72 LATENT SLOTS`,
    );
    expect(describeCurrentlyPlayingSource(status, undefined)).toBe(
      `playing · ${A.slice(0, 12)}… · LIBRARY DETAILS UNAVAILABLE · 72 LATENT SLOTS`,
    );
  });

  it("keeps an incompatible next-load draft separate from the prominent runtime readout", () => {
    const playing = cartridge("playing-wide.lc", "playing", A, 1344, 768, 243);
    const incompatibleDraft = cartridge(
      "draft-portrait.lc",
      "draft",
      B,
      448,
      800,
      107,
    );
    const status = {
      cartridgeId: playing.cartridgeId,
      archiveSha256: playing.archiveSha256,
      latentSlotCount: "72",
    };

    expect(currentlyPlayingReadout(status, playing)).toEqual({
      codecLabel: "h3",
      geometryLabel: "1344×768",
      frameLabel: "243 DECODED FRAMES",
      latentLabel: "72 LATENT SLOTS",
    });
    expect(
      shouldShowNextLoadDraftReadout(
        status,
        incompatibleDraft.archiveSha256,
        incompatibleDraft,
      ),
    ).toBe(true);
    expect(describeCurrentlyPlayingSource(status, playing)).toContain(
      "playing-wide.lc",
    );
    expect(describeCurrentlyPlayingSource(status, playing)).not.toContain(
      "draft-portrait.lc",
    );
  });

  it("never turns an active unresolved runtime source into a dash or zero-frame readout", () => {
    const status = {
      cartridgeId: "playing",
      archiveSha256: A,
      latentSlotCount: "32",
    };

    expect(currentlyPlayingReadout(status, undefined)).toEqual({
      codecLabel: "RUNTIME SOURCE VERIFIED",
      geometryLabel: "RUNTIME SOURCE",
      frameLabel: "32 LATENT SLOTS",
      latentLabel: "32 LATENT SLOTS",
    });
    expect(shouldShowNextLoadDraftReadout(status, A, undefined)).toBe(true);
  });

  it("bounds friendly source-metadata resolution retries", () => {
    expect([
      deckSourceResolutionRetryDelay(0),
      deckSourceResolutionRetryDelay(1),
      deckSourceResolutionRetryDelay(2),
      deckSourceResolutionRetryDelay(3),
    ]).toEqual([150, 500, 1_500, null]);
    expect(deckSourceResolutionRetryDelay(-1)).toBeNull();
  });

  it("keeps exact generic draft identities separate from runtime session identities", () => {
    expect(rendererSource).toContain("draft.sourceArchiveSha256s");
    expect(rendererSource).toContain("onDraftChange(cloneDraft(draft))");
    expect(workspaceSource).toContain("session?.runtime.status.playheads");
    expect(workspaceSource).toContain(
      "candidate.archiveSha256 === cartridge.archiveSha256",
    );
    expect(workspaceSource).toContain(
      "candidate.cartridgeId === cartridge.cartridgeId",
    );
    expect(workspaceSource).toContain("library_resolve_preset_sources");
    expect(workspaceSource).not.toMatch(/loadedSourceBySlot|sourceHashBySlot/);
  });
});

function cartridge(
  fileName: string,
  cartridgeId: string,
  archiveSha256: string,
  decodedWidth: number,
  decodedHeight: number,
  decodedFrameCount: number,
): CartridgeView {
  return {
    archiveSha256,
    cartridgeId,
    codecFamily: "minimax_h3",
    codecProfile: "h3",
    codecProfileVersion: "0.1.0",
    timingContract: "h3",
    timingContractVersion: "0.1.0",
    frameRateNumerator: 24,
    frameRateDenominator: 1,
    decodedWidth,
    decodedHeight,
    decodedFrameCount,
    durationNumerator: decodedFrameCount,
    durationDenominator: 24,
    signalGeometry: {
      codec_family: "minimax_h3",
      profile: "h3",
      profile_version: "0.1.0",
      runtime_dtype: "F16",
      batch: 1,
      latent_channels: 24,
      latent_slots: 72,
      latent_height: 48,
      latent_width: 84,
      decoded_frame_count: decodedFrameCount,
      decoded_height: decodedHeight,
      decoded_width: decodedWidth,
      timing_contract: "h3",
      timing_contract_version: "0.1.0",
      frame_rate: { numerator: 24, denominator: 1 },
    },
    signalPresentation: {
      orientation: decodedWidth > decodedHeight ? "landscape" : "portrait",
      aspect_ratio: { width: decodedWidth, height: decodedHeight },
      decoded_width: decodedWidth,
      decoded_height: decodedHeight,
    },
    signalWorkload: {
      latent_sites_per_slot: null,
      latent_values_per_slot: null,
      latent_values_per_clip: null,
      decoded_pixels_per_frame: null,
    },
    favorite: false,
    tags: [],
    availability: "present",
    paths: [
      {
        path: fileName,
        fileName,
        state: "present",
        warningCode: null,
      },
    ],
  };
}
