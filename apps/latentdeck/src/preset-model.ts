import type { D2Controls, D2Transport } from "./d2-model";
import { parseD2Seed } from "./d2-model";
import type { CartridgeView, CollectionView } from "./library-model";
import type { Q4Controls, Q4Roles, Q4Transport } from "./q4-model";
import { parseQ4Seed } from "./q4-model";

export const DECK_PRESET_SCHEMA_VERSION = "2.0.0" as const;
export const BUNDLED_DECK_VERSION = "0.2.0" as const;
export const D2_DECK_ID = "org.latentdeck.deck.d2" as const;
export const Q4_DECK_ID = "org.latentdeck.deck.q4" as const;

export interface PresetCartridgeIdentity {
  cartridge_id: string;
  archive_sha256: string;
}

export type PresetControlValue =
  | { type: "boolean"; value: boolean }
  | { type: "integer"; value: number }
  | { type: "number"; value: number }
  | { type: "enum"; value: string }
  | { type: "text"; value: string };

export interface PresetSlot {
  physical_slot: number;
  source: PresetCartridgeIdentity;
}

export interface PresetLoop {
  physical_slot: number;
  enabled: boolean;
}

export interface DeckPreset {
  schema_version: typeof DECK_PRESET_SCHEMA_VERSION;
  deck_id: string;
  deck_version: string;
  active_collection_id: string;
  slots: PresetSlot[];
  roles: Record<string, number>;
  controls: Record<string, PresetControlValue>;
  loops: PresetLoop[];
  seed: number;
}

export type D2DeckPreset = DeckPreset & {
  deck_id: typeof D2_DECK_ID;
  deck_version: typeof BUNDLED_DECK_VERSION;
};

export type Q4DeckPreset = DeckPreset & {
  deck_id: typeof Q4_DECK_ID;
  deck_version: typeof BUNDLED_DECK_VERSION;
};

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

function enumControl(value: string): PresetControlValue {
  return { type: "enum", value };
}

function numberControl(value: number): PresetControlValue {
  return { type: "number", value };
}

function integerControl(value: number): PresetControlValue {
  return { type: "integer", value };
}

function controlValue<T extends PresetControlValue["value"]>(
  preset: DeckPreset,
  key: string,
  type: PresetControlValue["type"],
): T {
  const control = preset.controls[key];
  if (control === undefined || control.type !== type) {
    throw new Error(`Preset control ${key} is missing or has the wrong type.`);
  }
  return control.value as T;
}

function physicalSlot(
  preset: DeckPreset,
  slot: number,
): PresetCartridgeIdentity {
  const entry = preset.slots.find(
    (candidate) => candidate.physical_slot === slot,
  );
  if (entry === undefined)
    throw new Error(`Preset physical slot ${slot} is missing.`);
  return entry.source;
}

function roleSlot(preset: DeckPreset, role: string): number {
  const slot = preset.roles[role];
  if (slot === undefined) throw new Error(`Preset role ${role} is missing.`);
  return slot;
}

function q4SlotName(slot: number): Q4Roles["carrier"] {
  const names = ["A", "B", "C", "D"] as const;
  const name = names[slot - 1];
  if (name === undefined) throw new Error(`Invalid Q4 physical slot ${slot}.`);
  return name;
}

export function presetSlotIdentities(
  preset: DeckPreset,
): PresetCartridgeIdentity[] {
  return [...preset.slots]
    .sort((left, right) => left.physical_slot - right.physical_slot)
    .map((slot) => slot.source);
}

export function presetLoopEnabled(
  preset: DeckPreset,
  physicalSlot: number,
): boolean {
  const loop = preset.loops.find(
    (candidate) => candidate.physical_slot === physicalSlot,
  );
  if (loop === undefined) {
    throw new Error(
      `Preset loop for physical slot ${physicalSlot} is missing.`,
    );
  }
  return loop.enabled;
}

export function isD2Preset(preset: DeckPreset): preset is D2DeckPreset {
  return (
    preset.deck_id === D2_DECK_ID &&
    preset.deck_version === BUNDLED_DECK_VERSION
  );
}

export function isQ4Preset(preset: DeckPreset): preset is Q4DeckPreset {
  return (
    preset.deck_id === Q4_DECK_ID &&
    preset.deck_version === BUNDLED_DECK_VERSION
  );
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
  const carrier = controls.routing === "A" ? 1 : 2;
  return {
    schema_version: DECK_PRESET_SCHEMA_VERSION,
    deck_id: D2_DECK_ID,
    deck_version: BUNDLED_DECK_VERSION,
    active_collection_id: activeCollectionId,
    slots: [
      { physical_slot: 1, source: identity(sourceA) },
      { physical_slot: 2, source: identity(sourceB) },
    ],
    roles: { carrier, donor: carrier === 1 ? 2 : 1 },
    controls: {
      algorithm: enumControl(controls.algorithm),
      mix: numberControl(controls.mix),
      mode: enumControl(controls.mode),
      interaction: numberControl(controls.interaction),
      preserve: numberControl(controls.preserve),
      chaos: numberControl(controls.chaos),
      xs1_channel_a: integerControl(controls.xs1ChannelA),
      xs1_channel_b: integerControl(controls.xs1ChannelB),
      xs1_angle_degrees: numberControl(controls.xs1AngleDegrees),
      xs2_radius: integerControl(controls.xs2Radius),
      xs3_high_gain: numberControl(controls.xs3HighGain),
      xs4_epsilon: numberControl(controls.xs4Epsilon),
      xs5_routing: enumControl(controls.xs5Routing),
      temperature: numberControl(controls.temperature),
      top_k: integerControl(controls.topK),
      sinkhorn_iterations: integerControl(controls.sinkhornIterations),
    },
    loops: [
      { physical_slot: 1, enabled: transport.loopA },
      { physical_slot: 2, enabled: transport.loopB },
    ],
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
  const slotNumber = (slot: Q4Roles["carrier"]): number =>
    ["A", "B", "C", "D"].indexOf(slot) + 1;
  return {
    schema_version: DECK_PRESET_SCHEMA_VERSION,
    deck_id: Q4_DECK_ID,
    deck_version: BUNDLED_DECK_VERSION,
    active_collection_id: activeCollectionId,
    slots: sources.map((source, index) => ({
      physical_slot: index + 1,
      source: identity(source),
    })),
    controls: {
      algorithm: enumControl(controls.algorithm),
      interaction: numberControl(controls.interaction),
      mode: enumControl(controls.mode),
      preserve: numberControl(controls.preserve),
      influence_mode: enumControl(controls.influenceMode),
      donor_weight_b: numberControl(controls.donorWeightB),
      donor_weight_c: numberControl(controls.donorWeightC),
      donor_weight_d: numberControl(controls.donorWeightD),
      triangle_x: numberControl(controls.triangleX),
      triangle_y: numberControl(controls.triangleY),
      xs5_routing: enumControl(controls.xs5Routing),
      temperature: numberControl(controls.temperature),
      top_k: integerControl(controls.topK),
      sinkhorn_iterations: integerControl(controls.sinkhornIterations),
      chaos: numberControl(controls.chaos),
    },
    roles: {
      carrier: slotNumber(roles.carrier),
      donor_b: slotNumber(roles.donorB),
      donor_c: slotNumber(roles.donorC),
      donor_d: slotNumber(roles.donorD),
    },
    loops: [
      transport.loopA,
      transport.loopB,
      transport.loopC,
      transport.loopD,
    ].map((enabled, index) => ({ physical_slot: index + 1, enabled })),
    seed,
  };
}

export function d2ControlsFromPreset(preset: D2DeckPreset): D2Controls {
  const controls = preset.controls;
  return {
    algorithm: controlValue<D2Controls["algorithm"]>(
      preset,
      "algorithm",
      "enum",
    ),
    mix: controlValue<number>(preset, "mix", "number"),
    mode: controlValue<D2Controls["mode"]>(preset, "mode", "enum"),
    routing: roleSlot(preset, "carrier") === 1 ? "A" : "B",
    interaction: controlValue<number>(preset, "interaction", "number"),
    preserve: controlValue<number>(preset, "preserve", "number"),
    chaos: controlValue<number>(preset, "chaos", "number"),
    xs1ChannelA: controlValue<number>(preset, "xs1_channel_a", "integer"),
    xs1ChannelB: controlValue<number>(preset, "xs1_channel_b", "integer"),
    xs1AngleDegrees: controlValue<number>(
      preset,
      "xs1_angle_degrees",
      "number",
    ),
    xs2Radius: controlValue<number>(preset, "xs2_radius", "integer"),
    xs3HighGain: controlValue<number>(preset, "xs3_high_gain", "number"),
    xs4Epsilon: controlValue<number>(preset, "xs4_epsilon", "number"),
    xs5Routing: controlValue<D2Controls["xs5Routing"]>(
      preset,
      "xs5_routing",
      "enum",
    ),
    temperature: controlValue<number>(preset, "temperature", "number"),
    topK: controlValue<number>(preset, "top_k", "integer"),
    sinkhornIterations: controlValue<number>(
      preset,
      "sinkhorn_iterations",
      "integer",
    ),
  };
}

export function q4ControlsFromPreset(preset: Q4DeckPreset): Q4Controls {
  return {
    algorithm: controlValue<Q4Controls["algorithm"]>(
      preset,
      "algorithm",
      "enum",
    ),
    interaction: controlValue<number>(preset, "interaction", "number"),
    mode: controlValue<Q4Controls["mode"]>(preset, "mode", "enum"),
    preserve: controlValue<number>(preset, "preserve", "number"),
    influenceMode: controlValue<Q4Controls["influenceMode"]>(
      preset,
      "influence_mode",
      "enum",
    ),
    donorWeightB: controlValue<number>(preset, "donor_weight_b", "number"),
    donorWeightC: controlValue<number>(preset, "donor_weight_c", "number"),
    donorWeightD: controlValue<number>(preset, "donor_weight_d", "number"),
    triangleX: controlValue<number>(preset, "triangle_x", "number"),
    triangleY: controlValue<number>(preset, "triangle_y", "number"),
    xs5Routing: controlValue<Q4Controls["xs5Routing"]>(
      preset,
      "xs5_routing",
      "enum",
    ),
    temperature: controlValue<number>(preset, "temperature", "number"),
    topK: controlValue<number>(preset, "top_k", "integer"),
    sinkhornIterations: controlValue<number>(
      preset,
      "sinkhorn_iterations",
      "integer",
    ),
    chaos: controlValue<number>(preset, "chaos", "number"),
  };
}

export function q4RolesFromPreset(preset: Q4DeckPreset): Q4Roles {
  return {
    carrier: q4SlotName(roleSlot(preset, "carrier")),
    donorB: q4SlotName(roleSlot(preset, "donor_b")),
    donorC: q4SlotName(roleSlot(preset, "donor_c")),
    donorD: q4SlotName(roleSlot(preset, "donor_d")),
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
