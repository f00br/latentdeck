<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { onMount, tick } from "svelte";
  import { d2Client, type StopD2Listener } from "./d2-client";
  import {
    DEFAULT_D2_BACKEND,
    DEFAULT_D2_CAPTURE,
    DEFAULT_D2_CONTROLS,
    DEFAULT_D2_STATUS,
    DEFAULT_D2_TRANSPORT,
    MAX_SAFE_D2_SEED,
    buildD2OpenRequest,
    chooseD2Sources,
    copyD2Controls,
    setSlotLoop,
    setSlotPlaying,
    parseD2Seed,
    isD2CaptureActive,
    type D2BackendView,
    type D2CaptureView,
    type D2Algorithm,
    type D2Controls,
    type D2ErrorEvent,
    type D2Slot,
    type D2Status,
    type D2Transport,
  } from "./d2-model";
  import {
    EMPTY_LIBRARY_VIEW,
    compatibilityReasonsByHash,
    describeCommandError,
    describeIntrinsicFormat,
    shortHash,
    type CartridgeView,
    type CollectionView,
    type LibraryView,
    type SignalCompatibilityReport,
  } from "./library-model";
  import {
    describeSpout,
    spoutControlsFor,
    type SpoutStatus,
  } from "./output-model";
  import {
    buildD2Preset,
    d2ControlsFromPreset,
    mergePresetSourceOptions,
    presetCollectionExists,
    resolvePresetLoopDraft,
    resolvePresetSources,
    stagePresetLibraryLoad,
    transitionPresetLoopDraft,
    type D2DeckPreset,
    type DeckPreset,
    type PresetLoopDraft,
  } from "./preset-model";

  type HostState = "checking" | "ready" | "pending" | "error";

  const algorithms: D2Algorithm[] = [
    "LINEAR",
    "XS1",
    "XS2",
    "XS3",
    "XS4",
    "XS5",
  ];

  let bankView: LibraryView = EMPTY_LIBRARY_VIEW;
  let backend: D2BackendView = { ...DEFAULT_D2_BACKEND };
  let status: D2Status = {
    ...DEFAULT_D2_STATUS,
    controls: copyD2Controls(DEFAULT_D2_CONTROLS),
    transport: { ...DEFAULT_D2_TRANSPORT },
    pendingResetReasons: [],
  };
  let controlsDraft: D2Controls = copyD2Controls(DEFAULT_D2_CONTROLS);
  let sourceAHash = "";
  let sourceBHash = "";
  let seedDraft = "0";
  let controlsDirty = false;
  let seedDirty = false;
  let bankBusy = false;
  let backendBusy = false;
  let hostBusy = false;
  let bankError = "";
  let hostState: HostState = "checking";
  let hostMessage = "Checking D2 host contract…";
  let resetMessage = "";
  let capture: D2CaptureView = { ...DEFAULT_D2_CAPTURE };
  let captureBusy = false;
  let lastImportedCaptureId = "";
  let spout: SpoutStatus | null = null;
  let spoutBusy = false;
  let spoutName = "LatentDeck LD-D2 Output";
  let spoutNameDirty = false;
  let spoutPending = false;
  let spoutError = "";
  let presetBusy = false;
  let presetMessage = "";
  let presetResolvedSources: (CartridgeView | null)[] = [];
  let compatibilityReasons: ReadonlyMap<string, readonly string[]> = new Map();
  let compatibilityReady = false;
  let compatibilityRequest = 0;
  type D2PresetLoops = Pick<D2Transport, "loopA" | "loopB">;
  let presetLoopDraft: PresetLoopDraft<D2PresetLoops> | null = null;

  let activeBank: CollectionView | undefined;
  let sourceA: CartridgeView | undefined;
  let sourceB: CartridgeView | undefined;
  let sourceOptions: CartridgeView[] = [];
  let presentCount = 0;
  let captureActive = false;
  let spoutControls = spoutControlsFor(null, false);
  let spoutState = describeSpout(null);
  let selectedCompatibilityReasons: readonly string[] = [];
  let selectedSourcesCompatible = false;
  $: activeBank = bankView.collections.find(
    (collection) => collection.id === bankView.deckSession.activeCollectionId,
  );
  $: sourceOptions = mergePresetSourceOptions(
    bankView.cartridges,
    presetResolvedSources.filter(
      (source) =>
        source !== null &&
        (source.archiveSha256 === sourceAHash ||
          source.archiveSha256 === sourceBHash),
    ),
  );
  $: sourceA = sourceOptions.find(
    (cartridge) => cartridge.archiveSha256 === sourceAHash,
  );
  $: sourceB = sourceOptions.find(
    (cartridge) => cartridge.archiveSha256 === sourceBHash,
  );
  $: selectedCompatibilityReasons =
    compatibilityReasons.get(sourceBHash) ?? [];
  $: selectedSourcesCompatible =
    compatibilityReady &&
    sourceA !== undefined &&
    sourceB !== undefined &&
    selectedCompatibilityReasons.length === 0;
  $: presentCount = bankView.cartridges.filter(
    (cartridge) => cartridge.availability === "present",
  ).length;
  $: captureActive = isD2CaptureActive(capture.state);
  $: spoutControls = spoutControlsFor(spout, hostBusy || spoutBusy);
  $: spoutState = describeSpout(spout);

  onMount(() => {
    let disposed = false;
    const stopListeners: StopD2Listener[] = [];

    void (async () => {
      try {
        const listeners = await Promise.all([
          d2Client.onStatus((incoming) => {
            if (!disposed) applyHostStatus(incoming);
          }),
          d2Client.onError((incoming) => {
            if (!disposed) applyHostError(incoming);
          }),
          d2Client.onCapture((incoming) => {
            if (!disposed) applyCaptureStatus(incoming);
          }),
          d2Client.onCaptureError((incoming) => {
            if (!disposed) applyCaptureError(incoming);
          }),
        ]);
        stopListeners.push(...listeners);
      } catch (error) {
        if (!disposed) markHostFailure(error);
      }

      if (!disposed) {
        await refreshBank();
        await refreshBackendStatus();
        await refreshHostStatus();
        await refreshCaptureStatus();
        await refreshSpoutStatus();
      }
    })();

    const spoutTimer = globalThis.setInterval(() => {
      if (!disposed) void refreshSpoutStatus();
    }, 250);

    return () => {
      disposed = true;
      globalThis.clearInterval(spoutTimer);
      for (const stop of stopListeners) stop();
    };
  });

  async function refreshBank(): Promise<void> {
    if (presetBusy) return;
    bankBusy = true;
    bankError = "";
    try {
      bankView = await invoke<LibraryView>("library_snapshot", {
        search: null,
      });
      presetResolvedSources = presetResolvedSources.filter(
        (source) =>
          source !== null &&
          (source.archiveSha256 === sourceAHash ||
            source.archiveSha256 === sourceBHash),
      );
      const availableSources = mergePresetSourceOptions(
        bankView.cartridges,
        presetResolvedSources,
      );
      const choices = chooseD2Sources(
        availableSources,
        sourceAHash,
        sourceBHash,
      );
      if (
        choices.sourceAHash !== sourceAHash ||
        choices.sourceBHash !== sourceBHash
      ) {
        discardPresetLoopDraft();
      }
      sourceAHash = choices.sourceAHash;
      sourceBHash = choices.sourceBHash;
      await tick();
      await refreshSpatialCompatibility();
    } catch (error) {
      bankError = describeCommandError(error);
    } finally {
      bankBusy = false;
    }
  }

  async function changeBank(event: Event): Promise<void> {
    const collectionId = (event.currentTarget as HTMLSelectElement).value;
    if (bankBusy || presetBusy) return;
    bankBusy = true;
    bankError = "";
    try {
      await invoke("library_set_active_collection", { collectionId });
      bankView = await invoke<LibraryView>("library_snapshot", {
        search: null,
      });
      presetResolvedSources = [];
      const choices = chooseD2Sources(
        bankView.cartridges,
        sourceAHash,
        sourceBHash,
      );
      discardPresetLoopDraft();
      sourceAHash = choices.sourceAHash;
      sourceBHash = choices.sourceBHash;
      await tick();
      await refreshSpatialCompatibility();
    } catch (error) {
      bankError = describeCommandError(error);
    } finally {
      bankBusy = false;
    }
  }

  async function savePreset(): Promise<void> {
    if (presetBusy || bankBusy || hostBusy || captureActive) return;
    if (
      activeBank === undefined ||
      sourceA?.availability !== "present" ||
      sourceB?.availability !== "present" ||
      !selectedSourcesCompatible
    ) {
      presetMessage =
        "Choose two compatible present cartridges and an active Bank before saving.";
      return;
    }
    const seed = parseD2Seed(seedDraft);
    if (seed === null) {
      presetMessage = `Seed must be an integer from 0 to ${MAX_SAFE_D2_SEED}.`;
      return;
    }
    presetBusy = true;
    presetMessage = "";
    try {
      const loops = resolvePresetLoopDraft(presetLoopDraft, status.transport);
      const preset = buildD2Preset(
        activeBank.id,
        sourceA,
        sourceB,
        controlsDraft,
        loops,
        seed,
      );
      const result = await invoke<{ saved: boolean } | null>(
        "deck_preset_save",
        {
          preset,
        },
      );
      presetMessage =
        result === null ? "Preset save cancelled." : "D2 preset saved.";
    } catch (error) {
      presetMessage = describeCommandError(error);
    } finally {
      presetBusy = false;
    }
  }

  async function loadPreset(): Promise<void> {
    if (presetBusy || bankBusy || backendBusy || hostBusy || captureActive)
      return;
    presetBusy = true;
    presetMessage = "";
    try {
      const document = await invoke<DeckPreset | null>("deck_preset_load");
      if (document === null) {
        presetMessage = "Preset load cancelled.";
        return;
      }
      if (document.deck_type !== "LD-D2") {
        presetMessage = `This is a ${document.deck_type} preset; LD-D2 was not changed.`;
        return;
      }
      const preset: D2DeckPreset = document;
      if (!presetCollectionExists(preset, bankView.collections)) {
        presetMessage =
          "The saved Collection is missing. The current Bank and sources were not changed.";
        return;
      }
      const identities = [preset.slots.a, preset.slots.b];
      const { sources: globallyResolved, library: incoming } =
        await stagePresetLibraryLoad(
          () =>
            invoke<(CartridgeView | null)[]>(
              "library_resolve_preset_sources",
              { identities },
            ),
          () =>
            invoke<LibraryView>("library_activate_collection_snapshot", {
              collectionId: preset.active_collection_id,
              search: null,
            }),
        );
      const sourceOptions = mergePresetSourceOptions(
        incoming.cartridges,
        globallyResolved,
      );
      const resolution = resolvePresetSources(
        identities,
        sourceOptions,
      );
      bankView = incoming;
      presetResolvedSources = globallyResolved;
      [sourceAHash, sourceBHash] = resolution.hashes;
      controlsDirty = true;
      seedDirty = true;
      controlsDraft = d2ControlsFromPreset(preset);
      seedDraft = String(preset.seed);
      presetLoopDraft = transitionPresetLoopDraft(presetLoopDraft, {
        type: "preset-loaded",
        loops: {
          loopA: preset.loops.loop_a,
          loopB: preset.loops.loop_b,
        },
      });
      await tick();
      await refreshSpatialCompatibility();
      presetMessage = [
        "D2 preset loaded as a draft. Press Load A + B to apply it.",
        ...resolution.warnings,
      ].join(" ");
    } catch (error) {
      presetMessage = describeCommandError(error);
    } finally {
      presetBusy = false;
    }
  }

  async function refreshSpatialCompatibility(): Promise<void> {
    const request = ++compatibilityRequest;
    const referenceArchiveSha256 = sourceAHash;
    const candidateArchiveSha256s = sourceOptions.map(
      (source) => source.archiveSha256,
    );
    compatibilityReady = false;
    if (
      referenceArchiveSha256 === "" ||
      candidateArchiveSha256s.length === 0
    ) {
      compatibilityReasons = new Map();
      return;
    }
    try {
      const report = await invoke<SignalCompatibilityReport>(
        "library_signal_compatibility",
        {
          referenceArchiveSha256,
          candidateArchiveSha256s,
          policy: "spatial_synthesis",
        },
      );
      if (request !== compatibilityRequest) return;
      compatibilityReasons = compatibilityReasonsByHash(
        candidateArchiveSha256s,
        report,
      );
      compatibilityReady = true;
    } catch (error) {
      if (request !== compatibilityRequest) return;
      compatibilityReasons = new Map(
        candidateArchiveSha256s.map((hash) => [
          hash,
          ["compatibility check unavailable"],
        ]),
      );
      bankError = describeCommandError(error);
    }
  }

  async function selectSourceA(event: Event): Promise<void> {
    if (presetBusy) return;
    discardPresetLoopDraft();
    sourceAHash = (event.currentTarget as HTMLSelectElement).value;
    await tick();
    await refreshSpatialCompatibility();
  }

  function selectSourceB(event: Event): void {
    if (presetBusy) return;
    discardPresetLoopDraft();
    sourceBHash = (event.currentTarget as HTMLSelectElement).value;
  }

  async function refreshHostStatus(): Promise<void> {
    await runHostAction(async () => {
      applyHostStatus(await d2Client.statusGet());
    });
  }

  async function refreshCaptureStatus(): Promise<void> {
    try {
      applyCaptureStatus(await d2Client.captureStatusGet());
    } catch (error) {
      applyCaptureError({
        code: "capture.status_unavailable",
        detail: describeCommandError(error),
      });
    }
  }

  async function refreshSpoutStatus(reportError = false): Promise<void> {
    if (spoutPending) return;
    spoutPending = true;
    try {
      const incoming = await d2Client.spoutStatusGet();
      spout = incoming;
      if (incoming !== null && !spoutNameDirty) {
        spoutName = incoming.requestedName;
      }
    } catch (error) {
      if (reportError) spoutError = describeCommandError(error);
    } finally {
      spoutPending = false;
    }
  }

  async function configureSpout(
    name: string | null,
    enabled: boolean | null,
  ): Promise<void> {
    if (spoutBusy) return;
    spoutBusy = true;
    spoutError = "";
    try {
      spout = await d2Client.spoutConfigure({ name, enabled });
      if (name !== null) {
        spoutNameDirty = false;
        spoutName = spout.requestedName;
      }
    } catch (error) {
      spoutError = describeCommandError(error);
    } finally {
      spoutBusy = false;
    }
  }

  async function refreshBackendStatus(): Promise<void> {
    if (backendBusy) return;
    backendBusy = true;
    try {
      backend = await d2Client.backendStatusGet();
    } catch (error) {
      backend = {
        ...DEFAULT_D2_BACKEND,
        state: "error",
        detail: describeCommandError(error),
      };
    } finally {
      backendBusy = false;
    }
  }

  async function selectDecoder(): Promise<void> {
    if (backendBusy) return;
    backendBusy = true;
    try {
      backend = await d2Client.selectDecoder();
      if (backend.state === "ready") {
        status = {
          ...DEFAULT_D2_STATUS,
          controls: copyD2Controls(DEFAULT_D2_CONTROLS),
          transport: { ...DEFAULT_D2_TRANSPORT },
          pendingResetReasons: [],
        };
        hostMessage = "Decoder validated · load A and B to begin.";
      }
    } catch (error) {
      backend = {
        ...backend,
        state: "error",
        detail: describeCommandError(error),
      };
    } finally {
      backendBusy = false;
    }
  }

  async function openDeck(): Promise<void> {
    if (presetBusy) return;
    if (backend.state !== "ready") {
      hostState = "error";
      hostMessage =
        backend.detail ??
        "Select a compatible TAEH3 decoder before loading LD-D2.";
      return;
    }
    if (sourceA === undefined || sourceB === undefined) {
      hostState = "error";
      hostMessage = "Choose two present cartridges from the active Bank.";
      return;
    }
    if (!selectedSourcesCompatible) {
      hostState = "error";
      hostMessage = compatibilityReady
        ? `Cartridge B is incompatible with A: ${selectedCompatibilityReasons.join("; ")}. Use explicit Toolkit Align/Crop to create a new cartridge.`
        : "Signal compatibility has not been verified; refresh the active Bank.";
      return;
    }
    const seed = parseD2Seed(seedDraft);
    if (seed === null) {
      hostState = "error";
      hostMessage = `Seed must be an integer from 0 to ${MAX_SAFE_D2_SEED}.`;
      return;
    }
    await runHostAction(async () => {
      const pendingLoops = presetLoopDraft?.loops;
      const transport =
        pendingLoops === undefined
          ? status.loaded
            ? status.transport
            : DEFAULT_D2_TRANSPORT
          : {
              ...DEFAULT_D2_TRANSPORT,
              loopA: pendingLoops.loopA,
              loopB: pendingLoops.loopB,
              playingA: false,
              playingB: false,
            };
      const request = buildD2OpenRequest(
        sourceA as CartridgeView,
        sourceB as CartridgeView,
        controlsDraft,
        transport,
        seed,
      );
      applyHostStatus(await d2Client.open(request));
      presetLoopDraft = null;
      controlsDirty = false;
      seedDirty = false;
    });
  }

  async function applyControls(): Promise<void> {
    await runHostAction(async () => {
      await d2Client.controlsSet(copyD2Controls(controlsDraft));
      applyHostStatus(await d2Client.statusGet());
      controlsDirty = false;
    });
  }

  async function applySeed(): Promise<void> {
    const seed = parseD2Seed(seedDraft);
    if (seed === null) {
      hostState = "error";
      hostMessage = `Seed must be an integer from 0 to ${MAX_SAFE_D2_SEED}.`;
      return;
    }
    await runHostAction(async () => {
      await d2Client.seedSet(seed);
      applyHostStatus(await d2Client.statusGet());
      seedDirty = false;
    });
  }

  async function togglePlaying(slot: D2Slot): Promise<void> {
    const playing =
      slot === "A" ? status.transport.playingA : status.transport.playingB;
    await setTransport(setSlotPlaying(status.transport, slot, !playing));
  }

  async function toggleLoop(slot: D2Slot, event: Event): Promise<void> {
    const loop = (event.currentTarget as HTMLInputElement).checked;
    discardPresetLoopDraft();
    await setTransport(setSlotLoop(status.transport, slot, loop));
  }

  async function setTransport(transport: D2Transport): Promise<void> {
    await runHostAction(async () => {
      await d2Client.transportSet(transport);
      applyHostStatus(await d2Client.statusGet());
    });
  }

  async function restart(): Promise<void> {
    await runHostAction(async () => {
      resetMessage = "Restart requested · waiting for causal reset barrier.";
      applyHostStatus(await d2Client.restart());
    });
  }

  async function snapshotCapture(): Promise<void> {
    if (!status.loaded || captureActive || captureBusy) return;
    captureBusy = true;
    try {
      const started = await d2Client.captureSnapshot();
      if (started !== null) applyCaptureStatus(started);
    } catch (error) {
      applyCaptureError({
        code: "capture.snapshot_failed",
        detail: describeCommandError(error),
      });
    } finally {
      captureBusy = false;
    }
  }

  async function toggleLiveCapture(): Promise<void> {
    if (!status.loaded || captureBusy) return;
    captureBusy = true;
    try {
      if (
        capture.mode === "live_capture" &&
        (capture.state === "capturing" || capture.state === "stop_armed")
      ) {
        applyCaptureStatus(await d2Client.captureLiveStop());
      } else if (!captureActive) {
        const started = await d2Client.captureLiveStart();
        if (started !== null) applyCaptureStatus(started);
      }
    } catch (error) {
      applyCaptureError({
        code: "capture.live_failed",
        detail: describeCommandError(error),
      });
    } finally {
      captureBusy = false;
    }
  }

  async function runHostAction(operation: () => Promise<void>): Promise<void> {
    if (hostBusy || presetBusy) return;
    hostBusy = true;
    try {
      await operation();
    } catch (error) {
      markHostFailure(error);
    } finally {
      hostBusy = false;
    }
  }

  function applyHostStatus(incoming: D2Status): void {
    status = incoming;
    hostState = "ready";
    hostMessage = incoming.loaded
      ? "D2 stream online · host status is authoritative."
      : "D2 host ready · load A and B to begin.";
    if (incoming.pendingReset) {
      resetMessage = `Causal reset pending · ${incoming.pendingResetReasons.join(" · ")}`;
    } else if (resetMessage !== "") {
      resetMessage = `Causal state ready · generation ${incoming.streamGeneration}`;
    }
    if (!controlsDirty) controlsDraft = copyD2Controls(incoming.controls);
    if (!seedDirty) seedDraft = String(incoming.seed);
  }

  function applyHostError(incoming: D2ErrorEvent): void {
    hostState = "error";
    hostMessage = `${incoming.code}: ${incoming.detail}`;
  }

  function applyCaptureStatus(incoming: D2CaptureView): void {
    capture = incoming;
    if (
      incoming.state === "finished" &&
      incoming.captureId !== null &&
      incoming.captureId !== lastImportedCaptureId
    ) {
      lastImportedCaptureId = incoming.captureId;
      void refreshBank();
    }
  }

  function applyCaptureError(incoming: D2ErrorEvent): void {
    capture = {
      ...capture,
      state: "error",
      detail: `${incoming.code}: ${incoming.detail}`,
    };
  }

  function captureStatusText(): string {
    if (capture.state === "idle") {
      return "Choose Snapshot or bounded Live Capture; output is selected natively.";
    }
    if (capture.state === "finished") {
      return capture.archiveSha256 === null
        ? "Capture finished and imported."
        : `Imported ${capture.archiveSha256.slice(0, 10)}… into the Library.`;
    }
    if (capture.state === "error" || capture.state === "aborted") {
      return capture.detail ?? "Capture stopped safely.";
    }
    const target =
      capture.targetLatentSlots === null || capture.targetLatentSlots === "0"
        ? ""
        : ` / ${capture.targetLatentSlots}`;
    return `${capture.state.replaceAll("_", " ")} · ${capture.latentSlots}${target} latent slots`;
  }

  function markHostFailure(error: unknown): void {
    const message = describeCommandError(error);
    const normalized = message.toLocaleLowerCase();
    hostState =
      normalized.includes("not found") ||
      normalized.includes("unknown command") ||
      normalized.includes("not registered")
        ? "pending"
        : "error";
    hostMessage =
      hostState === "pending"
        ? `D2 host integration pending · ${message}`
        : message;
  }

  function selectAlgorithm(algorithm: D2Algorithm): void {
    if (presetBusy) return;
    discardPresetLoopDraft();
    controlsDraft = { ...controlsDraft, algorithm };
    controlsDirty = true;
  }

  function updateSeedDraft(event: Event): void {
    if (presetBusy) return;
    discardPresetLoopDraft();
    seedDraft = (event.currentTarget as HTMLInputElement).value;
    seedDirty = true;
  }

  function discardPresetLoopDraft(): void {
    if (presetLoopDraft === null) return;
    presetLoopDraft = transitionPresetLoopDraft(presetLoopDraft, {
      type: "manual-divergence",
    });
    presetMessage =
      "Preset loop draft discarded after a manual change; current Deck loop state will be used.";
  }

  function cartridgeLabel(cartridge: CartridgeView): string {
    const fileName = cartridge.paths[0]?.fileName ?? cartridge.cartridgeId;
    const format = describeIntrinsicFormat(cartridge);
    const latentGrid =
      format.latentGrid === null ? "" : ` · LATENT ${format.latentGrid}`;
    const unavailable =
      cartridge.availability === "present"
        ? ""
        : ` · ${cartridge.availability}`;
    return `${fileName} · ${format.aspectLabel} · ${format.decodedGeometry}${latentGrid} · ${shortHash(cartridge.archiveSha256)}${unavailable}`;
  }

  function compatibilityLabel(cartridge: CartridgeView): string {
    const reasons = compatibilityReasons.get(cartridge.archiveSha256) ?? [];
    return reasons.length === 0
      ? cartridgeLabel(cartridge)
      : `${cartridgeLabel(cartridge)} · INCOMPATIBLE: ${reasons.join("; ")}`;
  }

  function isIncompatibleCandidate(cartridge: CartridgeView): boolean {
    return (
      !compatibilityReady ||
      (compatibilityReasons.get(cartridge.archiveSha256)?.length ?? 0) > 0
    );
  }

  function formatBytes(bytes: number): string {
    if (!Number.isFinite(bytes) || bytes < 0) return "invalid size";
    if (bytes < 1024) return `${bytes} B`;
    const mebibytes = bytes / (1024 * 1024);
    return `${mebibytes.toFixed(mebibytes >= 100 ? 0 : 1)} MiB`;
  }
</script>

<section
  class="d2-faceplate"
  aria-labelledby="d2-title"
  aria-busy={presetBusy}
  inert={presetBusy}
>
  <header class="d2-header">
    <div>
      <p class="d2-eyebrow">
        Dual-source latent instrument · pre-decode operator
      </p>
      <h2 id="d2-title">LD-D2</h2>
    </div>
    <div
      class="d2-host-state"
      class:pending={hostState === "pending"}
      class:error={hostState === "error"}
    >
      <span class="d2-state-lamp"></span>
      <div>
        <strong>{hostState}</strong>
        <small
          >{status.loaded
            ? `GEN ${status.streamGeneration} · SEQ ${status.streamSequence}`
            : "NO STREAM"}</small
        >
      </div>
    </div>
  </header>

  <div class="d2-status-line" class:error={hostState === "error"}>
    <span>{hostMessage}</span>
    <button
      type="button"
      onclick={() => void refreshHostStatus()}
      disabled={hostBusy}>Refresh host</button
    >
  </div>
  {#if resetMessage !== ""}<div class="d2-reset-line">{resetMessage}</div>{/if}
  {#if bankError !== ""}<div class="d2-bank-error" role="alert">
      {bankError}
    </div>{/if}

  <section class="d2-codec-strip" aria-label="H3 Codec Pack and decoder">
    <div class="d2-codec-identity">
      <span>H3 CODEC PACK</span>
      <strong>{backend.displayName ?? "Not installed"}</strong>
      <small
        >{backend.packId ?? "No compatible pack"}{backend.packVersion === null
          ? ""
          : ` · ${backend.packVersion}`}</small
      >
    </div>
    <div class="d2-codec-contract">
      <span>LD-D2 ENTRYPOINT</span>
      <strong
        >{backend.d2EntrypointAvailable ? "DECLARED" : "UNAVAILABLE"}</strong
      >
      <small>{backend.state.replaceAll("_", " ")}</small>
    </div>
    <div class="d2-decoder-identity">
      <span>EXTERNAL DECODER</span>
      {#if backend.decoder === null}
        <strong>TAEH3 not selected</strong>
        <small
          >{backend.detail ?? "Choose an accepted Safetensors weight."}</small
        >
      {:else}
        <strong>{backend.decoder.assetId} · {backend.decoder.variantId}</strong>
        <small
          >SHA-256 {backend.decoder.sha256.slice(0, 12)}… · {formatBytes(
            backend.decoder.byteLength,
          )}</small
        >
        <nav aria-label="Decoder provenance">
          <a href={backend.decoder.sourceUrl} target="_blank" rel="noreferrer"
            >Source</a
          >
          <a href={backend.decoder.licenseUrl} target="_blank" rel="noreferrer"
            >{backend.decoder.licenseLabel}</a
          >
        </nav>
      {/if}
    </div>
    <div class="d2-codec-action">
      <button
        type="button"
        onclick={() => void selectDecoder()}
        disabled={backendBusy || backend.packId === null}
        >{backendBusy ? "Checking…" : "Select TAEH3"}</button
      >
      <button
        class="secondary"
        type="button"
        onclick={() => void refreshBackendStatus()}
        disabled={backendBusy}>Refresh pack</button
      >
    </div>
  </section>

  <section class="d2-bank-strip" aria-label="Active cartridge collection">
    <div class="d2-field bank-field">
      <label for="d2-bank">Active Collection <span>/ Bank</span></label>
      <select
        id="d2-bank"
        value={bankView.deckSession.activeCollectionId}
        onchange={(event) => void changeBank(event)}
        disabled={bankBusy || presetBusy}
      >
        {#each bankView.collections as collection (collection.id)}
          <option value={collection.id}
            >{collection.name} · {collection.memberCount}</option
          >
        {/each}
      </select>
    </div>
    <div class="d2-bank-meter">
      <span>BANK SOURCE</span>
      <strong>{activeBank?.name ?? "Loading…"}</strong>
      <small
        >{presentCount} present / {bankView.activeMemberCount} indexed</small
      >
    </div>
    <p>
      All Cartridges and Unassigned remain normal Bank selections. No disk scan
      is performed.
    </p>
    <p>
      Release performance target: 448×800. Each D2 mode remains pending until
      its final 30-minute receipt passes; larger intrinsic grids are not yet
      benchmark-certified and are never downscaled implicitly.
    </p>
    <div class="d2-preset-controls">
      <span>DECK PRESET · JSON</span>
      <div>
        <button
          type="button"
          onclick={() => void loadPreset()}
          disabled={presetBusy || bankBusy || backendBusy || hostBusy || captureActive}
          >Load preset</button
        >
        <button
          type="button"
          onclick={() => void savePreset()}
          disabled={presetBusy || bankBusy || hostBusy || captureActive}
          >Save preset</button
        >
      </div>
      <small
        >{presetMessage ||
          "Exact Bank, cartridge IDs/hashes, controls, routing, loops and seed."}</small
      >
    </div>
  </section>

  <div class="d2-signal-grid">
    <section class="source-module source-a" aria-labelledby="source-a-title">
      <header>
        <span>A</span>
        <div>
          <p>Source load draft</p>
          <h3 id="source-a-title">Cartridge A</h3>
        </div>
      </header>
      <label for="d2-source-a">Bank cartridge</label>
      <select
        id="d2-source-a"
        value={sourceAHash}
        onchange={(event) => void selectSourceA(event)}
        disabled={presetBusy || bankBusy || bankView.cartridges.length === 0}
      >
        {#each sourceOptions as cartridge (cartridge.archiveSha256)}
          <option
            value={cartridge.archiveSha256}
            disabled={cartridge.availability !== "present"}
            >{cartridgeLabel(cartridge)}</option
          >
        {/each}
      </select>
      <div class="source-readout">
        <span>{sourceA?.codecProfile ?? "NO SOURCE"}</span>
        <strong
          >{sourceA === undefined
            ? "—"
            : `${sourceA.decodedWidth}×${sourceA.decodedHeight}`}</strong
        >
        <small>{sourceA?.decodedFrameCount ?? 0} decoded frames</small>
        {#if sourceA !== undefined}<small
            >{describeIntrinsicFormat(sourceA).aspectLabel} · LATENT {describeIntrinsicFormat(
              sourceA,
            ).latentGrid ?? "N/A"}</small
          >{/if}
      </div>
      <div class="source-transport">
        <button
          type="button"
          onclick={() => void togglePlaying("A")}
          disabled={!status.loaded || hostBusy}
        >
          {!status.loaded
            ? "Play A"
            : status.transport.playingA
              ? "Pause A"
              : "Play A"}
        </button>
        <label
          ><input
            type="checkbox"
            checked={status.transport.loopA}
            onchange={(event) => void toggleLoop("A", event)}
            disabled={!status.loaded || hostBusy}
          /> Loop A</label
        >
        <span>HEAD <strong>{status.playheadA}</strong></span>
      </div>
    </section>

    <section class="operator-module" aria-labelledby="operator-title">
      <header>
        <div>
          <p>Trusted built-in</p>
          <h3 id="operator-title">Latent Operator</h3>
        </div>
        <span class="operator-id">org.latentdeck.builtin.ld_d2</span>
      </header>

      <div class="algorithm-bank" aria-label="D2 algorithm">
        {#each algorithms as algorithm}
          <button
            type="button"
            class:active={controlsDraft.algorithm === algorithm}
            aria-pressed={controlsDraft.algorithm === algorithm}
            onclick={() => selectAlgorithm(algorithm)}>{algorithm}</button
          >
        {/each}
      </div>

      <form
        class="control-form"
        oninput={() => {
          discardPresetLoopDraft();
          controlsDirty = true;
        }}
        onsubmit={(event) => {
          event.preventDefault();
          void applyControls();
        }}
      >
        <div class="control-columns">
          <div class="control-stack">
            <label class="range-control" for="d2-mix">
              <span>MIX <output>{controlsDraft.mix.toFixed(2)}</output></span>
              <input
                id="d2-mix"
                type="range"
                min="0"
                max="1"
                step="0.01"
                bind:value={controlsDraft.mix}
              />
            </label>
            <label class="range-control" for="d2-interaction">
              <span
                >INTERACTION <output
                  >{controlsDraft.interaction.toFixed(2)}</output
                ></span
              >
              <input
                id="d2-interaction"
                type="range"
                min="0"
                max="1"
                step="0.01"
                bind:value={controlsDraft.interaction}
              />
            </label>
            <label class="range-control" for="d2-preserve">
              <span
                >PRESERVE <output>{controlsDraft.preserve.toFixed(2)}</output
                ></span
              >
              <input
                id="d2-preserve"
                type="range"
                min="0"
                max="1"
                step="0.01"
                bind:value={controlsDraft.preserve}
              />
            </label>
            <label class="range-control" for="d2-chaos">
              <span
                >CHAOS <output>{controlsDraft.chaos.toFixed(2)}</output></span
              >
              <input
                id="d2-chaos"
                type="range"
                min="0"
                max="1"
                step="0.01"
                bind:value={controlsDraft.chaos}
              />
            </label>
          </div>

          <div class="switch-stack">
            <fieldset>
              <legend>Mode</legend>
              <label
                ><input
                  type="radio"
                  name="d2-mode"
                  value="HYBRIDIZE"
                  bind:group={controlsDraft.mode}
                /> Hybridize</label
              >
              <label
                ><input
                  type="radio"
                  name="d2-mode"
                  value="INTERACT"
                  bind:group={controlsDraft.mode}
                /> Interact</label
              >
            </fieldset>
            <fieldset>
              <legend>Structural routing</legend>
              <label
                ><input
                  type="radio"
                  name="d2-routing"
                  value="A"
                  bind:group={controlsDraft.routing}
                /> Carrier A</label
              >
              <label
                ><input
                  type="radio"
                  name="d2-routing"
                  value="B"
                  bind:group={controlsDraft.routing}
                /> Carrier B</label
              >
            </fieldset>
          </div>
        </div>

        <div class="algorithm-controls">
          {#if controlsDraft.algorithm === "LINEAR"}
            <p>
              <strong>LINEAR</strong> uses the A/B mix baseline. XS routing controls
              remain stored but inactive.
            </p>
          {:else if controlsDraft.algorithm === "XS1"}
            <div class="number-grid three">
              <label
                >Channel A<input
                  type="number"
                  min="0"
                  max="23"
                  step="1"
                  bind:value={controlsDraft.xs1ChannelA}
                /></label
              >
              <label
                >Channel B<input
                  type="number"
                  min="0"
                  max="23"
                  step="1"
                  bind:value={controlsDraft.xs1ChannelB}
                /></label
              >
              <label
                >Angle °<input
                  type="number"
                  min="-180"
                  max="180"
                  step="1"
                  bind:value={controlsDraft.xs1AngleDegrees}
                /></label
              >
            </div>
          {:else if controlsDraft.algorithm === "XS2"}
            <div class="number-grid">
              <label
                >Spatial radius<input
                  type="number"
                  min="1"
                  max="8"
                  step="1"
                  bind:value={controlsDraft.xs2Radius}
                /></label
              >
            </div>
          {:else if controlsDraft.algorithm === "XS3"}
            <div class="number-grid">
              <label
                >High gain<input
                  type="number"
                  min="-2"
                  max="2"
                  step="0.05"
                  bind:value={controlsDraft.xs3HighGain}
                /></label
              >
            </div>
          {:else if controlsDraft.algorithm === "XS4"}
            <div class="number-grid">
              <label
                >Epsilon<input
                  type="number"
                  min="0.00000001"
                  max="0.001"
                  step="0.00000001"
                  bind:value={controlsDraft.xs4Epsilon}
                /></label
              >
            </div>
          {:else}
            <fieldset class="xs5-routing">
              <legend>XS5 routing</legend>
              <label
                ><input
                  type="radio"
                  name="xs5-routing"
                  value="TOPK"
                  bind:group={controlsDraft.xs5Routing}
                /> TOPK</label
              >
              <label
                ><input
                  type="radio"
                  name="xs5-routing"
                  value="SINKHORN"
                  bind:group={controlsDraft.xs5Routing}
                /> SINKHORN</label
              >
            </fieldset>
            <div class="number-grid three">
              <label
                >Temperature<input
                  type="number"
                  min="0.02"
                  max="1"
                  step="0.01"
                  bind:value={controlsDraft.temperature}
                /></label
              >
              <label
                >Top K<input
                  type="number"
                  min="1"
                  max="64"
                  step="1"
                  bind:value={controlsDraft.topK}
                /></label
              >
              <label
                >Iterations<input
                  type="number"
                  min="2"
                  max="12"
                  step="1"
                  bind:value={controlsDraft.sinkhornIterations}
                /></label
              >
            </div>
          {/if}
        </div>

        <div class="control-commit">
          <span class:dirty={controlsDirty || hostState !== "ready"}
            >{hostState !== "ready"
              ? "DRAFT · HOST PENDING"
              : controlsDirty
                ? "DRAFT CHANGED"
                : "HOST ACKNOWLEDGED"}</span
          >
          <button
            type="submit"
            disabled={!status.loaded || !controlsDirty || hostBusy}
            >Apply controls</button
          >
        </div>
      </form>

      <div class="seed-row">
        <label for="d2-seed">Deterministic seed</label>
        <input
          id="d2-seed"
          type="number"
          min="0"
          max={MAX_SAFE_D2_SEED}
          step="1"
          value={seedDraft}
          oninput={updateSeedDraft}
        />
        <button
          type="button"
          onclick={() => void applySeed()}
          disabled={!status.loaded || !seedDirty || hostBusy}>Set seed</button
        >
      </div>
    </section>

    <section class="source-module source-b" aria-labelledby="source-b-title">
      <header>
        <span>B</span>
        <div>
          <p>Source load draft</p>
          <h3 id="source-b-title">Cartridge B</h3>
        </div>
      </header>
      <label for="d2-source-b">Bank cartridge</label>
      <select
        id="d2-source-b"
        value={sourceBHash}
        onchange={selectSourceB}
        disabled={presetBusy || bankBusy || bankView.cartridges.length === 0}
      >
        {#each sourceOptions as cartridge (cartridge.archiveSha256)}
          <option
            value={cartridge.archiveSha256}
            disabled={cartridge.availability !== "present" ||
              isIncompatibleCandidate(cartridge)}
            >{compatibilityLabel(cartridge)}</option
          >
        {/each}
      </select>
      {#if selectedCompatibilityReasons.length > 0}<p
          class="d2-bank-error"
          role="status"
        >
          B cannot mix with A: {selectedCompatibilityReasons.join("; ")}. Use
          an explicit Toolkit Align/Crop node to create a compatible `.lc`.
        </p>{/if}
      <div class="source-readout">
        <span>{sourceB?.codecProfile ?? "NO SOURCE"}</span>
        <strong
          >{sourceB === undefined
            ? "—"
            : `${sourceB.decodedWidth}×${sourceB.decodedHeight}`}</strong
        >
        <small>{sourceB?.decodedFrameCount ?? 0} decoded frames</small>
        {#if sourceB !== undefined}<small
            >{describeIntrinsicFormat(sourceB).aspectLabel} · LATENT {describeIntrinsicFormat(
              sourceB,
            ).latentGrid ?? "N/A"}</small
          >{/if}
      </div>
      <div class="source-transport">
        <button
          type="button"
          onclick={() => void togglePlaying("B")}
          disabled={!status.loaded || hostBusy}
        >
          {!status.loaded
            ? "Play B"
            : status.transport.playingB
              ? "Pause B"
              : "Play B"}
        </button>
        <label
          ><input
            type="checkbox"
            checked={status.transport.loopB}
            onchange={(event) => void toggleLoop("B", event)}
            disabled={!status.loaded || hostBusy}
          /> Loop B</label
        >
        <span>HEAD <strong>{status.playheadB}</strong></span>
      </div>
    </section>
  </div>

  <footer class="d2-master-strip">
    <div class="load-module">
      <span>SOURCES</span>
      <button
        class="load-button"
        type="button"
        onclick={() => void openDeck()}
        disabled={hostBusy ||
          backend.state !== "ready" ||
          bankBusy ||
          sourceA === undefined ||
          sourceB === undefined ||
          !selectedSourcesCompatible}>Load A + B</button
      >
    </div>
    <div class="restart-module">
      <span>CAUSAL TRANSPORT</span>
      <button
        type="button"
        onclick={() => void restart()}
        disabled={!status.loaded || hostBusy}>Restart both</button
      >
      <small>Restart and loop require a decoder reset barrier.</small>
    </div>
    <div class="capture-module" aria-label="Resampling status">
      <span>POST-OPERATOR RESAMPLE</span>
      <button
        type="button"
        onclick={() => void snapshotCapture()}
        disabled={!status.loaded || captureActive || captureBusy || hostBusy}
        title="Capture one complete structural-carrier cycle"
        >{capture.mode === "snapshot" && captureActive
          ? "Snapshot running…"
          : "Snapshot"}</button
      >
      <button
        type="button"
        onclick={() => void toggleLiveCapture()}
        disabled={!status.loaded ||
          captureBusy ||
          hostBusy ||
          (captureActive && capture.mode !== "live_capture") ||
          capture.state === "stop_armed" ||
          capture.state === "finalizing"}
        title="Record a bounded changing post-operator latent stream"
        >{capture.mode === "live_capture" && capture.state === "capturing"
          ? "Stop Live Capture"
          : capture.mode === "live_capture" && captureActive
            ? "Live stopping…"
            : "Start Live Capture"}</button
      >
      <small
        class:error={capture.state === "error" || capture.state === "aborted"}
        >{captureStatusText()}</small
      >
    </div>
  </footer>

  <section class="d2-spout-strip" aria-label="Spout2 output">
    <div class="d2-spout-heading">
      <span
        class="d2-spout-lamp"
        class:ready={spout?.ready}
        class:sending={spout?.published}
      ></span>
      <div>
        <span>SPOUT2 · GPU TEXTURE</span>
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
    <button
      type="button"
      disabled={!spoutControls.rename || !spoutNameDirty}
      onclick={() => void configureSpout(spoutName, null)}>Apply name</button
    >
    <button
      class:active={spout?.enabled}
      aria-pressed={spout?.enabled ?? false}
      type="button"
      disabled={!spoutControls.toggle}
      onclick={() => void configureSpout(null, !(spout?.enabled ?? false))}
      >{spout?.enabled ? "Disable sender" : "Enable sender"}</button
    >
    <small>
      {spout === null
        ? "Load sources to create the native DX12 output."
        : `${spout.activeName || spout.requestedName} · ${spout.width}×${spout.height} · ${spout.format} · ${spout.submittedFrames} frames`}
    </small>
    {#if spout?.lastErrorCode || spoutError !== ""}
      <code>{spoutError || spout?.lastErrorCode}</code>
    {/if}
  </section>
</section>

<style>
  .d2-faceplate {
    --d2-line: #4d5c52;
    --d2-line-bright: #77877b;
    --d2-panel: #171e19;
    --d2-panel-raised: #202922;
    --d2-low: #101511;
    --d2-green: #9bdc88;
    --d2-amber: #d2b564;
    --d2-red: #dc796f;
    min-height: calc(100vh - 132px);
    margin-top: 8px;
    border: 1px solid var(--d2-line-bright);
    background:
      linear-gradient(120deg, rgb(255 255 255 / 2.5%), transparent 28%), #131914;
    box-shadow: 0 16px 38px rgb(0 0 0 / 30%);
    color: #dbe3dc;
  }

  .d2-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    min-height: 72px;
    border-bottom: 1px solid var(--d2-line-bright);
    padding: 11px 16px;
    background: linear-gradient(90deg, #26322a, #182019 58%, #111612);
  }

  .d2-header h2,
  .source-module h3,
  .operator-module h3 {
    margin: 0;
    font-family: "Arial Narrow", "Roboto Condensed", "Segoe UI", sans-serif;
    font-weight: 850;
    letter-spacing: 0.06em;
    text-transform: uppercase;
  }

  .d2-header h2 {
    font-size: 1.75rem;
  }

  .d2-eyebrow,
  .source-module header p,
  .operator-module header p {
    margin: 0 0 4px;
    color: #8d9b91;
    font-family: ui-monospace, "Cascadia Mono", Consolas, monospace;
    font-size: 0.57rem;
    font-weight: 700;
    letter-spacing: 0.12em;
    text-transform: uppercase;
  }

  .d2-host-state {
    display: flex;
    align-items: center;
    gap: 9px;
    min-width: 190px;
    border: 1px solid #4f6a53;
    padding: 7px 10px;
    background: #111813;
  }

  .d2-state-lamp {
    width: 9px;
    height: 9px;
    border-radius: 50%;
    background: var(--d2-green);
    box-shadow: 0 0 8px var(--d2-green);
  }

  .d2-host-state.pending .d2-state-lamp {
    background: var(--d2-amber);
    box-shadow: 0 0 8px var(--d2-amber);
  }

  .d2-host-state.error .d2-state-lamp {
    background: var(--d2-red);
    box-shadow: 0 0 8px var(--d2-red);
  }

  .d2-host-state div {
    display: grid;
    gap: 2px;
  }

  .d2-host-state strong,
  .d2-host-state small,
  .d2-bank-meter,
  .source-readout,
  .source-transport > span,
  .operator-id,
  .control-commit > span,
  .d2-master-strip span {
    font-family: ui-monospace, "Cascadia Mono", Consolas, monospace;
    text-transform: uppercase;
  }

  .d2-host-state strong {
    color: var(--d2-green);
    font-size: 0.66rem;
    letter-spacing: 0.1em;
  }

  .d2-host-state.pending strong {
    color: var(--d2-amber);
  }

  .d2-host-state.error strong {
    color: var(--d2-red);
  }

  .d2-host-state small {
    color: #76837a;
    font-size: 0.54rem;
  }

  .d2-status-line,
  .d2-reset-line,
  .d2-bank-error {
    display: flex;
    align-items: center;
    gap: 10px;
    min-height: 32px;
    border-bottom: 1px solid #364139;
    padding: 4px 12px;
    background: #101511;
    color: #9eaaa1;
    font-size: 0.65rem;
  }

  .d2-status-line button {
    min-height: 22px;
    margin-left: auto;
    padding: 2px 7px;
    font-size: 0.58rem;
  }

  .d2-status-line.error,
  .d2-bank-error {
    color: #e49a92;
  }

  .d2-reset-line {
    color: #d8c989;
  }

  .d2-codec-strip {
    display: grid;
    grid-template-columns:
      minmax(190px, 0.9fr) minmax(140px, 0.55fr) minmax(280px, 1.35fr)
      auto;
    align-items: stretch;
    gap: 1px;
    border-bottom: 1px solid var(--d2-line);
    background: #3c483f;
  }

  .d2-codec-identity,
  .d2-codec-contract,
  .d2-decoder-identity,
  .d2-codec-action {
    display: grid;
    align-content: center;
    gap: 3px;
    min-height: 68px;
    padding: 8px 11px;
    background: #151c17;
  }

  .d2-codec-strip span,
  .d2-codec-strip small,
  .d2-codec-strip strong,
  .d2-codec-strip a {
    font-family: ui-monospace, "Cascadia Mono", Consolas, monospace;
  }

  .d2-codec-strip span,
  .d2-codec-strip small {
    color: #78847b;
    font-size: 0.53rem;
  }

  .d2-codec-strip strong {
    overflow: hidden;
    color: #c6d1c8;
    font-size: 0.65rem;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .d2-codec-contract strong {
    color: var(--d2-green);
  }

  .d2-decoder-identity nav {
    display: flex;
    gap: 10px;
  }

  .d2-decoder-identity a {
    color: #9abf90;
    font-size: 0.55rem;
  }

  .d2-codec-action {
    grid-template-columns: 1fr;
    min-width: 124px;
  }

  .d2-codec-action .secondary {
    border-color: #465249;
    background: #1b231d;
    color: #89968d;
  }

  .d2-bank-strip {
    display: grid;
    grid-template-columns: minmax(260px, 420px) 200px minmax(220px, 1fr) minmax(
        240px,
        1fr
      );
    align-items: center;
    gap: 12px;
    border-bottom: 1px solid var(--d2-line);
    padding: 10px 12px;
    background: #1b231d;
  }

  .d2-field {
    display: grid;
    gap: 5px;
  }

  .d2-field label,
  .source-module > label,
  .seed-row > label,
  .number-grid label {
    color: #89968d;
    font-size: 0.59rem;
    font-weight: 800;
    letter-spacing: 0.08em;
    text-transform: uppercase;
  }

  .d2-field label span {
    color: var(--d2-green);
  }

  .d2-bank-meter {
    display: grid;
    gap: 2px;
    border-left: 1px solid var(--d2-line);
    padding-left: 12px;
  }

  .d2-bank-meter span,
  .d2-bank-meter small {
    color: #758178;
    font-size: 0.53rem;
  }

  .d2-bank-meter strong {
    overflow: hidden;
    color: var(--d2-green);
    font-size: 0.67rem;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .d2-bank-strip > p {
    margin: 0;
    color: #7f8c82;
    font-size: 0.65rem;
    line-height: 1.45;
  }

  .d2-preset-controls {
    display: grid;
    gap: 4px;
    border-left: 1px solid var(--d2-line);
    padding-left: 12px;
  }

  .d2-preset-controls > span,
  .d2-preset-controls > small {
    color: #758178;
    font:
      700 0.53rem ui-monospace,
      "Cascadia Mono",
      Consolas,
      monospace;
  }

  .d2-preset-controls > div {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 5px;
  }

  .d2-preset-controls > small {
    min-height: 2.3em;
    line-height: 1.35;
  }

  .d2-signal-grid {
    display: grid;
    grid-template-columns: minmax(210px, 0.72fr) minmax(430px, 1.7fr) minmax(
        210px,
        0.72fr
      );
    gap: 7px;
    padding: 7px;
  }

  .source-module,
  .operator-module {
    min-width: 0;
    border: 1px solid var(--d2-line);
    background:
      linear-gradient(135deg, rgb(255 255 255 / 2.5%), transparent 35%),
      var(--d2-panel);
    box-shadow: inset 0 1px rgb(255 255 255 / 4%);
  }

  .source-module {
    display: flex;
    min-height: 430px;
    flex-direction: column;
    gap: 11px;
    padding: 12px;
  }

  .source-module header,
  .operator-module > header {
    display: flex;
    align-items: center;
    gap: 9px;
    border-bottom: 1px solid #3c493f;
    padding-bottom: 10px;
  }

  .source-module header > span {
    display: grid;
    width: 34px;
    height: 34px;
    place-content: center;
    border: 1px solid #719069;
    background: #263a29;
    color: var(--d2-green);
    font-family: ui-monospace, "Cascadia Mono", Consolas, monospace;
    font-size: 1.1rem;
    font-weight: 800;
  }

  .source-module h3,
  .operator-module h3 {
    font-size: 0.9rem;
  }

  .source-module select {
    width: 100%;
    min-width: 0;
  }

  .source-readout {
    display: grid;
    place-items: center;
    min-height: 138px;
    border: 1px solid #3e4e42;
    padding: 17px 8px;
    background:
      repeating-linear-gradient(
        0deg,
        rgb(255 255 255 / 1.5%) 0 1px,
        transparent 1px 6px
      ),
      #0f1510;
    text-align: center;
  }

  .source-readout span,
  .source-readout small {
    color: #748178;
    font-size: 0.55rem;
  }

  .source-readout strong {
    color: #c8d8ca;
    font-size: 1.05rem;
    font-weight: 500;
  }

  .source-transport {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 6px;
    margin-top: auto;
  }

  .source-transport > label {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 5px;
    border: 1px solid #445047;
    background: #111712;
    color: #a5b0a8;
    font-size: 0.63rem;
  }

  .source-transport input {
    width: 13px;
    min-height: auto;
    accent-color: var(--d2-green);
  }

  .source-transport > span {
    grid-column: 1 / -1;
    border-top: 1px solid #344038;
    padding-top: 7px;
    color: #758178;
    font-size: 0.55rem;
    text-align: right;
  }

  .source-transport > span strong {
    color: var(--d2-green);
    font-size: 0.72rem;
  }

  .operator-module {
    padding: 12px;
  }

  .operator-module > header {
    justify-content: space-between;
  }

  .operator-id {
    color: #6f7c72;
    font-size: 0.5rem;
  }

  .algorithm-bank {
    display: grid;
    grid-template-columns: repeat(6, 1fr);
    gap: 4px;
    margin-top: 10px;
  }

  .algorithm-bank button {
    min-width: 0;
    padding-inline: 4px;
  }

  .algorithm-bank button.active {
    border-color: #8bb87f;
    background: linear-gradient(#3d5a3f, #253928);
    color: #eff9ec;
    box-shadow: inset 0 -2px var(--d2-green);
  }

  .control-form {
    margin-top: 10px;
  }

  .control-columns {
    display: grid;
    grid-template-columns: minmax(0, 1.3fr) minmax(180px, 0.7fr);
    gap: 10px;
  }

  .control-stack,
  .switch-stack {
    display: grid;
    gap: 7px;
  }

  .range-control {
    display: grid;
    gap: 3px;
  }

  .range-control span {
    display: flex;
    justify-content: space-between;
    color: #8d9990;
    font-size: 0.57rem;
    font-weight: 800;
    letter-spacing: 0.08em;
  }

  .range-control output {
    color: var(--d2-green);
    font-family: ui-monospace, "Cascadia Mono", Consolas, monospace;
  }

  .range-control input {
    width: 100%;
    min-height: 18px;
    padding: 0;
    accent-color: var(--d2-green);
  }

  fieldset {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 4px;
    margin: 0;
    border: 1px solid #3b483f;
    padding: 6px;
  }

  legend {
    padding: 0 4px;
    color: #7d8a80;
    font-size: 0.55rem;
    font-weight: 800;
    letter-spacing: 0.08em;
    text-transform: uppercase;
  }

  fieldset label {
    display: flex;
    align-items: center;
    gap: 4px;
    color: #a4b0a7;
    font-size: 0.58rem;
    text-transform: uppercase;
  }

  fieldset input {
    width: 12px;
    min-height: auto;
    accent-color: var(--d2-green);
  }

  .algorithm-controls {
    min-height: 83px;
    margin-top: 9px;
    border: 1px solid #3e4a41;
    padding: 8px;
    background: #111712;
  }

  .algorithm-controls > p {
    margin: 12px 4px;
    color: #7f8d82;
    font-size: 0.63rem;
    line-height: 1.45;
  }

  .algorithm-controls > p strong {
    color: var(--d2-green);
  }

  .number-grid {
    display: grid;
    grid-template-columns: minmax(120px, 180px);
    gap: 7px;
  }

  .number-grid.three {
    grid-template-columns: repeat(3, 1fr);
  }

  .number-grid label {
    display: grid;
    gap: 4px;
  }

  .xs5-routing {
    margin-bottom: 7px;
  }

  .control-commit {
    display: flex;
    align-items: center;
    gap: 8px;
    margin-top: 8px;
  }

  .control-commit > span {
    color: #758178;
    font-size: 0.54rem;
  }

  .control-commit > span.dirty {
    color: var(--d2-amber);
  }

  .control-commit button {
    margin-left: auto;
  }

  .seed-row {
    display: grid;
    grid-template-columns: auto minmax(100px, 1fr) auto;
    align-items: center;
    gap: 7px;
    margin-top: 9px;
    border-top: 1px solid #3b473e;
    padding-top: 9px;
  }

  .seed-row input {
    min-width: 0;
  }

  .d2-master-strip {
    display: grid;
    grid-template-columns: minmax(180px, 0.7fr) minmax(250px, 1fr) minmax(
        400px,
        1.5fr
      );
    gap: 7px;
    border-top: 1px solid var(--d2-line-bright);
    padding: 7px;
    background: #151c17;
  }

  .load-module,
  .restart-module,
  .capture-module {
    display: grid;
    grid-template-columns: 1fr;
    align-content: center;
    gap: 5px;
    min-height: 76px;
    border: 1px solid #445148;
    padding: 8px;
    background: #101511;
  }

  .d2-master-strip span {
    color: #7b877e;
    font-size: 0.53rem;
    letter-spacing: 0.08em;
  }

  .d2-master-strip small {
    color: #69756c;
    font-size: 0.57rem;
  }

  .load-button {
    border-color: #7ca372;
    background: linear-gradient(#405e40, #283d2a);
  }

  .restart-module {
    grid-template-columns: 1fr auto;
  }

  .restart-module > span,
  .restart-module > small {
    grid-column: 1;
  }

  .restart-module > button {
    grid-column: 2;
    grid-row: 1 / span 2;
  }

  .capture-module {
    grid-template-columns: 1fr 1fr;
  }

  .capture-module > span,
  .capture-module > small {
    grid-column: 1 / -1;
  }

  .capture-module button:disabled {
    border-style: dashed;
    color: #7d807a;
    opacity: 0.72;
  }

  .d2-spout-strip {
    display: grid;
    grid-template-columns: minmax(180px, 0.9fr) minmax(240px, 1.25fr) auto auto;
    align-items: end;
    gap: 7px;
    border-top: 1px solid var(--d2-line-bright);
    padding: 9px;
    background: #0e1410;
  }

  .d2-spout-heading {
    display: flex;
    align-items: center;
    align-self: center;
    gap: 9px;
  }

  .d2-spout-heading > div,
  .d2-spout-strip label {
    display: grid;
    gap: 4px;
  }

  .d2-spout-heading span,
  .d2-spout-strip label,
  .d2-spout-strip small,
  .d2-spout-strip code {
    color: #7b877e;
    font-size: 0.54rem;
    letter-spacing: 0.08em;
  }

  .d2-spout-heading strong {
    color: #dbe3dc;
    font-size: 0.68rem;
    letter-spacing: 0.04em;
  }

  .d2-spout-lamp {
    width: 9px;
    height: 9px;
    border: 1px solid #566159;
    border-radius: 50%;
    background: #303733;
  }

  .d2-spout-lamp.ready {
    background: var(--d2-amber);
    box-shadow: 0 0 8px rgb(210 181 100 / 42%);
  }

  .d2-spout-lamp.sending {
    background: var(--d2-green);
    box-shadow: 0 0 9px rgb(155 220 136 / 58%);
  }

  .d2-spout-strip input {
    min-width: 0;
    min-height: 31px;
    border: 1px solid #3e4a42;
    padding: 6px 8px;
    background: #111713;
    color: #dbe3dc;
    font:
      0.66rem/1 ui-monospace,
      SFMono-Regular,
      Consolas,
      monospace;
  }

  .d2-spout-strip button {
    min-height: 31px;
    white-space: nowrap;
  }

  .d2-spout-strip button.active {
    border-color: #7ca372;
    background: linear-gradient(#405e40, #283d2a);
  }

  .d2-spout-strip small,
  .d2-spout-strip code {
    grid-column: 1 / -1;
    overflow-wrap: anywhere;
  }

  .d2-spout-strip code {
    color: var(--d2-red);
  }

  @media (max-width: 1120px) {
    .d2-codec-strip {
      grid-template-columns: 1fr 1fr;
    }

    .d2-codec-action {
      grid-template-columns: 1fr 1fr;
    }

    .d2-signal-grid {
      grid-template-columns: 1fr 1fr;
    }

    .operator-module {
      grid-column: 1 / -1;
      grid-row: 2;
    }

    .source-module {
      min-height: 300px;
    }

    .d2-master-strip {
      grid-template-columns: 1fr 1fr;
    }

    .capture-module {
      grid-column: 1 / -1;
    }

    .d2-spout-strip {
      grid-template-columns: 1fr 1fr;
    }
  }
</style>
