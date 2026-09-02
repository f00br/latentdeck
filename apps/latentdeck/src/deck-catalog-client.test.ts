import { describe, expect, it } from "vitest";

import d2DeckPack from "../../../operators/builtin/d2/package/deck-pack.json";
import d2Faceplate from "../../../operators/builtin/d2/package/faceplate.json";
import d2Operator from "../../../operators/builtin/d2/package/operator.json";
import { loadDeckUiCatalog } from "./deck-catalog-client";

describe("Deck UI catalog client", () => {
  it("requests one bounded snapshot and preserves every exact version", async () => {
    const commands: string[] = [];
    const catalog = await loadDeckUiCatalog(async (command) => {
      commands.push(command);
      return {
        decks: ["0.2.0", "0.3.0"].map((version) => ({
          package: {
            kind: "deck_pack",
            packageId: d2DeckPack.deck_id,
            packageVersion: version,
          },
          deck: {
            deckId: d2DeckPack.deck_id,
            deckVersion: version,
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
        })),
        issues: [],
      };
    });

    expect(commands).toEqual(["extensions_deck_catalog"]);
    expect(catalog.decks.map((deck) => deck.exactKey)).toEqual([
      "org.latentdeck.deck.d2@0.2.0",
      "org.latentdeck.deck.d2@0.3.0",
    ]);
  });

  it("reloads each exact host snapshot without caching a stale version", async () => {
    let version = "0.2.0";
    const host = async () => ({
      decks: [
        {
          package: {
            kind: "deck_pack",
            packageId: d2DeckPack.deck_id,
            packageVersion: version,
          },
          deck: {
            deckId: d2DeckPack.deck_id,
            deckVersion: version,
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
    });

    const first = await loadDeckUiCatalog(host);
    version = "0.3.0";
    const refreshed = await loadDeckUiCatalog(host);

    expect(first.decks[0].exactKey).toBe("org.latentdeck.deck.d2@0.2.0");
    expect(refreshed.decks[0].exactKey).toBe("org.latentdeck.deck.d2@0.3.0");
  });
});
