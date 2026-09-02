import { describe, expect, it } from "vitest";

import d2DeckPack from "../../../operators/builtin/d2/package/deck-pack.json";
import d2Faceplate from "../../../operators/builtin/d2/package/faceplate.json";
import d2Operator from "../../../operators/builtin/d2/package/operator.json";
import q4DeckPack from "../../../operators/builtin/q4/package/deck-pack.json";
import q4Faceplate from "../../../operators/builtin/q4/package/faceplate.json";
import q4Operator from "../../../operators/builtin/q4/package/operator.json";
import {
  DeckUiContractError,
  createDeckUiDraft,
  parseDeckUiCatalog,
  serializeDeckControls,
  type DeckUiCatalogEntryInput,
} from "./deck-ui-model";

function bundledEntry(
  deckPack: typeof d2DeckPack | typeof q4DeckPack,
  operator: typeof d2Operator | typeof q4Operator,
  faceplate: object,
): DeckUiCatalogEntryInput {
  return {
    package: {
      kind: "deck_pack",
      packageId: deckPack.deck_id,
      packageVersion: deckPack.deck_version,
    },
    deck: {
      deckId: deckPack.deck_id,
      deckVersion: deckPack.deck_version,
      displayName: deckPack.display_name,
      summary: deckPack.summary,
      slots: deckPack.signal.slots,
      roles: deckPack.signal.roles.map((role) => ({
        roleId: role.role_id,
        displayName: role.display_name,
      })),
      defaultPermutation: deckPack.signal.default_permutation,
      structuralCarrierRole: deckPack.signal.structural_carrier_role,
      requiredCapabilities: deckPack.signal.required_capabilities,
    },
    operator: {
      operatorId: operator.operator_id,
      controls: operator.controls.map((control) => ({ ...control })),
    },
    faceplate: structuredClone(faceplate),
  };
}

describe("runtime Deck UI catalog", () => {
  it("loads the exact bundled D2 and Q4 packages through one validated model", () => {
    const catalog = parseDeckUiCatalog({
      decks: [
        bundledEntry(d2DeckPack, d2Operator, d2Faceplate),
        bundledEntry(q4DeckPack, q4Operator, q4Faceplate),
      ],
      issues: [],
    });

    expect(catalog.decks.map((deck) => deck.exactKey)).toEqual([
      "org.latentdeck.deck.d2@0.2.0",
      "org.latentdeck.deck.q4@0.2.0",
    ]);
    expect(catalog.decks[0].faceplate.title).toBe("LatentDeck D2");
    expect(
      catalog.decks[1].faceplate.sections
        .flatMap((section) => section.widgets)
        .some((widget) => widget.kind === "barycentric3"),
    ).toBe(true);
  });

  it("serializes exact typed controls without casts, clamps, or extra fields", () => {
    const catalog = parseDeckUiCatalog({
      decks: [bundledEntry(q4DeckPack, q4Operator, q4Faceplate)],
      issues: [],
    });
    const model = catalog.decks[0];
    const draft = createDeckUiDraft(model);
    draft.controls.algorithm = "xs5";
    draft.controls.top_k = 11;
    draft.controls.temperature = 0.37;

    expect(serializeDeckControls(model, draft.controls)).toEqual({
      algorithm: "xs5",
      interaction: 0,
      mode: "hybridize",
      preserve: 0.55,
      influence_mode: "manual",
      donor_weight_b: 1,
      donor_weight_c: 1,
      donor_weight_d: 1,
      triangle_x: 0.5,
      triangle_y: 1 / 3,
      xs5_routing: "topk",
      temperature: 0.37,
      top_k: 11,
      sinkhorn_iterations: 5,
      chaos: 0,
    });

    draft.controls.top_k = 11.5;
    expect(() => serializeDeckControls(model, draft.controls)).toThrowError(
      expect.objectContaining<Partial<DeckUiContractError>>({
        code: "deck_ui.control_invalid",
      }),
    );
  });

  it("preserves canonical SemVer prerelease and build identities", () => {
    const entry = bundledEntry(d2DeckPack, d2Operator, d2Faceplate);
    const exactVersion = "0.2.0-rc.1+windows.cuda-130";
    (entry.package as Record<string, unknown>).packageVersion = exactVersion;
    (entry.deck as Record<string, unknown>).deckVersion = exactVersion;

    const catalog = parseDeckUiCatalog({ decks: [entry], issues: [] });
    expect(catalog.decks[0].exactKey).toBe(
      `org.latentdeck.deck.d2@${exactVersion}`,
    );

    const overflow = bundledEntry(d2DeckPack, d2Operator, d2Faceplate);
    const overflowVersion = "18446744073709551616.0.0";
    (overflow.package as Record<string, unknown>).packageVersion =
      overflowVersion;
    (overflow.deck as Record<string, unknown>).deckVersion = overflowVersion;
    expect(() =>
      parseDeckUiCatalog({ decks: [overflow], issues: [] }),
    ).toThrowError(
      expect.objectContaining<Partial<DeckUiContractError>>({
        code: "deck_ui.package_invalid",
      }),
    );
  });

  it("accepts the full bounded reverse-DNS identity allowed by the host", () => {
    const entry = bundledEntry(d2DeckPack, d2Operator, d2Faceplate);
    const deckId = `org.${"a".repeat(63)}.${"b".repeat(63)}.deck`;
    expect(deckId.length).toBeGreaterThan(128);
    (entry.package as Record<string, unknown>).packageId = deckId;
    (entry.deck as Record<string, unknown>).deckId = deckId;

    const catalog = parseDeckUiCatalog({ decks: [entry], issues: [] });
    expect(catalog.decks[0].deckId).toBe(deckId);
  });

  it("rejects integer contracts that cannot round-trip through JSON safely", () => {
    const entry = bundledEntry(q4DeckPack, q4Operator, q4Faceplate);
    const controls = (
      entry.operator as { controls: Array<Record<string, unknown>> }
    ).controls;
    const topK = controls.find((control) => control.control_id === "top_k");
    expect(topK).toBeDefined();
    topK!.default = Number.MAX_SAFE_INTEGER + 1;
    topK!.maximum = Number.MAX_SAFE_INTEGER + 2;
    const sections = (
      entry.faceplate as {
        sections: Array<{ widgets: Array<Record<string, unknown>> }>;
      }
    ).sections;
    const topKWidget = sections
      .flatMap((section) => section.widgets)
      .find((widget) => widget.control_id === "top_k");
    expect(topKWidget).toBeDefined();
    topKWidget!.maximum = Number.MAX_SAFE_INTEGER + 2;

    expect(() =>
      parseDeckUiCatalog({ decks: [entry], issues: [] }),
    ).toThrowError(
      expect.objectContaining<Partial<DeckUiContractError>>({
        code: "deck_ui.control_invalid",
      }),
    );
  });

  it("rejects a barycentric control point outside the declared triangle", () => {
    const model = parseDeckUiCatalog({
      decks: [bundledEntry(q4DeckPack, q4Operator, q4Faceplate)],
      issues: [],
    }).decks[0];
    const draft = createDeckUiDraft(model);
    draft.controls.triangle_x = 0.1;
    draft.controls.triangle_y = 0.9;

    expect(() => serializeDeckControls(model, draft.controls)).toThrowError(
      expect.objectContaining<Partial<DeckUiContractError>>({
        code: "deck_ui.control_invalid",
      }),
    );
  });
});
