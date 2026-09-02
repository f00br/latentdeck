<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { open } from "@tauri-apps/plugin-dialog";
  import { onMount, tick } from "svelte";
  import {
    conversionCancelledCount,
    conversionControls,
    conversionIsActive,
    conversionProgressLabel,
    rawImportProfileKey,
    type ConversionSnapshot,
    type RawImportCodecOptions,
    type RawImportSelection,
  } from "./conversion-model";
  import {
    EMPTY_EXTENSIONS_SNAPSHOT,
    compatibilityReasonLabel,
    extensionPackageKey,
    inspectionMatchesPackage,
    publisherIdentityNotice,
    replaceVerifiedSummary,
    shaConfirmationMatches,
    type ExtensionCodecDevice,
    type ExtensionPackageSummary,
    type ExtensionsSnapshot,
    type InspectedExtension,
  } from "./extension-manager-model";
  import {
    EMPTY_PLAYER_VIEW,
    acceptTrustedSnapshot,
    buildNativeViewportBounds,
    controlsFor,
    describeAudioAvailability,
    describeDiagnosticSaveResult,
    describePlayerOperation,
    describeRuntimeStatus,
    diagnosticSaveEnabled,
    formatFrameRate,
    formatFramePosition,
    fullscreenActionLabel,
    hiddenNativeViewportBounds,
    nextNativeViewportRevision,
    progressPercent,
    selectDisplayedError,
    sameNativeViewportGeometry,
    spoutControlsFor,
    viewportRetryRequiresRemeasure,
    type DiagnosticSaveResult,
    type FullscreenStatus,
    type NativeViewportBounds,
    type NativeViewportSession,
    type PlayerError,
    type PlayerOperation,
    type PlayerView,
    type SpoutStatus,
  } from "./player-model";
  import { product } from "./product";

  type WorkspaceMode = "play" | "prepare" | "extensions";
  type ConversionSelection = { path: string; kind: "file" | "folder" };

  let player = $state<PlayerView>(EMPTY_PLAYER_VIEW);
  let workspaceMode = $state<WorkspaceMode>("play");
  let busy = $state(false);
  let operation = $state<PlayerOperation | null>(null);
  let transientError = $state<PlayerError | null>(null);
  let snapshotPending = false;
  let fullscreen = $state<FullscreenStatus | null>(null);
  let fullscreenPending = false;
  let spout = $state<SpoutStatus | null>(null);
  let spoutBusy = $state(false);
  let spoutName = $state("LatentPlayer Output");
  let spoutNameDirty = $state(false);
  let spoutPending = false;
  let diagnosticBusy = $state(false);
  let diagnosticTone = $state<"idle" | "success" | "error">("idle");
  let diagnosticStatus = $state(
    "Save a path-free support bundle from the current Player lifecycle state.",
  );
  let conversionInputs = $state<ConversionSelection[]>([]);
  let conversionOutputDirectory = $state<string | null>(null);
  let conversionRecursive = $state(false);
  let conversion = $state<ConversionSnapshot | null>(null);
  let conversionBusy = $state(false);
  let conversionError = $state<PlayerError | null>(null);
  let conversionSnapshotPending = false;
  let rawImportOptions = $state<RawImportCodecOptions | null>(null);
  let rawImportProfileKeyValue = $state("");
  let rawImportOptionsPending = false;
  let extensions = $state<ExtensionsSnapshot>(EMPTY_EXTENSIONS_SNAPSHOT);
  let extensionsBusy = $state(false);
  let extensionsSnapshotPending = $state(false);
  let extensionsError = $state<PlayerError | null>(null);
  let extensionsStatus = $state(
    "Installed versions are listed exactly; no version is selected automatically.",
  );
  let installArchivePath = $state<string | null>(null);
  let installInspection = $state<InspectedExtension | null>(null);
  let installExpectedSha256 = $state("");
  let repairTarget = $state<ExtensionPackageSummary | null>(null);
  let repairArchivePath = $state<string | null>(null);
  let repairInspection = $state<InspectedExtension | null>(null);
  let repairExpectedSha256 = $state("");
  let corruptRemovalAcknowledgement = $state<string | null>(null);
  let codecDeviceSelections = $state<Record<string, ExtensionCodecDevice | "">>(
    {},
  );
  const VIEWPORT_RETRY_DELAYS_MS = [250, 1000, 2500] as const;
  let viewportAnchor: HTMLDivElement | null = null;
  let viewportEpoch: number | null = null;
  let viewportRevision = 0;
  let viewportFrame: number | null = null;
  let viewportDesired: NativeViewportBounds | null = null;
  let viewportApplied: NativeViewportBounds | null = null;
  let viewportQueued: NativeViewportBounds | null = null;
  let viewportSyncPending = false;
  let viewportRetryTimer: ReturnType<typeof globalThis.setTimeout> | null =
    null;
  let viewportRetryAttempt = 0;
  let viewportMounted = false;

  const controls = $derived(controlsFor(player, busy));
  const progress = $derived(progressPercent(player));
  const audioAvailability = $derived(describeAudioAvailability(player));
  const displayedError = $derived(
    selectDisplayedError(player.error, transientError),
  );
  const selectedDecoder = $derived(
    player.codec.decoderVariants.find((variant) => variant.selected) ?? null,
  );
  const spoutControls = $derived(spoutControlsFor(spout, busy || spoutBusy));
  const canSaveDiagnostics = $derived(diagnosticSaveEnabled(diagnosticBusy));
  const selectedRawImportProfile = $derived(
    rawImportOptions?.profiles.find(
      (profile) => rawImportProfileKey(profile) === rawImportProfileKeyValue,
    ) ?? null,
  );
  const rawImportSelection = $derived<RawImportSelection | null>(
    rawImportOptions !== null && selectedRawImportProfile !== null
      ? {
          packageId: rawImportOptions.packageId,
          packageVersion: rawImportOptions.packageVersion,
          adapterId: rawImportOptions.adapterId,
          adapterVersion: rawImportOptions.adapterVersion,
          profile: selectedRawImportProfile,
        }
      : null,
  );
  const conversionControlState = $derived(
    conversionControls(
      conversion,
      conversionInputs.length,
      conversionOutputDirectory !== null,
      selectedRawImportProfile !== null,
      conversionBusy,
    ),
  );
  const conversionStatus = $derived(conversionProgressLabel(conversion));
  const conversionCancelled = $derived(conversionCancelledCount(conversion));
  const extensionsControlsBusy = $derived(
    extensionsBusy || extensionsSnapshotPending,
  );
  const spoutState = $derived(
    spout === null
      ? "Output inactive"
      : !spout.sdkBuilt
        ? "SDK not built"
        : !spout.ready
          ? "SDK unavailable"
          : spout.published
            ? "Sending"
            : spout.enabled
              ? "Waiting for frame"
              : "Ready / disabled",
  );

  function errorMessage(error: unknown): string {
    if (
      typeof error === "object" &&
      error !== null &&
      "message" in error &&
      typeof error.message === "string"
    ) {
      return error.message;
    }
    return error instanceof Error ? error.message : String(error);
  }

  function playerError(error: unknown, fallbackCode: string): PlayerError {
    const value =
      typeof error === "object" && error !== null
        ? (error as Record<string, unknown>)
        : null;
    return {
      code:
        value !== null && typeof value.code === "string"
          ? value.code
          : fallbackCode,
      message: errorMessage(error),
      recoverable: value?.recoverable === true,
    };
  }

  function errorIsRecoverable(error: unknown): boolean {
    return (
      typeof error === "object" &&
      error !== null &&
      "recoverable" in error &&
      error.recoverable === true
    );
  }

  function displayPathName(path: string): string {
    const segments = path.replaceAll("\\", "/").split("/");
    return segments.at(-1) || "selected folder";
  }

  function invalidateConversionPlan(): void {
    conversion = null;
    conversionError = null;
  }

  function addSelections(
    paths: string[],
    kind: ConversionSelection["kind"],
  ): void {
    const existing = new Set(
      conversionInputs.map((selection) => selection.path.toLocaleLowerCase()),
    );
    let changed = false;
    for (const path of paths) {
      const key = path.toLocaleLowerCase();
      if (!existing.has(key)) {
        conversionInputs.push({ path, kind });
        existing.add(key);
        changed = true;
      }
    }
    if (changed) invalidateConversionPlan();
  }

  async function selectRawFiles(): Promise<void> {
    conversionError = null;
    try {
      const selected = await open({
        multiple: true,
        directory: false,
      });
      const paths = Array.isArray(selected)
        ? selected
        : typeof selected === "string"
          ? [selected]
          : [];
      addSelections(paths, "file");
    } catch (error) {
      conversionError = playerError(error, "conversion.selection_failed");
    }
  }

  async function selectRawFolder(): Promise<void> {
    conversionError = null;
    try {
      const selected = await open({ multiple: false, directory: true });
      if (typeof selected === "string") addSelections([selected], "folder");
    } catch (error) {
      conversionError = playerError(error, "conversion.selection_failed");
    }
  }

  async function selectConversionOutput(): Promise<void> {
    conversionError = null;
    try {
      const selected = await open({ multiple: false, directory: true });
      if (typeof selected === "string") {
        conversionOutputDirectory = selected;
        invalidateConversionPlan();
      }
    } catch (error) {
      conversionError = playerError(
        error,
        "conversion.output_selection_failed",
      );
    }
  }

  async function prepareConversion(): Promise<void> {
    if (conversionOutputDirectory === null || rawImportSelection === null)
      return;
    conversionBusy = true;
    conversionError = null;
    try {
      conversion = await invoke<ConversionSnapshot>("player_conversion_plan", {
        inputs: conversionInputs.map((selection) => selection.path),
        outputDirectory: conversionOutputDirectory,
        recursive: conversionRecursive,
        selection: rawImportSelection,
      });
    } catch (error) {
      conversion = null;
      conversionError = playerError(error, "conversion.preflight_failed");
    } finally {
      conversionBusy = false;
    }
  }

  async function refreshConversion(reportError = false): Promise<void> {
    if (conversionSnapshotPending) return;
    conversionSnapshotPending = true;
    try {
      const snapshot = await invoke<ConversionSnapshot | null>(
        "player_conversion_snapshot",
      );
      if (snapshot !== null) conversion = snapshot;
    } catch (error) {
      if (reportError) {
        conversionError = playerError(error, "conversion.snapshot_failed");
      }
    } finally {
      conversionSnapshotPending = false;
    }
  }

  function startConversion(): void {
    if (conversion?.phase !== "planned") return;
    conversionError = null;
    conversion = { ...conversion, phase: "running" };
    void invoke("player_conversion_start")
      .then((snapshot) => {
        conversion = snapshot as ConversionSnapshot;
      })
      .catch((error) => {
        conversionError = playerError(error, "conversion.task_failed");
        void refreshConversion();
      });
  }

  async function stopConversion(): Promise<void> {
    conversionError = null;
    try {
      conversion = await invoke<ConversionSnapshot>("player_conversion_stop");
    } catch (error) {
      conversionError = playerError(error, "conversion.stop_failed");
    }
  }

  async function openConverted(index: number): Promise<void> {
    busy = true;
    conversionError = null;
    try {
      const snapshot = await invoke<PlayerView>("player_open_converted", {
        index,
      });
      player = acceptTrustedSnapshot(player, snapshot);
      await setWorkspaceMode("play");
    } catch (error) {
      conversionError = playerError(error, "conversion.output_unavailable");
    } finally {
      busy = false;
    }
  }

  async function setWorkspaceMode(mode: WorkspaceMode): Promise<void> {
    workspaceMode = mode;
    await tick();
    scheduleViewportSync();
    if (mode === "prepare") void refreshRawImportOptions(true);
  }

  async function refreshRawImportOptions(reportError = false): Promise<void> {
    if (rawImportOptionsPending) return;
    rawImportOptionsPending = true;
    try {
      const options = await invoke<RawImportCodecOptions>(
        "player_raw_import_options",
      );
      rawImportOptions = options;
      if (
        !options.profiles.some(
          (profile) =>
            rawImportProfileKey(profile) === rawImportProfileKeyValue,
        )
      ) {
        rawImportProfileKeyValue = "";
        invalidateConversionPlan();
      }
      if (reportError) conversionError = null;
    } catch (error) {
      rawImportOptions = null;
      rawImportProfileKeyValue = "";
      invalidateConversionPlan();
      if (reportError) {
        conversionError = playerError(error, "raw_import.selection_missing");
      }
    } finally {
      rawImportOptionsPending = false;
    }
  }

  async function refreshExtensions(reportError = false): Promise<void> {
    if (extensionsSnapshotPending) return;
    extensionsSnapshotPending = true;
    try {
      extensions = await invoke<ExtensionsSnapshot>("extensions_snapshot");
      if (reportError) extensionsError = null;
    } catch (error) {
      if (reportError) {
        extensionsError = playerError(error, "extensions.snapshot_failed");
      }
    } finally {
      extensionsSnapshotPending = false;
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
    extensionsError = null;
    extensionsStatus = `Repair target: ${summary.package.packageId} ${summary.package.packageVersion}.`;
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
    extensionsError = null;
    try {
      const path = await open({
        multiple: false,
        directory: false,
        filters: [
          { name: "LatentDeck Extension", extensions: ["ld", "ldcodec"] },
        ],
      });
      if (typeof path !== "string") return;

      extensionsBusy = true;
      const inspection = await invoke<InspectedExtension>(
        "extensions_inspect",
        { path },
      );
      if (purpose === "install") {
        installArchivePath = path;
        installInspection = inspection;
        installExpectedSha256 = "";
        extensionsStatus =
          "Archive inspected locally. Confirm its exact measured SHA-256 to install.";
      } else {
        repairArchivePath = path;
        repairInspection = inspection;
        repairExpectedSha256 = "";
        extensionsStatus =
          repairTarget !== null &&
          inspectionMatchesPackage(inspection, repairTarget.package)
            ? "Repair archive identity matches the selected exact version."
            : "Repair archive does not match the selected exact package version.";
      }
    } catch (error) {
      extensionsError = playerError(error, "extensions.inspect_failed");
    } finally {
      extensionsBusy = false;
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
    extensionsBusy = true;
    extensionsError = null;
    try {
      extensions = await invoke<ExtensionsSnapshot>("extensions_install", {
        path: installArchivePath,
        expectedSha256: installExpectedSha256,
      });
      extensionsStatus = `Installed exact version ${installInspection.package.packageId} ${installInspection.package.packageVersion}.`;
      clearInstallInspection();
    } catch (error) {
      extensionsError = playerError(error, "extensions.install_failed");
    } finally {
      extensionsBusy = false;
    }
  }

  async function verifyExtension(
    summary: ExtensionPackageSummary,
  ): Promise<void> {
    extensionsBusy = true;
    extensionsError = null;
    try {
      const verified = await invoke<ExtensionPackageSummary>(
        "extensions_verify",
        { package: summary.package },
      );
      extensions = replaceVerifiedSummary(extensions, verified);
      await refreshExtensions();
      extensionsStatus = `Verified ${summary.package.packageId} ${summary.package.packageVersion}: ${verified.health}.`;
    } catch (error) {
      extensionsError = playerError(error, "extensions.verify_failed");
    } finally {
      extensionsBusy = false;
    }
  }

  async function setExtensionEnabled(
    summary: ExtensionPackageSummary,
    enabled: boolean,
  ): Promise<void> {
    extensionsBusy = true;
    extensionsError = null;
    try {
      extensions = enabled
        ? await invoke<ExtensionsSnapshot>("extensions_enable", {
            package: summary.package,
          })
        : await invoke<ExtensionsSnapshot>("extensions_disable", {
            package: summary.package,
          });
      extensionsStatus = `${enabled ? "Enabled" : "Disabled"} ${summary.package.packageId} ${summary.package.packageVersion}.`;
    } catch (error) {
      extensionsError = playerError(
        error,
        enabled ? "extensions.enable_failed" : "extensions.disable_failed",
      );
    } finally {
      extensionsBusy = false;
    }
  }

  async function removeExtension(
    summary: ExtensionPackageSummary,
  ): Promise<void> {
    const exactKey = extensionPackageKey(summary.package);
    const allowCorrupt = summary.health === "corrupt";
    if (allowCorrupt && corruptRemovalAcknowledgement !== exactKey) return;

    extensionsBusy = true;
    extensionsError = null;
    try {
      extensions = await invoke<ExtensionsSnapshot>("extensions_remove", {
        package: summary.package,
        allowCorrupt: allowCorrupt,
      });
      extensionsStatus = `Removed exact version ${summary.package.packageId} ${summary.package.packageVersion}.`;
      corruptRemovalAcknowledgement = null;
      if (
        repairTarget !== null &&
        extensionPackageKey(repairTarget.package) === exactKey
      ) {
        clearRepair();
      }
    } catch (error) {
      extensionsError = playerError(error, "extensions.remove_failed");
    } finally {
      extensionsBusy = false;
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

    extensionsBusy = true;
    extensionsError = null;
    try {
      extensions = await invoke<ExtensionsSnapshot>("extensions_repair", {
        path: repairArchivePath,
        expectedSha256: repairExpectedSha256,
      });
      extensionsStatus = `Repaired exact version ${repairTarget.package.packageId} ${repairTarget.package.packageVersion}.`;
      clearRepair();
    } catch (error) {
      extensionsError = playerError(error, "extensions.repair_failed");
    } finally {
      extensionsBusy = false;
    }
  }

  function setCodecDevice(
    summary: ExtensionPackageSummary,
    device: ExtensionCodecDevice | "",
  ): void {
    codecDeviceSelections = {
      ...codecDeviceSelections,
      [extensionPackageKey(summary.package)]: device,
    };
  }

  async function useCodecInPlayer(
    summary: ExtensionPackageSummary,
  ): Promise<void> {
    const device = codecDeviceSelections[extensionPackageKey(summary.package)];
    if (
      extensionsSnapshotPending ||
      summary.package.kind !== "codec_pack" ||
      summary.health !== "healthy" ||
      !summary.enabled ||
      (device !== "cpu" && device !== "cuda")
    ) {
      return;
    }

    extensionsBusy = true;
    extensionsError = null;
    try {
      await invoke<void>("player_select_codec_exact", {
        packageId: summary.package.packageId,
        packageVersion: summary.package.packageVersion,
        device,
      });
      const snapshot = await invoke<PlayerView>("player_snapshot");
      player = acceptTrustedSnapshot(player, snapshot);
      extensionsStatus = `Player selected ${summary.package.packageId} ${summary.package.packageVersion} on ${device}.`;
      await setWorkspaceMode("play");
      void refreshRawImportOptions();
    } catch (error) {
      extensionsError = playerError(error, "extensions.player_select_failed");
    } finally {
      extensionsBusy = false;
    }
  }

  async function command(
    name: string,
    activeOperation: PlayerOperation,
    arguments_: Record<string, unknown> = {},
  ): Promise<void> {
    busy = true;
    operation = activeOperation;
    transientError = null;
    try {
      const snapshot = await invoke<PlayerView>(name, arguments_);
      player = acceptTrustedSnapshot(player, snapshot);
    } catch (error) {
      transientError = playerError(error, "player.command_failed");
    } finally {
      operation = null;
      busy = false;
    }
  }

  async function refreshSnapshot(reportError = false): Promise<void> {
    if (snapshotPending) {
      return;
    }
    snapshotPending = true;
    try {
      const snapshot = await invoke<PlayerView>("player_snapshot");
      player = acceptTrustedSnapshot(player, snapshot);
    } catch (error) {
      if (reportError) {
        transientError = playerError(error, "player.snapshot_failed");
      }
    } finally {
      snapshotPending = false;
    }
  }

  async function openCartridge(): Promise<void> {
    busy = true;
    operation = "open";
    transientError = null;
    try {
      const path = await open({
        multiple: false,
        directory: false,
        filters: [{ name: "Latent Cartridge", extensions: ["lc"] }],
      });
      if (typeof path === "string") {
        const snapshot = await invoke<PlayerView>("player_open", { path });
        player = acceptTrustedSnapshot(player, snapshot);
      }
    } catch (error) {
      transientError = playerError(error, "player.open_failed");
    } finally {
      operation = null;
      busy = false;
    }
  }

  async function selectDecoder(): Promise<void> {
    busy = true;
    operation = "decoder";
    transientError = null;
    try {
      const path = await open({
        multiple: false,
        directory: false,
        filters: [{ name: "TAEH3 Safetensors", extensions: ["safetensors"] }],
      });
      if (typeof path === "string") {
        const snapshot = await invoke<PlayerView>("player_select_decoder", {
          path,
        });
        player = acceptTrustedSnapshot(player, snapshot);
      }
    } catch (error) {
      transientError = playerError(error, "codec.asset_validation_failed");
    } finally {
      operation = null;
      busy = false;
    }
  }

  async function refreshFullscreen(reportError = false): Promise<void> {
    if (fullscreenPending) return;
    fullscreenPending = true;
    try {
      fullscreen = await invoke<FullscreenStatus | null>(
        "player_fullscreen_status",
      );
    } catch (error) {
      // A failed native read may represent a retained recovery snapshot after
      // a partial Win32 transition. Preserve an explicit Exit route.
      fullscreen ??= { active: true };
      if (reportError) {
        transientError = playerError(error, "output.fullscreen_status_failed");
      }
    } finally {
      fullscreenPending = false;
    }
  }

  async function setFullscreen(enabled: boolean): Promise<void> {
    const previous = fullscreen;
    busy = true;
    operation = enabled ? "fullscreen-enter" : "fullscreen-exit";
    transientError = null;
    try {
      fullscreen = await invoke<FullscreenStatus>("player_set_fullscreen", {
        enabled,
      });
      await tick();
      scheduleViewportSync();
    } catch (error) {
      fullscreen =
        enabled || previous?.active === true ? { active: true } : previous;
      transientError = playerError(error, "output.fullscreen_failed");
      await refreshFullscreen();
    } finally {
      operation = null;
      busy = false;
    }
  }

  function handleWindowKeydown(event: KeyboardEvent): void {
    if (event.key !== "Escape" || fullscreen?.active !== true || busy) return;
    event.preventDefault();
    void setFullscreen(false);
  }

  async function refreshSpout(reportError = false): Promise<void> {
    if (spoutPending) {
      return;
    }
    spoutPending = true;
    try {
      const status = await invoke<SpoutStatus | null>("player_spout_status");
      spout = status;
      if (status !== null && !spoutNameDirty) {
        spoutName = status.requestedName;
      }
    } catch (error) {
      if (reportError) {
        transientError = playerError(error, "output.spout_status_failed");
      }
    } finally {
      spoutPending = false;
    }
  }

  async function configureSpout(
    name: string | null,
    enabled: boolean | null,
  ): Promise<void> {
    spoutBusy = true;
    transientError = null;
    try {
      spout = await invoke<SpoutStatus>("player_spout_configure", {
        name,
        enabled,
      });
      if (name !== null) {
        spoutNameDirty = false;
        spoutName = spout.requestedName;
      }
    } catch (error) {
      transientError = playerError(error, "output.spout_configure_failed");
    } finally {
      spoutBusy = false;
    }
  }

  async function saveDiagnostics(): Promise<void> {
    diagnosticBusy = true;
    diagnosticTone = "idle";
    diagnosticStatus = "Choose a new .zip file in the native save dialog…";
    try {
      const result = await invoke<DiagnosticSaveResult>(
        "player_save_diagnostics",
      );
      diagnosticStatus = describeDiagnosticSaveResult(result);
      diagnosticTone = result.status === "saved" ? "success" : "idle";
    } catch (error) {
      const retry = errorIsRecoverable(error)
        ? " You can choose another file name and retry."
        : "";
      diagnosticStatus = `Diagnostic bundle was not saved: ${errorMessage(error)}${retry}`;
      diagnosticTone = "error";
    } finally {
      diagnosticBusy = false;
    }
  }

  function formatBytes(bytes: number): string {
    if (!Number.isFinite(bytes) || bytes < 0) return "invalid size";
    if (bytes < 1024) return `${bytes} B`;
    const mebibytes = bytes / (1024 * 1024);
    return `${mebibytes.toFixed(mebibytes >= 100 ? 0 : 1)} MiB`;
  }

  function scheduleViewportSync(): void {
    if (!viewportMounted || viewportFrame !== null) return;
    viewportFrame = globalThis.requestAnimationFrame(() => {
      viewportFrame = null;
      measureViewport();
    });
  }

  function clearViewportRetry(resetAttempt = false): void {
    if (viewportRetryTimer !== null) {
      globalThis.clearTimeout(viewportRetryTimer);
      viewportRetryTimer = null;
    }
    if (resetAttempt) viewportRetryAttempt = 0;
  }

  function scheduleViewportRetry(
    bounds: NativeViewportBounds,
    error: unknown,
  ): void {
    if (
      !viewportMounted ||
      viewportDesired?.epoch !== bounds.epoch ||
      viewportDesired?.revision !== bounds.revision ||
      sameNativeViewportGeometry(viewportApplied, bounds)
    ) {
      return;
    }

    const viewportError = playerError(error, "output.viewport_sync_failed");
    transientError = viewportError;
    if (viewportRetryTimer !== null) return;

    const delay = VIEWPORT_RETRY_DELAYS_MS[viewportRetryAttempt];
    if (delay === undefined) return;
    viewportRetryAttempt += 1;
    viewportRetryTimer = globalThis.setTimeout(() => {
      viewportRetryTimer = null;
      if (
        !viewportMounted ||
        viewportDesired?.epoch !== bounds.epoch ||
        viewportDesired?.revision !== bounds.revision ||
        sameNativeViewportGeometry(viewportApplied, bounds)
      ) {
        return;
      }
      if (viewportRetryRequiresRemeasure(viewportError.code)) {
        // A DPI transition makes the old measurement permanently invalid.
        // Re-measure under the current devicePixelRatio instead of replaying
        // stale CSS coordinates with the same scale.
        measureViewport();
        if (
          viewportDesired?.epoch === bounds.epoch &&
          viewportDesired.revision === bounds.revision
        ) {
          viewportQueued = bounds;
          void flushViewportSync();
        }
        return;
      }
      viewportQueued = bounds;
      void flushViewportSync();
    }, delay);
  }

  function measureViewport(): void {
    const epoch = viewportEpoch;
    const anchor = viewportAnchor;
    if (epoch === null || anchor === null || !anchor.isConnected) return;
    const rect = anchor.getBoundingClientRect();
    const style = globalThis.getComputedStyle(anchor);
    const scaleFactor = globalThis.devicePixelRatio;
    const cssTolerance = 2 / scaleFactor;
    const fullyInsideClient =
      Number.isFinite(scaleFactor) &&
      scaleFactor >= 0.5 &&
      scaleFactor <= 8 &&
      rect.left >= 0 &&
      rect.top >= 0 &&
      rect.right <= document.documentElement.clientWidth + cssTolerance &&
      rect.bottom <= document.documentElement.clientHeight + cssTolerance;
    const visible =
      !document.hidden &&
      anchor.offsetParent !== null &&
      style.display !== "none" &&
      style.visibility !== "hidden" &&
      style.opacity !== "0" &&
      fullyInsideClient;
    const revision = nextNativeViewportRevision(viewportRevision);
    if (revision === null) {
      transientError = {
        code: "output.viewport_revision_overflow",
        message:
          "The Player exhausted its embedded video-area revision counter.",
        recoverable: false,
      };
      return;
    }
    const bounds = visible
      ? buildNativeViewportBounds(epoch, revision, rect, scaleFactor, true)
      : hiddenNativeViewportBounds(epoch, revision, scaleFactor);
    if (bounds === null) {
      transientError = {
        code: "output.viewport_bounds_invalid",
        message:
          "The Player could not measure a safe native-video viewport. Resize the window and retry.",
        recoverable: true,
      };
      return;
    }
    if (sameNativeViewportGeometry(viewportDesired, bounds)) return;

    viewportRevision = revision;
    viewportDesired = bounds;
    clearViewportRetry(true);
    viewportQueued = bounds;
    void flushViewportSync();
  }

  async function flushViewportSync(): Promise<void> {
    if (viewportSyncPending || viewportQueued === null) return;
    const bounds = viewportQueued;
    viewportQueued = null;
    viewportSyncPending = true;
    try {
      await invoke("player_viewport_set_bounds", { bounds });
      viewportApplied = bounds;
      if (
        viewportDesired?.epoch === bounds.epoch &&
        viewportDesired.revision === bounds.revision
      ) {
        clearViewportRetry(true);
        if (transientError?.code.startsWith("output.viewport_")) {
          transientError = null;
        }
      }
    } catch (error) {
      scheduleViewportRetry(bounds, error);
    } finally {
      viewportSyncPending = false;
      if (viewportQueued !== null) void flushViewportSync();
    }
  }

  onMount(() => {
    let disposed = false;
    viewportMounted = true;
    const viewportObserver = new ResizeObserver(scheduleViewportSync);
    const intersectionObserver = new IntersectionObserver(scheduleViewportSync);
    if (viewportAnchor !== null) viewportObserver.observe(viewportAnchor);
    if (viewportAnchor !== null) intersectionObserver.observe(viewportAnchor);
    globalThis.addEventListener("resize", scheduleViewportSync);
    globalThis.addEventListener("scroll", scheduleViewportSync, true);
    globalThis.visualViewport?.addEventListener("resize", scheduleViewportSync);
    globalThis.visualViewport?.addEventListener("scroll", scheduleViewportSync);
    document.addEventListener("visibilitychange", scheduleViewportSync);
    void (async () => {
      try {
        const session = await invoke<NativeViewportSession>(
          "player_viewport_session_begin",
        );
        if (disposed) return;
        viewportEpoch = session.epoch;
        viewportRevision = 0;
        viewportDesired = null;
        viewportApplied = null;
        viewportQueued = null;
        clearViewportRetry(true);
        await tick();
        if (!disposed) scheduleViewportSync();
      } catch (error) {
        if (!disposed) {
          transientError = playerError(error, "output.viewport_session_failed");
        }
      }
    })();
    void refreshSnapshot(true);
    void refreshFullscreen();
    void refreshSpout();
    void refreshConversion();
    void refreshExtensions();
    void refreshRawImportOptions();
    const snapshotTimer = globalThis.setInterval(() => {
      void refreshSnapshot();
    }, 100);
    const spoutTimer = globalThis.setInterval(() => {
      void refreshSpout();
      void refreshFullscreen();
    }, 250);
    const conversionTimer = globalThis.setInterval(() => {
      if (conversionIsActive(conversion)) void refreshConversion();
    }, 200);
    return () => {
      disposed = true;
      viewportMounted = false;
      viewportObserver.disconnect();
      intersectionObserver.disconnect();
      globalThis.removeEventListener("resize", scheduleViewportSync);
      globalThis.removeEventListener("scroll", scheduleViewportSync, true);
      globalThis.visualViewport?.removeEventListener(
        "resize",
        scheduleViewportSync,
      );
      globalThis.visualViewport?.removeEventListener(
        "scroll",
        scheduleViewportSync,
      );
      document.removeEventListener("visibilitychange", scheduleViewportSync);
      if (viewportFrame !== null) {
        globalThis.cancelAnimationFrame(viewportFrame);
        viewportFrame = null;
      }
      clearViewportRetry();
      viewportQueued = null;
      const epoch = viewportEpoch;
      const revision = nextNativeViewportRevision(viewportRevision);
      if (epoch !== null && revision !== null) {
        const hidden = hiddenNativeViewportBounds(
          epoch,
          revision,
          globalThis.devicePixelRatio,
        );
        if (hidden !== null) {
          viewportRevision = revision;
          viewportDesired = hidden;
          void invoke("player_viewport_set_bounds", { bounds: hidden }).catch(
            () => undefined,
          );
        }
      }
      viewportEpoch = null;
      globalThis.clearInterval(snapshotTimer);
      globalThis.clearInterval(spoutTimer);
      globalThis.clearInterval(conversionTimer);
    };
  });
</script>

<svelte:window onkeydown={handleWindowKeydown} />

<svelte:head>
  <title>{product.name}</title>
</svelte:head>

<main
  class="player-shell"
  class:fullscreen-shell={workspaceMode === "play" &&
    fullscreen?.active &&
    player.outputAvailable}
  aria-busy={busy || conversionBusy || extensionsControlsBusy}
>
  <header class="masthead">
    <div class="brand-lockup">
      <p class="eyebrow">Latent cartridge playback and preparation</p>
      <h1>{product.name}</h1>
    </div>
    <nav class="mode-switch" aria-label="LatentPlayer workspace">
      <button
        class:active={workspaceMode === "play"}
        aria-pressed={workspaceMode === "play"}
        onclick={() => setWorkspaceMode("play")}>Play</button
      >
      <button
        class:active={workspaceMode === "prepare"}
        aria-pressed={workspaceMode === "prepare"}
        onclick={() => setWorkspaceMode("prepare")}>Prepare</button
      >
      <button
        class:active={workspaceMode === "extensions"}
        aria-pressed={workspaceMode === "extensions"}
        onclick={() => setWorkspaceMode("extensions")}>Extensions</button
      >
    </nav>
    <div class="masthead-actions">
      <span class="phase-badge"
        >{workspaceMode === "play"
          ? player.phase
          : workspaceMode === "prepare"
            ? (conversion?.phase ?? "selection")
            : `${extensions.packages.length} installed`}</span
      >
      {#if workspaceMode === "play"}
        <button class="open" disabled={!controls.open} onclick={openCartridge}
          >{operation === "open" ? "Opening…" : "Open cartridge"}</button
        >
      {/if}
      <p class="version">v{product.version}</p>
    </div>
  </header>

  <div
    class="player-workspace"
    class:workspace-hidden={workspaceMode !== "play"}
    aria-hidden={workspaceMode !== "play"}
  >
    <section class="output-monitor" aria-label="Native video output">
      <header class="monitor-header">
        <p class="monitor-state">
          {player.outputAvailable ? "Native DX12 output" : "Output standby"}
        </p>
        <p
          class="monitor-title"
          title={player.cartridge?.fileName ?? undefined}
        >
          {player.cartridge?.fileName ?? "No cartridge loaded"}
        </p>
      </header>

      <div class:viewport-live={player.outputAvailable} class="viewport-frame">
        <div
          bind:this={viewportAnchor}
          class="native-viewport-anchor"
          data-native-viewport
          aria-hidden="true"
        ></div>
        {#if !player.outputAvailable}
          <div class="viewport-placeholder" aria-live="polite">
            <div class="monitor-grid" aria-hidden="true"></div>
            <span>{player.phase}</span>
            <strong>
              {player.cartridge === null
                ? "Open a validated .lc cartridge"
                : "Press Play to start native presentation"}
            </strong>
            <small>Reserved native presentation viewport</small>
          </div>
        {/if}
      </div>

      <footer class="monitor-footer">
        {#if player.cartridge}
          <p class="monitor-detail">
            {player.cartridge.width} × {player.cartridge.height} ·
            {player.cartridge.frameCount} frames · {formatFrameRate(player)}
          </p>
          <p
            class:preserved={player.cartridge.audioPresent}
            class="audio-notice"
          >
            {audioAvailability}
          </p>
        {:else}
          <p class="monitor-detail">Intrinsic geometry · centered aspect-fit</p>
          <p class="audio-notice">Audio playback is outside v0.1</p>
        {/if}
      </footer>
    </section>

    <aside class="utility-rail" aria-label="Player details and output tools">
      {#if displayedError}
        <section class="error-panel" role="alert">
          <strong>{displayedError.code}</strong>
          <span>{displayedError.message}</span>
        </section>
      {/if}

      {#if player.codec.packId !== null}
        <section class="codec-manager" aria-label="Codec Manager">
          <header>
            <div>
              <span>H3 CODEC PACK</span>
              <strong>{player.codec.displayName}</strong>
              <small
                >{player.codec.packId} · {player.codec.packVersion} ·
                {player.codec.packLicenseLabel}</small
              >
            </div>
            <div class="codec-compatibility">
              <span>COMPATIBILITY</span>
              <strong class:ready={player.codec.state === "ready"}
                >{player.codec.state}</strong
              >
              <small
                >{selectedDecoder === null
                  ? "No accepted decoder selected"
                  : `Selected ${selectedDecoder.variantId}`}</small
              >
            </div>
          </header>
          <div class="decoder-variants">
            {#each player.codec.decoderVariants as variant (variant.variantId)}
              <article class:selected={variant.selected}>
                <div>
                  <span
                    >{player.codec.decoderDisplayName ??
                      player.codec.decoderAssetId}</span
                  >
                  <strong>{variant.variantId}</strong>
                  <small>{formatBytes(variant.byteLength)}</small>
                </div>
                <code title={variant.sha256}>SHA-256 {variant.sha256}</code>
                <nav aria-label={`${variant.variantId} provenance`}>
                  <a href={variant.sourceUrl} target="_blank" rel="noreferrer"
                    >Source</a
                  >
                  <a href={variant.licenseUrl} target="_blank" rel="noreferrer"
                    >{variant.licenseLabel}</a
                  >
                </nav>
              </article>
            {/each}
          </div>
        </section>
      {/if}

      <section class="spout-strip" aria-label="Spout2 output">
        <div class="spout-heading">
          <span
            class:ready={spout?.ready}
            class:sending={spout?.published}
            class="status-dot"
          ></span>
          <div>
            <span>Spout2 · intrinsic GPU texture</span>
            <strong>{spoutState}</strong>
          </div>
        </div>
        <label>
          Sender name
          <input
            maxlength="240"
            value={spoutName}
            disabled={!spoutControls.rename}
            oninput={(event) => {
              spoutName = event.currentTarget.value;
              spoutNameDirty = true;
            }}
          />
        </label>
        <div class="spout-actions">
          <button
            disabled={!spoutControls.rename || !spoutNameDirty}
            onclick={() => configureSpout(spoutName, null)}>Apply name</button
          >
          <button
            class:active={spout?.enabled}
            aria-pressed={spout?.enabled ?? false}
            disabled={!spoutControls.toggle}
            onclick={() => configureSpout(null, !(spout?.enabled ?? false))}
            >{spout?.enabled ? "Disable sender" : "Enable sender"}</button
          >
        </div>
        <p>
          {spout === null
            ? "Start playback to create the native DX12 output."
            : `${spout.activeName} · ${spout.width}×${spout.height} · ${spout.format} · ${spout.submittedFrames} frames`}
        </p>
        {#if spout?.lastErrorCode}
          <code>{spout.lastErrorCode}</code>
        {/if}
      </section>

      <section class="diagnostic-strip" aria-label="Support diagnostics">
        <div>
          <span>Support diagnostics</span>
          <strong>Bounded · path-free · native save</strong>
        </div>
        <p
          class:success={diagnosticTone === "success"}
          class:error={diagnosticTone === "error"}
          aria-live="polite"
        >
          {diagnosticStatus}
        </p>
        <button disabled={!canSaveDiagnostics} onclick={saveDiagnostics}
          >{diagnosticBusy ? "Saving…" : "Save diagnostics"}</button
        >
      </section>
    </aside>
  </div>

  <section
    class="prepare-workspace"
    class:workspace-hidden={workspaceMode !== "prepare"}
    aria-hidden={workspaceMode !== "prepare"}
    aria-label="Prepare raw codec cartridges"
  >
    <header class="prepare-heading">
      <div>
        <p class="eyebrow">Raw codec source → Latent Cartridge</p>
        <h2>Prepare performance files</h2>
      </div>
      <p>
        An explicitly selected Codec Pack v2 performs bounded CPU preflight and
        staging. Core validates and commits every <code>.lc</code> without overwrite;
        no GPU is used.
      </p>
    </header>

    <div class="prepare-grid">
      <section class="codec-builder" aria-label="Raw import codec selection">
        <header>
          <span>1 · Exact codec authority</span>
          <strong>
            {rawImportOptions === null
              ? "No raw-import codec selected"
              : `${rawImportOptions.packageId} ${rawImportOptions.packageVersion}`}
          </strong>
        </header>
        {#if rawImportOptions === null}
          <p>
            Select and enable one exact raw-import-capable <code>.ldcodec</code> version
            in Extensions, then return here.
          </p>
        {:else}
          <p>
            Adapter <code>{rawImportOptions.adapterId}</code>
            <code>{rawImportOptions.adapterVersion}</code> · publisher metadata is
            self-declared.
          </p>
          <label>
            <span>Profile (explicit)</span>
            <select
              value={rawImportProfileKeyValue}
              disabled={!conversionControlState.changeSelection}
              onchange={(event) => {
                rawImportProfileKeyValue = event.currentTarget.value;
                invalidateConversionPlan();
              }}
            >
              <option value="">Choose an exact profile…</option>
              {#each rawImportOptions.profiles as profile (rawImportProfileKey(profile))}
                <option value={rawImportProfileKey(profile)}>
                  {profile.codecFamily} / {profile.profile} / {profile.profileVersion}
                </option>
              {/each}
            </select>
          </label>
        {/if}
      </section>

      <section class="source-builder" aria-label="Conversion inputs">
        <header>
          <span>2 · Sources</span>
          <strong>{conversionInputs.length} selected</strong>
        </header>
        <div class="prepare-actions">
          <button
            disabled={!conversionControlState.changeSelection}
            onclick={selectRawFiles}>Add raw files</button
          >
          <button
            disabled={!conversionControlState.changeSelection}
            onclick={selectRawFolder}>Add folder</button
          >
          <button
            disabled={!conversionControlState.changeSelection ||
              conversionInputs.length === 0}
            onclick={() => {
              conversionInputs = [];
              invalidateConversionPlan();
            }}>Clear selection</button
          >
        </div>
        <label class="recursive-option">
          <input
            type="checkbox"
            checked={conversionRecursive}
            disabled={!conversionControlState.changeSelection}
            onchange={(event) => {
              conversionRecursive = event.currentTarget.checked;
              invalidateConversionPlan();
            }}
          />
          <span>
            Include nested folders
            <small>Off by default; applies only to selected folders.</small>
          </span>
        </label>
        <div class="selection-list">
          {#if conversionInputs.length === 0}
            <p>
              No raw files selected. The exact codec adapter decides which
              source format is valid.
            </p>
          {:else}
            {#each conversionInputs as selection (selection.path)}
              <article>
                <span>{selection.kind}</span>
                <strong title={displayPathName(selection.path)}
                  >{displayPathName(selection.path)}</strong
                >
                <button
                  aria-label={`Remove ${displayPathName(selection.path)}`}
                  disabled={!conversionControlState.changeSelection}
                  onclick={() => {
                    conversionInputs = conversionInputs.filter(
                      (candidate) => candidate.path !== selection.path,
                    );
                    invalidateConversionPlan();
                  }}>Remove</button
                >
              </article>
            {/each}
          {/if}
        </div>
      </section>

      <section class="output-builder" aria-label="Conversion output">
        <header>
          <span>3 · Destination</span>
          <strong
            >{conversionOutputDirectory === null
              ? "Not selected"
              : displayPathName(conversionOutputDirectory)}</strong
          >
        </header>
        <button
          disabled={!conversionControlState.changeSelection}
          onclick={selectConversionOutput}>Choose output folder</button
        >
        <p>
          Folder structure is preserved for recursive sources. Preflight blocks
          duplicate names and every existing output before the batch starts.
        </p>
        <div class="conversion-primary-actions">
          <button
            disabled={!conversionControlState.preflight}
            onclick={prepareConversion}
            >{conversionBusy ? "Validating…" : "Validate batch"}</button
          >
          <button
            class="convert"
            disabled={!conversionControlState.start}
            onclick={startConversion}>Convert to .lc</button
          >
          <button
            class="stop"
            disabled={!conversionControlState.stopAfterCurrent}
            onclick={stopConversion}>Stop after current</button
          >
        </div>
        <small>
          Stop is cooperative: the file currently being written finishes and
          validates atomically; queued files do not start.
        </small>
      </section>
    </div>

    <section class="conversion-results" aria-label="Conversion queue">
      <header>
        <div>
          <span>4 · Preflight and progress</span>
          <strong aria-live="polite">{conversionStatus}</strong>
        </div>
        {#if conversion !== null}
          <small
            >{conversion.completed} complete · {conversion.failed} failed ·
            {conversionCancelled} cancelled · {conversion.items.length} total</small
          >
        {/if}
      </header>
      {#if conversionError}
        <section class="error-panel conversion-error" role="alert">
          <strong>{conversionError.code}</strong>
          <span>{conversionError.message}</span>
        </section>
      {/if}
      <div class="conversion-queue">
        {#if conversion === null}
          <div class="queue-placeholder">
            Choose sources and an output folder, then validate the batch.
          </div>
        {:else}
          {#each conversion.items as item, index (item.relativeOutput)}
            <article class:failed={item.status === "failed"}>
              <div class="queue-item-heading">
                <span class={`conversion-state ${item.status}`}
                  >{item.status}</span
                >
                <strong>{item.sourceName}</strong>
                <code>→ {item.relativeOutput}</code>
              </div>
              {#if item.metadata}
                <p>
                  {item.metadata.decodedWidth} × {item.metadata.decodedHeight} ·
                  {item.metadata.decodedFrames} frames ·
                  {item.metadata.storageDtype} · {formatBytes(
                    item.metadata.sourceBytes,
                  )} · {item.metadata.audioPresent ? "AV" : "visual-only"}
                </p>
                <p class="item-fingerprint">
                  latent {item.metadata.latentSlots} ×
                  {item.metadata.latentHeight} × {item.metadata.latentWidth} · SHA-256
                  <code title={item.metadata.sourceSha256}
                    >{item.metadata.sourceSha256}</code
                  >
                </p>
              {/if}
              {#if item.error}
                <p class="item-error">
                  {item.error.code} · {item.error.message}
                </p>
              {/if}
              {#if item.status === "complete"}
                <button onclick={() => openConverted(index)}
                  >Open in Player</button
                >
              {/if}
            </article>
          {/each}
        {/if}
      </div>
    </section>
  </section>

  <section
    class="extensions-workspace"
    class:workspace-hidden={workspaceMode !== "extensions"}
    aria-hidden={workspaceMode !== "extensions"}
    aria-label="Extensions Manager"
  >
    <header class="extensions-heading">
      <div>
        <p class="eyebrow">Local packages · exact immutable versions</p>
        <h2>Extensions Manager</h2>
      </div>
      <p>
        Inspect and manage local <code>.ld</code> Decks and
        <code>.ldcodec</code> Codec Packs. Publisher metadata is self-declared; an
        exact SHA-256 confirms archive bytes, not publisher identity.
      </p>
      <button
        disabled={extensionsControlsBusy}
        onclick={() => refreshExtensions(true)}
        >{extensionsSnapshotPending
          ? "Refreshing…"
          : "Refresh snapshot"}</button
      >
    </header>

    {#if extensionsError}
      <section class="error-panel extensions-error" role="alert">
        <strong>{extensionsError.code}</strong>
        <span>{extensionsError.message}</span>
      </section>
    {/if}
    <p class="extensions-status" aria-live="polite">{extensionsStatus}</p>

    <div class="extensions-grid">
      <section
        class="extension-install-card"
        aria-label="Install local extension"
      >
        <header>
          <div>
            <span>Local archive preflight</span>
            <strong>Install exact bytes</strong>
          </div>
          <button
            disabled={extensionsControlsBusy}
            onclick={() => inspectExtensionArchive("install")}
            >Inspect local package</button
          >
        </header>
        {#if installInspection === null}
          <p class="extension-placeholder">
            Select one local .ld or .ldcodec archive. Nothing is installed until
            inspection succeeds and you enter its exact measured SHA-256.
          </p>
        {:else}
          <article class="inspection-card">
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
                )} archive · {formatBytes(
                  installInspection.extractedByteLength,
                )} extracted</small
              >
            </header>
            <strong class="publisher-warning"
              >Publisher identity is self-declared</strong
            >
            <p>{publisherIdentityNotice(installInspection)}</p>
            <code class="extension-hash" title={installInspection.archiveSha256}
              >SHA-256 {installInspection.archiveSha256}</code
            >
            <label class="sha-confirmation">
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
            <div class="extension-primary-actions">
              <button
                disabled={extensionsControlsBusy ||
                  !shaConfirmationMatches(
                    installExpectedSha256,
                    installInspection.archiveSha256,
                  )}
                onclick={installExtension}>Install exact version</button
              >
              <button
                disabled={extensionsControlsBusy}
                onclick={clearInstallInspection}>Clear</button
              >
            </div>
          </article>
        {/if}
      </section>

      <section
        class="installed-extensions"
        aria-label="Installed exact versions"
      >
        <header>
          <div>
            <span>Installed snapshot</span>
            <strong>{extensions.packages.length} exact versions</strong>
          </div>
          <small>Side-by-side · explicit activation · no automatic newest</small
          >
        </header>
        <div class="installed-extension-list">
          {#if extensions.packages.length === 0}
            <p class="extension-placeholder">
              No installed Deck or Codec Pack versions.
            </p>
          {:else}
            {#each extensions.packages as summary (extensionPackageKey(summary.package))}
              <article
                class:extension-corrupt={summary.health === "corrupt"}
                class:extension-untrusted={summary.health === "untrusted"}
              >
                <header>
                  <div>
                    <span>{summary.package.kind}</span>
                    <strong
                      >{summary.displayName ??
                        summary.package.packageId}</strong
                    >
                    <small
                      >{summary.package.packageId} ·
                      {summary.package.packageVersion}</small
                    >
                  </div>
                  <div class="extension-state">
                    <span class:enabled={summary.enabled}
                      >{summary.enabled ? "enabled" : "disabled"}</span
                    >
                    <strong class:healthy={summary.health === "healthy"}
                      >{summary.health}</strong
                    >
                  </div>
                </header>
                <p>
                  {summary.publisherName ?? "Publisher not declared"} · self-declared
                  metadata
                </p>
                {#if summary.errorCode !== null}
                  <code class="extension-package-error"
                    >{summary.errorCode} · {summary.errorDetail ??
                      "No detail"}</code
                  >
                {/if}
                <div class="extension-actions">
                  <button
                    disabled={extensionsControlsBusy}
                    onclick={() => verifyExtension(summary)}>Verify</button
                  >
                  <button
                    disabled={extensionsControlsBusy ||
                      summary.health !== "healthy"}
                    onclick={() =>
                      setExtensionEnabled(summary, !summary.enabled)}
                    >{summary.enabled ? "Disable" : "Enable"}</button
                  >
                  <button
                    disabled={extensionsControlsBusy}
                    onclick={() => startRepair(summary)}>Repair…</button
                  >
                  <button
                    class="remove-extension"
                    disabled={extensionsControlsBusy ||
                      (summary.health === "corrupt" &&
                        corruptRemovalAcknowledgement !==
                          extensionPackageKey(summary.package))}
                    onclick={() => removeExtension(summary)}
                    >Remove exact version</button
                  >
                </div>
                {#if summary.package.kind === "codec_pack" && summary.health === "healthy" && summary.enabled}
                  <div class="codec-player-selection">
                    <label>
                      Player device
                      <select
                        value={codecDeviceSelections[
                          extensionPackageKey(summary.package)
                        ] ?? ""}
                        disabled={extensionsControlsBusy}
                        onchange={(event) => {
                          const value = event.currentTarget.value;
                          setCodecDevice(
                            summary,
                            value === "cpu" || value === "cuda" ? value : "",
                          );
                        }}
                      >
                        <option value="" disabled>Select device</option>
                        <option value="cpu">CPU</option>
                        <option value="cuda">CUDA</option>
                      </select>
                    </label>
                    <button
                      disabled={extensionsControlsBusy ||
                        !["cpu", "cuda"].includes(
                          codecDeviceSelections[
                            extensionPackageKey(summary.package)
                          ] ?? "",
                        )}
                      onclick={() => useCodecInPlayer(summary)}
                      >Use in Player</button
                    >
                  </div>
                {/if}
                {#if summary.health === "corrupt"}
                  <label class="corrupt-removal-confirmation">
                    <input
                      type="checkbox"
                      checked={corruptRemovalAcknowledgement ===
                        extensionPackageKey(summary.package)}
                      disabled={extensionsControlsBusy}
                      onchange={(event) => {
                        corruptRemovalAcknowledgement = event.currentTarget
                          .checked
                          ? extensionPackageKey(summary.package)
                          : null;
                      }}
                    />
                    Allow removing this corrupt exact version
                  </label>
                {/if}
              </article>
            {/each}
          {/if}
        </div>
      </section>
    </div>

    {#if repairTarget !== null}
      <section
        class="extension-repair"
        aria-label="Repair exact extension version"
      >
        <header>
          <div>
            <span>Repair exact version</span>
            <strong
              >{repairTarget.package.packageId} ·
              {repairTarget.package.packageVersion}</strong
            >
          </div>
          <div class="extension-primary-actions">
            <button
              disabled={extensionsControlsBusy}
              onclick={() => inspectExtensionArchive("repair")}
              >Inspect repair archive</button
            >
            <button disabled={extensionsControlsBusy} onclick={clearRepair}
              >Cancel</button
            >
          </div>
        </header>
        {#if repairInspection === null}
          <p class="extension-placeholder">
            Choose a separate local archive. Its kind, ID, and version must
            match this repair target exactly.
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
            <code class="extension-hash" title={repairInspection.archiveSha256}
              >SHA-256 {repairInspection.archiveSha256}</code
            >
            <label class="sha-confirmation">
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
              disabled={extensionsControlsBusy ||
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

    <section
      class="extension-matrix"
      aria-label="Deck and Codec compatibility matrix"
    >
      <header>
        <div>
          <span>Compatibility matrix</span>
          <strong>{extensions.matrix.length} exact Deck × Codec pairs</strong>
        </div>
        <small
          >Reasons are explicit; incompatible signals are never converted.</small
        >
      </header>
      <div class="matrix-table">
        {#if extensions.matrix.length === 0}
          <p class="extension-placeholder">
            Install at least one Deck and one Codec Pack to resolve
            compatibility.
          </p>
        {:else}
          <div class="matrix-row matrix-labels" aria-hidden="true">
            <span>Deck</span><span>Codec</span><span>Result</span>
          </div>
          {#each extensions.matrix as pair (`${extensionPackageKey(pair.deck)}:${extensionPackageKey(pair.codec)}`)}
            <article
              class:compatible={pair.reason === "compatible"}
              class="matrix-row"
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
      </div>
    </section>
  </section>

  <section
    class="control-dock"
    class:workspace-hidden={workspaceMode !== "play"}
    aria-hidden={workspaceMode !== "play"}
    aria-label="Playback and position"
  >
    <div class="transport" role="group" aria-label="Playback controls">
      <div class="transport-group">
        <button
          disabled={!controls.play}
          onclick={() => command("player_play", "play")}
          >{operation === "play" ? "Starting…" : "Play"}</button
        >
        <button
          disabled={!controls.pause}
          onclick={() => command("player_pause", "pause")}
          >{operation === "pause" ? "Pausing…" : "Pause"}</button
        >
        <button
          class:active={player.loopEnabled}
          aria-pressed={player.loopEnabled}
          disabled={!controls.loop}
          onclick={() =>
            command("player_set_loop", "loop", {
              enabled: !player.loopEnabled,
            })}>{operation === "loop" ? "Updating…" : "Loop"}</button
        >
        <button
          disabled={!controls.restart}
          onclick={() => command("player_restart", "restart")}
          >{operation === "restart" ? "Restarting…" : "Restart"}</button
        >
      </div>
      <button
        class:active={fullscreen?.active}
        aria-pressed={fullscreen?.active ?? false}
        disabled={fullscreen === null ||
          (fullscreen.active !== true && !controls.fullscreen)}
        onclick={() => setFullscreen(!(fullscreen?.active ?? false))}
        >{fullscreenActionLabel(fullscreen)}</button
      >
    </div>

    <div class="dock-readout">
      {#if operation !== null}
        <p class="operation-status" role="status" aria-live="polite">
          {describePlayerOperation(operation)}
        </p>
      {/if}
      <section class="readout" aria-label="Playback position">
        <div
          class="progress-track"
          role="progressbar"
          aria-label="Decoded frame position"
          aria-valuemin="0"
          aria-valuemax="100"
          aria-valuenow={Math.round(progress)}
        >
          <span style:width={`${progress}%`}></span>
        </div>
        <p>{formatFramePosition(player)}</p>
      </section>
    </div>
  </section>

  <footer
    class="status-strip"
    class:workspace-hidden={workspaceMode !== "play"}
    aria-hidden={workspaceMode !== "play"}
  >
    <div>
      <span class:ready={player.codec.state === "ready"} class="status-dot"
      ></span>
      <span>Codec</span>
      <strong>{player.codec.displayName ?? player.codec.state}</strong>
    </div>
    <p>{describeRuntimeStatus(player)}</p>
    <div>
      <span
        class:ready={spout?.ready}
        class:sending={spout?.published}
        class="status-dot"
      ></span>
      <span>Spout</span>
      <strong>{spoutState}</strong>
    </div>
    {#if player.codec.displayName !== null}
      <button
        class="codec-select"
        disabled={!controls.configureCodec}
        onclick={selectDecoder}
        >{operation === "decoder"
          ? "Validating decoder…"
          : "Select decoder"}</button
      >
    {/if}
  </footer>
</main>
