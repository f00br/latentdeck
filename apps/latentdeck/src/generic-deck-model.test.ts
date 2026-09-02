import { describe, expect, it } from "vitest";

import d2DeckPack from "../../../operators/builtin/d2/package/deck-pack.json";
import d2Faceplate from "../../../operators/builtin/d2/package/faceplate.json";
import d2Operator from "../../../operators/builtin/d2/package/operator.json";
import {
  MAX_WARM_DECK_SESSIONS,
  buildGenericDeckOpenDraft,
  buildGenericDeckPreset,
  codecOptionsForExactDeck,
  genericDeckDraftFromSessionSnapshot,
  genericDeckDraftFromPreset,
  retainExactSelection,
  sessionCapacityState,
} from "./generic-deck-model";
import { createDeckUiDraft, parseDeckUiCatalog } from "./deck-ui-model";
import type { CartridgeView } from "./library-model";

function model() {
  return parseDeckUiCatalog({
    decks: [
      {
        package: {
          kind: "deck_pack",
          packageId: d2DeckPack.deck_id,
          packageVersion: d2DeckPack.deck_version,
        },
        deck: {
          deckId: d2DeckPack.deck_id,
          deckVersion: d2DeckPack.deck_version,
          displayName: d2DeckPack.display_name,
          summary: d2DeckPack.summary,
          slots: d2DeckPack.signal.slots,
          roles: d2DeckPack.signal.roles.map((role) => ({
            roleId: role.role_id,
            displayName: role.display_name,
          })),
          defaultPermutation: d2DeckPack.signal.default_permutation,
          structuralCarrierRole: d2DeckPack.signal.structural_carrier_role,
          requiredCapabilities: d2DeckPack.signal.required_capabilities,
        },
        operator: {
          operatorId: d2Operator.operator_id,
          controls: d2Operator.controls,
        },
        faceplate: d2Faceplate,
      },
    ],
    issues: [],
  }).decks[0];
}

function cartridge(cartridgeId: string, archiveSha256: string): CartridgeView {
  return {
    archiveSha256,
    cartridgeId,
    codecFamily: "synthetic",
    codecProfile: "grid",
    codecProfileVersion: "1.0.0",
    timingContract: "synthetic_24fps",
    timingContractVersion: "1.0.0",
    frameRateNumerator: 24,
    frameRateDenominator: 1,
    decodedWidth: 64,
    decodedHeight: 64,
    decodedFrameCount: 24,
    durationNumerator: 1,
    durationDenominator: 1,
    signalGeometry: {
      codec_family: "synthetic",
      profile: "grid",
      profile_version: "1.0.0",
      runtime_dtype: "F16",
      batch: 1,
      latent_channels: 24,
      latent_slots: 1,
      latent_height: 30,
      latent_width: 45,
      decoded_frame_count: 24,
      decoded_height: 64,
      decoded_width: 64,
      timing_contract: "synthetic_24fps",
      timing_contract_version: "1.0.0",
      frame_rate: { numerator: 24, denominator: 1 },
    },
    signalPresentation: {
      orientation: "square",
      aspect_ratio: { width: 1, height: 1 },
      decoded_width: 64,
      decoded_height: 64,
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
    paths: [],
  };
}

describe("generic exact Deck frontend model", () => {
  it("never auto-selects a newest or sole remaining exact version", () => {
    expect(retainExactSelection("", ["codec@1.0.0"])).toBe("");
    expect(retainExactSelection("codec@1.0.0", ["codec@1.0.0"])).toBe(
      "codec@1.0.0",
    );
    expect(retainExactSelection("codec@1.0.0", ["codec@2.0.0"])).toBe("");
  });

  it("shows every stable matrix reason for the exact Deck version", () => {
    const deck = {
      kind: "deck_pack" as const,
      packageId: "org.example.deck",
      packageVersion: "1.2.3",
    };
    const pairs = [
      "compatible",
      "untrusted",
      "missing_asset",
      "package_invalid",
      "unsupported_protocol",
      "unsupported_host_api",
      "unsupported_tensor_abi",
      "unsupported_profile",
      "unsupported_signal",
      "unsupported_timing",
      "unsupported_capability",
    ].map((reason, index) => ({
      deck,
      codec: {
        kind: "codec_pack" as const,
        packageId: "org.example.codec",
        packageVersion: `${index + 1}.0.0`,
      },
      reason: reason as Parameters<
        typeof codecOptionsForExactDeck
      >[1][number]["reason"],
      compatibleProfile: null,
    }));

    expect(codecOptionsForExactDeck("org.example.deck@1.2.3", pairs)).toEqual(
      pairs.map((pair) => ({
        exactKey: `${pair.codec.packageId}@${pair.codec.packageVersion}`,
        codecId: pair.codec.packageId,
        codecVersion: pair.codec.packageVersion,
        reason: pair.reason,
      })),
    );
  });

  it("serializes exact sources, closed controls, and one-based physical slots", () => {
    const deck = model();
    const draft = createDeckUiDraft(deck);
    draft.sourceArchiveSha256s = ["a".repeat(64), "b".repeat(64)];
    draft.roleBindings = { carrier: 1, donor: 0 };
    draft.playing = [true, false];
    draft.loops = [false, true];
    draft.seed = 17;
    const sources = [
      cartridge("10000000-0000-4000-8000-000000000001", "a".repeat(64)),
      cartridge("10000000-0000-4000-8000-000000000002", "b".repeat(64)),
    ];

    const open = buildGenericDeckOpenDraft(deck, draft, sources);

    expect(open.sources).toEqual(
      sources.map(({ cartridgeId, archiveSha256 }) => ({
        cartridgeId,
        archiveSha256,
      })),
    );
    expect(open.roles).toEqual([
      { role: "carrier", physical_slot: 2 },
      { role: "donor", physical_slot: 1 },
    ]);
    expect(open.sourceTransport).toEqual([
      { physical_slot: 1, playing: true, loop_enabled: false },
      { physical_slot: 2, playing: false, loop_enabled: true },
    ]);
    expect(open.controls).toHaveLength(deck.controls.length);
    expect(
      open.controls.find((binding) => binding.name === "top_k")?.value,
    ).toEqual({ kind: "integer", value: 8 });
    expect(
      open.controls.find((binding) => binding.name === "algorithm")?.value,
    ).toEqual({ kind: "text", value: "linear" });
  });

  it("writes generic preset v2 with exact Deck and immutable source identities", () => {
    const deck = model();
    const draft = createDeckUiDraft(deck);
    draft.sourceArchiveSha256s = ["a".repeat(64), "b".repeat(64)];
    draft.roleBindings = { carrier: 1, donor: 0 };
    draft.loops = [false, true];
    draft.seed = 91;
    const sources = [
      cartridge("10000000-0000-4000-8000-000000000001", "a".repeat(64)),
      cartridge("10000000-0000-4000-8000-000000000002", "b".repeat(64)),
    ];

    const preset = buildGenericDeckPreset(deck, draft, sources, "collection-1");

    expect(preset.deck_id).toBe("org.latentdeck.deck.d2");
    expect(preset.deck_version).toBe("0.2.0");
    expect(preset.slots[0]).toEqual({
      physical_slot: 1,
      source: {
        cartridge_id: sources[0].cartridgeId,
        archive_sha256: sources[0].archiveSha256,
      },
    });
    expect(preset.roles).toEqual({ carrier: 2, donor: 1 });
    expect(preset.controls.algorithm).toEqual({
      type: "enum",
      value: "linear",
    });

    expect(genericDeckDraftFromPreset(deck, preset, sources)).toEqual({
      sourceArchiveSha256s: sources.map((source) => source.archiveSha256),
      controls: draft.controls,
      roleBindings: { carrier: 1, donor: 0 },
      playing: [true, true],
      loops: [false, true],
      seed: 91,
    });
  });

  it("rejects preset version drift instead of applying another installed Deck version", () => {
    const deck = model();
    const draft = createDeckUiDraft(deck);
    draft.sourceArchiveSha256s = ["a".repeat(64), "b".repeat(64)];
    const sources = [
      cartridge("10000000-0000-4000-8000-000000000001", "a".repeat(64)),
      cartridge("10000000-0000-4000-8000-000000000002", "b".repeat(64)),
    ];
    const preset = buildGenericDeckPreset(deck, draft, sources, "collection-1");

    expect(() =>
      genericDeckDraftFromPreset(
        deck,
        { ...preset, deck_version: "0.3.0" },
        sources,
      ),
    ).toThrow(/exact Deck version/i);
  });

  it("exposes four warm sessions without an LRU or fifth-session action", () => {
    expect(MAX_WARM_DECK_SESSIONS).toBe(4);
    expect(sessionCapacityState(0)).toEqual({ canOpen: true, remaining: 4 });
    expect(sessionCapacityState(4)).toEqual({ canOpen: false, remaining: 0 });
    expect(sessionCapacityState(5)).toEqual({ canOpen: false, remaining: 0 });
  });

  it("rehydrates the editable faceplate from the selected warm session snapshot", () => {
    const deck = model();
    const sources = [
      cartridge("10000000-0000-4000-8000-000000000001", "a".repeat(64)),
      cartridge("10000000-0000-4000-8000-000000000002", "b".repeat(64)),
    ];
    const warmDraft = createDeckUiDraft(deck);
    warmDraft.sourceArchiveSha256s = ["b".repeat(64), "a".repeat(64)];
    warmDraft.controls.mix = 0.75;
    warmDraft.roleBindings = { carrier: 1, donor: 0 };
    warmDraft.playing = [false, true];
    warmDraft.loops = [true, false];
    warmDraft.seed = 73;
    const snapshot = buildGenericDeckOpenDraft(deck, warmDraft, sources);

    expect(genericDeckDraftFromSessionSnapshot(deck, snapshot)).toEqual({
      sourceArchiveSha256s: ["b".repeat(64), "a".repeat(64)],
      controls: {
        ...createDeckUiDraft(deck).controls,
        mix: 0.75,
      },
      roleBindings: { carrier: 1, donor: 0 },
      playing: [false, true],
      loops: [true, false],
      seed: 73,
    });
  });

  it("rejects an incomplete warm-session snapshot instead of retaining stale UI state", () => {
    const deck = model();
    expect(() =>
      genericDeckDraftFromSessionSnapshot(deck, {
        sources: [],
        roles: [],
        controls: [],
        sourceTransport: [],
        seed: 0,
      }),
    ).toThrow(/physical slot/i);
  });
});
