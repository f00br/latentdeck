import type { D2Controls, D2Transport } from "./d2-model";
import { parseD2Seed } from "./d2-model";
import type { CartridgeView, CollectionView } from "./library-model";
import type { Q4Controls, Q4Roles, Q4Transport } from "./q4-model";
import { parseQ4Seed } from "./q4-model";

export const DECK_PRESET_SCHEMA_VERSION = "0.1.0" as const;

export interface PresetCartridgeIdentity {
  cartridge_id: string;
  archive_sha256: string;
}

interface D2PresetControls {
  algorithm: D2Controls["algorithm"];
  mix: number;
  mode: D2Controls["mode"];
  routing: D2Controls["routing"];
  interaction: number;
  preserve: number;
  chaos: number;
  xs1_channel_a: number;
  xs1_channel_b: number;
  xs1_angle_degrees: number;
  xs2_radius: number;
  xs3_high_gain: number;
  xs4_epsilon: number;
  xs5_routing: D2Controls["xs5Routing"];
  temperature: number;
  top_k: number;
  sinkhorn_iterations: number;
}

interface Q4PresetControls {
  algorithm: Q4Controls["algorithm"];
  interaction: number;
  mode: Q4Controls["mode"];
  preserve: number;
  influence_mode: Q4Controls["influenceMode"];
  donor_weight_b: number;
  donor_weight_c: number;
  donor_weight_d: number;
  triangle_x: number;
  triangle_y: number;
  xs5_routing: Q4Controls["xs5Routing"];
  temperature: number;
  top_k: number;
  sinkhorn_iterations: number;
  chaos: number;
}

export interface D2DeckPreset {
  deck_type: "LD-D2";
  schema_version: typeof DECK_PRESET_SCHEMA_VERSION;
  active_collection_id: string;
  slots: { a: PresetCartridgeIdentity; b: PresetCartridgeIdentity };
  controls: D2PresetControls;
  loops: { loop_a: boolean; loop_b: boolean };
  seed: number;
}

export interface Q4DeckPreset {
  deck_type: "LD-Q4";
  schema_version: typeof DECK_PRESET_SCHEMA_VERSION;
  active_collection_id: string;
  slots: {
    a: PresetCartridgeIdentity;
    b: PresetCartridgeIdentity;
    c: PresetCartridgeIdentity;
    d: PresetCartridgeIdentity;
  };
  controls: Q4PresetControls;
  routing: {
    carrier: Q4Roles["carrier"];
    donor_b: Q4Roles["donorB"];
    donor_c: Q4Roles["donorC"];
    donor_d: Q4Roles["donorD"];
  };
  loops: {
    loop_a: boolean;
    loop_b: boolean;
    loop_c: boolean;
    loop_d: boolean;
  };
  seed: number;
}

export type DeckPreset = D2DeckPreset | Q4DeckPreset;

export interface PresetSourceResolution {
  hashes: string[];
  warnings: string[];
}

export interface PresetLoopDraft<TLoops> {
  source: "loaded-preset";
  loops: TLoops;
}

export type PresetLoopDraftTransition<TLoops> =
  { type: "preset-loaded"; loops: TLoops } | { type: "manual-divergence" };

export function transitionPresetLoopDraft<TLoops>(
  _current: PresetLoopDraft<TLoops> | null,
  transition: PresetLoopDraftTransition<TLoops>,
): PresetLoopDraft<TLoops> | null {
  return transition.type === "preset-loaded"
    ? { source: "loaded-preset", loops: transition.loops }
    : null;
}

export function resolvePresetLoopDraft<TLoops>(
  draft: PresetLoopDraft<TLoops> | null,
  fallback: TLoops,
): TLoops {
  return draft?.loops ?? fallback;
}

export function mergePresetSourceOptions(
  bankCartridges: readonly CartridgeView[],
  globallyResolved: readonly (CartridgeView | null)[],
): CartridgeView[] {
  const merged = new Map(
    bankCartridges.map((cartridge) => [cartridge.archiveSha256, cartridge]),
  );
  for (const cartridge of globallyResolved) {
    if (cartridge !== null && !merged.has(cartridge.archiveSha256)) {
      merged.set(cartridge.archiveSha256, cartridge);
    }
  }
  return [...merged.values()];
}

export async function stagePresetLibraryLoad<TSources, TLibrary>(
  resolveSources: () => Promise<TSources>,
  activateCollectionAndSnapshot: () => Promise<TLibrary>,
): Promise<{ sources: TSources; library: TLibrary }> {
  // Resolve every immutable cartridge identity before mutating the active Bank.
  // The native activation command then changes Collection + builds its snapshot
  // under one Library mutex, so a failure cannot leave a half-applied preset.
  const sources = await resolveSources();
  const library = await activateCollectionAndSnapshot();
  return { sources, library };
}

function identity(source: CartridgeView): PresetCartridgeIdentity {
  return {
    cartridge_id: source.cartridgeId,
    archive_sha256: source.archiveSha256,
  };
}

export function buildD2Preset(
  activeCollectionId: string,
  sourceA: CartridgeView,
  sourceB: CartridgeView,
  controls: D2Controls,
  transport: Pick<D2Transport, "loopA" | "loopB">,
  seed: number,
): D2DeckPreset {
  if (parseD2Seed(seed) === null) throw new Error("Invalid D2 preset seed.");
  return {
    deck_type: "LD-D2",
    schema_version: DECK_PRESET_SCHEMA_VERSION,
    active_collection_id: activeCollectionId,
    slots: { a: identity(sourceA), b: identity(sourceB) },
    controls: {
      algorithm: controls.algorithm,
      mix: controls.mix,
      mode: controls.mode,
      routing: controls.routing,
      interaction: controls.interaction,
      preserve: controls.preserve,
      chaos: controls.chaos,
      xs1_channel_a: controls.xs1ChannelA,
      xs1_channel_b: controls.xs1ChannelB,
      xs1_angle_degrees: controls.xs1AngleDegrees,
      xs2_radius: controls.xs2Radius,
      xs3_high_gain: controls.xs3HighGain,
      xs4_epsilon: controls.xs4Epsilon,
      xs5_routing: controls.xs5Routing,
      temperature: controls.temperature,
      top_k: controls.topK,
      sinkhorn_iterations: controls.sinkhornIterations,
    },
    loops: { loop_a: transport.loopA, loop_b: transport.loopB },
    seed,
  };
}

export function buildQ4Preset(
  activeCollectionId: string,
  sources: readonly [
    CartridgeView,
    CartridgeView,
    CartridgeView,
    CartridgeView,
  ],
  controls: Q4Controls,
  roles: Q4Roles,
  transport: Pick<Q4Transport, "loopA" | "loopB" | "loopC" | "loopD">,
  seed: number,
): Q4DeckPreset {
  if (parseQ4Seed(seed) === null) throw new Error("Invalid Q4 preset seed.");
  return {
    deck_type: "LD-Q4",
    schema_version: DECK_PRESET_SCHEMA_VERSION,
    active_collection_id: activeCollectionId,
    slots: {
      a: identity(sources[0]),
      b: identity(sources[1]),
      c: identity(sources[2]),
      d: identity(sources[3]),
    },
    controls: {
      algorithm: controls.algorithm,
      interaction: controls.interaction,
      mode: controls.mode,
      preserve: controls.preserve,
      influence_mode: controls.influenceMode,
      donor_weight_b: controls.donorWeightB,
      donor_weight_c: controls.donorWeightC,
      donor_weight_d: controls.donorWeightD,
      triangle_x: controls.triangleX,
      triangle_y: controls.triangleY,
      xs5_routing: controls.xs5Routing,
      temperature: controls.temperature,
      top_k: controls.topK,
      sinkhorn_iterations: controls.sinkhornIterations,
      chaos: controls.chaos,
    },
    routing: {
      carrier: roles.carrier,
      donor_b: roles.donorB,
      donor_c: roles.donorC,
      donor_d: roles.donorD,
    },
    loops: {
      loop_a: transport.loopA,
      loop_b: transport.loopB,
      loop_c: transport.loopC,
      loop_d: transport.loopD,
    },
    seed,
  };
}

export function d2ControlsFromPreset(preset: D2DeckPreset): D2Controls {
  const controls = preset.controls;
  return {
    algorithm: controls.algorithm,
    mix: controls.mix,
    mode: controls.mode,
    routing: controls.routing,
    interaction: controls.interaction,
    preserve: controls.preserve,
    chaos: controls.chaos,
    xs1ChannelA: controls.xs1_channel_a,
    xs1ChannelB: controls.xs1_channel_b,
    xs1AngleDegrees: controls.xs1_angle_degrees,
    xs2Radius: controls.xs2_radius,
    xs3HighGain: controls.xs3_high_gain,
    xs4Epsilon: controls.xs4_epsilon,
    xs5Routing: controls.xs5_routing,
    temperature: controls.temperature,
    topK: controls.top_k,
    sinkhornIterations: controls.sinkhorn_iterations,
  };
}

export function q4ControlsFromPreset(preset: Q4DeckPreset): Q4Controls {
  const controls = preset.controls;
  return {
    algorithm: controls.algorithm,
    interaction: controls.interaction,
    mode: controls.mode,
    preserve: controls.preserve,
    influenceMode: controls.influence_mode,
    donorWeightB: controls.donor_weight_b,
    donorWeightC: controls.donor_weight_c,
    donorWeightD: controls.donor_weight_d,
    triangleX: controls.triangle_x,
    triangleY: controls.triangle_y,
    xs5Routing: controls.xs5_routing,
    temperature: controls.temperature,
    topK: controls.top_k,
    sinkhornIterations: controls.sinkhorn_iterations,
    chaos: controls.chaos,
  };
}

export function q4RolesFromPreset(preset: Q4DeckPreset): Q4Roles {
  return {
    carrier: preset.routing.carrier,
    donorB: preset.routing.donor_b,
    donorC: preset.routing.donor_c,
    donorD: preset.routing.donor_d,
  };
}

export function presetCollectionExists(
  preset: DeckPreset,
  collections: readonly CollectionView[],
): boolean {
  return collections.some(
    (collection) => collection.id === preset.active_collection_id,
  );
}

export function resolvePresetSources(
  identities: readonly PresetCartridgeIdentity[],
  cartridges: readonly CartridgeView[],
): PresetSourceResolution {
  const warnings: string[] = [];
  const hashes = identities.map((expected, index) => {
    const exact = cartridges.find(
      (cartridge) =>
        cartridge.archiveSha256 === expected.archive_sha256 &&
        cartridge.cartridgeId === expected.cartridge_id &&
        cartridge.availability === "present",
    );
    if (exact !== undefined) return exact.archiveSha256;
    const sameHash = cartridges.find(
      (cartridge) => cartridge.archiveSha256 === expected.archive_sha256,
    );
    const slot = String.fromCharCode("A".charCodeAt(0) + index);
    warnings.push(
      sameHash === undefined
        ? `Slot ${slot}: cartridge ${expected.archive_sha256.slice(0, 12)}… is missing from the Library.`
        : sameHash.cartridgeId !== expected.cartridge_id
          ? `Slot ${slot}: hash exists but cartridge ID differs; no replacement was selected.`
          : `Slot ${slot}: cartridge is not currently available; no replacement was selected.`,
    );
    return "";
  });
  return { hashes, warnings };
}
