import { describe, expect, it } from "vitest";
import { DEFAULT_D2_CONTROLS, DEFAULT_D2_TRANSPORT } from "./d2-model";
import type { CartridgeView, CollectionView } from "./library-model";
import {
  buildD2Preset,
  buildQ4Preset,
  d2ControlsFromPreset,
  mergePresetSourceOptions,
  presetCollectionExists,
  resolvePresetLoopDraft,
  q4ControlsFromPreset,
  q4RolesFromPreset,
  resolvePresetSources,
  stagePresetLibraryLoad,
  transitionPresetLoopDraft,
} from "./preset-model";
import {
  DEFAULT_Q4_CONTROLS,
  DEFAULT_Q4_ROLES,
  DEFAULT_Q4_TRANSPORT,
} from "./q4-model";

function cartridge(
  marker: string,
  id: string,
  availability: CartridgeView["availability"] = "present",
): CartridgeView {
  return {
    cartridgeId: id,
    archiveSha256: marker.repeat(64),
    codecFamily: "minimax_h3",
    codecProfile: "h3_av_latent",
    codecProfileVersion: "0.1.0",
    timingContract: "minimax_h3_causal",
    timingContractVersion: "0.1.0",
    frameRateNumerator: 24,
    frameRateDenominator: 1,
    decodedWidth: 448,
    decodedHeight: 800,
    decodedFrameCount: 107,
    durationNumerator: 107,
    durationDenominator: 24,
    signalGeometry: {
      codec_family: "minimax_h3",
      profile: "h3_av_latent",
      profile_version: "0.1.0",
      runtime_dtype: "F16",
      batch: 1,
      latent_channels: 24,
      latent_slots: 32,
      latent_height: 50,
      latent_width: 28,
      decoded_frame_count: 107,
      decoded_height: 800,
      decoded_width: 448,
      timing_contract: "minimax_h3_causal",
      timing_contract_version: "0.1.0",
      frame_rate: { numerator: 24, denominator: 1 },
    },
    signalPresentation: {
      orientation: "portrait",
      aspect_ratio: { width: 14, height: 25 },
      decoded_width: 448,
      decoded_height: 800,
    },
    signalWorkload: {
      latent_sites_per_slot: 1400,
      latent_values_per_slot: 33600,
      latent_values_per_clip: 1075200,
      decoded_pixels_per_frame: 358400,
    },
    favorite: false,
    tags: [],
    availability,
    paths: [],
  };
}

describe("Deck preset model", () => {
  it("maps D2 controls to the stable snake-case contract and back exactly", () => {
    const a = cartridge("a", "11111111-1111-4111-8111-111111111111");
    const b = cartridge("b", "22222222-2222-4222-8222-222222222222");
    const controls = {
      ...DEFAULT_D2_CONTROLS,
      algorithm: "XS5" as const,
      chaos: 0.25,
    };
    const preset = buildD2Preset(
      "latentdeck.virtual.all",
      a,
      b,
      controls,
      DEFAULT_D2_TRANSPORT,
      44,
    );

    expect(preset.deck_type).toBe("LD-D2");
    expect(preset.controls.xs5_routing).toBe(controls.xs5Routing);
    expect(preset.slots.a).toEqual({
      cartridge_id: a.cartridgeId,
      archive_sha256: a.archiveSha256,
    });
    expect(d2ControlsFromPreset(preset)).toEqual(controls);
  });

  it("preserves Q4 duplicate slot identities and exact carrier routing", () => {
    const a = cartridge("a", "11111111-1111-4111-8111-111111111111");
    const b = cartridge("b", "22222222-2222-4222-8222-222222222222");
    const c = cartridge("c", "33333333-3333-4333-8333-333333333333");
    const controls = { ...DEFAULT_Q4_CONTROLS, algorithm: "XS5" as const };
    const roles = {
      ...DEFAULT_Q4_ROLES,
      carrier: "C" as const,
      donorC: "A" as const,
    };
    const preset = buildQ4Preset(
      "latentdeck.virtual.all",
      [a, b, c, a],
      controls,
      roles,
      DEFAULT_Q4_TRANSPORT,
      9,
    );

    expect(preset.slots.d).toEqual(preset.slots.a);
    expect(q4ControlsFromPreset(preset)).toEqual(controls);
    expect(q4RolesFromPreset(preset)).toEqual(roles);
  });

  it("never substitutes missing, unavailable, or identity-mismatched cartridges", () => {
    const exact = cartridge("a", "11111111-1111-4111-8111-111111111111");
    const mismatch = cartridge("b", "99999999-9999-4999-8999-999999999999");
    const unavailable = cartridge(
      "c",
      "33333333-3333-4333-8333-333333333333",
      "missing",
    );
    const resolution = resolvePresetSources(
      [
        {
          cartridge_id: exact.cartridgeId,
          archive_sha256: exact.archiveSha256,
        },
        {
          cartridge_id: "22222222-2222-4222-8222-222222222222",
          archive_sha256: mismatch.archiveSha256,
        },
        {
          cartridge_id: unavailable.cartridgeId,
          archive_sha256: unavailable.archiveSha256,
        },
        {
          cartridge_id: "44444444-4444-4444-8444-444444444444",
          archive_sha256: "d".repeat(64),
        },
      ],
      [exact, mismatch, unavailable],
    );

    expect(resolution.hashes).toEqual([exact.archiveSha256, "", "", ""]);
    expect(resolution.warnings).toHaveLength(3);
  });

  it("requires the exact saved collection instead of silently choosing another Bank", () => {
    const a = cartridge("a", "11111111-1111-4111-8111-111111111111");
    const b = cartridge("b", "22222222-2222-4222-8222-222222222222");
    const preset = buildD2Preset(
      "550e8400-e29b-41d4-a716-446655440000",
      a,
      b,
      DEFAULT_D2_CONTROLS,
      DEFAULT_D2_TRANSPORT,
      0,
    );
    const collections = [
      {
        id: "latentdeck.virtual.all",
        name: "All Cartridges",
        isVirtual: true,
        position: 0,
        memberCount: 2,
      },
    ] satisfies CollectionView[];

    expect(presetCollectionExists(preset, collections)).toBe(false);
  });

  it("discards loaded preset loops after the draft manually diverges", () => {
    const runtimeLoops = { loopA: false, loopB: false };
    const loaded = transitionPresetLoopDraft(null, {
      type: "preset-loaded",
      loops: { loopA: true, loopB: true },
    });

    expect(resolvePresetLoopDraft(loaded, runtimeLoops)).toEqual({
      loopA: true,
      loopB: true,
    });

    const diverged = transitionPresetLoopDraft(loaded, {
      type: "manual-divergence",
    });
    expect(diverged).toBeNull();
    expect(resolvePresetLoopDraft(diverged, runtimeLoops)).toBe(runtimeLoops);
  });

  it("keeps an exact global preset source outside the truncated Bank options", () => {
    const bank = Array.from({ length: 1_000 }, (_, index) => ({
      ...cartridge(
        "a",
        `11111111-1111-4111-8111-${String(index).padStart(12, "0")}`,
      ),
      archiveSha256: index.toString(16).padStart(64, "0"),
    }));
    const outside = cartridge("f", "ffffffff-ffff-4fff-8fff-ffffffffffff");

    const options = mergePresetSourceOptions(bank, [outside]);
    const resolution = resolvePresetSources(
      [
        {
          cartridge_id: outside.cartridgeId,
          archive_sha256: outside.archiveSha256,
        },
      ],
      options,
    );

    expect(options).toHaveLength(1_001);
    expect(resolution).toEqual({
      hashes: [outside.archiveSha256],
      warnings: [],
    });
  });

  it("resolves immutable preset sources before activating the saved Bank", async () => {
    let releaseSources!: (value: readonly string[]) => void;
    let activationCalls = 0;
    const sources = new Promise<readonly string[]>((resolve) => {
      releaseSources = resolve;
    });
    const staged = stagePresetLibraryLoad(
      () => sources,
      async () => {
        activationCalls += 1;
        return { activeCollectionId: "saved-bank" };
      },
    );

    expect(activationCalls).toBe(0);
    releaseSources(["exact-source"]);
    await Promise.resolve();
    expect(activationCalls).toBe(1);
    await expect(staged).resolves.toEqual({
      sources: ["exact-source"],
      library: { activeCollectionId: "saved-bank" },
    });
  });
});
