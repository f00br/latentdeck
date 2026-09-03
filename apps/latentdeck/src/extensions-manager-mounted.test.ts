import { flushSync, mount, tick, unmount } from "svelte";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type {
  ExtensionCompatibilityReason,
  ExtensionPackageReference,
  ExtensionPackageSummary,
  ExtensionsSnapshot,
  InspectedExtension,
} from "./extension-manager-model";

const { invokeMock, openMock } = vi.hoisted(() => ({
  invokeMock: vi.fn(),
  openMock: vi.fn(),
}));

vi.mock("@tauri-apps/api/core", () => ({ invoke: invokeMock }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: openMock }));

import ExtensionsManager from "./ExtensionsManager.svelte";

const packageReference: ExtensionPackageReference = {
  kind: "deck_pack",
  packageId: "org.example.deck",
  packageVersion: "1.2.3",
};
const archiveSha256 = "a".repeat(64);
const installPath = "fixtures/example.ld";
const mismatchedRepairPath = "fixtures/other.ld";
const matchingRepairPath = "fixtures/example-repair.ld";

const healthySummary: ExtensionPackageSummary = {
  package: packageReference,
  displayName: "Example Deck",
  publisherName: "Example Publisher",
  enabled: false,
  health: "healthy",
  errorCode: null,
  errorDetail: null,
};

const installInspection: InspectedExtension = {
  package: packageReference,
  displayName: "Example Deck",
  publisherName: "Example Publisher",
  publisherIdentityClaim: "self_declared",
  archiveSha256,
  archiveByteLength: 8192,
  fileCount: 5,
  extractedByteLength: 12_288,
};

const compatibilityReasons: readonly ExtensionCompatibilityReason[] = [
  "compatible",
  "untrusted",
  "missing_asset",
  "package_invalid",
  "unsupported_protocol",
  "unsupported_host_api",
  "unsupported_tensor_abi",
  "unsupported_profile",
  "unsupported_signal",
  "unsupported_timing",
  "unsupported_capability",
];

const compatibilityLabels = [
  "Compatible",
  "Package is not trusted",
  "Required external asset missing",
  "Package is invalid",
  "Unsupported worker protocol",
  "Unsupported host API",
  "Unsupported tensor ABI",
  "Unsupported codec profile",
  "Unsupported signal geometry",
  "Unsupported timing",
  "Required capability unavailable",
] as const;

function initialSnapshot(): ExtensionsSnapshot {
  return {
    packages: [],
    matrix: compatibilityReasons.map((reason, index) => ({
      deck: {
        kind: "deck_pack",
        packageId: `org.example.deck${index}`,
        packageVersion: "1.0.0",
      },
      codec: {
        kind: "codec_pack",
        packageId: "org.example.codec",
        packageVersion: `${index + 1}.0.0`,
      },
      reason,
      compatibleProfiles:
        reason === "compatible"
          ? [
              {
                codecFamily: "synthetic",
                profile: "grid",
                profileVersion: "1.0.0",
              },
            ]
          : [],
      compatibleProfile:
        reason === "compatible"
          ? {
              codecFamily: "synthetic",
              profile: "grid",
              profileVersion: "1.0.0",
            }
          : null,
    })),
  };
}

async function settleUi(): Promise<void> {
  await new Promise<void>((resolve) => setTimeout(resolve, 0));
  await tick();
  flushSync();
}

function button(target: HTMLElement, label: string): HTMLButtonElement {
  const result = Array.from(target.querySelectorAll("button")).find(
    (candidate) => candidate.textContent?.trim() === label,
  );
  if (!(result instanceof HTMLButtonElement)) {
    throw new Error(`Missing button: ${label}`);
  }
  return result;
}

function enter(input: HTMLInputElement, value: string): void {
  input.value = value;
  input.dispatchEvent(new Event("input", { bubbles: true }));
  flushSync();
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

describe("mounted LatentDeck Extensions Manager", () => {
  beforeEach(() => {
    invokeMock.mockReset();
    openMock.mockReset();
  });

  it("disables cached lifecycle actions while a package snapshot is pending", async () => {
    const snapshot: ExtensionsSnapshot = {
      packages: [healthySummary],
      matrix: [],
    };
    const pendingSnapshot = deferred<ExtensionsSnapshot>();
    let snapshotCount = 0;
    invokeMock.mockImplementation(async (command: string) => {
      if (command !== "extensions_snapshot") {
        throw new Error(`Unexpected native command ${command}`);
      }
      snapshotCount += 1;
      return snapshotCount === 1 ? snapshot : pendingSnapshot.promise;
    });

    const target = document.createElement("div");
    document.body.append(target);
    const component = mount(ExtensionsManager, { target });
    await settleUi();

    button(target, "Refresh snapshot").click();
    await settleUi();
    expect(snapshotCount).toBe(2);
    expect(button(target, "Refreshing…").disabled).toBe(true);
    expect(button(target, "Verify").disabled).toBe(true);
    expect(button(target, "Repair…").disabled).toBe(true);

    pendingSnapshot.resolve(snapshot);
    await settleUi();
    expect(button(target, "Refresh snapshot").disabled).toBe(false);
    expect(button(target, "Verify").disabled).toBe(false);

    await unmount(component);
    target.remove();
  });

  it("runs the exact local lifecycle and renders every stable matrix reason", async () => {
    let snapshot = initialSnapshot();
    let exposeCorruptionOnSnapshot = false;
    const onPackagesChanged = vi.fn();

    invokeMock.mockImplementation(
      async (command: string, args?: Record<string, unknown>) => {
        switch (command) {
          case "extensions_snapshot":
            if (exposeCorruptionOnSnapshot) {
              snapshot = {
                packages: [
                  {
                    ...healthySummary,
                    health: "corrupt",
                    errorCode: "package.hash_mismatch",
                    errorDetail:
                      "Installed bytes no longer match integrity.json.",
                  },
                ],
                matrix: [],
              };
              exposeCorruptionOnSnapshot = false;
            }
            return snapshot;
          case "extensions_inspect":
            if (args?.path === mismatchedRepairPath) {
              return {
                ...installInspection,
                package: {
                  ...packageReference,
                  packageVersion: "1.2.4",
                },
                archiveSha256: "c".repeat(64),
              } satisfies InspectedExtension;
            }
            return installInspection;
          case "extensions_install":
            snapshot = { packages: [healthySummary], matrix: [] };
            return snapshot;
          case "extensions_verify":
            return healthySummary;
          case "extensions_enable":
            snapshot = {
              packages: [{ ...healthySummary, enabled: true }],
              matrix: [],
            };
            return snapshot;
          case "extensions_disable":
            snapshot = { packages: [healthySummary], matrix: [] };
            return snapshot;
          case "extensions_repair":
            snapshot = { packages: [healthySummary], matrix: [] };
            exposeCorruptionOnSnapshot = true;
            return snapshot;
          case "extensions_remove":
            snapshot = { packages: [], matrix: [] };
            return snapshot;
          default:
            throw new Error(`Unexpected command: ${command}`);
        }
      },
    );
    openMock
      .mockResolvedValueOnce(installPath)
      .mockResolvedValueOnce(mismatchedRepairPath)
      .mockResolvedValueOnce(matchingRepairPath);

    const target = document.createElement("div");
    document.body.append(target);
    const component = mount(ExtensionsManager, {
      target,
      props: { onPackagesChanged },
    });
    await settleUi();

    expect(invokeMock.mock.calls[0]).toEqual(["extensions_snapshot"]);
    expect(target.textContent).toContain("0 exact versions");
    expect(target.textContent).toContain("11 exact Deck × Codec pairs");
    for (const label of compatibilityLabels) {
      expect(target.textContent, label).toContain(label);
    }
    expect(target.textContent?.replace(/\s+/gu, " ")).toContain(
      "synthetic / grid / 1.0.0",
    );

    button(target, "Inspect local package").click();
    await settleUi();
    expect(openMock).toHaveBeenNthCalledWith(1, {
      multiple: false,
      directory: false,
      filters: [
        { name: "LatentDeck Extension", extensions: ["ld", "ldcodec"] },
      ],
    });
    expect(invokeMock).toHaveBeenCalledWith("extensions_inspect", {
      path: installPath,
    });
    expect(target.textContent).toContain(
      "Measured exact bytes for org.example.deck 1.2.3.",
    );
    expect(target.textContent).toContain(
      "Example Publisher · self-declared metadata; SHA-256 confirms bytes, not publisher identity.",
    );

    const installInput = target.querySelector<HTMLInputElement>(
      '[aria-label="Install local extension"] input',
    );
    expect(installInput).not.toBeNull();
    enter(installInput!, "b".repeat(64));
    expect(installInput!.getAttribute("aria-invalid")).toBe("true");
    expect(button(target, "Install exact version").disabled).toBe(true);
    expect(
      invokeMock.mock.calls.filter(
        ([command]) => command === "extensions_install",
      ),
    ).toHaveLength(0);

    enter(installInput!, archiveSha256);
    expect(installInput!.getAttribute("aria-invalid")).toBe("false");
    expect(button(target, "Install exact version").disabled).toBe(false);
    button(target, "Install exact version").click();
    await settleUi();
    expect(invokeMock).toHaveBeenCalledWith("extensions_install", {
      path: installPath,
      expectedSha256: archiveSha256,
    });
    expect(target.textContent).toContain(
      "Installed exact version org.example.deck 1.2.3.",
    );
    expect(target.textContent).toContain("1 exact versions");

    const snapshotsBeforeVerify = invokeMock.mock.calls.filter(
      ([command]) => command === "extensions_snapshot",
    ).length;
    button(target, "Verify").click();
    await settleUi();
    expect(invokeMock).toHaveBeenCalledWith("extensions_verify", {
      package: packageReference,
    });
    expect(target.textContent).toContain(
      "Verified org.example.deck 1.2.3: healthy.",
    );
    expect(
      invokeMock.mock.calls.filter(
        ([command]) => command === "extensions_snapshot",
      ),
    ).toHaveLength(snapshotsBeforeVerify);

    button(target, "Enable").click();
    await settleUi();
    expect(invokeMock).toHaveBeenCalledWith("extensions_enable", {
      package: packageReference,
    });
    expect(target.textContent).toContain("Enabled org.example.deck 1.2.3.");
    expect(button(target, "Disable").disabled).toBe(false);

    button(target, "Disable").click();
    await settleUi();
    expect(invokeMock).toHaveBeenCalledWith("extensions_disable", {
      package: packageReference,
    });
    expect(target.textContent).toContain("Disabled org.example.deck 1.2.3.");

    button(target, "Repair…").click();
    flushSync();
    expect(target.textContent).toContain(
      "Repair target: org.example.deck 1.2.3.",
    );
    button(target, "Inspect repair archive").click();
    await settleUi();
    expect(invokeMock).toHaveBeenCalledWith("extensions_inspect", {
      path: mismatchedRepairPath,
    });
    expect(target.textContent).toContain(
      "Mismatch: deck_pack org.example.deck 1.2.4",
    );
    expect(button(target, "Repair from exact archive").disabled).toBe(true);

    button(target, "Inspect repair archive").click();
    await settleUi();
    expect(invokeMock).toHaveBeenCalledWith("extensions_inspect", {
      path: matchingRepairPath,
    });
    expect(target.textContent).toContain("Exact package identity matches");
    const repairInput = target.querySelector<HTMLInputElement>(
      '[aria-label="Repair exact version"] input',
    );
    expect(repairInput).not.toBeNull();
    enter(repairInput!, "d".repeat(64));
    expect(button(target, "Repair from exact archive").disabled).toBe(true);
    enter(repairInput!, archiveSha256);
    expect(button(target, "Repair from exact archive").disabled).toBe(false);
    button(target, "Repair from exact archive").click();
    await settleUi();
    expect(invokeMock).toHaveBeenCalledWith("extensions_repair", {
      path: matchingRepairPath,
      expectedSha256: archiveSha256,
    });
    expect(target.textContent).toContain(
      "Repaired exact version org.example.deck 1.2.3.",
    );
    expect(
      target.querySelector('[aria-label="Repair exact version"]'),
    ).toBeNull();

    button(target, "Refresh snapshot").click();
    await settleUi();
    expect(target.textContent).toContain("package.hash_mismatch");
    const remove = button(target, "Remove exact version");
    expect(remove.disabled).toBe(true);
    const acknowledgement = target.querySelector<HTMLInputElement>(
      ".corrupt-confirmation input",
    );
    expect(acknowledgement).not.toBeNull();
    acknowledgement!.click();
    flushSync();
    expect(remove.disabled).toBe(false);
    remove.click();
    await settleUi();
    expect(invokeMock).toHaveBeenCalledWith("extensions_remove", {
      package: packageReference,
      allowCorrupt: true,
    });
    expect(target.textContent).toContain(
      "Removed exact version org.example.deck 1.2.3.",
    );
    expect(target.textContent).toContain("0 exact versions");
    expect(onPackagesChanged).toHaveBeenCalledTimes(5);

    await unmount(component);
    target.remove();
  });

  it("keeps a disabled verification-required Codec removable after strict checks fail", async () => {
    const codec: ExtensionPackageReference = {
      kind: "codec_pack",
      packageId: "org.example.codec",
      packageVersion: "2.0.0",
    };
    const verificationRequired: ExtensionPackageSummary = {
      package: codec,
      displayName: "Example Codec",
      publisherName: "Example Publisher",
      enabled: false,
      health: "verification_required",
      errorCode: null,
      errorDetail: null,
    };
    let snapshot: ExtensionsSnapshot = {
      packages: [verificationRequired],
      matrix: [],
    };
    invokeMock.mockImplementation(
      async (command: string, args?: Record<string, unknown>) => {
        switch (command) {
          case "extensions_snapshot":
            return snapshot;
          case "extensions_verify":
          case "extensions_enable":
            throw {
              code: "extension.integrity_failed",
              detail: "Installed bytes changed.",
            };
          case "extensions_remove":
            snapshot = { packages: [], matrix: [] };
            return snapshot;
          default:
            throw new Error(
              `Unexpected command ${command} ${JSON.stringify(args)}`,
            );
        }
      },
    );

    const target = document.createElement("div");
    document.body.append(target);
    const component = mount(ExtensionsManager, { target });
    await settleUi();

    expect(target.textContent?.replace(/\s+/gu, " ")).toContain(
      "verification required",
    );
    expect(target.textContent?.replace(/\s+/gu, " ")).toContain(
      "Verify or Enable performs strict full payload validation before use.",
    );
    expect(button(target, "Enable").disabled).toBe(false);
    expect(button(target, "Repair…").disabled).toBe(false);

    button(target, "Verify").click();
    await settleUi();
    expect(target.textContent).toContain("extension.integrity_failed");
    button(target, "Enable").click();
    await settleUi();
    expect(target.textContent).toContain("extension.integrity_failed");

    const remove = button(target, "Remove exact version");
    expect(remove.disabled).toBe(true);
    const acknowledgement = target.querySelector<HTMLInputElement>(
      ".corrupt-confirmation input",
    );
    expect(acknowledgement).not.toBeNull();
    acknowledgement!.click();
    flushSync();
    expect(remove.disabled).toBe(false);
    remove.click();
    await settleUi();
    expect(invokeMock).toHaveBeenCalledWith("extensions_remove", {
      package: codec,
      allowCorrupt: true,
    });
    expect(target.textContent).toContain("0 exact versions");

    await unmount(component);
    target.remove();
  });
});
