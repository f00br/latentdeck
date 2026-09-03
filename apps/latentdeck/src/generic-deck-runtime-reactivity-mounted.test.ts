import { flushSync, mount, tick, unmount } from "svelte";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import d2DeckPack from "../../../operators/builtin/d2/package/deck-pack.json";
import d2Faceplate from "../../../operators/builtin/d2/package/faceplate.json";
import d2Operator from "../../../operators/builtin/d2/package/operator.json";
import { parseDeckUiCatalog } from "./deck-ui-model";
import type {
  GenericProfileKey,
  GenericRuntimeOptions,
} from "./generic-deck-client";
import GenericDeckWorkspace from "./GenericDeckWorkspace.svelte";
import type { CartridgeView, LibraryView } from "./library-model";

const { invokeMock } = vi.hoisted(() => ({ invokeMock: vi.fn() }));

vi.mock("@tauri-apps/api/core", () => ({ invoke: invokeMock }));

const CODEC_ID = "org.example.codec";
const CODEC_VERSION = "2.0.0";
const SOURCE_HASH = "a".repeat(64);
const ASSET_HASH = "b".repeat(64);
const PROFILE: GenericProfileKey = {
  codecFamily: "test",
  profile: "latent",
  profileVersion: "1.0.0",
};

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

function cartridge(): CartridgeView {
  return {
    archiveSha256: SOURCE_HASH,
    cartridgeId: "source-a",
    codecFamily: "test",
    codecProfile: "latent",
    codecProfileVersion: "1.0.0",
    timingContract: "test_24fps",
    timingContractVersion: "1.0.0",
    frameRateNumerator: 24,
    frameRateDenominator: 1,
    decodedWidth: 64,
    decodedHeight: 64,
    decodedFrameCount: 24,
    durationNumerator: 1,
    durationDenominator: 1,
    signalGeometry: {
      codec_family: "test",
      profile: "latent",
      profile_version: "1.0.0",
      runtime_dtype: "F16",
      batch: 1,
      latent_channels: 24,
      latent_slots: 24,
      latent_height: 30,
      latent_width: 45,
      decoded_frame_count: 24,
      decoded_height: 64,
      decoded_width: 64,
      timing_contract: "test_24fps",
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
    paths: [
      {
        path: "source-a.lc",
        fileName: "source-a.lc",
        state: "present",
        warningCode: null,
      },
    ],
  };
}

function library(): LibraryView {
  const source = cartridge();
  return {
    deckSession: {
      activeCollectionId: "latentdeck.virtual.all",
      loadedSlots: [],
    },
    collections: [],
    cartridges: [source],
    recent: [source],
    search: "",
    totalIndexed: 1,
    activeMemberCount: 1,
  };
}

function runtimeOptions(
  profileKey: GenericProfileKey | null,
  assetBound: boolean,
): GenericRuntimeOptions {
  return {
    deck: {
      packageId: d2DeckPack.deck_id,
      packageVersion: d2DeckPack.deck_version,
    },
    codec: { packageId: CODEC_ID, packageVersion: CODEC_VERSION },
    reason: assetBound ? "compatible" : "missing_asset",
    profiles: [PROFILE],
    device: "cuda",
    slots: 2,
    externalAssets: [
      {
        assetId: "decoder",
        displayName: "Test decoder",
        requiredSha256: ASSET_HASH,
        byteLength: 1024,
        required: true,
        bound: assetBound,
        boundSha256: assetBound ? ASSET_HASH : null,
      },
    ],
    sources:
      profileKey === null
        ? []
        : [
            {
              cartridgeId: "source-a",
              archiveSha256: SOURCE_HASH,
              reason: assetBound ? "compatible" : "missing_asset",
            },
          ],
  };
}

async function settleUi(): Promise<void> {
  for (let index = 0; index < 8; index += 1) {
    await Promise.resolve();
    await tick();
    flushSync();
  }
}

function changeSelect(select: HTMLSelectElement, value: string): void {
  select.value = value;
  select.dispatchEvent(new Event("change", { bubbles: true }));
  flushSync();
}

function findLoadButton(target: HTMLElement): HTMLButtonElement {
  const button = Array.from(target.querySelectorAll("button")).find(
    (candidate) => candidate.textContent?.trim() === "Load exact Deck draft",
  );
  if (!(button instanceof HTMLButtonElement)) {
    throw new Error("Missing Load exact Deck draft button");
  }
  return button;
}

async function configureReadyDraft(target: HTMLElement): Promise<void> {
  const selects = target.querySelectorAll<HTMLSelectElement>(
    ".runtime-config .config-grid select",
  );
  changeSelect(selects[0], `${CODEC_ID}@${CODEC_VERSION}`);
  await settleUi();
  changeSelect(selects[1], "cuda");
  await settleUi();
  changeSelect(
    selects[2],
    [PROFILE.codecFamily, PROFILE.profile, PROFILE.profileVersion].join(
      "\u0000",
    ),
  );
  await settleUi();
  target
    .querySelectorAll<HTMLSelectElement>(
      '[data-widget-kind="source_picker"] select',
    )
    .forEach((select) => changeSelect(select, SOURCE_HASH));
  await settleUi();
}

function deferred<T>(): {
  promise: Promise<T>;
  resolve(value: T): void;
} {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((accept) => {
    resolve = accept;
  });
  return { promise, resolve };
}

describe("generic Deck runtime negotiation reactivity", () => {
  beforeEach(() => {
    invokeMock.mockReset();
    vi.stubGlobal(
      "ResizeObserver",
      class {
        observe(): void {}
        disconnect(): void {}
      },
    );
    let nextFrame = 0;
    vi.stubGlobal("requestAnimationFrame", (callback: FrameRequestCallback) => {
      const frame = ++nextFrame;
      void Promise.resolve().then(() => callback(0));
      return frame;
    });
    vi.stubGlobal("cancelAnimationFrame", () => undefined);
    vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockReturnValue({
      x: 120,
      y: 160,
      top: 160,
      left: 120,
      right: 920,
      bottom: 608,
      width: 800,
      height: 448,
      toJSON: () => ({}),
    } as DOMRect);
    vi.spyOn(document.documentElement, "clientWidth", "get").mockReturnValue(
      1440,
    );
    vi.spyOn(document.documentElement, "clientHeight", "get").mockReturnValue(
      1000,
    );
  });

  afterEach(() => {
    vi.useRealTimers();
    vi.restoreAllMocks();
    vi.unstubAllGlobals();
  });

  it("refreshes faceplate readiness and source eligibility after profile and asset selection", async () => {
    let assetBound = false;
    invokeMock.mockImplementation(
      (command: string, args?: Record<string, unknown>) => {
        switch (command) {
          case "extensions_snapshot":
            return Promise.resolve({
              packages: [],
              matrix: [
                {
                  deck: {
                    kind: "deck_pack",
                    packageId: d2DeckPack.deck_id,
                    packageVersion: d2DeckPack.deck_version,
                  },
                  codec: {
                    kind: "codec_pack",
                    packageId: CODEC_ID,
                    packageVersion: CODEC_VERSION,
                  },
                  reason: "compatible",
                  compatibleProfile: PROFILE,
                },
              ],
            });
          case "deck_generic_sessions_get":
            return Promise.resolve({
              sessions: [],
              foregroundOutput: null,
              outputPin: null,
              recentFaults: [],
            });
          case "deck_generic_runtime_options": {
            const request = args?.request as
              { profileKey: GenericProfileKey | null } | undefined;
            return Promise.resolve(
              runtimeOptions(request?.profileKey ?? null, assetBound),
            );
          }
          case "deck_generic_external_asset_select":
            assetBound = true;
            return Promise.resolve({
              codecId: CODEC_ID,
              codecVersion: CODEC_VERSION,
              assetId: "decoder",
              bound: true,
              sha256: ASSET_HASH,
              byteLength: 1024,
            });
          case "deck_generic_external_asset_clear":
            assetBound = false;
            return Promise.resolve({
              codecId: CODEC_ID,
              codecVersion: CODEC_VERSION,
              assetId: "decoder",
              bound: false,
              sha256: null,
              byteLength: null,
            });
          case "deck_generic_viewport_session_begin":
            return Promise.resolve({ epoch: 1 });
          case "deck_generic_viewport_set_bounds":
            return Promise.resolve();
          case "deck_generic_viewport_hide":
            return Promise.resolve();
          default:
            return Promise.reject(new Error(`unexpected command: ${command}`));
        }
      },
    );

    const deck = model();
    const target = document.createElement("div");
    document.body.append(target);
    const component = mount(GenericDeckWorkspace, {
      target,
      props: {
        model: deck,
        models: [deck],
        library: library(),
        active: true,
        registerLeave: () => undefined,
      },
    });
    await settleUi();

    const configSelects = target.querySelectorAll<HTMLSelectElement>(
      ".runtime-config .config-grid select",
    );
    expect(configSelects).toHaveLength(3);
    changeSelect(configSelects[0], `${CODEC_ID}@${CODEC_VERSION}`);
    await settleUi();
    changeSelect(configSelects[1], "cuda");
    await settleUi();

    expect(target.textContent).toContain("Required external asset missing");
    expect(target.textContent).not.toContain("Choose an exact Codec version.");
    const sourceOption = target.querySelector<HTMLOptionElement>(
      `[data-widget-kind="source_picker"] option[value="${SOURCE_HASH}"]`,
    );
    expect(sourceOption?.disabled).toBe(true);
    expect(sourceOption?.textContent).toContain(
      "INCOMPATIBLE: Select an exact Codec profile",
    );

    const chooseAsset = Array.from(target.querySelectorAll("button")).find(
      (button) => button.textContent?.trim() === "Choose file…",
    );
    expect(chooseAsset).toBeInstanceOf(HTMLButtonElement);
    chooseAsset!.click();
    await settleUi();

    expect(target.textContent).toContain(
      "Choose one exact compatible Codec profile.",
    );
    expect(target.textContent).not.toContain("Required external asset missing");
    expect(
      Array.from(target.querySelectorAll("button")).find(
        (button) => button.textContent?.trim() === "Clear",
      )?.disabled,
    ).toBe(false);

    const clearAsset = Array.from(target.querySelectorAll("button")).find(
      (button) => button.textContent?.trim() === "Clear",
    );
    expect(clearAsset).toBeInstanceOf(HTMLButtonElement);
    clearAsset!.click();
    await settleUi();
    expect(target.textContent).toContain("Required external asset missing");

    changeSelect(
      configSelects[2],
      [PROFILE.codecFamily, PROFILE.profile, PROFILE.profileVersion].join(
        "\u0000",
      ),
    );
    await settleUi();

    expect(target.textContent).toContain("Required external asset missing");
    expect(target.textContent).not.toContain("Choose an exact Codec version.");
    expect(sourceOption?.disabled).toBe(true);
    expect(sourceOption?.textContent).toContain(
      "INCOMPATIBLE: Required external asset missing",
    );

    chooseAsset!.click();
    await settleUi();

    expect(target.textContent).toContain("Exact runtime preflight complete.");
    expect(target.textContent).not.toContain("Required external asset missing");
    expect(target.querySelector(".runtime-unavailable")).toBeNull();
    expect(sourceOption?.disabled).toBe(false);
    expect(sourceOption?.textContent).not.toContain("INCOMPATIBLE");
    const sourceSelects = target.querySelectorAll<HTMLSelectElement>(
      '[data-widget-kind="source_picker"] select',
    );
    expect(sourceSelects).toHaveLength(2);
    sourceSelects.forEach((select) => changeSelect(select, SOURCE_HASH));
    await settleUi();
    const loadButton = Array.from(target.querySelectorAll("button")).find(
      (button) => button.textContent?.trim() === "Load exact Deck draft",
    );
    expect(loadButton).toBeInstanceOf(HTMLButtonElement);
    expect(loadButton?.disabled).toBe(false);

    const ordinal = target.querySelector<HTMLInputElement>(
      '.runtime-config input[type="number"]',
    );
    expect(ordinal).toBeInstanceOf(HTMLInputElement);
    ordinal!.value = "1";
    ordinal!.dispatchEvent(new Event("input", { bubbles: true }));
    ordinal!.dispatchEvent(new Event("change", { bubbles: true }));
    flushSync();
    await settleUi();

    const runtimeRequests = invokeMock.mock.calls
      .filter(([command]) => command === "deck_generic_runtime_options")
      .map(
        ([, args]) => (args as { request: Record<string, unknown> }).request,
      );
    expect(runtimeRequests.slice(-2)).toEqual([
      expect.objectContaining({ deviceOrdinal: 1, profileKey: null }),
      expect.objectContaining({ deviceOrdinal: 1, profileKey: PROFILE }),
    ]);
    expect(target.textContent).toContain("Exact runtime preflight complete.");
    expect(loadButton?.disabled).toBe(false);

    const clearAssetAfterOrdinal = Array.from(
      target.querySelectorAll("button"),
    ).find((button) => button.textContent?.trim() === "Clear");
    expect(clearAssetAfterOrdinal).toBeInstanceOf(HTMLButtonElement);
    clearAssetAfterOrdinal!.click();
    await settleUi();

    expect(target.textContent).toContain("Required external asset missing");
    expect(target.querySelector(".runtime-unavailable")).not.toBeNull();
    expect(sourceOption?.disabled).toBe(true);
    expect(loadButton?.disabled).toBe(true);

    changeSelect(configSelects[0], "");
    await settleUi();
    expect(target.textContent).toContain("Choose an exact Codec version.");
    changeSelect(configSelects[0], `${CODEC_ID}@${CODEC_VERSION}`);
    await settleUi();
    expect(target.textContent).toContain("Required external asset missing");
    expect(
      Array.from(configSelects[2].options).some(
        (option) =>
          option.value ===
          [PROFILE.codecFamily, PROFILE.profile, PROFILE.profileVersion].join(
            "\u0000",
          ),
      ),
    ).toBe(true);

    await unmount(component);
    target.remove();
  });

  it("bootstraps visible native bounds before enabling or opening the first Deck session", async () => {
    const viewportBegin = deferred<{ epoch: number }>();
    const firstBounds = deferred<void>();
    let boundsCalls = 0;
    invokeMock.mockImplementation(
      (command: string, args?: Record<string, unknown>) => {
        switch (command) {
          case "extensions_snapshot":
            return Promise.resolve({
              packages: [],
              matrix: [
                {
                  deck: {
                    kind: "deck_pack",
                    packageId: d2DeckPack.deck_id,
                    packageVersion: d2DeckPack.deck_version,
                  },
                  codec: {
                    kind: "codec_pack",
                    packageId: CODEC_ID,
                    packageVersion: CODEC_VERSION,
                  },
                  reason: "compatible",
                  compatibleProfile: PROFILE,
                },
              ],
            });
          case "deck_generic_sessions_get":
            return Promise.resolve({
              sessions: [],
              foregroundOutput: null,
              outputPin: null,
              recentFaults: [],
            });
          case "deck_generic_runtime_options": {
            const request = args?.request as
              { profileKey: GenericProfileKey | null } | undefined;
            return Promise.resolve(
              runtimeOptions(request?.profileKey ?? null, true),
            );
          }
          case "deck_generic_viewport_session_begin":
            return viewportBegin.promise;
          case "deck_generic_viewport_set_bounds":
            boundsCalls += 1;
            return boundsCalls === 1 ? firstBounds.promise : Promise.resolve();
          case "deck_generic_open":
            return Promise.resolve({ sessionId: "session-bootstrap" });
          case "deck_generic_foreground_set":
            return Promise.resolve({
              sessions: [],
              foregroundOutput: null,
              outputPin: null,
              recentFaults: [],
            });
          default:
            return Promise.reject(new Error(`unexpected command: ${command}`));
        }
      },
    );

    const deck = model();
    const target = document.createElement("div");
    document.body.append(target);
    const component = mount(GenericDeckWorkspace, {
      target,
      props: {
        model: deck,
        models: [deck],
        library: library(),
        active: true,
        registerLeave: () => undefined,
      },
    });
    await settleUi();

    const loadButton = Array.from(target.querySelectorAll("button")).find(
      (button) => button.textContent?.trim() === "Load exact Deck draft",
    );
    expect(loadButton).toBeInstanceOf(HTMLButtonElement);
    expect(loadButton?.disabled).toBe(true);

    const selects = target.querySelectorAll<HTMLSelectElement>(
      ".runtime-config .config-grid select",
    );
    changeSelect(selects[0], `${CODEC_ID}@${CODEC_VERSION}`);
    await settleUi();
    changeSelect(selects[1], "cuda");
    await settleUi();
    changeSelect(
      selects[2],
      [PROFILE.codecFamily, PROFILE.profile, PROFILE.profileVersion].join(
        "\u0000",
      ),
    );
    await settleUi();
    target
      .querySelectorAll<HTMLSelectElement>(
        '[data-widget-kind="source_picker"] select',
      )
      .forEach((select) => changeSelect(select, SOURCE_HASH));
    await settleUi();

    expect(loadButton?.disabled).toBe(true);
    expect(
      invokeMock.mock.calls.filter(
        ([command]) => command === "deck_generic_open",
      ),
    ).toHaveLength(0);

    viewportBegin.resolve({ epoch: 7 });
    await settleUi();
    const firstBoundsCall = invokeMock.mock.calls.find(
      ([command]) => command === "deck_generic_viewport_set_bounds",
    );
    expect(firstBoundsCall?.[1]).toEqual({
      bounds: expect.objectContaining({ epoch: 7, visible: true }),
    });
    expect(loadButton?.disabled).toBe(true);

    firstBounds.resolve();
    await settleUi();
    expect(loadButton?.disabled).toBe(false);
    loadButton!.click();
    await settleUi();

    const commandOrder = invokeMock.mock.calls.map(([command]) => command);
    expect(
      commandOrder.indexOf("deck_generic_viewport_session_begin"),
    ).toBeLessThan(commandOrder.indexOf("deck_generic_viewport_set_bounds"));
    expect(
      commandOrder.indexOf("deck_generic_viewport_set_bounds"),
    ).toBeLessThan(commandOrder.indexOf("deck_generic_open"));

    await unmount(component);
    target.remove();
  });

  it("re-establishes after transient begin and bounds failures before enabling Load", async () => {
    vi.useFakeTimers({
      toFake: ["setTimeout", "clearTimeout", "setInterval", "clearInterval"],
    });
    let beginCalls = 0;
    let boundsCalls = 0;
    invokeMock.mockImplementation(
      (command: string, args?: Record<string, unknown>) => {
        switch (command) {
          case "extensions_snapshot":
            return Promise.resolve({
              packages: [],
              matrix: [
                {
                  deck: {
                    kind: "deck_pack",
                    packageId: d2DeckPack.deck_id,
                    packageVersion: d2DeckPack.deck_version,
                  },
                  codec: {
                    kind: "codec_pack",
                    packageId: CODEC_ID,
                    packageVersion: CODEC_VERSION,
                  },
                  reason: "compatible",
                  compatibleProfile: PROFILE,
                },
              ],
            });
          case "deck_generic_sessions_get":
            return Promise.resolve({
              sessions: [],
              foregroundOutput: null,
              outputPin: null,
              recentFaults: [],
            });
          case "deck_generic_runtime_options": {
            const request = args?.request as
              { profileKey: GenericProfileKey | null } | undefined;
            return Promise.resolve(
              runtimeOptions(request?.profileKey ?? null, true),
            );
          }
          case "deck_generic_viewport_session_begin":
            beginCalls += 1;
            return beginCalls === 1
              ? Promise.reject({
                  code: "output.viewport_begin_transient",
                  message: "Transient viewport begin failure.",
                })
              : Promise.resolve({ epoch: beginCalls + 10 });
          case "deck_generic_viewport_set_bounds":
            boundsCalls += 1;
            return boundsCalls === 1
              ? Promise.reject({
                  code: "output.viewport_bounds_transient",
                  message: "Transient viewport bounds failure.",
                })
              : Promise.resolve();
          case "deck_generic_open":
            return Promise.resolve({ sessionId: "session-recovered" });
          case "deck_generic_foreground_set":
            return Promise.resolve({
              sessions: [],
              foregroundOutput: null,
              outputPin: null,
              recentFaults: [],
            });
          default:
            return Promise.reject(new Error(`unexpected command: ${command}`));
        }
      },
    );

    const deck = model();
    const target = document.createElement("div");
    document.body.append(target);
    const component = mount(GenericDeckWorkspace, {
      target,
      props: {
        model: deck,
        models: [deck],
        library: library(),
        active: true,
        registerLeave: () => undefined,
      },
    });
    await settleUi();
    await configureReadyDraft(target);
    const loadButton = findLoadButton(target);

    expect(beginCalls).toBe(1);
    expect(boundsCalls).toBe(0);
    expect(loadButton.disabled).toBe(true);

    await vi.advanceTimersByTimeAsync(99);
    await settleUi();
    expect(beginCalls).toBe(1);
    await vi.advanceTimersByTimeAsync(1);
    await settleUi();
    expect(beginCalls).toBe(2);
    expect(boundsCalls).toBe(1);
    expect(loadButton.disabled).toBe(true);

    await vi.advanceTimersByTimeAsync(249);
    await settleUi();
    expect(beginCalls).toBe(2);
    await vi.advanceTimersByTimeAsync(1);
    await settleUi();
    expect(beginCalls).toBe(3);
    expect(boundsCalls).toBe(2);
    expect(loadButton.disabled).toBe(false);
    expect(target.textContent).not.toContain(
      "Transient viewport bounds failure.",
    );

    loadButton.click();
    await settleUi();
    const commands = invokeMock.mock.calls.map(([command]) => command);
    const openIndex = commands.indexOf("deck_generic_open");
    expect(openIndex).toBeGreaterThan(-1);
    expect(
      commands
        .slice(0, openIndex)
        .filter((command) => command === "deck_generic_viewport_set_bounds"),
    ).toHaveLength(2);

    await unmount(component);
    target.remove();
  });

  it("stops a failed retry burst and lets resize start a fresh bounded recovery", async () => {
    vi.useFakeTimers({
      toFake: ["setTimeout", "clearTimeout", "setInterval", "clearInterval"],
    });
    let beginCalls = 0;
    let boundsCalls = 0;
    invokeMock.mockImplementation(
      (command: string, args?: Record<string, unknown>) => {
        switch (command) {
          case "extensions_snapshot":
            return Promise.resolve({
              packages: [],
              matrix: [
                {
                  deck: {
                    kind: "deck_pack",
                    packageId: d2DeckPack.deck_id,
                    packageVersion: d2DeckPack.deck_version,
                  },
                  codec: {
                    kind: "codec_pack",
                    packageId: CODEC_ID,
                    packageVersion: CODEC_VERSION,
                  },
                  reason: "compatible",
                  compatibleProfile: PROFILE,
                },
              ],
            });
          case "deck_generic_sessions_get":
            return Promise.resolve({
              sessions: [],
              foregroundOutput: null,
              outputPin: null,
              recentFaults: [],
            });
          case "deck_generic_runtime_options": {
            const request = args?.request as
              { profileKey: GenericProfileKey | null } | undefined;
            return Promise.resolve(
              runtimeOptions(request?.profileKey ?? null, true),
            );
          }
          case "deck_generic_viewport_session_begin":
            beginCalls += 1;
            return beginCalls <= 4
              ? Promise.reject({
                  code: "output.viewport_begin_transient",
                  message: "Transient viewport begin failure.",
                })
              : Promise.resolve({ epoch: 40 + beginCalls });
          case "deck_generic_viewport_set_bounds":
            boundsCalls += 1;
            return Promise.resolve();
          default:
            return Promise.reject(new Error(`unexpected command: ${command}`));
        }
      },
    );

    const deck = model();
    const target = document.createElement("div");
    document.body.append(target);
    const component = mount(GenericDeckWorkspace, {
      target,
      props: {
        model: deck,
        models: [deck],
        library: library(),
        active: true,
        registerLeave: () => undefined,
      },
    });
    await settleUi();
    await configureReadyDraft(target);
    const loadButton = findLoadButton(target);

    for (const delay of [100, 250, 500]) {
      await vi.advanceTimersByTimeAsync(delay);
      await settleUi();
    }
    expect(beginCalls).toBe(4);
    expect(boundsCalls).toBe(0);
    expect(loadButton.disabled).toBe(true);

    await vi.advanceTimersByTimeAsync(10_000);
    await settleUi();
    expect(beginCalls).toBe(4);

    globalThis.dispatchEvent(new Event("resize"));
    await settleUi();
    expect(beginCalls).toBe(5);
    expect(boundsCalls).toBeGreaterThanOrEqual(1);
    expect(loadButton.disabled).toBe(false);

    await unmount(component);
    target.remove();
  });
});
