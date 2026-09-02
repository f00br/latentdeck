import { invoke } from "@tauri-apps/api/core";

import { parseDeckUiCatalog, type DeckUiCatalog } from "./deck-ui-model";

export type DeckCatalogInvoke = (
  command: string,
  args?: Record<string, unknown>,
) => Promise<unknown>;

const tauriInvoke: DeckCatalogInvoke = (command, args = {}) =>
  invoke<unknown>(command, args);

export async function loadDeckUiCatalog(
  hostInvoke: DeckCatalogInvoke = tauriInvoke,
): Promise<DeckUiCatalog> {
  return parseDeckUiCatalog(await hostInvoke("extensions_deck_catalog", {}));
}
