import { flushSync, mount, tick, unmount } from "svelte";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { EMPTY_LIBRARY_VIEW } from "./library-model";

const { invokeMock, openMock, loadDeckUiCatalogMock } = vi.hoisted(() => ({
  invokeMock: vi.fn(),
  openMock: vi.fn(),
  loadDeckUiCatalogMock: vi.fn(),
}));

vi.mock("@tauri-apps/api/core", () => ({ invoke: invokeMock }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: openMock }));
vi.mock("./deck-catalog-client", () => ({
  loadDeckUiCatalog: loadDeckUiCatalogMock,
}));

import App from "./App.svelte";

async function settleUi(): Promise<void> {
  await new Promise<void>((resolve) => setTimeout(resolve, 0));
  await tick();
  flushSync();
}

describe("LatentDeck startup discovery boundary", () => {
  beforeEach(() => {
    invokeMock.mockReset();
    openMock.mockReset();
    loadDeckUiCatalogMock.mockReset();
    invokeMock.mockImplementation((command: string) => {
      if (command === "library_snapshot") return Promise.resolve(EMPTY_LIBRARY_VIEW);
      if (command === "extensions_snapshot") {
        return Promise.resolve({ packages: [], matrix: [] });
      }
      return Promise.reject(new Error(`unexpected startup command: ${command}`));
    });
    loadDeckUiCatalogMock.mockResolvedValue({ decks: [], issues: [] });
  });

  it("loads Deck navigation without global Codec or cartridge discovery", async () => {
    const target = document.createElement("div");
    document.body.append(target);

    const component = mount(App, { target });
    await settleUi();

    expect(invokeMock.mock.calls.map(([command]) => command)).toEqual([
      "library_snapshot",
    ]);
    expect(loadDeckUiCatalogMock).toHaveBeenCalledTimes(1);

    await unmount(component);
    target.remove();
  });

  it("coalesces an immediate Extensions click with pending Deck catalog discovery", async () => {
    let resolveCatalog: ((value: { decks: []; issues: [] }) => void) | undefined;
    loadDeckUiCatalogMock.mockImplementation(
      () =>
        new Promise<{ decks: []; issues: [] }>((resolve) => {
          resolveCatalog = resolve;
        }),
    );
    const target = document.createElement("div");
    document.body.append(target);
    const component = mount(App, { target });
    await settleUi();

    const extensions = Array.from(target.querySelectorAll("button")).find(
      (candidate) => candidate.textContent?.includes("Extensions"),
    );
    if (!(extensions instanceof HTMLButtonElement)) {
      throw new Error("Missing Extensions surface button");
    }
    extensions.click();
    await settleUi();

    expect(loadDeckUiCatalogMock).toHaveBeenCalledTimes(1);
    resolveCatalog?.({ decks: [], issues: [] });
    await settleUi();
    await unmount(component);
    target.remove();
  });

  it("reruns Deck catalog discovery when a package mutation lands during a pending request", async () => {
    const catalogResolvers: Array<
      (value: {
        decks: Array<{
          exactKey: string;
          displayName: string;
          deckId: string;
          deckVersion: string;
        }>;
        issues: [];
      }) => void
    > = [];
    loadDeckUiCatalogMock.mockImplementation(
      () =>
        new Promise((resolve) => {
          catalogResolvers.push(resolve);
        }),
    );
    invokeMock.mockImplementation((command: string) => {
      if (command === "library_snapshot") return Promise.resolve(EMPTY_LIBRARY_VIEW);
      if (command === "extensions_snapshot") {
        return Promise.resolve({
          packages: [
            {
              package: {
                kind: "deck_pack",
                packageId: "org.example.old",
                packageVersion: "1.0.0",
              },
              displayName: "Old Deck",
              publisherName: "Example",
              enabled: true,
              health: "healthy",
              errorCode: null,
              errorDetail: null,
            },
          ],
          matrix: [],
        });
      }
      if (command === "extensions_disable") {
        return Promise.resolve({ packages: [], matrix: [] });
      }
      return Promise.reject(new Error(`unexpected command: ${command}`));
    });

    const target = document.createElement("div");
    document.body.append(target);
    const component = mount(App, { target });
    await settleUi();

    const extensions = Array.from(target.querySelectorAll("button")).find(
      (candidate) => candidate.textContent?.includes("Extensions"),
    );
    if (!(extensions instanceof HTMLButtonElement)) {
      throw new Error("Missing Extensions surface button");
    }
    extensions.click();
    await settleUi();

    const disable = Array.from(target.querySelectorAll("button")).find(
      (candidate) => candidate.textContent?.trim() === "Disable",
    );
    if (!(disable instanceof HTMLButtonElement) || disable.disabled) {
      throw new Error("Missing enabled Disable package action");
    }
    disable.click();
    await settleUi();
    expect(loadDeckUiCatalogMock).toHaveBeenCalledTimes(1);

    catalogResolvers[0]?.({
      decks: [
        {
          exactKey: "old",
          displayName: "Stale Deck",
          deckId: "org.example.old",
          deckVersion: "1.0.0",
        },
      ],
      issues: [],
    });
    await settleUi();
    expect(loadDeckUiCatalogMock).toHaveBeenCalledTimes(2);

    catalogResolvers[1]?.({
      decks: [
        {
          exactKey: "new",
          displayName: "Current Deck",
          deckId: "org.example.current",
          deckVersion: "2.0.0",
        },
      ],
      issues: [],
    });
    await settleUi();
    expect(target.textContent).toContain("Current Deck");
    expect(target.textContent).not.toContain("Stale Deck");

    await unmount(component);
    target.remove();
  });
});
