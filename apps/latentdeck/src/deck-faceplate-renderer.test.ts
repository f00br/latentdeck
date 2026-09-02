import { flushSync, mount, unmount } from "svelte";
import { describe, expect, it, vi } from "vitest";

import d2DeckPack from "../../../operators/builtin/d2/package/deck-pack.json";
import d2Faceplate from "../../../operators/builtin/d2/package/faceplate.json";
import d2Operator from "../../../operators/builtin/d2/package/operator.json";
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

function bundledD2Deck(): DeckUiCatalogEntryInput {
  return {
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
      controls: d2Operator.controls.map((control) => ({ ...control })),
    },
    faceplate: d2Faceplate,
  };
}

function clickButton(target: HTMLElement, label: string): void {
  const result = Array.from(target.querySelectorAll("button")).find(
    (candidate) => candidate.textContent?.trim() === label,
  );
  if (!(result instanceof HTMLButtonElement)) {
    throw new Error(`Missing button: ${label}`);
  }
  result.click();
  flushSync();
}

describe("host-rendered declarative Deck faceplate", () => {
  it("mounts the actual bundled D2 faceplate and dispatches its host actions", async () => {
    const model = parseDeckUiCatalog({
      decks: [bundledD2Deck()],
      issues: [],
    }).decks[0];
    const sourceHashes = ["a".repeat(64), "b".repeat(64)];
    const onDraftChange = vi.fn();
    const onLoad = vi.fn();
    const onControlsCommit = vi.fn();
    const onRolesCommit = vi.fn();
    const onTransportCommit = vi.fn();
    const onSeedCommit = vi.fn();
    const onRestart = vi.fn();
    const onCapture = vi.fn();
    const onFullscreenToggle = vi.fn();
    const onProcessOnce = vi.fn();
    const onMonitorAnchor = vi.fn();
    const target = document.createElement("div");
    document.body.append(target);
    const component = mount(DeckFaceplateRenderer, {
      target,
      props: {
        model,
        initialDraft: createDeckUiDraft(model),
        sourceOptions: sourceHashes.map((archiveSha256, index) => ({
          archiveSha256,
          label: `D2 cartridge ${index + 1}`,
          available: true,
        })),
        active: true,
        runtimeAvailable: true,
        runtimeLoaded: true,
        captureAvailable: true,
        outputFullscreen: false,
        onDraftChange,
        onLoad,
        onControlsCommit,
        onRolesCommit,
        onTransportCommit,
        onSeedCommit,
        onRestart,
        onCapture,
        onFullscreenToggle,
        onProcessOnce,
        onMonitorAnchor,
      },
    });
    flushSync();

    expect(
      target.querySelector(
        '[data-deck-exact-key="org.latentdeck.deck.d2@0.2.0"]',
      ),
    ).not.toBeNull();
    expect(target.textContent).toContain("LatentDeck D2");
    expect(
      target.querySelectorAll('[data-widget-kind="source_picker"]'),
    ).toHaveLength(2);
    expect(
      target.querySelectorAll('[data-widget-kind="barycentric3"]'),
    ).toHaveLength(0);
    expect(onMonitorAnchor).toHaveBeenCalledWith(expect.any(HTMLDivElement));

    const sourceSelects = target.querySelectorAll<HTMLSelectElement>(
      '[data-widget-kind="source_picker"] select',
    );
    sourceSelects.forEach((select, index) => {
      select.value = sourceHashes[index];
      select.dispatchEvent(new Event("change", { bubbles: true }));
      flushSync();
    });
    expect(onDraftChange.mock.calls.at(-1)?.[0].sourceArchiveSha256s).toEqual(
      sourceHashes,
    );
    clickButton(target, "Load exact Deck draft");
    expect(onLoad).toHaveBeenCalledWith(
      expect.objectContaining({ sourceArchiveSha256s: sourceHashes }),
    );

    const carrier = target.querySelector<HTMLSelectElement>(
      '[data-widget-kind="role_editor"] select',
    );
    expect(carrier).not.toBeNull();
    carrier!.value = "1";
    carrier!.dispatchEvent(new Event("change", { bubbles: true }));
    flushSync();
    clickButton(target, "Apply role permutation");
    expect(onRolesCommit).toHaveBeenCalledWith({ carrier: 1, donor: 0 });

    const algorithm = target.querySelector<HTMLSelectElement>(
      'select[data-control-id="algorithm"]',
    );
    const mix = target.querySelector<HTMLInputElement>(
      'input[data-control-id="mix"]',
    );
    expect(algorithm).not.toBeNull();
    expect(mix).not.toBeNull();
    algorithm!.value = "xs5";
    algorithm!.dispatchEvent(new Event("change", { bubbles: true }));
    mix!.value = "0.75";
    mix!.dispatchEvent(new Event("input", { bubbles: true }));
    flushSync();
    clickButton(target, "Apply controls");
    expect(onControlsCommit).toHaveBeenCalledWith(
      expect.objectContaining({ algorithm: "xs5", mix: 0.75 }),
    );

    const seed = target.querySelector<HTMLInputElement>(
      '[data-widget-kind="seed"] input',
    );
    expect(seed).not.toBeNull();
    seed!.value = "4242";
    seed!.dispatchEvent(new Event("input", { bubbles: true }));
    flushSync();
    clickButton(target, "Set seed");
    expect(onSeedCommit).toHaveBeenCalledWith(4242);

    const transportButtons = target.querySelectorAll<HTMLButtonElement>(
      ".transport-grid button",
    );
    expect(transportButtons).toHaveLength(2);
    expect(transportButtons[0].textContent?.trim()).toBe("Pause");
    transportButtons[0].click();
    flushSync();
    expect(onTransportCommit).toHaveBeenLastCalledWith(
      [false, true],
      [true, true],
    );
    const loopInputs = target.querySelectorAll<HTMLInputElement>(
      '.transport-grid input[type="checkbox"]',
    );
    loopInputs[0].click();
    flushSync();
    expect(onTransportCommit).toHaveBeenLastCalledWith(
      [false, true],
      [false, true],
    );

    clickButton(target, "Restart all");
    clickButton(target, "Snapshot");
    clickButton(target, "Start Live Capture");
    clickButton(target, "Fullscreen output");
    clickButton(target, "Process once");
    expect(onRestart).toHaveBeenCalledOnce();
    expect(onCapture.mock.calls).toEqual([["snapshot"], ["live_capture"]]);
    expect(onFullscreenToggle).toHaveBeenCalledOnce();
    expect(onProcessOnce).toHaveBeenCalledOnce();

    await unmount(component);
    expect(onMonitorAnchor).toHaveBeenLastCalledWith(null);
    target.remove();
  });

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
