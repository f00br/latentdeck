import { flushSync, mount, tick, unmount } from "svelte";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type {
  ExtensionCompatibilityPair,
  ExtensionPackageReference,
  ExtensionPackageSummary,
  ExtensionsSnapshot,
  InspectedExtension,
} from "./extension-manager-model";
import { EMPTY_PLAYER_VIEW } from "./player-model";

const native = vi.hoisted(() => ({
  invoke: vi.fn(),
  open: vi.fn(),
}));

vi.mock("@tauri-apps/api/core", () => ({ invoke: native.invoke }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: native.open }));

import App from "./App.svelte";

const CODEC: ExtensionPackageReference = {
  kind: "codec_pack",
  packageId: "org.example.codec",
  packageVersion: "2.0.0",
};

const DECK_COMPATIBLE: ExtensionPackageReference = {
  kind: "deck_pack",
  packageId: "org.example.deck-compatible",
  packageVersion: "1.0.0",
};

const DECK_INCOMPATIBLE: ExtensionPackageReference = {
  kind: "deck_pack",
  packageId: "org.example.deck-incompatible",
  packageVersion: "1.0.0",
};

const SHA256 = "a".repeat(64);

const INSPECTION: InspectedExtension = {
  package: CODEC,
  displayName: "Example Codec",
  publisherName: "Example Publisher",
  publisherIdentityClaim: "self_declared",
  archiveSha256: SHA256,
  archiveByteLength: 4096,
  fileCount: 8,
  extractedByteLength: 8192,
};

function summary(
  packageReference: ExtensionPackageReference,
  overrides: Partial<ExtensionPackageSummary> = {},
): ExtensionPackageSummary {
  return {
    package: packageReference,
    displayName: packageReference.packageId,
    publisherName: "Example Publisher",
    enabled: false,
    health: "healthy",
    errorCode: null,
    errorDetail: null,
    ...overrides,
  };
}

const DECKS = [summary(DECK_COMPATIBLE), summary(DECK_INCOMPATIBLE)];

const MATRIX: ExtensionCompatibilityPair[] = [
  {
    deck: DECK_COMPATIBLE,
    codec: CODEC,
    reason: "compatible",
    compatibleProfile: {
      codecFamily: "example",
      profile: "latent",
      profileVersion: "1.0.0",
    },
  },
  {
    deck: DECK_INCOMPATIBLE,
    codec: CODEC,
    reason: "unsupported_signal",
    compatibleProfile: null,
  },
];

class PassiveObserver {
  observe(): void {}
  unobserve(): void {}
  disconnect(): void {}
  takeRecords(): never[] {
    return [];
  }
}

async function settle(): Promise<void> {
  for (let attempt = 0; attempt < 5; attempt += 1) {
    await Promise.resolve();
    await tick();
    flushSync();
  }
}

function button(root: ParentNode, label: string): HTMLButtonElement {
  const match = Array.from(root.querySelectorAll("button")).find(
    (candidate) => candidate.textContent?.trim() === label,
  );
  expect(match, `button ${JSON.stringify(label)}`).toBeDefined();
  return match!;
}

function extensionCard(root: ParentNode, packageId: string): HTMLElement {
  const match = Array.from(
    root.querySelectorAll<HTMLElement>(".installed-extension-list > article"),
  ).find((candidate) => candidate.textContent?.includes(packageId));
  expect(match, `extension card ${packageId}`).toBeDefined();
  return match!;
}

async function click(root: ParentNode, label: string): Promise<void> {
  button(root, label).click();
  await settle();
}

function enter(input: HTMLInputElement, value: string): void {
  input.value = value;
  input.dispatchEvent(new Event("input", { bubbles: true }));
  flushSync();
}

function text(root: ParentNode): string {
  return root.textContent?.replace(/\s+/g, " ").trim() ?? "";
}

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

describe("mounted LatentPlayer Extensions Manager", () => {
  beforeEach(() => {
    native.invoke.mockReset();
    native.open.mockReset();
    vi.stubGlobal("ResizeObserver", PassiveObserver);
    vi.stubGlobal("IntersectionObserver", PassiveObserver);
    vi.stubGlobal(
      "requestAnimationFrame",
      vi.fn(() => 1),
    );
    vi.stubGlobal("cancelAnimationFrame", vi.fn());
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("keeps cached package actions disabled while a snapshot owns the lifecycle lock", async () => {
    const codec = summary(CODEC, { enabled: true });
    const snapshot: ExtensionsSnapshot = { packages: [codec], matrix: [] };
    const pendingSnapshot = deferred<ExtensionsSnapshot>();
    const pendingRawImportOptions = deferred<{
      packageId: string;
      packageVersion: string;
      adapterId: string;
      adapterVersion: string;
      displayName: string;
      profiles: never[];
    }>();
    let snapshotCount = 0;
    let rawImportOptionsCount = 0;
    native.invoke.mockImplementation(async (command: string) => {
      switch (command) {
        case "player_viewport_session_begin":
          return { epoch: 1 };
        case "player_viewport_set_bounds":
          return undefined;
        case "player_snapshot":
          return EMPTY_PLAYER_VIEW;
        case "player_fullscreen_status":
        case "player_spout_status":
        case "player_conversion_snapshot":
          return null;
        case "player_raw_import_options":
          rawImportOptionsCount += 1;
          if (rawImportOptionsCount > 1) {
            return pendingRawImportOptions.promise;
          }
          return {
            packageId: CODEC.packageId,
            packageVersion: CODEC.packageVersion,
            adapterId: "org.example.adapter",
            adapterVersion: "2.0.0",
            displayName: "Example Codec",
            profiles: [],
          };
        case "extensions_snapshot":
          snapshotCount += 1;
          return snapshotCount === 1 ? snapshot : pendingSnapshot.promise;
        case "player_select_codec_exact":
          return undefined;
        default:
          throw new Error(`Unexpected native command ${command}`);
      }
    });

    const target = document.createElement("div");
    document.body.append(target);
    const component = mount(App, { target });
    await settle();

    let codecCard = extensionCard(target, CODEC.packageId);
    const device = codecCard.querySelector<HTMLSelectElement>("select")!;
    device.value = "cuda";
    device.dispatchEvent(new Event("change", { bubbles: true }));
    flushSync();

    button(target, "Refresh snapshot").click();
    await settle();
    expect(snapshotCount).toBe(2);
    codecCard = extensionCard(target, CODEC.packageId);
    expect(button(target, "Refreshing…").disabled).toBe(true);
    expect(button(codecCard, "Use in Player").disabled).toBe(true);
    button(codecCard, "Use in Player").click();
    await settle();
    expect(
      native.invoke.mock.calls.filter(
        ([name]) => name === "player_select_codec_exact",
      ),
    ).toHaveLength(0);

    pendingSnapshot.resolve(snapshot);
    await settle();
    codecCard = extensionCard(target, CODEC.packageId);
    expect(button(codecCard, "Use in Player").disabled).toBe(false);
    await click(codecCard, "Use in Player");
    expect(native.invoke).toHaveBeenCalledWith("player_select_codec_exact", {
      packageId: CODEC.packageId,
      packageVersion: CODEC.packageVersion,
      device: "cuda",
    });
    expect(rawImportOptionsCount).toBe(2);
    expect(button(target, "Open cartridge").disabled).toBe(false);

    pendingRawImportOptions.resolve({
      packageId: CODEC.packageId,
      packageVersion: CODEC.packageVersion,
      adapterId: "org.example.adapter",
      adapterVersion: "2.0.0",
      displayName: "Example Codec",
      profiles: [],
    });
    await settle();

    await unmount(component);
    target.remove();
  });

  it("drives the local exact-version lifecycle through the rendered surface", async () => {
    let verifyCount = 0;
    let snapshot: ExtensionsSnapshot = { packages: DECKS, matrix: [] };
    native.open
      .mockResolvedValueOnce("fixtures/install.ldcodec")
      .mockResolvedValueOnce("fixtures/repair.ldcodec");
    native.invoke.mockImplementation(
      async (command: string, arguments_?: Record<string, unknown>) => {
        switch (command) {
          case "player_viewport_session_begin":
            return { epoch: 1 };
          case "player_viewport_set_bounds":
            return undefined;
          case "player_snapshot":
            return EMPTY_PLAYER_VIEW;
          case "player_fullscreen_status":
          case "player_spout_status":
          case "player_conversion_snapshot":
            return null;
          case "player_raw_import_options":
            return {
              packageId: "org.example.codec",
              packageVersion: "2.0.0",
              adapterId: "org.example.adapter",
              adapterVersion: "2.0.0",
              displayName: "Example Codec",
              profiles: [],
            };
          case "extensions_snapshot":
            return snapshot;
          case "extensions_inspect":
            return INSPECTION;
          case "extensions_install":
            snapshot = {
              packages: [...DECKS, summary(CODEC)],
              matrix: MATRIX,
            };
            return snapshot;
          case "extensions_verify": {
            verifyCount += 1;
            const verified = summary(CODEC, {
              health: verifyCount === 1 ? "healthy" : "corrupt",
              errorCode:
                verifyCount === 1 ? null : "package.integrity_mismatch",
              errorDetail:
                verifyCount === 1 ? null : "Installed bytes changed.",
            });
            snapshot = {
              ...snapshot,
              packages: snapshot.packages.map((candidate) =>
                candidate.package.packageId === CODEC.packageId
                  ? verified
                  : candidate,
              ),
            };
            return verified;
          }
          case "extensions_enable":
          case "extensions_disable": {
            const enabled = command === "extensions_enable";
            snapshot = {
              ...snapshot,
              packages: snapshot.packages.map((candidate) =>
                candidate.package.packageId === CODEC.packageId
                  ? { ...candidate, enabled }
                  : candidate,
              ),
            };
            return snapshot;
          }
          case "extensions_repair":
            snapshot = {
              ...snapshot,
              packages: snapshot.packages.map((candidate) =>
                candidate.package.packageId === CODEC.packageId
                  ? summary(CODEC)
                  : candidate,
              ),
            };
            return snapshot;
          case "extensions_remove":
            snapshot = {
              packages: DECKS,
              matrix: [],
            };
            return snapshot;
          default:
            throw new Error(
              `Unexpected native command ${command} ${JSON.stringify(arguments_)}`,
            );
        }
      },
    );

    const target = document.createElement("div");
    document.body.append(target);
    const component = mount(App, { target });
    await settle();

    await click(target, "Extensions");
    expect(native.invoke).toHaveBeenCalledWith("extensions_snapshot");
    expect(text(target)).toContain(
      "Publisher metadata is self-declared; an exact SHA-256 confirms archive bytes, not publisher identity.",
    );

    await click(target, "Inspect local package");
    expect(native.open).toHaveBeenNthCalledWith(1, {
      multiple: false,
      directory: false,
      filters: [
        { name: "LatentDeck Extension", extensions: ["ld", "ldcodec"] },
      ],
    });
    expect(native.invoke).toHaveBeenCalledWith("extensions_inspect", {
      path: "fixtures/install.ldcodec",
    });
    expect(text(target)).toContain("Publisher identity is self-declared");
    expect(text(target)).toContain(
      "Example Publisher · self-declared metadata; SHA-256 confirms bytes, not publisher identity.",
    );
    expect(text(target)).toContain(`SHA-256 ${SHA256}`);

    const installCard = target.querySelector<HTMLElement>(
      '[aria-label="Install local extension"]',
    )!;
    const installSha = installCard.querySelector<HTMLInputElement>(
      'input[maxlength="64"]',
    )!;
    enter(installSha, "b".repeat(64));
    expect(button(installCard, "Install exact version").disabled).toBe(true);
    button(installCard, "Install exact version").click();
    await settle();
    expect(
      native.invoke.mock.calls.filter(
        ([name]) => name === "extensions_install",
      ),
    ).toHaveLength(0);

    enter(installSha, SHA256);
    expect(button(installCard, "Install exact version").disabled).toBe(false);
    await click(installCard, "Install exact version");
    expect(native.invoke).toHaveBeenCalledWith("extensions_install", {
      path: "fixtures/install.ldcodec",
      expectedSha256: SHA256,
    });
    expect(target.textContent).toContain("Compatible");
    expect(target.textContent).toContain("Unsupported signal geometry");

    let codecCard = extensionCard(target, CODEC.packageId);
    await click(codecCard, "Verify");
    expect(native.invoke).toHaveBeenCalledWith("extensions_verify", {
      package: CODEC,
    });
    expect(text(target)).toContain(
      "Verified org.example.codec 2.0.0: healthy.",
    );

    codecCard = extensionCard(target, CODEC.packageId);
    await click(codecCard, "Enable");
    expect(native.invoke).toHaveBeenCalledWith("extensions_enable", {
      package: CODEC,
    });
    codecCard = extensionCard(target, CODEC.packageId);
    expect(codecCard.textContent).toContain("enabled");
    await click(codecCard, "Disable");
    expect(native.invoke).toHaveBeenCalledWith("extensions_disable", {
      package: CODEC,
    });

    codecCard = extensionCard(target, CODEC.packageId);
    await click(codecCard, "Repair…");
    const repair = target.querySelector<HTMLElement>(
      '[aria-label="Repair exact extension version"]',
    )!;
    await click(repair, "Inspect repair archive");
    expect(native.open).toHaveBeenNthCalledWith(2, {
      multiple: false,
      directory: false,
      filters: [
        { name: "LatentDeck Extension", extensions: ["ld", "ldcodec"] },
      ],
    });
    expect(native.invoke).toHaveBeenCalledWith("extensions_inspect", {
      path: "fixtures/repair.ldcodec",
    });
    expect(repair.textContent).toContain("Exact package identity matches");

    const repairSha = repair.querySelector<HTMLInputElement>(
      'input[maxlength="64"]',
    )!;
    enter(repairSha, "b".repeat(64));
    expect(button(repair, "Repair from exact archive").disabled).toBe(true);
    enter(repairSha, SHA256);
    expect(button(repair, "Repair from exact archive").disabled).toBe(false);
    await click(repair, "Repair from exact archive");
    expect(native.invoke).toHaveBeenCalledWith("extensions_repair", {
      path: "fixtures/repair.ldcodec",
      expectedSha256: SHA256,
    });

    codecCard = extensionCard(target, CODEC.packageId);
    await click(codecCard, "Verify");
    codecCard = extensionCard(target, CODEC.packageId);
    expect(codecCard.textContent).toContain("package.integrity_mismatch");
    const remove = button(codecCard, "Remove exact version");
    expect(remove.disabled).toBe(true);
    const acknowledgement = codecCard.querySelector<HTMLInputElement>(
      '.corrupt-removal-confirmation input[type="checkbox"]',
    )!;
    acknowledgement.checked = true;
    acknowledgement.dispatchEvent(new Event("change", { bubbles: true }));
    flushSync();
    expect(remove.disabled).toBe(false);
    remove.click();
    await settle();
    expect(native.invoke).toHaveBeenCalledWith("extensions_remove", {
      package: CODEC,
      allowCorrupt: true,
    });
    expect(target.textContent).not.toContain("org.example.codec · 2.0.0");

    await unmount(component);
    target.remove();
  });
});
