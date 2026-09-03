export const FACEPLATE_SCHEMA_VERSION = 2 as const;
export type FaceplateSchemaVersion = 1 | typeof FACEPLATE_SCHEMA_VERSION;

export type FaceplateCaptureMode = "snapshot" | "live_capture";
export type FaceplateSectionRegion = "output" | "actions" | "controls";

export interface FaceplateVisibilityPredicate {
  control_id: string;
  one_of: readonly (string | boolean)[];
}

interface FaceplateWidgetBase {
  id: string;
  label: string;
  visible_when?: readonly FaceplateVisibilityPredicate[];
}

export interface SourcePickerWidget extends FaceplateWidgetBase {
  kind: "source_picker";
  slot_index: number;
}

export interface NumericWidget extends FaceplateWidgetBase {
  kind: "slider" | "number";
  control_id: string;
  minimum: number;
  maximum: number;
  step: number;
}

export interface ToggleWidget extends FaceplateWidgetBase {
  kind: "toggle";
  control_id: string;
}

export interface SelectWidget extends FaceplateWidgetBase {
  kind: "select";
  control_id: string;
  options: readonly { value: string; label: string }[];
}

export interface RoleEditorWidget extends FaceplateWidgetBase {
  kind: "role_editor";
  role_ids: readonly string[];
}

export interface Barycentric3Widget extends FaceplateWidgetBase {
  kind: "barycentric3";
  x_control_id: string;
  y_control_id: string;
  vertex_role_ids: readonly [string, string, string];
}

export interface TransportWidget extends FaceplateWidgetBase {
  kind: "transport";
  slot_indices: readonly number[];
}

export interface SeedWidget extends FaceplateWidgetBase {
  kind: "seed";
}

export interface CaptureWidget extends FaceplateWidgetBase {
  kind: "capture";
  modes: readonly FaceplateCaptureMode[];
}

export interface MonitorWidget extends FaceplateWidgetBase {
  kind: "monitor";
}

export type FaceplateWidget =
  | SourcePickerWidget
  | NumericWidget
  | ToggleWidget
  | SelectWidget
  | RoleEditorWidget
  | Barycentric3Widget
  | TransportWidget
  | SeedWidget
  | CaptureWidget
  | MonitorWidget;

export interface FaceplateSection {
  section_id: string;
  title: string;
  region?: FaceplateSectionRegion;
  columns?: number;
  widgets: readonly FaceplateWidget[];
}

export interface FaceplateDefinition {
  schema_version: FaceplateSchemaVersion;
  title: string;
  sections: readonly FaceplateSection[];
}

export type DeckControlContract =
  | {
      control_id: string;
      value_type: "number" | "integer";
      minimum: number;
      maximum: number;
      step: number;
    }
  | {
      control_id: string;
      value_type: "boolean";
    }
  | {
      control_id: string;
      value_type: "enum";
      options: readonly string[];
    };

export interface FaceplateDeckContract {
  slots: number;
  role_ids: readonly string[];
  controls: readonly DeckControlContract[];
  capabilities: readonly string[];
}

const IDENTIFIER = /^[a-z][a-z0-9]*(?:[._-][a-z0-9]+)*$/;
const MAX_SECTIONS = 16;
const MAX_WIDGETS = 128;
const MAX_TEXT_BYTES = 256;

export class FaceplateContractError extends Error {
  constructor(
    readonly code: string,
    readonly pointer: string,
    message: string,
  ) {
    super(message);
    this.name = "FaceplateContractError";
  }
}

export function parseFaceplateDefinition(value: unknown): FaceplateDefinition {
  const root = record(value, "");
  exactKeys(root, ["schema_version", "title", "sections"], "");
  if (
    root.schema_version !== 1 &&
    root.schema_version !== FACEPLATE_SCHEMA_VERSION
  ) {
    fail(
      "faceplate.unsupported_schema",
      "/schema_version",
      "Unsupported faceplate schema.",
    );
  }
  const schemaVersion = root.schema_version as FaceplateSchemaVersion;
  const sections = array(root.sections, "/sections");
  if (sections.length < 1 || sections.length > MAX_SECTIONS) {
    fail(
      "faceplate.limit_exceeded",
      "/sections",
      "Faceplate section count is out of bounds.",
    );
  }
  const sectionIds = new Set<string>();
  const widgetIds = new Set<string>();
  let widgetCount = 0;
  const parsedSections = sections.map((section, sectionIndex) => {
    const pointer = `/sections/${sectionIndex}`;
    const item = record(section, pointer);
    exactKeys(
      item,
      ["section_id", "title", "widgets"],
      pointer,
      schemaVersion === FACEPLATE_SCHEMA_VERSION ? ["region", "columns"] : [],
    );
    const sectionId = identifier(item.section_id, `${pointer}/section_id`);
    unique(sectionIds, sectionId, `${pointer}/section_id`, "section");
    const widgets = array(item.widgets, `${pointer}/widgets`);
    widgetCount += widgets.length;
    if (widgetCount > MAX_WIDGETS) {
      fail(
        "faceplate.limit_exceeded",
        `${pointer}/widgets`,
        "Faceplate widget count is out of bounds.",
      );
    }
    const region =
      item.region === undefined
        ? undefined
        : sectionRegion(item.region, `${pointer}/region`);
    const columns =
      item.columns === undefined
        ? undefined
        : integer(item.columns, `${pointer}/columns`, 1, 4);
    return {
      section_id: sectionId,
      title: text(item.title, `${pointer}/title`),
      ...(region === undefined ? {} : { region }),
      ...(columns === undefined ? {} : { columns }),
      widgets: widgets.map((widget, widgetIndex) => {
        const widgetPointer = `${pointer}/widgets/${widgetIndex}`;
        const parsed = parseWidget(widget, widgetPointer, schemaVersion);
        unique(widgetIds, parsed.id, `${widgetPointer}/id`, "widget");
        return parsed;
      }),
    } satisfies FaceplateSection;
  });
  return {
    schema_version: schemaVersion,
    title: text(root.title, "/title"),
    sections: parsedSections,
  };
}

export function validateFaceplateAgainstDeck(
  faceplate: FaceplateDefinition,
  deck: FaceplateDeckContract,
): void {
  if (!Number.isInteger(deck.slots) || deck.slots < 1 || deck.slots > 16) {
    fail(
      "deck.package_invalid",
      "/signal/slots",
      "Deck slot count is invalid.",
    );
  }
  const controls = new Map(
    deck.controls.map((control) => [control.control_id, control]),
  );
  if (controls.size !== deck.controls.length) {
    fail(
      "deck.package_invalid",
      "/controls",
      "Deck control IDs are duplicated.",
    );
  }
  const roleIds = new Set(deck.role_ids);
  if (
    roleIds.size !== deck.role_ids.length ||
    [...roleIds].some((role) => !IDENTIFIER.test(role))
  ) {
    fail(
      "deck.package_invalid",
      "/signal/roles",
      "Deck role IDs are invalid or duplicated.",
    );
  }
  const boundControls = new Set<string>();
  const sourceSlots = new Set<number>();
  let roleEditorCount = 0;
  let transportCount = 0;
  let seedCount = 0;
  let captureCount = 0;
  let monitorCount = 0;
  let outputRegionCount = 0;
  for (const [sectionIndex, section] of faceplate.sections.entries()) {
    if (faceplate.schema_version === FACEPLATE_SCHEMA_VERSION) {
      if (section.region === "output") outputRegionCount += 1;
      if (section.region === undefined || section.columns === undefined) {
        fail(
          "faceplate.layout_mismatch",
          `/sections/${sectionIndex}`,
          "Faceplate schema v2 sections require a bounded region and column count.",
        );
      }
    }
    for (const [widgetIndex, widget] of section.widgets.entries()) {
      const pointer = `/sections/${sectionIndex}/widgets/${widgetIndex}`;
      validateVisibility(widget, controls, pointer);
      if (faceplate.schema_version === FACEPLATE_SCHEMA_VERSION) {
        if (widget.kind === "monitor" && section.region !== "output") {
          fail(
            "faceplate.layout_mismatch",
            pointer,
            "The monitor widget must occupy the output region.",
          );
        }
        if (widget.kind === "capture" && section.region !== "actions") {
          fail(
            "faceplate.layout_mismatch",
            pointer,
            "The capture widget must occupy the actions region.",
          );
        }
        if (
          widget.kind !== "monitor" &&
          widget.kind !== "capture" &&
          section.region !== "controls"
        ) {
          fail(
            "faceplate.layout_mismatch",
            pointer,
            "Interactive faceplate controls must occupy the controls region.",
          );
        }
      }
      switch (widget.kind) {
        case "source_picker":
          if (widget.slot_index >= deck.slots) {
            fail(
              "faceplate.slot_mismatch",
              `${pointer}/slot_index`,
              "Source picker references an absent slot.",
            );
          }
          unique(
            sourceSlots,
            widget.slot_index,
            `${pointer}/slot_index`,
            "source slot",
          );
          break;
        case "slider":
        case "number": {
          const control = requireControl(controls, widget.control_id, pointer);
          if (
            control.value_type !== "number" &&
            control.value_type !== "integer"
          ) {
            fail(
              "faceplate.control_mismatch",
              `${pointer}/control_id`,
              "Numeric widget is bound to a non-numeric control.",
            );
          }
          if (
            control.minimum !== widget.minimum ||
            control.maximum !== widget.maximum ||
            control.step !== widget.step ||
            (control.value_type === "integer" && widget.kind !== "number")
          ) {
            fail(
              "faceplate.control_mismatch",
              pointer,
              "Numeric widget constraints differ from operator.json.",
            );
          }
          bindControl(boundControls, widget.control_id, pointer);
          break;
        }
        case "toggle": {
          const control = requireControl(controls, widget.control_id, pointer);
          if (control.value_type !== "boolean") {
            fail(
              "faceplate.control_mismatch",
              `${pointer}/control_id`,
              "Toggle is bound to a non-boolean control.",
            );
          }
          bindControl(boundControls, widget.control_id, pointer);
          break;
        }
        case "select": {
          const control = requireControl(controls, widget.control_id, pointer);
          if (
            control.value_type !== "enum" ||
            !sameSet(
              control.options,
              widget.options.map((option) => option.value),
            )
          ) {
            fail(
              "faceplate.control_mismatch",
              pointer,
              "Select options differ from operator.json.",
            );
          }
          bindControl(boundControls, widget.control_id, pointer);
          break;
        }
        case "barycentric3":
          requireBarycentricControl(controls, widget.x_control_id, pointer);
          requireBarycentricControl(controls, widget.y_control_id, pointer);
          bindControl(boundControls, widget.x_control_id, pointer);
          bindControl(boundControls, widget.y_control_id, pointer);
          for (const role of widget.vertex_role_ids) {
            if (!roleIds.has(role)) {
              fail(
                "faceplate.role_mismatch",
                `${pointer}/vertex_role_ids`,
                "Barycentric vertex references an absent role.",
              );
            }
          }
          break;
        case "role_editor":
          roleEditorCount += 1;
          if (!sameSet(widget.role_ids, deck.role_ids)) {
            fail(
              "faceplate.role_mismatch",
              `${pointer}/role_ids`,
              "Role editor differs from deck-pack.json.",
            );
          }
          break;
        case "transport":
          transportCount += 1;
          if (!sameSet(widget.slot_indices, [...Array(deck.slots).keys()])) {
            fail(
              "faceplate.slot_mismatch",
              `${pointer}/slot_indices`,
              "Transport does not cover every physical slot.",
            );
          }
          break;
        case "seed":
          seedCount += 1;
          break;
        case "capture": {
          captureCount += 1;
          const required = new Set(
            widget.modes.map((mode) =>
              mode === "snapshot" ? "snapshot_capture" : "live_capture",
            ),
          );
          for (const capability of required) {
            if (!deck.capabilities.includes(capability)) {
              fail(
                "faceplate.capability_mismatch",
                `${pointer}/modes`,
                "Capture widget requires an undeclared capability.",
              );
            }
          }
          break;
        }
        case "monitor":
          monitorCount += 1;
          break;
      }
    }
  }
  if (!sameSet(sourceSlots, [...Array(deck.slots).keys()])) {
    fail(
      "faceplate.slot_mismatch",
      "/sections",
      "Faceplate must expose every physical source slot exactly once.",
    );
  }
  if (!sameSet(boundControls, controls.keys())) {
    fail(
      "faceplate.control_mismatch",
      "/sections",
      "Faceplate must expose every operator control exactly once.",
    );
  }
  if (
    roleEditorCount !== 1 ||
    transportCount !== 1 ||
    seedCount !== 1 ||
    monitorCount !== 1
  ) {
    fail(
      "faceplate.widget_mismatch",
      "/sections",
      "Faceplate requires one role editor, transport, seed, and monitor widget.",
    );
  }
  if (captureCount > 1) {
    fail(
      "faceplate.widget_mismatch",
      "/sections",
      "Faceplate may contain at most one capture widget.",
    );
  }
  if (
    faceplate.schema_version === FACEPLATE_SCHEMA_VERSION &&
    outputRegionCount !== 1
  ) {
    fail(
      "faceplate.layout_mismatch",
      "/sections",
      "Faceplate schema v2 requires exactly one output region.",
    );
  }
}

export function isFaceplateWidgetVisible(
  widget: FaceplateWidget,
  controls: Readonly<Record<string, unknown>>,
): boolean {
  return (
    widget.visible_when?.every((predicate) =>
      predicate.one_of.some(
        (expected) => controls[predicate.control_id] === expected,
      ),
    ) ?? true
  );
}

function parseWidget(
  value: unknown,
  pointer: string,
  schemaVersion: FaceplateSchemaVersion,
): FaceplateWidget {
  const widget = record(value, pointer);
  const kind = widget.kind;
  const visibility = parseVisibility(
    widget.visible_when,
    `${pointer}/visible_when`,
    schemaVersion,
  );
  const base = {
    id: identifier(widget.id, `${pointer}/id`),
    label: text(widget.label, `${pointer}/label`),
    ...(visibility === undefined ? {} : { visible_when: visibility }),
  };
  const optionalKeys =
    schemaVersion === FACEPLATE_SCHEMA_VERSION ? ["visible_when"] : [];
  switch (kind) {
    case "source_picker":
      exactKeys(
        widget,
        ["id", "kind", "label", "slot_index"],
        pointer,
        optionalKeys,
      );
      return {
        ...base,
        kind,
        slot_index: integer(widget.slot_index, `${pointer}/slot_index`, 0, 15),
      };
    case "slider":
    case "number": {
      exactKeys(
        widget,
        ["id", "kind", "label", "control_id", "minimum", "maximum", "step"],
        pointer,
        optionalKeys,
      );
      const minimum = finite(widget.minimum, `${pointer}/minimum`);
      const maximum = finite(widget.maximum, `${pointer}/maximum`);
      const step = finite(widget.step, `${pointer}/step`);
      if (minimum >= maximum || step <= 0) {
        fail(
          "faceplate.invalid_widget",
          pointer,
          "Numeric widget bounds are invalid.",
        );
      }
      return {
        ...base,
        kind,
        control_id: identifier(widget.control_id, `${pointer}/control_id`),
        minimum,
        maximum,
        step,
      };
    }
    case "toggle":
      exactKeys(
        widget,
        ["id", "kind", "label", "control_id"],
        pointer,
        optionalKeys,
      );
      return {
        ...base,
        kind,
        control_id: identifier(widget.control_id, `${pointer}/control_id`),
      };
    case "select": {
      exactKeys(
        widget,
        ["id", "kind", "label", "control_id", "options"],
        pointer,
        optionalKeys,
      );
      const options = array(widget.options, `${pointer}/options`).map(
        (option, index) => {
          const optionPointer = `${pointer}/options/${index}`;
          const item = record(option, optionPointer);
          exactKeys(item, ["value", "label"], optionPointer);
          return {
            value: identifier(item.value, `${optionPointer}/value`),
            label: text(item.label, `${optionPointer}/label`),
          };
        },
      );
      if (
        options.length < 1 ||
        options.length > 64 ||
        new Set(options.map((option) => option.value)).size !== options.length
      ) {
        fail(
          "faceplate.invalid_widget",
          `${pointer}/options`,
          "Select options are empty, duplicated, or oversized.",
        );
      }
      return {
        ...base,
        kind,
        control_id: identifier(widget.control_id, `${pointer}/control_id`),
        options,
      };
    }
    case "role_editor":
      exactKeys(
        widget,
        ["id", "kind", "label", "role_ids"],
        pointer,
        optionalKeys,
      );
      return {
        ...base,
        kind,
        role_ids: identifiers(widget.role_ids, `${pointer}/role_ids`),
      };
    case "barycentric3": {
      exactKeys(
        widget,
        [
          "id",
          "kind",
          "label",
          "x_control_id",
          "y_control_id",
          "vertex_role_ids",
        ],
        pointer,
        optionalKeys,
      );
      const roles = identifiers(
        widget.vertex_role_ids,
        `${pointer}/vertex_role_ids`,
      );
      if (roles.length !== 3 || new Set(roles).size !== 3) {
        fail(
          "faceplate.invalid_widget",
          `${pointer}/vertex_role_ids`,
          "Barycentric widget requires three distinct roles.",
        );
      }
      return {
        ...base,
        kind,
        x_control_id: identifier(
          widget.x_control_id,
          `${pointer}/x_control_id`,
        ),
        y_control_id: identifier(
          widget.y_control_id,
          `${pointer}/y_control_id`,
        ),
        vertex_role_ids: [roles[0], roles[1], roles[2]],
      };
    }
    case "transport":
      exactKeys(
        widget,
        ["id", "kind", "label", "slot_indices"],
        pointer,
        optionalKeys,
      );
      return {
        ...base,
        kind,
        slot_indices: integers(
          widget.slot_indices,
          `${pointer}/slot_indices`,
          0,
          15,
        ),
      };
    case "seed":
    case "monitor":
      exactKeys(widget, ["id", "kind", "label"], pointer, optionalKeys);
      return { ...base, kind };
    case "capture": {
      exactKeys(
        widget,
        ["id", "kind", "label", "modes"],
        pointer,
        optionalKeys,
      );
      const modes = array(widget.modes, `${pointer}/modes`);
      if (
        modes.length < 1 ||
        modes.length > 2 ||
        modes.some((mode) => mode !== "snapshot" && mode !== "live_capture") ||
        new Set(modes).size !== modes.length
      ) {
        fail(
          "faceplate.invalid_widget",
          `${pointer}/modes`,
          "Capture modes are invalid or duplicated.",
        );
      }
      return { ...base, kind, modes: modes as FaceplateCaptureMode[] };
    }
    default:
      fail(
        "faceplate.unknown_widget",
        `${pointer}/kind`,
        "Unknown host-rendered widget kind.",
      );
  }
}

function parseVisibility(
  value: unknown,
  pointer: string,
  schemaVersion: FaceplateSchemaVersion,
): FaceplateVisibilityPredicate[] | undefined {
  if (value === undefined) return undefined;
  if (schemaVersion !== FACEPLATE_SCHEMA_VERSION) {
    fail(
      "faceplate.closed_schema",
      pointer,
      "Visibility predicates require faceplate schema v2.",
    );
  }
  const predicates = array(value, pointer);
  if (predicates.length < 1 || predicates.length > 8) {
    fail(
      "faceplate.limit_exceeded",
      pointer,
      "Visibility predicate count is out of bounds.",
    );
  }
  return predicates.map((predicate, index) => {
    const predicatePointer = `${pointer}/${index}`;
    const item = record(predicate, predicatePointer);
    exactKeys(item, ["control_id", "one_of"], predicatePointer);
    const values = array(item.one_of, `${predicatePointer}/one_of`);
    if (
      values.length < 1 ||
      values.length > 16 ||
      values.some(
        (candidate) =>
          typeof candidate !== "boolean" &&
          (typeof candidate !== "string" || !IDENTIFIER.test(candidate)),
      ) ||
      new Set(values.map((candidate) => JSON.stringify(candidate))).size !==
        values.length
    ) {
      fail(
        "faceplate.invalid_widget",
        `${predicatePointer}/one_of`,
        "Visibility values are empty, duplicated, unsafe, or oversized.",
      );
    }
    return {
      control_id: identifier(item.control_id, `${predicatePointer}/control_id`),
      one_of: values as (string | boolean)[],
    };
  });
}

function record(value: unknown, pointer: string): Record<string, unknown> {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    fail("faceplate.invalid_json", pointer, "Expected a JSON object.");
  }
  return value as Record<string, unknown>;
}

function sectionRegion(
  value: unknown,
  pointer: string,
): FaceplateSectionRegion {
  if (value !== "output" && value !== "actions" && value !== "controls") {
    fail(
      "faceplate.invalid_layout",
      pointer,
      "Section region must be output, actions, or controls.",
    );
  }
  return value;
}

function array(value: unknown, pointer: string): unknown[] {
  if (!Array.isArray(value))
    fail("faceplate.invalid_json", pointer, "Expected a JSON array.");
  return value;
}

function exactKeys(
  value: Record<string, unknown>,
  required: readonly string[],
  pointer: string,
  optional: readonly string[] = [],
): void {
  const allowedSet = new Set([...required, ...optional]);
  const actual = Object.keys(value);
  if (
    actual.some((key) => !allowedSet.has(key)) ||
    required.some((key) => !Object.hasOwn(value, key))
  ) {
    fail(
      "faceplate.closed_schema",
      pointer,
      "Object fields do not match the closed faceplate schema.",
    );
  }
}

function validateVisibility(
  widget: FaceplateWidget,
  controls: ReadonlyMap<string, DeckControlContract>,
  pointer: string,
): void {
  for (const [predicateIndex, predicate] of (
    widget.visible_when ?? []
  ).entries()) {
    const predicatePointer = `${pointer}/visible_when/${predicateIndex}`;
    const control = controls.get(predicate.control_id);
    if (control === undefined) {
      fail(
        "faceplate.visibility_mismatch",
        `${predicatePointer}/control_id`,
        "Visibility predicate references an absent operator control.",
      );
    }
    if (control.value_type === "enum") {
      if (
        predicate.one_of.some(
          (value) =>
            typeof value !== "string" || !control.options.includes(value),
        )
      ) {
        fail(
          "faceplate.visibility_mismatch",
          `${predicatePointer}/one_of`,
          "Visibility predicate contains an invalid enum option.",
        );
      }
      continue;
    }
    if (
      control.value_type !== "boolean" ||
      predicate.one_of.some((value) => typeof value !== "boolean")
    ) {
      fail(
        "faceplate.visibility_mismatch",
        `${predicatePointer}/control_id`,
        "Visibility predicates may reference only matching enum or boolean controls.",
      );
    }
  }
}

function text(value: unknown, pointer: string): string {
  if (
    typeof value !== "string" ||
    value.length < 1 ||
    new TextEncoder().encode(value).length > MAX_TEXT_BYTES ||
    /[\u0000-\u001f\u007f]/u.test(value)
  ) {
    fail(
      "faceplate.invalid_text",
      pointer,
      "Faceplate text is empty, unsafe, or oversized.",
    );
  }
  return value;
}

function identifier(value: unknown, pointer: string): string {
  if (
    typeof value !== "string" ||
    !IDENTIFIER.test(value) ||
    value.length > 128
  ) {
    fail(
      "faceplate.invalid_identifier",
      pointer,
      "Identifier is not canonical.",
    );
  }
  return value;
}

function identifiers(value: unknown, pointer: string): string[] {
  const values = array(value, pointer).map((item, index) =>
    identifier(item, `${pointer}/${index}`),
  );
  if (
    values.length < 1 ||
    values.length > 16 ||
    new Set(values).size !== values.length
  ) {
    fail(
      "faceplate.invalid_widget",
      pointer,
      "Identifier list is empty, duplicated, or oversized.",
    );
  }
  return values;
}

function finite(value: unknown, pointer: string): number {
  if (typeof value !== "number" || !Number.isFinite(value)) {
    fail("faceplate.invalid_number", pointer, "Expected a finite JSON number.");
  }
  return value;
}

function integer(
  value: unknown,
  pointer: string,
  minimum: number,
  maximum: number,
): number {
  if (
    !Number.isInteger(value) ||
    (value as number) < minimum ||
    (value as number) > maximum
  ) {
    fail("faceplate.invalid_number", pointer, "Expected a bounded integer.");
  }
  return value as number;
}

function integers(
  value: unknown,
  pointer: string,
  minimum: number,
  maximum: number,
): number[] {
  const values = array(value, pointer).map((item, index) =>
    integer(item, `${pointer}/${index}`, minimum, maximum),
  );
  if (
    values.length < 1 ||
    values.length > 16 ||
    new Set(values).size !== values.length
  ) {
    fail(
      "faceplate.invalid_widget",
      pointer,
      "Integer list is empty, duplicated, or oversized.",
    );
  }
  return values;
}

function unique<T>(
  values: Set<T>,
  value: T,
  pointer: string,
  label: string,
): void {
  if (values.has(value))
    fail("faceplate.duplicate", pointer, `Duplicate ${label}.`);
  values.add(value);
}

function bindControl(
  values: Set<string>,
  controlId: string,
  pointer: string,
): void {
  unique(values, controlId, `${pointer}/control_id`, "control binding");
}

function requireControl(
  controls: ReadonlyMap<string, DeckControlContract>,
  controlId: string,
  pointer: string,
): DeckControlContract {
  const control = controls.get(controlId);
  if (!control)
    fail(
      "faceplate.control_mismatch",
      `${pointer}/control_id`,
      "Widget references an absent operator control.",
    );
  return control;
}

function requireBarycentricControl(
  controls: ReadonlyMap<string, DeckControlContract>,
  controlId: string,
  pointer: string,
): void {
  const control = requireControl(controls, controlId, pointer);
  if (
    control.value_type !== "number" ||
    control.minimum !== 0 ||
    control.maximum !== 1
  ) {
    fail(
      "faceplate.control_mismatch",
      pointer,
      "Barycentric coordinate must bind a normalized [0, 1] number control.",
    );
  }
}

function sameSet<T>(left: Iterable<T>, right: Iterable<T>): boolean {
  const a = new Set(left);
  const b = new Set(right);
  return a.size === b.size && [...a].every((value) => b.has(value));
}

function fail(code: string, pointer: string, message: string): never {
  throw new FaceplateContractError(code, pointer || "/", message);
}
