import { flushSync, mount, unmount } from "svelte";
import { describe, expect, it } from "vitest";

import q4DeckPack from "../../../operators/builtin/q4/package/deck-pack.json";
import q4Faceplate from "../../../operators/builtin/q4/package/faceplate.json";
import q4Operator from "../../../operators/builtin/q4/package/operator.json";
import DeckFaceplateRenderer from "./DeckFaceplateRenderer.svelte";
import {
  createDeckUiDraft,
  parseDeckUiCatalog,
  type DeckUiCatalogEntryInput,
  type DeckUiDraft,
} from "./deck-ui-model";

function externalDeck(): DeckUiCatalogEntryInput {
  const deckId = "org.example.deck.dynamic";
  return {
    package: {
      kind: "deck_pack",
      packageId: deckId,
      packageVersion: "1.0.0",
    },
    deck: {
      deckId,
      deckVersion: "1.0.0",
      displayName: "Dynamic External Deck",
      summary: "A test-only package installed after the frontend was built.",
      slots: q4DeckPack.signal.slots,
      roles: q4DeckPack.signal.roles.map((role) => ({
        roleId: role.role_id,
        displayName: role.display_name,
      })),
      defaultPermutation: q4DeckPack.signal.default_permutation,
      structuralCarrierRole: q4DeckPack.signal.structural_carrier_role,
      requiredCapabilities: q4DeckPack.signal.required_capabilities,
    },
    operator: {
      operatorId: "org.example.operator.dynamic",
      controls: q4Operator.controls.map((control) => ({ ...control })),
    },
    faceplate: q4Faceplate,
  };
}

describe("host-rendered declarative Deck faceplate", () => {
  it("renders and edits a test-only external Deck without compiled package-specific UI", async () => {
    const model = parseDeckUiCatalog({
      decks: [externalDeck()],
      issues: [],
    }).decks[0];
    const changes: DeckUiDraft[] = [];
    const target = document.createElement("div");
    document.body.append(target);
    const component = mount(DeckFaceplateRenderer, {
      target,
      props: {
        model,
        initialDraft: createDeckUiDraft(model),
        sourceOptions: [0, 1, 2, 3].map((slot) => ({
          archiveSha256: `${slot}`.repeat(64),
          label: `Cartridge ${slot + 1}`,
          available: true,
        })),
        active: true,
        runtimeAvailable: false,
        runtimeUnavailableReason:
          "No generic runtime controller is available for this exact version.",
        onDraftChange: (draft: DeckUiDraft) => changes.push(draft),
      },
    });
    flushSync();

    expect(
      target.querySelector(
        '[data-deck-exact-key="org.example.deck.dynamic@1.0.0"]',
      ),
    ).not.toBeNull();
    expect(
      target.querySelectorAll('[data-widget-kind="source_picker"]'),
    ).toHaveLength(4);
    expect(
      target.querySelectorAll('[data-widget-kind="barycentric3"]'),
    ).toHaveLength(1);
    expect(
      target.querySelectorAll('[data-widget-kind="role_editor"] select'),
    ).toHaveLength(4);
    expect(target.textContent).toContain("Dynamic External Deck");
    expect(target.textContent).toContain(
      "No generic runtime controller is available for this exact version.",
    );

    const triangleX = target.querySelector<HTMLInputElement>(
      '[data-control-id="triangle_x"]',
    );
    const triangleY = target.querySelector<HTMLInputElement>(
      '[data-control-id="triangle_y"]',
    );
    expect(triangleX).not.toBeNull();
    expect(triangleY).not.toBeNull();
    expect(Number(triangleX!.min)).toBeCloseTo(1 / 6);
    expect(Number(triangleX!.max)).toBeCloseTo(5 / 6);
    triangleY!.value = "0.8";
    triangleY!.dispatchEvent(new Event("input", { bubbles: true }));
    flushSync();
    expect(Number(triangleX!.min)).toBeCloseTo(0.4);
    expect(Number(triangleX!.max)).toBeCloseTo(0.6);

    const topK = target.querySelector<HTMLInputElement>(
      '[data-control-id="top_k"]',
    );
    expect(topK).not.toBeNull();
    topK!.value = "12";
    topK!.dispatchEvent(new Event("input", { bubbles: true }));
    flushSync();
    expect(changes.at(-1)?.controls.top_k).toBe(12);
    expect(typeof changes.at(-1)?.controls.top_k).toBe("number");

    await unmount(component);
    target.remove();
  });

  it("keeps an authoritative warm session operable when new-session negotiation is unavailable", async () => {
    const model = parseDeckUiCatalog({
      decks: [externalDeck()],
      issues: [],
    }).decks[0];
    const draft = createDeckUiDraft(model);
    draft.sourceArchiveSha256s = [0, 1, 2, 3].map((slot) =>
      `${slot}`.repeat(64),
    );
    const target = document.createElement("div");
    document.body.append(target);
    const component = mount(DeckFaceplateRenderer, {
      target,
      props: {
        model,
        initialDraft: draft,
        sourceOptions: draft.sourceArchiveSha256s.map(
          (archiveSha256, slot) => ({
            archiveSha256,
            label: `Cartridge ${slot + 1}`,
            available: true,
            incompatibilityReason: "Different new-session profile",
          }),
        ),
        active: true,
        runtimeAvailable: false,
        runtimeUnavailableReason:
          "New-session negotiation has not selected a Codec profile.",
        runtimeLoaded: true,
        captureAvailable: true,
        outputFullscreen: false,
      },
    });
    flushSync();

    for (const label of [
      "Apply role permutation",
      "Restart all",
      "Set seed",
      "Snapshot",
      "Start Live Capture",
      "Fullscreen output",
      "Apply controls",
      "Process once",
    ]) {
      const button = Array.from(target.querySelectorAll("button")).find(
        (candidate) => candidate.textContent?.trim() === label,
      );
      expect(button, label).toBeDefined();
      expect(button!.disabled, label).toBe(false);
    }
    const load = Array.from(target.querySelectorAll("button")).find(
      (candidate) => candidate.textContent?.trim() === "Load exact Deck draft",
    );
    expect(load?.disabled).toBe(true);
    expect(target.textContent).not.toContain(
      "New-session negotiation has not selected a Codec profile.",
    );

    await unmount(component);
    target.remove();
  });
});
