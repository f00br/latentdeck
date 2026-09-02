import {
  parseFaceplateDefinition,
  validateFaceplateAgainstDeck,
  type DeckControlContract,
  type FaceplateDefinition,
} from "./faceplate-model";

export type DeckUiScalar = string | number | boolean;

export interface DeckUiPackageReference {
  kind: "deck_pack";
  packageId: string;
  packageVersion: string;
}

export interface DeckUiRole {
  roleId: string;
  displayName: string;
}

export type DeckUiControl =
  | {
      controlId: string;
      valueType: "number" | "integer";
      defaultValue: number;
      minimum: number;
      maximum: number;
      step: number;
    }
  | {
      controlId: string;
      valueType: "boolean";
      defaultValue: boolean;
    }
  | {
      controlId: string;
      valueType: "enum";
      defaultValue: string;
      options: readonly string[];
    };

export interface DeckUiCatalogEntryInput {
  package: unknown;
  deck: unknown;
  operator: unknown;
  faceplate: unknown;
}

export interface DeckUiCatalogInput {
  decks: unknown;
  issues: unknown;
}

export interface DeckUiCatalogIssue {
  package: DeckUiPackageReference;
  code: string;
  detail: string;
}

export interface DeckUiModel {
  exactKey: string;
  package: DeckUiPackageReference;
  deckId: string;
  deckVersion: string;
  displayName: string;
  summary: string;
  slots: number;
  roles: readonly DeckUiRole[];
  defaultPermutation: readonly string[];
  structuralCarrierRole: string;
  requiredCapabilities: readonly string[];
  operatorId: string;
  controls: readonly DeckUiControl[];
  faceplate: FaceplateDefinition;
}

export interface DeckUiCatalog {
  decks: readonly DeckUiModel[];
  issues: readonly DeckUiCatalogIssue[];
}

export interface DeckUiDraft {
  sourceArchiveSha256s: string[];
  controls: Record<string, DeckUiScalar>;
  roleBindings: Record<string, number>;
  playing: boolean[];
  loops: boolean[];
  seed: number;
}

const IDENTIFIER = /^[a-z][a-z0-9]*(?:[._-][a-z0-9]+)*$/;
const MAX_CATALOG_DECKS = 256;
const MAX_ISSUES = 256;
const MAX_TEXT_BYTES = 512;
const MAX_REVERSE_DNS_ID_BYTES = 160;
const MAX_U64_DECIMAL = "18446744073709551615";

export class DeckUiContractError extends Error {
  constructor(
    readonly code: string,
    readonly pointer: string,
    message: string,
  ) {
    super(message);
    this.name = "DeckUiContractError";
  }
}

export function parseDeckUiCatalog(value: unknown): DeckUiCatalog {
  const root = record(value, "");
  exactKeys(root, ["decks", "issues"], "");
  const rawDecks = array(root.decks, "/decks", MAX_CATALOG_DECKS);
  const decks = rawDecks.map((item, index) =>
    parseCatalogDeck(item, `/decks/${index}`),
  );
  const exactKeysSeen = new Set<string>();
  for (const [index, deck] of decks.entries()) {
    if (exactKeysSeen.has(deck.exactKey)) {
      fail(
        "deck_ui.duplicate_identity",
        `/decks/${index}/package`,
        "The exact Deck identity is duplicated.",
      );
    }
    exactKeysSeen.add(deck.exactKey);
  }
  const issues = array(root.issues, "/issues", MAX_ISSUES).map((item, index) =>
    parseIssue(item, `/issues/${index}`),
  );
  return { decks, issues };
}

export function createDeckUiDraft(model: DeckUiModel): DeckUiDraft {
  const roleBindings: Record<string, number> = {};
  for (const [slotIndex, roleId] of model.defaultPermutation.entries()) {
    roleBindings[roleId] = slotIndex;
  }
  return {
    sourceArchiveSha256s: Array.from({ length: model.slots }, () => ""),
    controls: Object.fromEntries(
      model.controls.map((control) => [
        control.controlId,
        control.defaultValue,
      ]),
    ),
    roleBindings,
    playing: Array.from({ length: model.slots }, () => true),
    loops: Array.from({ length: model.slots }, () => true),
    seed: 0,
  };
}

export function serializeDeckControls(
  model: Pick<DeckUiModel, "controls" | "faceplate">,
  values: Readonly<Record<string, unknown>>,
): Record<string, DeckUiScalar> {
  const expected = new Set(model.controls.map((control) => control.controlId));
  if (
    Object.keys(values).length !== expected.size ||
    Object.keys(values).some((key) => !expected.has(key))
  ) {
    fail(
      "deck_ui.control_invalid",
      "/controls",
      "The control draft must contain exactly the declared controls.",
    );
  }
  const result: Record<string, DeckUiScalar> = {};
  for (const control of model.controls) {
    const value = values[control.controlId];
    switch (control.valueType) {
      case "number":
        if (
          typeof value !== "number" ||
          !Number.isFinite(value) ||
          value < control.minimum ||
          value > control.maximum
        ) {
          invalidControl(control.controlId);
        }
        result[control.controlId] = value;
        break;
      case "integer":
        if (
          !Number.isSafeInteger(value) ||
          (value as number) < control.minimum ||
          (value as number) > control.maximum
        ) {
          invalidControl(control.controlId);
        }
        result[control.controlId] = value as number;
        break;
      case "boolean":
        if (typeof value !== "boolean") invalidControl(control.controlId);
        result[control.controlId] = value as boolean;
        break;
      case "enum":
        if (typeof value !== "string" || !control.options.includes(value)) {
          invalidControl(control.controlId);
        }
        result[control.controlId] = value as string;
        break;
    }
  }
  validateBarycentricValues(model.faceplate, result);
  return result;
}

export function serializeRoleBindings(
  model: Pick<DeckUiModel, "roles" | "slots">,
  bindings: Readonly<Record<string, unknown>>,
): Record<string, number> {
  const roleIds = model.roles.map((role) => role.roleId);
  if (
    Object.keys(bindings).length !== roleIds.length ||
    Object.keys(bindings).some((role) => !roleIds.includes(role))
  ) {
    fail(
      "deck_ui.role_invalid",
      "/roles",
      "Role bindings must contain every declared role exactly once.",
    );
  }
  const result: Record<string, number> = {};
  const usedSlots = new Set<number>();
  for (const roleId of roleIds) {
    const slot = bindings[roleId];
    if (
      !Number.isInteger(slot) ||
      (slot as number) < 0 ||
      (slot as number) >= model.slots ||
      usedSlots.has(slot as number)
    ) {
      fail(
        "deck_ui.role_invalid",
        `/roles/${roleId}`,
        "Role bindings must be an exact physical-slot permutation.",
      );
    }
    usedSlots.add(slot as number);
    result[roleId] = slot as number;
  }
  return result;
}

function parseCatalogDeck(value: unknown, pointer: string): DeckUiModel {
  const root = record(value, pointer);
  exactKeys(root, ["package", "deck", "operator", "faceplate"], pointer);
  const packageReference = parsePackageReference(
    root.package,
    `${pointer}/package`,
  );
  const deck = record(root.deck, `${pointer}/deck`);
  exactKeys(
    deck,
    [
      "deckId",
      "deckVersion",
      "displayName",
      "summary",
      "slots",
      "roles",
      "defaultPermutation",
      "structuralCarrierRole",
      "requiredCapabilities",
    ],
    `${pointer}/deck`,
  );
  const deckId = reverseDnsIdentifier(deck.deckId, `${pointer}/deck/deckId`);
  const deckVersion = semver(deck.deckVersion, `${pointer}/deck/deckVersion`);
  if (
    deckId !== packageReference.packageId ||
    deckVersion !== packageReference.packageVersion
  ) {
    fail(
      "deck_ui.identity_mismatch",
      `${pointer}/deck`,
      "Catalog and Deck manifest identities differ.",
    );
  }
  const slots = boundedInteger(deck.slots, `${pointer}/deck/slots`, 1, 16);
  const roles = array(deck.roles, `${pointer}/deck/roles`, 16).map(
    (item, index) => {
      const rolePointer = `${pointer}/deck/roles/${index}`;
      const role = record(item, rolePointer);
      exactKeys(role, ["roleId", "displayName"], rolePointer);
      return {
        roleId: identifier(role.roleId, `${rolePointer}/roleId`),
        displayName: text(role.displayName, `${rolePointer}/displayName`),
      };
    },
  );
  if (
    roles.length !== slots ||
    new Set(roles.map((role) => role.roleId)).size !== roles.length
  ) {
    fail(
      "deck_ui.role_invalid",
      `${pointer}/deck/roles`,
      "Deck roles must be unique and match the physical slot count.",
    );
  }
  const roleIds = roles.map((role) => role.roleId);
  const defaultPermutation = identifiers(
    deck.defaultPermutation,
    `${pointer}/deck/defaultPermutation`,
    16,
  );
  if (
    !sameSet(roleIds, defaultPermutation) ||
    defaultPermutation.length !== slots
  ) {
    fail(
      "deck_ui.role_invalid",
      `${pointer}/deck/defaultPermutation`,
      "The default role permutation must cover every role exactly once.",
    );
  }
  const structuralCarrierRole = identifier(
    deck.structuralCarrierRole,
    `${pointer}/deck/structuralCarrierRole`,
  );
  if (!roleIds.includes(structuralCarrierRole)) {
    fail(
      "deck_ui.role_invalid",
      `${pointer}/deck/structuralCarrierRole`,
      "The structural carrier role is not declared.",
    );
  }
  const requiredCapabilities = identifiers(
    deck.requiredCapabilities,
    `${pointer}/deck/requiredCapabilities`,
    32,
  );

  const operator = record(root.operator, `${pointer}/operator`);
  exactKeys(operator, ["operatorId", "controls"], `${pointer}/operator`);
  const controls = array(
    operator.controls,
    `${pointer}/operator/controls`,
    128,
  ).map((item, index) =>
    parseControl(item, `${pointer}/operator/controls/${index}`),
  );
  if (
    new Set(controls.map((control) => control.controlId)).size !==
    controls.length
  ) {
    fail(
      "deck_ui.control_invalid",
      `${pointer}/operator/controls`,
      "Operator control IDs are duplicated.",
    );
  }

  const faceplate = parseFaceplateDefinition(root.faceplate);
  validateFaceplateAgainstDeck(faceplate, {
    slots,
    role_ids: roleIds,
    controls: controls.map(faceplateControlContract),
    capabilities: requiredCapabilities,
  });
  serializeDeckControls(
    { controls, faceplate },
    Object.fromEntries(
      controls.map((control) => [control.controlId, control.defaultValue]),
    ),
  );

  return {
    exactKey: `${deckId}@${deckVersion}`,
    package: packageReference,
    deckId,
    deckVersion,
    displayName: text(deck.displayName, `${pointer}/deck/displayName`),
    summary: text(deck.summary, `${pointer}/deck/summary`),
    slots,
    roles,
    defaultPermutation,
    structuralCarrierRole,
    requiredCapabilities,
    operatorId: identifier(
      operator.operatorId,
      `${pointer}/operator/operatorId`,
    ),
    controls,
    faceplate,
  };
}

function parseControl(value: unknown, pointer: string): DeckUiControl {
  const control = record(value, pointer);
  const valueType = control.value_type;
  switch (valueType) {
    case "number":
    case "integer": {
      exactKeys(
        control,
        ["control_id", "value_type", "default", "minimum", "maximum", "step"],
        pointer,
      );
      const minimum = finite(control.minimum, `${pointer}/minimum`);
      const maximum = finite(control.maximum, `${pointer}/maximum`);
      const step = finite(control.step, `${pointer}/step`);
      const defaultValue = finite(control.default, `${pointer}/default`);
      if (
        minimum >= maximum ||
        step <= 0 ||
        defaultValue < minimum ||
        defaultValue > maximum ||
        (valueType === "integer" &&
          (!Number.isSafeInteger(minimum) ||
            !Number.isSafeInteger(maximum) ||
            !Number.isSafeInteger(step) ||
            !Number.isSafeInteger(defaultValue)))
      ) {
        fail(
          "deck_ui.control_invalid",
          pointer,
          "Numeric control bounds or default are invalid.",
        );
      }
      return {
        controlId: identifier(control.control_id, `${pointer}/control_id`),
        valueType,
        defaultValue,
        minimum,
        maximum,
        step,
      };
    }
    case "boolean":
      exactKeys(control, ["control_id", "value_type", "default"], pointer);
      if (typeof control.default !== "boolean") {
        fail(
          "deck_ui.control_invalid",
          `${pointer}/default`,
          "Boolean control default is invalid.",
        );
      }
      return {
        controlId: identifier(control.control_id, `${pointer}/control_id`),
        valueType,
        defaultValue: control.default,
      };
    case "enum": {
      exactKeys(
        control,
        ["control_id", "value_type", "default", "options"],
        pointer,
      );
      const options = identifiers(control.options, `${pointer}/options`, 64);
      const defaultValue = identifier(control.default, `${pointer}/default`);
      if (!options.includes(defaultValue)) {
        fail(
          "deck_ui.control_invalid",
          `${pointer}/default`,
          "Enum default is not one of the exact options.",
        );
      }
      return {
        controlId: identifier(control.control_id, `${pointer}/control_id`),
        valueType,
        defaultValue,
        options,
      };
    }
    default:
      fail(
        "deck_ui.control_invalid",
        `${pointer}/value_type`,
        "Unknown operator control type.",
      );
  }
}

function faceplateControlContract(control: DeckUiControl): DeckControlContract {
  switch (control.valueType) {
    case "number":
    case "integer":
      return {
        control_id: control.controlId,
        value_type: control.valueType,
        minimum: control.minimum,
        maximum: control.maximum,
        step: control.step,
      };
    case "boolean":
      return { control_id: control.controlId, value_type: "boolean" };
    case "enum":
      return {
        control_id: control.controlId,
        value_type: "enum",
        options: control.options,
      };
  }
}

function parsePackageReference(
  value: unknown,
  pointer: string,
): DeckUiPackageReference {
  const packageReference = record(value, pointer);
  exactKeys(packageReference, ["kind", "packageId", "packageVersion"], pointer);
  if (packageReference.kind !== "deck_pack") {
    fail(
      "deck_ui.package_invalid",
      `${pointer}/kind`,
      "Expected a Deck package.",
    );
  }
  return {
    kind: "deck_pack",
    packageId: reverseDnsIdentifier(
      packageReference.packageId,
      `${pointer}/packageId`,
    ),
    packageVersion: semver(
      packageReference.packageVersion,
      `${pointer}/packageVersion`,
    ),
  };
}

function parseIssue(value: unknown, pointer: string): DeckUiCatalogIssue {
  const issue = record(value, pointer);
  exactKeys(issue, ["package", "code", "detail"], pointer);
  return {
    package: parsePackageReference(issue.package, `${pointer}/package`),
    code: identifier(issue.code, `${pointer}/code`),
    detail: text(issue.detail, `${pointer}/detail`),
  };
}

function record(value: unknown, pointer: string): Record<string, unknown> {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    fail("deck_ui.invalid_json", pointer, "Expected a JSON object.");
  }
  return value as Record<string, unknown>;
}

function array(value: unknown, pointer: string, maximum: number): unknown[] {
  if (!Array.isArray(value) || value.length > maximum) {
    fail("deck_ui.limit_exceeded", pointer, "Expected a bounded JSON array.");
  }
  return value;
}

function exactKeys(
  value: Record<string, unknown>,
  allowed: readonly string[],
  pointer: string,
): void {
  const allowedSet = new Set(allowed);
  if (
    Object.keys(value).some((key) => !allowedSet.has(key)) ||
    allowed.some((key) => !Object.hasOwn(value, key))
  ) {
    fail(
      "deck_ui.closed_schema",
      pointer,
      "Object fields do not match the closed Deck UI schema.",
    );
  }
}

function identifier(value: unknown, pointer: string): string {
  if (
    typeof value !== "string" ||
    value.length > 128 ||
    !IDENTIFIER.test(value)
  ) {
    fail("deck_ui.invalid_identifier", pointer, "Identifier is not canonical.");
  }
  return value;
}

function reverseDnsIdentifier(value: unknown, pointer: string): string {
  if (
    typeof value !== "string" ||
    new TextEncoder().encode(value).length > MAX_REVERSE_DNS_ID_BYTES
  ) {
    fail(
      "deck_ui.invalid_identifier",
      pointer,
      "Package identifier is not canonical reverse-DNS.",
    );
  }
  const segments = value.split(".");
  if (
    segments.length < 2 ||
    segments.some(
      (segment) =>
        segment.length < 1 ||
        segment.length > 63 ||
        segment.startsWith("-") ||
        segment.endsWith("-") ||
        ![...segment].every(
          (character) =>
            (character >= "a" && character <= "z") ||
            (character >= "0" && character <= "9") ||
            character === "-",
        ),
    )
  ) {
    fail(
      "deck_ui.invalid_identifier",
      pointer,
      "Package identifier is not canonical reverse-DNS.",
    );
  }
  return value;
}

function identifiers(
  value: unknown,
  pointer: string,
  maximum: number,
): string[] {
  const result = array(value, pointer, maximum).map((item, index) =>
    identifier(item, `${pointer}/${index}`),
  );
  if (result.length < 1 || new Set(result).size !== result.length) {
    fail(
      "deck_ui.invalid_identifier",
      pointer,
      "Identifier list is empty or duplicated.",
    );
  }
  return result;
}

function semver(value: unknown, pointer: string): string {
  if (typeof value !== "string" || !isCanonicalSemver(value)) {
    fail("deck_ui.package_invalid", pointer, "Version must be exact SemVer.");
  }
  return value;
}

function isCanonicalSemver(value: string): boolean {
  const plus = value.indexOf("+");
  if (plus !== -1 && value.indexOf("+", plus + 1) !== -1) return false;
  const coreAndPrerelease = plus === -1 ? value : value.slice(0, plus);
  const build = plus === -1 ? undefined : value.slice(plus + 1);
  if (build !== undefined && !validSemverIdentifiers(build, false))
    return false;

  const dash = coreAndPrerelease.indexOf("-");
  const core =
    dash === -1 ? coreAndPrerelease : coreAndPrerelease.slice(0, dash);
  const prerelease =
    dash === -1 ? undefined : coreAndPrerelease.slice(dash + 1);
  const coreParts = core.split(".");
  if (
    coreParts.length !== 3 ||
    coreParts.some((part) => !isCanonicalU64(part))
  ) {
    return false;
  }
  return prerelease === undefined || validSemverIdentifiers(prerelease, true);
}

function isCanonicalU64(value: string): boolean {
  if (!/^[0-9]+$/u.test(value) || (value.length > 1 && value.startsWith("0"))) {
    return false;
  }
  return (
    value.length < MAX_U64_DECIMAL.length ||
    (value.length === MAX_U64_DECIMAL.length && value <= MAX_U64_DECIMAL)
  );
}

function validSemverIdentifiers(
  value: string,
  rejectNumericLeadingZeros: boolean,
): boolean {
  const identifiers = value.split(".");
  return identifiers.every((identifier) => {
    if (
      identifier.length === 0 ||
      ![...identifier].every(
        (character) =>
          (character >= "A" && character <= "Z") ||
          (character >= "a" && character <= "z") ||
          (character >= "0" && character <= "9") ||
          character === "-",
      )
    ) {
      return false;
    }
    return !(
      rejectNumericLeadingZeros &&
      /^[0-9]+$/u.test(identifier) &&
      identifier.length > 1 &&
      identifier.startsWith("0")
    );
  });
}

function text(value: unknown, pointer: string): string {
  if (
    typeof value !== "string" ||
    value.length < 1 ||
    new TextEncoder().encode(value).length > MAX_TEXT_BYTES ||
    /[\u0000-\u001f\u007f]/u.test(value)
  ) {
    fail(
      "deck_ui.invalid_text",
      pointer,
      "Text is empty, unsafe, or oversized.",
    );
  }
  return value;
}

function finite(value: unknown, pointer: string): number {
  if (typeof value !== "number" || !Number.isFinite(value)) {
    fail("deck_ui.control_invalid", pointer, "Expected a finite number.");
  }
  return value;
}

function boundedInteger(
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
    fail("deck_ui.package_invalid", pointer, "Expected a bounded integer.");
  }
  return value as number;
}

function sameSet<T>(left: Iterable<T>, right: Iterable<T>): boolean {
  const a = new Set(left);
  const b = new Set(right);
  return a.size === b.size && [...a].every((item) => b.has(item));
}

function invalidControl(controlId: string): never {
  fail(
    "deck_ui.control_invalid",
    `/controls/${controlId}`,
    "Control value does not match its exact declared type and bounds.",
  );
}

function validateBarycentricValues(
  faceplate: FaceplateDefinition,
  controls: Readonly<Record<string, DeckUiScalar>>,
): void {
  for (const widget of faceplate.sections.flatMap(
    (section) => section.widgets,
  )) {
    if (widget.kind !== "barycentric3") continue;
    const x = controls[widget.x_control_id];
    const y = controls[widget.y_control_id];
    if (
      typeof x !== "number" ||
      typeof y !== "number" ||
      x < 0.5 * y - 1e-12 ||
      x > 1 - 0.5 * y + 1e-12
    ) {
      fail(
        "deck_ui.control_invalid",
        `/controls/${widget.x_control_id},${widget.y_control_id}`,
        "Barycentric controls must identify a point inside the declared triangle.",
      );
    }
  }
}

function fail(code: string, pointer: string, message: string): never {
  throw new DeckUiContractError(code, pointer || "/", message);
}
