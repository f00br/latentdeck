import type {
  ExtensionCompatibilityPair,
  ExtensionCompatibilityReason,
} from "./extension-manager-model";
import {
  serializeDeckControls,
  serializeRoleBindings,
  type DeckUiDraft,
  type DeckUiModel,
  type DeckUiScalar,
} from "./deck-ui-model";
import type { CartridgeView } from "./library-model";
import {
  DECK_PRESET_SCHEMA_VERSION,
  type DeckPreset,
  type PresetControlValue,
} from "./preset-model";

export const MAX_WARM_DECK_SESSIONS = 4;

export interface GenericCodecOption {
  exactKey: string;
  codecId: string;
  codecVersion: string;
  reason: ExtensionCompatibilityReason;
}

export type GenericControlValue =
  | { kind: "boolean"; value: boolean }
  | { kind: "integer"; value: number }
  | { kind: "number"; value: number }
  | { kind: "text"; value: string };

export interface GenericControlBinding {
  name: string;
  value: GenericControlValue;
}

export interface GenericRoleBinding {
  role: string;
  physical_slot: number;
}

export interface GenericSourceTransportBinding {
  physical_slot: number;
  playing: boolean;
  loop_enabled: boolean;
}

export interface GenericDeckSourceIdentity {
  cartridgeId: string;
  archiveSha256: string;
}

export interface GenericDeckOpenDraft {
  sources: GenericDeckSourceIdentity[];
  roles: GenericRoleBinding[];
  controls: GenericControlBinding[];
  sourceTransport: GenericSourceTransportBinding[];
  seed: number;
}

export type GenericDeckSessionDraftSnapshot = GenericDeckOpenDraft;

export function exactPackageKey(packageId: string, packageVersion: string) {
  return `${packageId}@${packageVersion}`;
}

/** Keep an explicit exact choice only while those exact bytes remain listed. */
export function retainExactSelection(
  current: string,
  availableExactKeys: readonly string[],
): string {
  return current !== "" && availableExactKeys.includes(current) ? current : "";
}

/**
 * Return every exact Codec version for one exact Deck version, including
 * incompatible rows. The UI must expose the stable refusal reason instead of
 * collapsing the matrix to a compatible or newest candidate.
 */
export function codecOptionsForExactDeck(
  deckExactKey: string,
  matrix: readonly ExtensionCompatibilityPair[],
): GenericCodecOption[] {
  return matrix
    .filter(
      (pair) =>
        exactPackageKey(pair.deck.packageId, pair.deck.packageVersion) ===
        deckExactKey,
    )
    .map((pair) => ({
      exactKey: exactPackageKey(
        pair.codec.packageId,
        pair.codec.packageVersion,
      ),
      codecId: pair.codec.packageId,
      codecVersion: pair.codec.packageVersion,
      reason: pair.reason,
    }));
}

export function sessionCapacityState(sessionCount: number): {
  canOpen: boolean;
  remaining: number;
} {
  const boundedCount = Number.isSafeInteger(sessionCount)
    ? Math.max(0, sessionCount)
    : MAX_WARM_DECK_SESSIONS;
  const remaining = Math.max(0, MAX_WARM_DECK_SESSIONS - boundedCount);
  return { canOpen: remaining > 0, remaining };
}

export function buildGenericDeckOpenDraft(
  model: DeckUiModel,
  draft: DeckUiDraft,
  availableSources: readonly CartridgeView[],
): GenericDeckOpenDraft {
  if (
    draft.sourceArchiveSha256s.length !== model.slots ||
    draft.playing.length !== model.slots ||
    draft.loops.length !== model.slots
  ) {
    throw new Error("The Deck draft does not cover every physical slot.");
  }

  const sources = draft.sourceArchiveSha256s.map((archiveSha256) => {
    const source = availableSources.find(
      (candidate) =>
        candidate.archiveSha256 === archiveSha256 &&
        candidate.availability === "present",
    );
    if (source === undefined) {
      throw new Error(
        "Every Deck source must be a present immutable Library identity.",
      );
    }
    return {
      cartridgeId: source.cartridgeId,
      archiveSha256: source.archiveSha256,
    };
  });

  const serializedRoles = serializeRoleBindings(model, draft.roleBindings);
  if (!Number.isSafeInteger(draft.seed) || draft.seed < 0) {
    throw new Error("Seed must be a non-negative safe integer.");
  }

  return {
    sources,
    roles: model.roles.map((role) => ({
      role: role.roleId,
      physical_slot: serializedRoles[role.roleId] + 1,
    })),
    controls: buildGenericControlBindings(model, draft.controls),
    sourceTransport: draft.playing.map((playing, index) => ({
      physical_slot: index + 1,
      playing,
      loop_enabled: draft.loops[index],
    })),
    seed: draft.seed,
  };
}

/**
 * Serialize only the closed control snapshot. Realtime control dispatch must
 * not revalidate or depend on the current source-picker draft.
 */
export function buildGenericControlBindings(
  model: DeckUiModel,
  controls: Record<string, DeckUiScalar>,
): GenericControlBinding[] {
  const serialized = serializeDeckControls(model, controls);
  return model.controls.map((control) => ({
    name: control.controlId,
    value: genericControlValue(
      control.valueType,
      serialized[control.controlId],
    ),
  }));
}

/**
 * Rebuild the host-rendered draft from one authoritative warm-session
 * snapshot. Switching sessions must never leave controls, roles, transports,
 * or sources from the previously selected worker visible or writable.
 */
export function genericDeckDraftFromSessionSnapshot(
  model: DeckUiModel,
  snapshot: GenericDeckSessionDraftSnapshot,
): DeckUiDraft {
  if (
    snapshot.sources.length !== model.slots ||
    snapshot.sourceTransport.length !== model.slots
  ) {
    throw new Error(
      "The warm-session snapshot does not cover every physical slot.",
    );
  }

  const sourceArchiveSha256s = snapshot.sources.map((source) => {
    if (!/^[0-9a-f]{64}$/.test(source.archiveSha256)) {
      throw new Error("The warm-session source identity is invalid.");
    }
    return source.archiveSha256;
  });

  const roleBindings = Object.fromEntries(
    model.roles.map((role) => {
      const binding = snapshot.roles.find(
        (candidate) => candidate.role === role.roleId,
      );
      if (binding === undefined || !Number.isInteger(binding.physical_slot)) {
        throw new Error(
          `The warm-session role ${role.roleId} is missing or invalid.`,
        );
      }
      return [role.roleId, binding.physical_slot - 1];
    }),
  );
  if (
    snapshot.roles.length !== model.roles.length ||
    new Set(snapshot.roles.map((binding) => binding.role)).size !==
      snapshot.roles.length
  ) {
    throw new Error("The warm-session role bindings are not exact.");
  }
  serializeRoleBindings(model, roleBindings);

  const controls = Object.fromEntries(
    model.controls.map((control) => {
      const binding = snapshot.controls.find(
        (candidate) => candidate.name === control.controlId,
      );
      if (binding === undefined) {
        throw new Error(
          `The warm-session control ${control.controlId} is missing.`,
        );
      }
      return [
        control.controlId,
        scalarFromGenericControl(control.valueType, binding.value),
      ];
    }),
  );
  if (
    snapshot.controls.length !== model.controls.length ||
    new Set(snapshot.controls.map((binding) => binding.name)).size !==
      snapshot.controls.length
  ) {
    throw new Error("The warm-session controls are not exact.");
  }
  serializeDeckControls(model, controls);

  const playing = Array.from({ length: model.slots }, () => false);
  const loops = Array.from({ length: model.slots }, () => false);
  const transportSlots = new Set<number>();
  for (const transport of snapshot.sourceTransport) {
    const index = transport.physical_slot - 1;
    if (
      !Number.isInteger(transport.physical_slot) ||
      index < 0 ||
      index >= model.slots ||
      transportSlots.has(transport.physical_slot)
    ) {
      throw new Error("The warm-session source transport is not exact.");
    }
    transportSlots.add(transport.physical_slot);
    playing[index] = transport.playing;
    loops[index] = transport.loop_enabled;
  }

  if (!Number.isSafeInteger(snapshot.seed) || snapshot.seed < 0) {
    throw new Error("The warm-session seed is outside the safe range.");
  }
  return {
    sourceArchiveSha256s,
    controls,
    roleBindings,
    playing,
    loops,
    seed: snapshot.seed,
  };
}

export function buildGenericDeckPreset(
  model: DeckUiModel,
  draft: DeckUiDraft,
  availableSources: readonly CartridgeView[],
  activeCollectionId: string,
): DeckPreset {
  if (activeCollectionId.trim() === "") {
    throw new Error(
      "An exact active Collection is required for a Deck preset.",
    );
  }
  const wire = buildGenericDeckOpenDraft(model, draft, availableSources);
  const controls = Object.fromEntries(
    model.controls.map((control) => [
      control.controlId,
      presetControlValue(control.valueType, draft.controls[control.controlId]),
    ]),
  );
  return {
    schema_version: DECK_PRESET_SCHEMA_VERSION,
    deck_id: model.deckId,
    deck_version: model.deckVersion,
    active_collection_id: activeCollectionId,
    slots: wire.sources.map((source, index) => ({
      physical_slot: index + 1,
      source: {
        cartridge_id: source.cartridgeId,
        archive_sha256: source.archiveSha256,
      },
    })),
    roles: Object.fromEntries(
      wire.roles.map((role) => [role.role, role.physical_slot]),
    ),
    controls,
    loops: wire.sourceTransport.map((source) => ({
      physical_slot: source.physical_slot,
      enabled: source.loop_enabled,
    })),
    seed: wire.seed,
  };
}

/**
 * Convert an already schema-validated preset into an editable declarative
 * draft. Exact Deck version and immutable source identities are required;
 * missing data never causes version or cartridge substitution.
 */
export function genericDeckDraftFromPreset(
  model: DeckUiModel,
  preset: DeckPreset,
  resolvedSources: readonly CartridgeView[],
): DeckUiDraft {
  if (
    preset.deck_id !== model.deckId ||
    preset.deck_version !== model.deckVersion
  ) {
    throw new Error("The preset targets a different exact Deck version.");
  }
  if (
    preset.slots.length !== model.slots ||
    preset.loops.length !== model.slots
  ) {
    throw new Error("The preset does not cover every physical Deck slot.");
  }

  const sourceArchiveSha256s = Array.from(
    { length: model.slots },
    (_, index) => {
      const physicalSlot = index + 1;
      const slot = preset.slots.find(
        (candidate) => candidate.physical_slot === physicalSlot,
      );
      if (slot === undefined) {
        throw new Error(`Preset physical slot ${physicalSlot} is missing.`);
      }
      const source = resolvedSources.find(
        (candidate) =>
          candidate.cartridgeId === slot.source.cartridge_id &&
          candidate.archiveSha256 === slot.source.archive_sha256 &&
          candidate.availability === "present",
      );
      if (source === undefined) {
        throw new Error(
          `Preset physical slot ${physicalSlot} has no exact present Library source.`,
        );
      }
      return source.archiveSha256;
    },
  );

  const controls = Object.fromEntries(
    model.controls.map((control) => [
      control.controlId,
      scalarFromPresetControl(
        control.valueType,
        preset.controls[control.controlId],
      ),
    ]),
  );
  serializeDeckControls(model, controls);

  const roleBindings = Object.fromEntries(
    model.roles.map((role) => {
      const physicalSlot = preset.roles[role.roleId];
      if (!Number.isInteger(physicalSlot)) {
        throw new Error(`Preset role ${role.roleId} is missing.`);
      }
      return [role.roleId, physicalSlot - 1];
    }),
  );
  serializeRoleBindings(model, roleBindings);

  const loops = Array.from({ length: model.slots }, (_, index) => {
    const physicalSlot = index + 1;
    const loop = preset.loops.find(
      (candidate) => candidate.physical_slot === physicalSlot,
    );
    if (loop === undefined) {
      throw new Error(
        `Preset loop for physical slot ${physicalSlot} is missing.`,
      );
    }
    return loop.enabled;
  });
  if (!Number.isSafeInteger(preset.seed) || preset.seed < 0) {
    throw new Error(
      "Preset seed is outside the JavaScript-safe integer range.",
    );
  }
  return {
    sourceArchiveSha256s,
    controls,
    roleBindings,
    playing: Array.from({ length: model.slots }, () => true),
    loops,
    seed: preset.seed,
  };
}

function genericControlValue(
  type: DeckUiModel["controls"][number]["valueType"],
  value: DeckUiScalar,
): GenericControlValue {
  switch (type) {
    case "boolean":
      return { kind: "boolean", value: value as boolean };
    case "integer":
      return { kind: "integer", value: value as number };
    case "number":
      return { kind: "number", value: value as number };
    case "enum":
      return { kind: "text", value: value as string };
  }
}

function scalarFromGenericControl(
  expected: DeckUiModel["controls"][number]["valueType"],
  control: GenericControlValue,
): DeckUiScalar {
  switch (expected) {
    case "boolean":
      if (control.kind === "boolean") return control.value;
      break;
    case "integer":
      if (control.kind === "integer") return control.value;
      break;
    case "number":
      if (control.kind === "number") return control.value;
      break;
    case "enum":
      if (control.kind === "text") return control.value;
      break;
  }
  throw new Error("A warm-session control has the wrong closed type.");
}

function presetControlValue(
  type: DeckUiModel["controls"][number]["valueType"],
  value: DeckUiScalar,
): PresetControlValue {
  switch (type) {
    case "boolean":
      return { type: "boolean", value: value as boolean };
    case "integer":
      return { type: "integer", value: value as number };
    case "number":
      return { type: "number", value: value as number };
    case "enum":
      return { type: "enum", value: value as string };
  }
}

function scalarFromPresetControl(
  expected: DeckUiModel["controls"][number]["valueType"],
  control: PresetControlValue | undefined,
): DeckUiScalar {
  const expectedPresetType = expected === "enum" ? "enum" : expected;
  if (control === undefined || control.type !== expectedPresetType) {
    throw new Error(
      "A preset control is missing or has the wrong closed type.",
    );
  }
  return control.value;
}
