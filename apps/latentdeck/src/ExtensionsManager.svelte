<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { open } from "@tauri-apps/plugin-dialog";
  import { onMount } from "svelte";
  import {
    EMPTY_EXTENSIONS_SNAPSHOT,
    compatibilityReasonLabel,
    extensionPackageKey,
    inspectionMatchesPackage,
    publisherIdentityNotice,
    replaceVerifiedSummary,
    shaConfirmationMatches,
    type ExtensionPackageSummary,
    type ExtensionsSnapshot,
    type InspectedExtension,
  } from "./extension-manager-model";

  export let onPackagesChanged: () => void = () => undefined;

  let snapshot: ExtensionsSnapshot = EMPTY_EXTENSIONS_SNAPSHOT;
  let busy = false;
  let snapshotPending = false;
  $: controlsBusy = busy || snapshotPending;
  let errorMessage = "";
  let statusMessage =
    "Local packages only. Choose every active version explicitly.";

  let installArchivePath: string | null = null;
  let installInspection: InspectedExtension | null = null;
  let installExpectedSha256 = "";

  let repairTarget: ExtensionPackageSummary | null = null;
  let repairArchivePath: string | null = null;
  let repairInspection: InspectedExtension | null = null;
  let repairExpectedSha256 = "";
  let corruptRemovalAcknowledgement: string | null = null;

  onMount(() => {
    void refreshSnapshot(false);
  });

  function describeError(error: unknown, fallbackCode: string): string {
    if (typeof error === "string") return `${fallbackCode}: ${error}`;
    if (error !== null && typeof error === "object") {
      const value = error as Record<string, unknown>;
      const code = typeof value.code === "string" ? value.code : fallbackCode;
      const message =
        typeof value.message === "string" ? value.message : "Command failed.";
      return `${code}: ${message}`;
    }
    return `${fallbackCode}: Command failed.`;
  }

  function formatBytes(value: number): string {
    if (!Number.isFinite(value) || value < 0) return "invalid size";
    if (value < 1024) return `${value} B`;
    if (value < 1024 * 1024) return `${(value / 1024).toFixed(1)} KiB`;
    if (value < 1024 * 1024 * 1024) {
      return `${(value / (1024 * 1024)).toFixed(1)} MiB`;
    }
    return `${(value / (1024 * 1024 * 1024)).toFixed(1)} GiB`;
  }

  async function refreshSnapshot(reportError = true): Promise<void> {
    if (snapshotPending) return;
    snapshotPending = true;
    try {
      snapshot = await invoke<ExtensionsSnapshot>("extensions_snapshot");
      if (reportError) errorMessage = "";
    } catch (error) {
      if (reportError) {
        errorMessage = describeError(error, "extensions.snapshot_failed");
      }
    } finally {
      snapshotPending = false;
    }
  }

  function clearInstallInspection(): void {
    installArchivePath = null;
    installInspection = null;
    installExpectedSha256 = "";
  }

  function startRepair(summary: ExtensionPackageSummary): void {
    repairTarget = summary;
    repairArchivePath = null;
    repairInspection = null;
    repairExpectedSha256 = "";
    errorMessage = "";
    statusMessage = `Repair target: ${summary.package.packageId} ${summary.package.packageVersion}.`;
  }

  function clearRepair(): void {
    repairTarget = null;
    repairArchivePath = null;
    repairInspection = null;
    repairExpectedSha256 = "";
  }

  async function inspectExtensionArchive(
    purpose: "install" | "repair",
  ): Promise<void> {
    errorMessage = "";
    const selection = await open({
      multiple: false,
      directory: false,
      filters: [
        { name: "LatentDeck Extension", extensions: ["ld", "ldcodec"] },
      ],
    });
    if (selection === null || Array.isArray(selection)) return;

    busy = true;
    try {
      const inspection = await invoke<InspectedExtension>(
        "extensions_inspect",
        { path: selection },
      );
      if (purpose === "install") {
        installArchivePath = selection;
        installInspection = inspection;
        installExpectedSha256 = "";
        statusMessage = `Measured exact bytes for ${inspection.package.packageId} ${inspection.package.packageVersion}.`;
      } else {
        repairArchivePath = selection;
        repairInspection = inspection;
        repairExpectedSha256 = "";
        statusMessage =
          repairTarget !== null &&
          inspectionMatchesPackage(inspection, repairTarget.package)
            ? "Repair archive identity matches the exact target."
            : "Repair archive identity does not match the exact target.";
      }
    } catch (error) {
      errorMessage = describeError(error, "extensions.inspect_failed");
    } finally {
      busy = false;
    }
  }

  async function installExtension(): Promise<void> {
    if (
      installArchivePath === null ||
      installInspection === null ||
      !shaConfirmationMatches(
        installExpectedSha256,
        installInspection.archiveSha256,
      )
    ) {
      return;
    }
    busy = true;
    errorMessage = "";
    try {
      snapshot = await invoke<ExtensionsSnapshot>("extensions_install", {
        path: installArchivePath,
        expectedSha256: installExpectedSha256,
      });
      onPackagesChanged();
      statusMessage = `Installed exact version ${installInspection.package.packageId} ${installInspection.package.packageVersion}.`;
      clearInstallInspection();
    } catch (error) {
      errorMessage = describeError(error, "extensions.install_failed");
    } finally {
      busy = false;
    }
  }

  async function verifyExtension(
    summary: ExtensionPackageSummary,
  ): Promise<void> {
    busy = true;
    errorMessage = "";
    try {
      const verified = await invoke<ExtensionPackageSummary>(
        "extensions_verify",
        { package: summary.package },
      );
      snapshot = replaceVerifiedSummary(snapshot, verified);
      statusMessage = `Verified ${summary.package.packageId} ${summary.package.packageVersion}: ${verified.health}.`;
    } catch (error) {
      errorMessage = describeError(error, "extensions.verify_failed");
    } finally {
      busy = false;
    }
  }

  async function setExtensionEnabled(
    summary: ExtensionPackageSummary,
    enabled: boolean,
  ): Promise<void> {
    busy = true;
    errorMessage = "";
    try {
      snapshot = enabled
        ? await invoke<ExtensionsSnapshot>("extensions_enable", {
            package: summary.package,
          })
        : await invoke<ExtensionsSnapshot>("extensions_disable", {
            package: summary.package,
          });
      onPackagesChanged();
      statusMessage = `${enabled ? "Enabled" : "Disabled"} ${summary.package.packageId} ${summary.package.packageVersion}.`;
    } catch (error) {
      errorMessage = describeError(
        error,
        enabled ? "extensions.enable_failed" : "extensions.disable_failed",
      );
    } finally {
      busy = false;
    }
  }

  async function removeExtension(
    summary: ExtensionPackageSummary,
  ): Promise<void> {
    const exactKey = extensionPackageKey(summary.package);
    const allowCorrupt =
      summary.health === "corrupt" ||
      summary.health === "verification_required";
    if (allowCorrupt && corruptRemovalAcknowledgement !== exactKey) return;

    busy = true;
    errorMessage = "";
    try {
      snapshot = await invoke<ExtensionsSnapshot>("extensions_remove", {
        package: summary.package,
        allowCorrupt,
      });
      onPackagesChanged();
      statusMessage = `Removed exact version ${summary.package.packageId} ${summary.package.packageVersion}.`;
      corruptRemovalAcknowledgement = null;
      if (
        repairTarget !== null &&
        extensionPackageKey(repairTarget.package) === exactKey
      ) {
        clearRepair();
      }
    } catch (error) {
      errorMessage = describeError(error, "extensions.remove_failed");
    } finally {
      busy = false;
    }
  }

  async function repairExtension(): Promise<void> {
    if (
      repairTarget === null ||
      repairArchivePath === null ||
      repairInspection === null ||
      !inspectionMatchesPackage(repairInspection, repairTarget.package) ||
      !shaConfirmationMatches(
        repairExpectedSha256,
        repairInspection.archiveSha256,
      )
    ) {
      return;
    }

    busy = true;
    errorMessage = "";
    try {
      snapshot = await invoke<ExtensionsSnapshot>("extensions_repair", {
        path: repairArchivePath,
        expectedSha256: repairExpectedSha256,
      });
      onPackagesChanged();
      statusMessage = `Repaired exact version ${repairTarget.package.packageId} ${repairTarget.package.packageVersion}.`;
      clearRepair();
    } catch (error) {
      errorMessage = describeError(error, "extensions.repair_failed");
    } finally {
      busy = false;
    }
  }
</script>

<div class="extensions-manager" aria-busy={controlsBusy}>
  <header class="manager-heading">
    <div>
      <p>Local packages · exact immutable versions</p>
      <h2>Extensions Manager</h2>
    </div>
    <p>
      Inspect and manage local <code>.ld</code> Decks and
      <code>.ldcodec</code> Codec Packs. Publisher identity is self-declared; SHA-256
      confirms exact archive bytes, not the publisher.
    </p>
    <button disabled={controlsBusy} onclick={() => refreshSnapshot(true)}>
      {snapshotPending ? "Refreshing…" : "Refresh snapshot"}
    </button>
  </header>

  {#if errorMessage !== ""}
    <div class="manager-message error" role="alert">{errorMessage}</div>
  {/if}
  <p class="manager-message" aria-live="polite">{statusMessage}</p>

  <div class="manager-grid">
    <section class="manager-card" aria-label="Install local extension">
      <header>
        <div>
          <span>Local archive preflight</span>
          <strong>Install exact bytes</strong>
        </div>
        <button
          disabled={controlsBusy}
          onclick={() => inspectExtensionArchive("install")}
          >Inspect local package</button
        >
      </header>
      {#if installInspection === null}
        <p class="placeholder">
          Select one local .ld or .ldcodec archive. Installation starts only
          after inspection and exact hash confirmation.
        </p>
      {:else}
        <article class="inspection">
          <header>
            <div>
              <span>{installInspection.package.kind}</span>
              <strong>{installInspection.displayName}</strong>
              <small
                >{installInspection.package.packageId} ·
                {installInspection.package.packageVersion}</small
              >
            </div>
            <small
              >{installInspection.fileCount} files · {formatBytes(
                installInspection.archiveByteLength,
              )} archive · {formatBytes(installInspection.extractedByteLength)} extracted</small
            >
          </header>
          <strong class="warning">Publisher identity is self-declared</strong>
          <p>{publisherIdentityNotice(installInspection)}</p>
          <code title={installInspection.archiveSha256}
            >SHA-256 {installInspection.archiveSha256}</code
          >
          <label>
            Exact lowercase SHA-256
            <input
              autocomplete="off"
              autocapitalize="off"
              spellcheck="false"
              maxlength="64"
              value={installExpectedSha256}
              aria-invalid={!shaConfirmationMatches(
                installExpectedSha256,
                installInspection.archiveSha256,
              )}
              oninput={(event) => {
                installExpectedSha256 = event.currentTarget.value;
              }}
            />
          </label>
          <div class="actions">
            <button
              disabled={controlsBusy ||
                !shaConfirmationMatches(
                  installExpectedSha256,
                  installInspection.archiveSha256,
                )}
              onclick={installExtension}>Install exact version</button
            >
            <button disabled={controlsBusy} onclick={clearInstallInspection}
              >Clear</button
            >
          </div>
        </article>
      {/if}
    </section>

    <section class="manager-card" aria-label="Installed exact versions">
      <header>
        <div>
          <span>Installed snapshot</span>
          <strong>{snapshot.packages.length} exact versions</strong>
        </div>
        <small>Side-by-side · explicit activation · no automatic choice</small>
      </header>
      <div class="package-list">
        {#if snapshot.packages.length === 0}
          <p class="placeholder">No installed Deck or Codec Pack versions.</p>
        {:else}
          {#each snapshot.packages as summary (extensionPackageKey(summary.package))}
            <article
              class:corrupt={summary.health === "corrupt"}
              class:verification-required={summary.health ===
                "verification_required"}
            >
              <header>
                <div>
                  <span>{summary.package.kind}</span>
                  <strong
                    >{summary.displayName ?? summary.package.packageId}</strong
                  >
                  <small
                    >{summary.package.packageId} ·
                    {summary.package.packageVersion}</small
                  >
                </div>
                <div class="state">
                  <span>{summary.enabled ? "enabled" : "disabled"}</span>
                  <strong
                    >{summary.health === "verification_required"
                      ? "verification required"
                      : summary.health}</strong
                  >
                </div>
              </header>
              <p>
                {summary.publisherName ?? "Publisher not declared"} · self-declared
                metadata
              </p>
              {#if summary.errorCode !== null}
                <code
                  >{summary.errorCode} · {summary.errorDetail ??
                    "No detail"}</code
                >
              {/if}
              {#if summary.health === "verification_required"}
                <p>
                  Payload is not read while this Codec Pack is disabled. Verify
                  or Enable performs strict full payload validation before use.
                </p>
              {/if}
              <div class="actions">
                <button
                  disabled={controlsBusy}
                  onclick={() => verifyExtension(summary)}>Verify</button
                >
                <button
                  disabled={controlsBusy ||
                    (!summary.enabled &&
                      summary.health !== "healthy" &&
                      summary.health !== "verification_required")}
                  onclick={() => setExtensionEnabled(summary, !summary.enabled)}
                  >{summary.enabled ? "Disable" : "Enable"}</button
                >
                <button
                  disabled={controlsBusy}
                  onclick={() => startRepair(summary)}>Repair…</button
                >
                <button
                  class="remove"
                  disabled={controlsBusy ||
                    ((summary.health === "corrupt" ||
                      summary.health === "verification_required") &&
                      corruptRemovalAcknowledgement !==
                        extensionPackageKey(summary.package))}
                  onclick={() => removeExtension(summary)}
                  >Remove exact version</button
                >
              </div>
              {#if summary.health === "corrupt" || summary.health === "verification_required"}
                <label class="corrupt-confirmation">
                  <input
                    type="checkbox"
                    checked={corruptRemovalAcknowledgement ===
                      extensionPackageKey(summary.package)}
                    disabled={controlsBusy}
                    onchange={(event) => {
                      corruptRemovalAcknowledgement = event.currentTarget
                        .checked
                        ? extensionPackageKey(summary.package)
                        : null;
                    }}
                  />
                  {summary.health === "corrupt"
                    ? "Allow removing this corrupt exact version"
                    : "Allow removing this disabled exact version without payload verification"}
                </label>
              {/if}
            </article>
          {/each}
        {/if}
      </div>
    </section>
  </div>

  {#if repairTarget !== null}
    <section class="manager-card repair" aria-label="Repair exact version">
      <header>
        <div>
          <span>Repair exact version</span>
          <strong
            >{repairTarget.package.packageId} ·
            {repairTarget.package.packageVersion}</strong
          >
        </div>
        <div class="actions">
          <button
            disabled={controlsBusy}
            onclick={() => inspectExtensionArchive("repair")}
            >Inspect repair archive</button
          >
          <button disabled={controlsBusy} onclick={clearRepair}>Cancel</button>
        </div>
      </header>
      {#if repairInspection === null}
        <p class="placeholder">
          Choose a local archive whose kind, ID, and version match the target
          exactly.
        </p>
      {:else}
        <div class="repair-inspection">
          <p
            class:matching={inspectionMatchesPackage(
              repairInspection,
              repairTarget.package,
            )}
          >
            {inspectionMatchesPackage(repairInspection, repairTarget.package)
              ? "Exact package identity matches"
              : `Mismatch: ${repairInspection.package.kind} ${repairInspection.package.packageId} ${repairInspection.package.packageVersion}`}
          </p>
          <p>{publisherIdentityNotice(repairInspection)}</p>
          <code title={repairInspection.archiveSha256}
            >SHA-256 {repairInspection.archiveSha256}</code
          >
          <label>
            Exact lowercase SHA-256
            <input
              autocomplete="off"
              autocapitalize="off"
              spellcheck="false"
              maxlength="64"
              value={repairExpectedSha256}
              aria-invalid={!shaConfirmationMatches(
                repairExpectedSha256,
                repairInspection.archiveSha256,
              )}
              oninput={(event) => {
                repairExpectedSha256 = event.currentTarget.value;
              }}
            />
          </label>
          <button
            disabled={controlsBusy ||
              !inspectionMatchesPackage(
                repairInspection,
                repairTarget.package,
              ) ||
              !shaConfirmationMatches(
                repairExpectedSha256,
                repairInspection.archiveSha256,
              )}
            onclick={repairExtension}>Repair from exact archive</button
          >
        </div>
      {/if}
    </section>
  {/if}

  <section class="manager-card matrix" aria-label="Compatibility matrix">
    <header>
      <div>
        <span>Compatibility matrix</span>
        <strong>{snapshot.matrix.length} exact Deck × Codec pairs</strong>
      </div>
      <small>Incompatible signals are rejected without conversion.</small>
    </header>
    {#if snapshot.matrix.length === 0}
      <p class="placeholder">
        Install at least one Deck and one Codec Pack to resolve compatibility.
      </p>
    {:else}
      <div class="matrix-row labels" aria-hidden="true">
        <span>Deck</span><span>Codec</span><span>Result</span>
      </div>
      {#each snapshot.matrix as pair (`${extensionPackageKey(pair.deck)}:${extensionPackageKey(pair.codec)}`)}
        <article
          class="matrix-row"
          class:compatible={pair.reason === "compatible"}
        >
          <span
            >{pair.deck.packageId}<small>{pair.deck.packageVersion}</small
            ></span
          >
          <span
            >{pair.codec.packageId}<small>{pair.codec.packageVersion}</small
            ></span
          >
          <strong>
            {compatibilityReasonLabel(pair.reason)}
            {#if pair.compatibleProfile !== null}
              <small
                >{pair.compatibleProfile.codecFamily} /
                {pair.compatibleProfile.profile} /
                {pair.compatibleProfile.profileVersion}</small
              >
            {/if}
          </strong>
        </article>
      {/each}
    {/if}
  </section>
</div>

<style>
  .extensions-manager {
    min-height: 100%;
    padding: 28px;
    color: #e8e7df;
    background:
      linear-gradient(135deg, rgba(213, 78, 38, 0.08), transparent 34%), #111412;
    overflow: auto;
  }

  .manager-heading,
  .manager-card > header,
  .inspection > header,
  .package-list article > header {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: 18px;
  }

  .manager-heading {
    display: grid;
    grid-template-columns: minmax(220px, 0.8fr) minmax(320px, 1.4fr) auto;
    padding-bottom: 22px;
    border-bottom: 1px solid #3f433e;
  }

  h2,
  p {
    margin: 0;
  }

  .manager-heading h2 {
    margin-top: 4px;
    font-size: clamp(1.7rem, 3vw, 2.7rem);
    font-weight: 500;
    letter-spacing: -0.04em;
  }

  .manager-heading > div > p,
  header span,
  header small {
    color: #9da198;
    font-size: 0.72rem;
    letter-spacing: 0.1em;
    text-transform: uppercase;
  }

  .manager-heading > p,
  .placeholder,
  .inspection p,
  .package-list article > p,
  .repair-inspection p {
    color: #b9bcb3;
    line-height: 1.5;
  }

  button,
  input {
    border: 1px solid #555b53;
    border-radius: 2px;
    color: inherit;
    background: #20241f;
    font: inherit;
  }

  button {
    min-height: 34px;
    padding: 7px 11px;
    cursor: pointer;
  }

  button:hover:not(:disabled) {
    border-color: #d65e34;
  }

  button:disabled {
    cursor: not-allowed;
    opacity: 0.45;
  }

  .manager-message {
    margin-top: 14px;
    padding: 10px 12px;
    border-left: 2px solid #72786e;
    background: #1a1e1a;
    color: #b9bcb3;
  }

  .manager-message.error {
    border-color: #e45d4d;
    color: #ffb2a8;
  }

  .manager-grid {
    display: grid;
    grid-template-columns: minmax(320px, 0.85fr) minmax(420px, 1.15fr);
    gap: 18px;
    margin-top: 18px;
  }

  .manager-card {
    margin-top: 18px;
    padding: 18px;
    border: 1px solid #3f433e;
    background: rgba(25, 29, 25, 0.94);
  }

  .manager-grid .manager-card {
    margin-top: 0;
  }

  header strong {
    display: block;
    margin-top: 3px;
    font-weight: 600;
  }

  .placeholder {
    margin-top: 18px;
    padding: 16px;
    border: 1px dashed #484d46;
  }

  .inspection,
  .package-list article,
  .repair-inspection {
    margin-top: 16px;
    padding: 15px;
    border: 1px solid #454a43;
    background: #151815;
  }

  .inspection code,
  .package-list code,
  .repair-inspection code {
    display: block;
    margin-top: 12px;
    padding: 8px;
    overflow-wrap: anywhere;
    color: #d4d8ce;
    background: #0d0f0d;
  }

  .warning {
    display: block;
    margin-top: 14px;
    color: #f0b06a;
  }

  label:not(.corrupt-confirmation) {
    display: grid;
    gap: 7px;
    margin-top: 14px;
    color: #b9bcb3;
    font-size: 0.8rem;
  }

  input {
    min-width: 0;
    padding: 8px 9px;
    font-family: ui-monospace, SFMono-Regular, Consolas, monospace;
  }

  input[aria-invalid="true"] {
    border-color: #8c5746;
  }

  .actions {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
    margin-top: 14px;
  }

  .package-list article.corrupt {
    border-color: #9c4f43;
  }

  .state {
    text-align: right;
  }

  .state strong {
    color: #8fd09f;
  }

  .corrupt .state strong {
    color: #ff8677;
  }

  .verification-required .state strong {
    color: #d6c66f;
  }

  .remove {
    color: #ffb2a8;
  }

  .corrupt-confirmation {
    display: flex;
    gap: 8px;
    align-items: center;
    margin-top: 12px;
    color: #ffb2a8;
    font-size: 0.8rem;
  }

  .matching {
    color: #8fd09f !important;
  }

  .matrix-row {
    display: grid;
    grid-template-columns: 1fr 1fr 1fr;
    gap: 12px;
    margin-top: 8px;
    padding: 10px;
    border-left: 2px solid #7a4c42;
    background: #151815;
  }

  .matrix-row.compatible {
    border-color: #4f9c65;
  }

  .matrix-row span,
  .matrix-row strong,
  .matrix-row small {
    min-width: 0;
    overflow-wrap: anywhere;
  }

  .matrix-row small {
    display: block;
    margin-top: 4px;
    color: #8f948b;
  }

  .matrix-row.labels {
    color: #91968d;
    text-transform: uppercase;
    letter-spacing: 0.08em;
    font-size: 0.7rem;
  }

  @media (max-width: 960px) {
    .manager-heading,
    .manager-grid {
      grid-template-columns: 1fr;
    }
  }
</style>
