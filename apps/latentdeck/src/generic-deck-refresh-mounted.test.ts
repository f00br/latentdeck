import { flushSync, mount, tick, unmount } from "svelte";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import d2DeckPack from "../../../operators/builtin/d2/package/deck-pack.json";
import d2Faceplate from "../../../operators/builtin/d2/package/faceplate.json";
import d2Operator from "../../../operators/builtin/d2/package/operator.json";
import { parseDeckUiCatalog } from "./deck-ui-model";
import type { ExtensionsSnapshot } from "./extension-manager-model";
import GenericDeckWorkspaceRefreshHarness from "./GenericDeckWorkspaceRefreshHarness.test.svelte";
import { EMPTY_LIBRARY_VIEW } from "./library-model";

const { invokeMock } = vi.hoisted(() => ({ invokeMock: vi.fn() }));

vi.mock("@tauri-apps/api/core", () => ({ invoke: invokeMock }));

function deferred<T>(): {
  promise: Promise<T>;
  resolve: (value: T) => void;
} {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((accept) => {
    resolve = accept;
  });
  return { promise, resolve };
}

async function settleUi(): Promise<void> {
  await new Promise<void>((resolve) => setTimeout(resolve, 0));
  await tick();
  flushSync();
}

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

function snapshot(codecId: string): ExtensionsSnapshot {
  const deck = model();
  return {
    packages: [],
    matrix: [
      {
        deck: deck.package,
        codec: {
          kind: "codec_pack",
          packageId: codecId,
          packageVersion: "2.0.0",
        },
        reason: "compatible",
        compatibleProfiles: [
          {
            codecFamily: "test",
            profile: "latent",
            profileVersion: "1.0.0",
          },
        ],
        compatibleProfile: {
          codecFamily: "test",
          profile: "latent",
          profileVersion: "1.0.0",
        },
      },
    ],
  };
}

describe("generic Deck Extensions refresh", () => {
  beforeEach(() => {
    invokeMock.mockReset();
    vi.stubGlobal(
      "ResizeObserver",
      class {
        observe(): void {}
        disconnect(): void {}
      },
    );
    vi.stubGlobal("requestAnimationFrame", () => 1);
    vi.stubGlobal("cancelAnimationFrame", () => undefined);
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("queues one newer snapshot and never applies the stale pending result", async () => {
    const first = deferred<ExtensionsSnapshot>();
    const second = deferred<ExtensionsSnapshot>();
    let refreshCount = 0;
    invokeMock.mockImplementation((command: string) => {
      if (command === "extensions_snapshot") {
        refreshCount += 1;
        return refreshCount === 1 ? first.promise : second.promise;
      }
      if (command === "deck_generic_sessions_get") {
        return Promise.resolve({
          sessions: [],
          foregroundOutput: null,
          outputPin: null,
          recentFaults: [],
        });
      }
      if (command === "deck_generic_viewport_session_begin") {
        return Promise.resolve({ epoch: 1 });
      }
      if (
        command === "deck_generic_viewport_set_bounds" ||
        command === "deck_generic_viewport_hide"
      ) {
        return Promise.resolve();
      }
      return Promise.reject(new Error(`unexpected command: ${command}`));
    });
    const deck = model();
    const target = document.createElement("div");
    document.body.append(target);
    const component = mount(GenericDeckWorkspaceRefreshHarness, {
      target,
      props: {
        model: deck,
        initialModels: [deck],
        library: EMPTY_LIBRARY_VIEW,
      },
    });
    await settleUi();
    expect(refreshCount).toBe(1);

    component.setActive(true);
    await settleUi();
    component.replaceModels([deck]);
    await settleUi();
    expect(refreshCount).toBe(1);

    first.resolve(snapshot("org.example.codec.stale"));
    await settleUi();
    expect(refreshCount).toBe(2);
    expect(target.textContent).not.toContain("org.example.codec.stale");

    second.resolve(snapshot("org.example.codec.latest"));
    await settleUi();
    expect(target.textContent).toContain("org.example.codec.latest");
    expect(target.textContent).not.toContain("org.example.codec.stale");

    await unmount(component);
    target.remove();
  });

  it("defers hidden model refreshes and requests one snapshot when active again", async () => {
    let refreshCount = 0;
    invokeMock.mockImplementation((command: string) => {
      if (command === "extensions_snapshot") {
        refreshCount += 1;
        return Promise.resolve(snapshot(`org.example.codec.${refreshCount}`));
      }
      if (command === "deck_generic_sessions_get") {
        return Promise.resolve({
          sessions: [],
          foregroundOutput: null,
          outputPin: null,
          recentFaults: [],
        });
      }
      if (command === "deck_generic_viewport_session_begin") {
        return Promise.resolve({ epoch: 1 });
      }
      if (
        command === "deck_generic_viewport_set_bounds" ||
        command === "deck_generic_viewport_hide"
      ) {
        return Promise.resolve();
      }
      return Promise.reject(new Error(`unexpected command: ${command}`));
    });
    const deck = model();
    const target = document.createElement("div");
    document.body.append(target);
    const component = mount(GenericDeckWorkspaceRefreshHarness, {
      target,
      props: {
        model: deck,
        initialModels: [deck],
        library: EMPTY_LIBRARY_VIEW,
      },
    });
    await settleUi();
    expect(refreshCount).toBe(1);

    component.replaceModels([deck]);
    component.replaceModels([deck]);
    await settleUi();
    expect(refreshCount).toBe(1);

    component.setActive(true);
    await settleUi();
    expect(refreshCount).toBe(2);
    await settleUi();
    expect(refreshCount).toBe(2);

    await unmount(component);
    target.remove();
  });
});
