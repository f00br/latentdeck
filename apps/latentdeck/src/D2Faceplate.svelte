<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { onMount, tick } from "svelte";
  import {
    d2Client,
    selectD2DecoderAndStatus,
    type StopD2Listener,
  } from "./d2-client";
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
    d2ControlsValidationError,
    setSlotLoop,
    setSlotPlaying,
    parseD2Seed,
    type D2BackendView,
    type D2CaptureView,
    type D2Algorithm,
    type D2Controls,
    type D2ErrorEvent,
    type D2LoadedSources,
    type D2Slot,
    type D2Status,
    type D2Transport,
  } from "./d2-model";
  import { deckCaptureActions, deckCaptureUiPolicy } from "./capture-policy";
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
    createLibraryRefreshController,
    notifyLibraryInvalidated,
  } from "./library-refresh";
  import {
    describeSpout,
    spoutControlsFor,
    type SpoutStatus,
  } from "./output-model";
  import {
    IDLE_DECODED_RECORDING,
    decodedRecordingControls,
    describeDecodedRecording,
    type DecodedRecordingControls,
    type DecodedRecordingStatus,
  } from "./recording-model";
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
    type PresetCartridgeIdentity,
    type PresetLoopDraft,
  } from "./preset-model";
  import {
    LatestValueDispatcher,
    sameControlSnapshot,
  } from "./realtime-controls";
  import {
    currentlyPlayingReadout,
    createDeckSourceTruthState,
    deckSourceResolutionRetryDelay,
    deckSourceDraftDiffers,
    describeCurrentlyPlayingSource,
    markDeckSourceDraftEdited,
    reconcileDeckSourceTruth,
    resolvePlayingSourceView,
    shouldShowNextLoadDraftReadout,
    type DeckSourceTruthState,
  } from "./deck-source-truth";
  import {
    createExclusiveOperationGate,
    playDraftAwareSlot,
    replaceDraftSource,
    retainDraftSourceOptions,
    transportForDraftLoad,
  } from "./source-replacement";
  import {
    canSetDeckFullscreen,
    shouldExitFullscreenForHiddenDeck,
  } from "./deck-fullscreen-policy";
  import type { DeckFullscreenCoordinator } from "./deck-fullscreen-coordinator";
  import {
    EMBEDDED_VIEWPORT_RETRY_DELAYS_MS,
    buildEmbeddedViewportBounds,
    embeddedViewportFullyInsideClient,
    hiddenEmbeddedViewportBounds,
    nextEmbeddedViewportRevision,
    observeEmbeddedViewportReflow,
    sameEmbeddedViewportGeometry,
    type EmbeddedViewportBounds,
  } from "./embedded-viewport";

  export let active = false;
  export let fullscreenCoordinator: DeckFullscreenCoordinator;

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
  let sourceTruth: DeckSourceTruthState = createDeckSourceTruthState();
  let loadedSourceViews: (CartridgeView | null)[] = [];
  let loadedSourceResolutionKey = "";
  let loadedSourceResolutionRequest = 0;
  let loadedSourceResolutionRetryTimer: ReturnType<
    typeof globalThis.setTimeout
  > | null = null;
  let loadedSourceResolutionRetryAttempt = 0;
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
  let sourceReplaceBusy = false;
  let recording: DecodedRecordingStatus = { ...IDLE_DECODED_RECORDING };
  let recordingBusy = false;
  let recordingPending = false;
  let recordingMessage = "";
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
  let controlsDispatchRunning = false;
  let controlsDispatchPending = false;
  let controlsValidation: string | null = null;
  let fullscreenBusy = false;
  let fullscreenStatusPending = false;
  let outputFullscreen: boolean | null = null;
  let fullscreenAutoExitAttempted = false;
  let faceplateRoot: HTMLElement | null = null;
  let viewportAnchor: HTMLDivElement | null = null;
  let viewportFrame: number | null = null;
  let viewportDesired: EmbeddedViewportBounds | null = null;
  let viewportApplied: EmbeddedViewportBounds | null = null;
  let viewportQueued: EmbeddedViewportBounds | null = null;
  let viewportSyncPending = false;
  let viewportRetryTimer: ReturnType<typeof globalThis.setTimeout> | null =
    null;
  let viewportRetryAttempt = 0;
  let viewportMounted = false;
  let viewportEpoch: number | null = null;
  let viewportClientRevision = 0;
  let viewportError = "";
  let viewportReady = false;

  let activeBank: CollectionView | undefined;
  let sourceA: CartridgeView | undefined;
  let sourceB: CartridgeView | undefined;
  let loadedSourceA: CartridgeView | undefined;
  let loadedSourceB: CartridgeView | undefined;
  let sourceOptions: CartridgeView[] = [];
  let presentCount = 0;
  let captureActive = false;
  let recordingControls: DecodedRecordingControls = {
    start: false,
    stop: false,
  };
  let recordingActive = false;
  let recordingStatusText = describeDecodedRecording(recording);
  let spoutControls = spoutControlsFor(null, false);
  let spoutState = describeSpout(null);
  let selectedCompatibilityReasons: readonly string[] = [];
  let selectedSourcesCompatible = false;
  let sourceDraftDiffers = false;
  const sourceReplaceGate = createExclusiveOperationGate((active) => {
    sourceReplaceBusy = active;
  });
  let captureUi = deckCaptureUiPolicy(capture.mode, capture.state);
  let captureActions = deckCaptureActions(capture.mode, capture.state, {
    loaded: status.loaded,
    hostBusy,
    captureBusy,
    controlsDirty,
    controlsDispatchRunning,
    controlsDispatchPending,
    seedDirty,
    rolesDirty: false,
  });
  let realtimeControlsEnabled = true;
  const controlsDispatcher = new LatestValueDispatcher<D2Controls>({
    throttleMs: 75,
    apply: async (controls) => {
      if (d2ControlsValidationError(controls) !== null) return;
      const acknowledgement = await d2Client.controlsSet(controls);
      status = {
        ...status,
        controls: copyD2Controls(acknowledgement.controls),
      };
      hostState = "ready";
      hostMessage = "Realtime controls acknowledged.";
      if (sameControlSnapshot(controlsDraft, acknowledgement.controls)) {
        controlsDirty = false;
      }
    },
    onError: (error) => {
      controlsDirty = true;
      markHostFailure(error);
    },
    onStateChange: ({ running, pending }) => {
      controlsDispatchRunning = running;
      controlsDispatchPending = pending;
    },
  });
  $: activeBank = bankView.collections.find(
    (collection) => collection.id === bankView.deckSession.activeCollectionId,
  );
  $: sourceOptions = mergePresetSourceOptions(
    bankView.cartridges,
    [...presetResolvedSources, ...loadedSourceViews].filter(
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
  $: loadedSourceA =
    status.sources === null
      ? undefined
      : resolvePlayingSourceView(status.sources.sourceA, [
          ...sourceOptions,
          ...loadedSourceViews,
        ]);
  $: loadedSourceB =
    status.sources === null
      ? undefined
      : resolvePlayingSourceView(status.sources.sourceB, [
          ...sourceOptions,
          ...loadedSourceViews,
        ]);
  $: selectedCompatibilityReasons = compatibilityReasons.get(sourceBHash) ?? [];
  $: selectedSourcesCompatible =
    compatibilityReady &&
    sourceA !== undefined &&
    sourceB !== undefined &&
    selectedCompatibilityReasons.length === 0;
  $: sourceDraftDiffers = deckSourceDraftDiffers(
    [sourceAHash, sourceBHash],
    sourceTruth.loadedArchiveSha256s,
  );
  $: presentCount = bankView.cartridges.filter(
    (cartridge) => cartridge.availability === "present",
  ).length;
  $: captureUi = deckCaptureUiPolicy(capture.mode, capture.state);
  $: captureActive = captureUi.active;
  $: recordingActive =
    recording.state === "armed" ||
    recording.state === "recording" ||
    recording.state === "finalizing";
  $: recordingControls = decodedRecordingControls(
    recording,
    status.loaded,
    recordingBusy || captureBusy || captureActive,
  );
  $: recordingStatusText = describeDecodedRecording(recording);
  $: captureActions = deckCaptureActions(capture.mode, capture.state, {
    loaded: status.loaded,
    hostBusy: hostBusy || sourceReplaceBusy,
    captureBusy,
    controlsDirty,
    controlsDispatchRunning,
    controlsDispatchPending,
    seedDirty,
    rolesDirty: false,
  });
  $: realtimeControlsEnabled = captureUi.realtimeControls && !captureBusy;
  $: controlsValidation = d2ControlsValidationError(controlsDraft);
  $: spoutControls = spoutControlsFor(spout, hostBusy || spoutBusy);
  $: spoutState = describeSpout(spout);
  $: viewportReady =
    active && viewportEpoch !== null && viewportApplied?.visible === true;
  $: {
    const deckActive = active;
    if (deckActive) fullscreenAutoExitAttempted = false;
    if (
      shouldExitFullscreenForHiddenDeck(
        deckActive,
        outputFullscreen,
        fullscreenBusy || fullscreenStatusPending,
      ) &&
      !fullscreenAutoExitAttempted
    ) {
      fullscreenAutoExitAttempted = true;
      void setFullscreenState(false);
    } else if (!deckActive && outputFullscreen !== true) {
      outputFullscreen = null;
    }
  }
  $: if (viewportMounted) void syncViewportAfterSurfaceChange(active);
  $: bankRefresh.setActive(active);

  onMount(() => {
    let disposed = false;
    const stopListeners: StopD2Listener[] = [];
    viewportMounted = true;
    const viewportObserver = new ResizeObserver(scheduleViewportSync);
    if (viewportAnchor !== null) viewportObserver.observe(viewportAnchor);
    const disconnectViewportReflow =
      faceplateRoot === null
        ? undefined
        : observeEmbeddedViewportReflow(faceplateRoot, scheduleViewportSync);
    globalThis.addEventListener("resize", scheduleViewportSync);
    globalThis.addEventListener("scroll", scheduleViewportSync, true);
    globalThis.visualViewport?.addEventListener("resize", scheduleViewportSync);
    void (async () => {
      try {
        const session = await d2Client.viewportSessionBegin();
        if (!disposed) {
          viewportEpoch = session.epoch;
          viewportClientRevision = 0;
          viewportDesired = null;
          viewportApplied = null;
          viewportQueued = null;
          clearViewportRetry(true);
          await tick();
          if (!disposed) scheduleViewportSync();
        }
      } catch (error) {
        if (!disposed) {
          viewportError = describeCommandError(error);
          markHostFailure(error);
        }
      }

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
        if (disposed) {
          for (const stop of listeners) stop();
        } else {
          stopListeners.push(...listeners);
        }
      } catch (error) {
        if (!disposed) markHostFailure(error);
      }

      if (!disposed) {
        await refreshBackendStatus();
        await refreshHostStatus();
        await refreshFullscreenStatus();
        await refreshCaptureStatus();
        await refreshRecordingStatus();
        await refreshSpoutStatus();
      }
    })();

    const spoutTimer = globalThis.setInterval(() => {
      if (!disposed) void refreshSpoutStatus();
    }, 250);
    const recordingTimer = globalThis.setInterval(() => {
      if (!disposed) void refreshRecordingStatus(false);
    }, 500);

    return () => {
      disposed = true;
      viewportMounted = false;
      viewportObserver.disconnect();
      disconnectViewportReflow?.();
      globalThis.removeEventListener("resize", scheduleViewportSync);
      globalThis.removeEventListener("scroll", scheduleViewportSync, true);
      globalThis.visualViewport?.removeEventListener(
        "resize",
        scheduleViewportSync,
      );
      if (viewportFrame !== null) {
        globalThis.cancelAnimationFrame(viewportFrame);
        viewportFrame = null;
      }
      clearViewportRetry();
      clearLoadedSourceResolutionRetry();
      viewportQueued = null;
      const epoch = viewportEpoch;
      const revision = nextEmbeddedViewportRevision(viewportClientRevision);
      if (epoch !== null && revision !== null) {
        const hidden = hiddenEmbeddedViewportBounds(
          epoch,
          revision,
          globalThis.devicePixelRatio,
        );
        if (hidden !== null) {
          viewportClientRevision = revision;
          viewportDesired = hidden;
          void d2Client.viewportSetBounds(hidden).catch(() => undefined);
        }
      }
      viewportEpoch = null;
      globalThis.clearInterval(spoutTimer);
      globalThis.clearInterval(recordingTimer);
      bankRefresh.dispose();
      controlsDispatcher.dispose();
      for (const stop of stopListeners) stop();
    };
  });

  function scheduleViewportSync(): void {
    if (!viewportMounted || viewportFrame !== null) return;
    viewportFrame = globalThis.requestAnimationFrame(() => {
      viewportFrame = null;
      measureViewport();
    });
  }

  async function syncViewportAfterSurfaceChange(
    deckActive: boolean,
  ): Promise<void> {
    await tick();
    scheduleViewportSync();
    if (deckActive) await refreshFullscreenStatus();
  }

  function clearViewportRetry(resetAttempt = false): void {
    if (viewportRetryTimer !== null) {
      globalThis.clearTimeout(viewportRetryTimer);
      viewportRetryTimer = null;
    }
    if (resetAttempt) viewportRetryAttempt = 0;
  }

  function scheduleViewportRetry(
    bounds: EmbeddedViewportBounds,
    error: unknown,
  ): void {
    if (
      !viewportMounted ||
      viewportDesired?.epoch !== bounds.epoch ||
      viewportDesired?.revision !== bounds.revision ||
      sameEmbeddedViewportGeometry(viewportApplied, bounds)
    ) {
      return;
    }
    viewportError = describeCommandError(error);
    if (viewportRetryTimer !== null) return;
    const delay = EMBEDDED_VIEWPORT_RETRY_DELAYS_MS[viewportRetryAttempt];
    if (delay === undefined) return;
    viewportRetryAttempt += 1;
    viewportRetryTimer = globalThis.setTimeout(() => {
      viewportRetryTimer = null;
      if (
        !viewportMounted ||
        viewportDesired?.epoch !== bounds.epoch ||
        viewportDesired?.revision !== bounds.revision ||
        sameEmbeddedViewportGeometry(viewportApplied, bounds)
      ) {
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
    const fullyInsideClient = embeddedViewportFullyInsideClient(
      rect,
      document.documentElement.clientWidth,
      document.documentElement.clientHeight,
      scaleFactor,
    );
    const visible =
      active &&
      !document.hidden &&
      anchor.offsetParent !== null &&
      style.display !== "none" &&
      style.visibility !== "hidden" &&
      style.opacity !== "0" &&
      fullyInsideClient;
    const revision = nextEmbeddedViewportRevision(viewportClientRevision);
    if (revision === null) {
      viewportError =
        "LatentDeck exhausted the embedded LD-D2 video-area revision counter.";
      return;
    }
    const bounds =
      visible && rect.width >= 1 && rect.height >= 1
        ? buildEmbeddedViewportBounds(epoch, revision, rect, scaleFactor, true)
        : hiddenEmbeddedViewportBounds(epoch, revision, scaleFactor);
    if (bounds === null) {
      viewportError =
        "LatentDeck could not measure a safe LD-D2 video area. Resize the window and retry.";
      return;
    }
    if (sameEmbeddedViewportGeometry(viewportDesired, bounds)) return;
    viewportClientRevision = revision;
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
      await d2Client.viewportSetBounds(bounds);
      viewportApplied = bounds;
      if (
        viewportDesired?.epoch === bounds.epoch &&
        viewportDesired.revision === bounds.revision
      ) {
        clearViewportRetry(true);
        viewportError = "";
      }
    } catch (error) {
      // A monitor-DPI transition can invalidate the in-flight scale. Re-read
      // the DOM immediately; the bounded retry remains for transient host IO.
      scheduleViewportSync();
      scheduleViewportRetry(bounds, error);
    } finally {
      viewportSyncPending = false;
      if (viewportQueued !== null) void flushViewportSync();
    }
  }

  async function refreshBank(): Promise<void> {
    if (presetBusy) return;
    await bankRefresh.refresh();
  }

  const bankRefresh = createLibraryRefreshController<LibraryView>({
    load: async () => {
      bankBusy = true;
      bankError = "";
      return invoke<LibraryView>("library_snapshot", { search: null });
    },
    apply: async (nextView) => {
      bankView = nextView;
      presetResolvedSources = presetResolvedSources.filter(
        (source) =>
          source !== null &&
          (source.archiveSha256 === sourceAHash ||
            source.archiveSha256 === sourceBHash),
      );
      const availableSources = retainDraftSourceOptions(
        bankView.cartridges,
        [...presetResolvedSources, ...loadedSourceViews],
        currentSourceDraft(),
      );
      const choices =
        sourceTruth.loadedArchiveSha256s !== null &&
        !sourceTruth.draftEditedAfterLoad
          ? { sourceAHash, sourceBHash }
          : chooseD2Sources(availableSources, sourceAHash, sourceBHash);
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
      bankBusy = false;
    },
    onError: (error) => {
      bankError = describeCommandError(error);
      bankBusy = false;
    },
  });

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
      markSourceDraftEdited();
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
            invoke<(CartridgeView | null)[]>("library_resolve_preset_sources", {
              identities,
            }),
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
      const resolution = resolvePresetSources(identities, sourceOptions);
      bankView = incoming;
      presetResolvedSources = globallyResolved;
      [sourceAHash, sourceBHash] = resolution.hashes;
      markSourceDraftEdited();
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
    if (referenceArchiveSha256 === "" || candidateArchiveSha256s.length === 0) {
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
    markSourceDraftEdited();
    await tick();
    await refreshSpatialCompatibility();
  }

  function selectSourceB(event: Event): void {
    if (presetBusy) return;
    discardPresetLoopDraft();
    sourceBHash = (event.currentTarget as HTMLSelectElement).value;
    markSourceDraftEdited();
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

  async function refreshRecordingStatus(reportError = true): Promise<void> {
    if (recordingPending) return;
    recordingPending = true;
    try {
      recording = await d2Client.recordingStatusGet();
      if (recording.state === "failed" && recording.errorCode !== null) {
        recordingMessage = recording.errorCode;
      }
    } catch (error) {
      if (reportError) recordingMessage = describeCommandError(error);
    } finally {
      recordingPending = false;
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

  async function rediscoverBackend(): Promise<void> {
    if (backendBusy || captureBusy || !captureUi.decoder) return;
    backendBusy = true;
    try {
      backend = await d2Client.backendRediscover();
      hostMessage =
        backend.state === "missing"
          ? "No compatible H3 Codec Pack was found."
          : "Codec Pack discovery refreshed.";
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

  async function selectDecoder(): Promise<void> {
    if (backendBusy || captureBusy || !captureUi.decoder) return;
    backendBusy = true;
    try {
      const selection = await selectD2DecoderAndStatus(d2Client);
      backend = selection.backend;
      applyHostStatus(selection.status);
      if (backend.state === "ready") {
        hostMessage = selection.status.loaded
          ? "Decoder selection cancelled · current D2 stream retained."
          : "Decoder ready · load A and B to begin.";
      } else {
        hostState = backend.state === "error" ? "error" : "pending";
        hostMessage =
          backend.detail ?? "No compatible TAEH3 decoder is selected.";
      }
    } catch (error) {
      try {
        applyHostStatus(await d2Client.statusGet());
      } catch {
        // Preserve the picker failure while status remains unavailable.
      }
      const detail = describeCommandError(error);
      backend = {
        ...backend,
        state: "error",
        detail,
      };
      hostState = "error";
      hostMessage = detail;
    } finally {
      backendBusy = false;
    }
  }

  async function openDeck(playSlot: D2Slot | null = null): Promise<void> {
    if (presetBusy || captureBusy || !captureUi.load) return;
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
    if (controlsValidation !== null) {
      hostState = "error";
      hostMessage = controlsValidation;
      return;
    }
    await runHostAction(async () => {
      const pendingLoops = presetLoopDraft?.loops;
      const retainedTransport =
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
      const transport = transportForDraftLoad(
        retainedTransport,
        playSlot,
        setSlotPlaying,
      );
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
    await refreshFullscreenStatus();
  }

  async function useCapturedSource(slot: D2Slot): Promise<void> {
    await sourceReplaceGate.run(async () => {
      if (
        capture.state !== "finished" ||
        capture.cartridgeId === null ||
        capture.archiveSha256 === null
      ) {
        markHostFailure(
          new Error("The finished capture is not available yet."),
        );
        return;
      }
      const identity: PresetCartridgeIdentity = {
        cartridge_id: capture.cartridgeId,
        archive_sha256: capture.archiveSha256,
      };
      try {
        const [captured] = await invoke<(CartridgeView | null)[]>(
          "library_resolve_preset_sources",
          { identities: [identity] },
        );
        if (captured === null || captured === undefined) {
          throw new Error(
            "The captured cartridge is not present in the Library.",
          );
        }
        presetResolvedSources = [
          ...presetResolvedSources.filter(
            (source) => source?.archiveSha256 !== captured.archiveSha256,
          ),
          captured,
        ];
        discardPresetLoopDraft();
        [sourceAHash, sourceBHash] = replaceDraftSource(
          currentSourceDraft(),
          slot === "A" ? 0 : 1,
          captured.archiveSha256,
        );
        markSourceDraftEdited();
        await tick();
        await refreshSpatialCompatibility();
        await tick();
        if (!selectedSourcesCompatible) {
          hostState = "error";
          hostMessage = `Captured source ${slot} is incompatible with the other draft source.`;
          return;
        }
        const expectedHash = captured.archiveSha256;
        await openDeck();
        const loadedSource =
          slot === "A" ? status.sources?.sourceA : status.sources?.sourceB;
        if (status.loaded && loadedSource?.archiveSha256 === expectedHash) {
          hostState = "ready";
          hostMessage = `Capture inserted in ${slot}. The bounded worker restart preserved the other draft source, controls, seed and transport intent; causal state restarts at the replacement boundary.`;
        }
      } catch (error) {
        markHostFailure(error);
      }
    });
  }

  async function applyControls(): Promise<void> {
    if (
      !status.loaded ||
      !realtimeControlsEnabled ||
      controlsValidation !== null
    )
      return;
    controlsDispatcher.push(copyD2Controls(controlsDraft), true);
  }

  async function applySeed(): Promise<void> {
    if (!captureUi.seed || captureBusy) return;
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
    const loadedSource =
      slot === "A" ? status.sources?.sourceA : status.sources?.sourceB;
    const draftArchiveSha256 = slot === "A" ? sourceAHash : sourceBHash;
    await playDraftAwareSlot({
      loadedArchiveSha256: loadedSource?.archiveSha256 ?? "",
      draftArchiveSha256,
      loadDraftAndPlay: () => openDeck(slot),
      toggleCurrent: async () => {
        const playing =
          slot === "A" ? status.transport.playingA : status.transport.playingB;
        await setTransport(setSlotPlaying(status.transport, slot, !playing));
      },
    });
  }

  function slotDraftRequiresLoad(slot: D2Slot): boolean {
    const loadedSource =
      slot === "A" ? status.sources?.sourceA : status.sources?.sourceB;
    const draftArchiveSha256 = slot === "A" ? sourceAHash : sourceBHash;
    return (
      status.loaded &&
      loadedSource !== undefined &&
      loadedSource.archiveSha256 !== draftArchiveSha256
    );
  }

  function draftLoadBlocked(): boolean {
    return (
      hostBusy ||
      sourceReplaceBusy ||
      captureBusy ||
      !captureUi.load ||
      !viewportReady ||
      backend.state !== "ready" ||
      bankBusy ||
      sourceA === undefined ||
      sourceB === undefined ||
      !selectedSourcesCompatible ||
      controlsValidation !== null
    );
  }

  async function toggleLoop(slot: D2Slot, event: Event): Promise<void> {
    const loop = (event.currentTarget as HTMLInputElement).checked;
    discardPresetLoopDraft();
    await setTransport(setSlotLoop(status.transport, slot, loop));
  }

  async function setTransport(transport: D2Transport): Promise<void> {
    if (!captureUi.transport || captureBusy) return;
    await runHostAction(async () => {
      await d2Client.transportSet(transport);
      applyHostStatus(await d2Client.statusGet());
    });
  }

  async function restart(): Promise<void> {
    if (!captureUi.transport || captureBusy) return;
    await runHostAction(async () => {
      resetMessage = "Restarting playback…";
      applyHostStatus(await d2Client.restart());
    });
  }

  async function snapshotCapture(): Promise<void> {
    if (!captureActions.snapshotEnabled) return;
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
    const action = captureActions.liveAction;
    if (action === null) return;
    captureBusy = true;
    try {
      if (action === "stop") {
        applyCaptureStatus(await d2Client.captureLiveStop());
      } else {
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

  async function toggleDecodedRecording(): Promise<void> {
    const action = recordingControls.stop
      ? "stop"
      : recordingControls.start
        ? "start"
        : null;
    if (action === null || recordingBusy) return;
    recordingBusy = true;
    recordingMessage = "";
    try {
      recording =
        action === "stop"
          ? await d2Client.recordingStop()
          : await d2Client.recordingStart();
    } catch (error) {
      recordingMessage = describeCommandError(error);
      await refreshRecordingStatus(false);
    } finally {
      recordingBusy = false;
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

  function currentSourceDraft(): readonly [string, string] {
    return [sourceAHash, sourceBHash];
  }

  function markSourceDraftEdited(): void {
    sourceTruth = markDeckSourceDraftEdited(sourceTruth, currentSourceDraft());
  }

  function reconcileSourceTruth(incoming: D2Status): void {
    const loadedHashes =
      incoming.loaded && incoming.sources !== null
        ? ([
            incoming.sources.sourceA.archiveSha256,
            incoming.sources.sourceB.archiveSha256,
          ] as const)
        : null;
    const reconciliation = reconcileDeckSourceTruth(
      sourceTruth,
      currentSourceDraft(),
      loadedHashes,
    );
    sourceTruth = reconciliation.state;
    [sourceAHash, sourceBHash] = reconciliation.draftArchiveSha256s;
    resolveLoadedSourceViews(incoming.loaded ? incoming.sources : null);
    if (reconciliation.synchronized) {
      void tick().then(() => refreshSpatialCompatibility());
    }
  }

  function clearLoadedSourceResolutionRetry(resetAttempt = false): void {
    if (loadedSourceResolutionRetryTimer !== null) {
      globalThis.clearTimeout(loadedSourceResolutionRetryTimer);
      loadedSourceResolutionRetryTimer = null;
    }
    if (resetAttempt) loadedSourceResolutionRetryAttempt = 0;
  }

  function resolveLoadedSourceViews(
    sources: D2LoadedSources | null,
    retry = false,
  ): void {
    if (sources === null) {
      clearLoadedSourceResolutionRetry(true);
      loadedSourceResolutionKey = "";
      loadedSourceResolutionRequest += 1;
      loadedSourceViews = [];
      return;
    }
    const identities: PresetCartridgeIdentity[] = [
      {
        cartridge_id: sources.sourceA.cartridgeId,
        archive_sha256: sources.sourceA.archiveSha256,
      },
      {
        cartridge_id: sources.sourceB.cartridgeId,
        archive_sha256: sources.sourceB.archiveSha256,
      },
    ];
    const key = JSON.stringify(identities);
    const identityChanged = key !== loadedSourceResolutionKey;
    if (!retry && !identityChanged) return;
    if (identityChanged) clearLoadedSourceResolutionRetry(true);
    loadedSourceResolutionKey = key;
    const request = ++loadedSourceResolutionRequest;

    if (identityChanged) {
      loadedSourceViews = identities.map((identity) => {
        const candidate = sourceOptions.find(
          (source) =>
            source.cartridgeId === identity.cartridge_id &&
            source.archiveSha256 === identity.archive_sha256,
        );
        return candidate ?? null;
      });
    }
    void invoke<(CartridgeView | null)[]>("library_resolve_preset_sources", {
      identities,
    })
      .then((resolved) => {
        if (
          request !== loadedSourceResolutionRequest ||
          key !== loadedSourceResolutionKey
        )
          return;
        loadedSourceViews = resolved;
        clearLoadedSourceResolutionRetry(true);
        void tick().then(() => refreshSpatialCompatibility());
      })
      .catch(() => {
        if (
          request !== loadedSourceResolutionRequest ||
          key !== loadedSourceResolutionKey
        )
          return;
        // Runtime IDs/hashes remain authoritative even when the friendly
        // Library lookup is unavailable. Retry only a bounded number of times.
        const delay = deckSourceResolutionRetryDelay(
          loadedSourceResolutionRetryAttempt,
        );
        if (delay === null) return;
        loadedSourceResolutionRetryAttempt += 1;
        clearLoadedSourceResolutionRetry();
        loadedSourceResolutionRetryTimer = globalThis.setTimeout(() => {
          loadedSourceResolutionRetryTimer = null;
          if (key !== loadedSourceResolutionKey) return;
          resolveLoadedSourceViews(sources, true);
        }, delay);
      });
  }

  function applyHostStatus(incoming: D2Status): void {
    reconcileSourceTruth(incoming);
    status = incoming;
    hostState = "ready";
    hostMessage = incoming.loaded
      ? "D2 ready."
      : "D2 ready · load A and B to begin.";
    if (incoming.pendingReset) {
      resetMessage = "Restarting playback…";
    } else resetMessage = "";
    if (!controlsDirty) controlsDraft = copyD2Controls(incoming.controls);
    if (!seedDirty) seedDraft = String(incoming.seed);
  }

  function applyHostError(incoming: D2ErrorEvent): void {
    hostState = "error";
    hostMessage = incoming.detail;
  }

  function applyCaptureStatus(incoming: D2CaptureView): void {
    capture = incoming;
    if (!deckCaptureUiPolicy(incoming.mode, incoming.state).realtimeControls) {
      controlsDispatcher.cancelPending();
    }
    if (
      incoming.state === "finished" &&
      incoming.captureId !== null &&
      incoming.captureId !== lastImportedCaptureId
    ) {
      lastImportedCaptureId = incoming.captureId;
      notifyLibraryInvalidated();
    }
  }

  function applyCaptureError(incoming: D2ErrorEvent): void {
    capture = {
      ...capture,
      state: "error",
      detail: incoming.detail,
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
    if (presetBusy || !realtimeControlsEnabled) return;
    discardPresetLoopDraft();
    controlsDraft = { ...controlsDraft, algorithm };
    controlsDirty = true;
    queueRealtimeControls();
  }

  function controlsChanged(): void {
    if (!realtimeControlsEnabled) return;
    discardPresetLoopDraft();
    controlsDirty = true;
    queueRealtimeControls();
  }

  function queueRealtimeControls(): void {
    void tick().then(() => {
      if (
        status.loaded &&
        !presetBusy &&
        realtimeControlsEnabled &&
        controlsDirty &&
        d2ControlsValidationError(controlsDraft) === null
      ) {
        controlsDispatcher.push(copyD2Controls(controlsDraft));
      }
    });
  }

  async function refreshFullscreenStatus(): Promise<void> {
    if (!active) {
      if (outputFullscreen !== true) outputFullscreen = null;
      return;
    }
    if (fullscreenBusy || fullscreenStatusPending) return;
    fullscreenStatusPending = true;
    try {
      outputFullscreen = await fullscreenCoordinator.run(() =>
        d2Client.fullscreenStatusGet(),
      );
    } catch (error) {
      // A status failure can mean a partially mutated host with retained
      // recovery state. Keep an Exit route visible instead of assuming the
      // HWND is safely windowed.
      if (outputFullscreen === null) outputFullscreen = true;
      markHostFailure(error);
    } finally {
      fullscreenStatusPending = false;
    }
  }

  async function toggleFullscreen(): Promise<void> {
    if (outputFullscreen === null) return;
    await setFullscreenState(!outputFullscreen);
  }

  async function setFullscreenState(enabled: boolean): Promise<void> {
    if (
      !canSetDeckFullscreen(
        {
          active,
          runtimeLoaded: status.loaded,
          viewportReady,
          busy: fullscreenBusy || fullscreenStatusPending,
          current: outputFullscreen,
        },
        enabled,
      )
    ) {
      return;
    }
    const previous = outputFullscreen;
    fullscreenBusy = true;
    try {
      outputFullscreen = await fullscreenCoordinator.run(() =>
        d2Client.fullscreenSet(enabled),
      );
      await tick();
      scheduleViewportSync();
    } catch (error) {
      markHostFailure(error);
      try {
        outputFullscreen = await fullscreenCoordinator.run(() =>
          d2Client.fullscreenStatusGet(),
        );
      } catch {
        outputFullscreen = enabled || previous === true;
      }
    } finally {
      fullscreenBusy = false;
    }
  }

  function handleWindowKeydown(event: KeyboardEvent): void {
    if (
      event.key !== "Escape" ||
      outputFullscreen !== true ||
      fullscreenBusy ||
      fullscreenStatusPending
    )
      return;
    event.preventDefault();
    void setFullscreenState(false);
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

  function compatibilityReasonsFor(
    cartridge: CartridgeView,
    referenceArchiveSha256: string,
    reasonsByHash: ReadonlyMap<string, readonly string[]>,
  ): readonly string[] {
    return referenceArchiveSha256 === ""
      ? []
      : (reasonsByHash.get(cartridge.archiveSha256) ?? []);
  }

  function compatibilityLabel(
    cartridge: CartridgeView,
    referenceArchiveSha256: string,
    reasonsByHash: ReadonlyMap<string, readonly string[]>,
  ): string {
    const reasons = compatibilityReasonsFor(
      cartridge,
      referenceArchiveSha256,
      reasonsByHash,
    );
    return reasons.length === 0
      ? cartridgeLabel(cartridge)
      : `${cartridgeLabel(cartridge)} · INCOMPATIBLE: ${reasons.join("; ")}`;
  }

  function isIncompatibleCandidate(
    cartridge: CartridgeView,
    referenceArchiveSha256: string,
    ready: boolean,
    reasonsByHash: ReadonlyMap<string, readonly string[]>,
  ): boolean {
    return (
      referenceArchiveSha256 === "" ||
      !ready ||
      compatibilityReasonsFor(cartridge, referenceArchiveSha256, reasonsByHash)
        .length > 0
    );
  }

  function formatBytes(bytes: number): string {
    if (!Number.isFinite(bytes) || bytes < 0) return "invalid size";
    if (bytes < 1024) return `${bytes} B`;
    const mebibytes = bytes / (1024 * 1024);
    return `${mebibytes.toFixed(mebibytes >= 100 ? 0 : 1)} MiB`;
  }
</script>

<svelte:window onkeydown={handleWindowKeydown} />

<section
  bind:this={faceplateRoot}
  class="d2-faceplate"
  class:fullscreen-faceplate={active &&
    outputFullscreen === true &&
    status.loaded}
  aria-labelledby="d2-title"
  aria-busy={presetBusy || captureBusy}
  inert={presetBusy || captureBusy}
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
        <small>{status.loaded ? "STREAM ACTIVE" : "NO STREAM"}</small>
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
        disabled={backendBusy ||
          captureBusy ||
          !captureUi.decoder ||
          backend.packId === null}
        >{backendBusy ? "Checking…" : "Select TAEH3"}</button
      >
      <button
        class="secondary"
        type="button"
        onclick={() => void rediscoverBackend()}
        disabled={backendBusy || captureBusy || !captureUi.decoder}
        >Refresh Codec Pack</button
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
      Portrait and landscape sources are supported at their intrinsic geometry.
      Direct synthesis requires the same compatible spatial grid; LatentDeck
      never performs a hidden resize or re-encode.
    </p>
    <div class="d2-preset-controls">
      <span>DECK PRESET · JSON</span>
      <div>
        <button
          type="button"
          onclick={() => void loadPreset()}
          disabled={presetBusy ||
            bankBusy ||
            backendBusy ||
            hostBusy ||
            captureActive}>Load preset</button
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

  <section class="d2-output-monitor" aria-label="LD-D2 native video output">
    <header>
      <div>
        <span>NATIVE DX12 OUTPUT</span>
        <strong>{status.loaded ? "POST-OPERATOR STREAM" : "STANDBY"}</strong>
      </div>
      <small>
        {spout?.width && spout?.height
          ? `${spout.width}×${spout.height} intrinsic`
          : sourceA === undefined
            ? "Awaiting compatible sources"
            : `${sourceA.decodedWidth}×${sourceA.decodedHeight} intrinsic`}
      </small>
    </header>
    <div class="d2-viewport-frame" class:live={status.loaded}>
      <div
        bind:this={viewportAnchor}
        class="d2-native-viewport-anchor"
        data-native-viewport="d2"
        aria-hidden="true"
      ></div>
      {#if !status.loaded}
        <div class="d2-output-placeholder" aria-live="polite">
          <span>EMBEDDED PRESENTATION</span>
          <strong>Load A + B to start the native output</strong>
          <small>Intrinsic aspect-fit · no hidden resize or re-encode</small>
        </div>
      {/if}
    </div>
    <footer>
      <span>
        {status.loaded
          ? "Embedded stream active"
          : "Native child surface reserved inside LD-D2"}
      </span>
      <small class:error={viewportError !== ""}>
        {viewportError ||
          "Viewport follows resize, scroll and fullscreen changes."}
      </small>
    </footer>
  </section>

  <div class="d2-signal-grid">
    {#if status.loaded && sourceDraftDiffers}<p
        class="d2-next-load-notice"
        role="status"
      >
        NEXT LOAD DRAFT differs from CURRENTLY PLAYING. Runtime playback is
        unchanged until Load A + B or Load + Play succeeds. Either action
        applies the complete A + B draft.
      </p>{/if}
    <section class="source-module source-a" aria-labelledby="source-a-title">
      <header>
        <span>A</span>
        <div>
          <p>NEXT LOAD DRAFT</p>
          <h3 id="source-a-title">Cartridge A</h3>
        </div>
      </header>
      <label for="d2-source-a">Next load cartridge</label>
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
      {#if status.loaded && status.sources !== null}<p
          class="loaded-source-label"
          title={status.sources.sourceA.archiveSha256}
        >
          CURRENTLY PLAYING · {describeCurrentlyPlayingSource(
            status.sources.sourceA,
            loadedSourceA,
          )}{status.sources.sourceA.archiveSha256 !== sourceAHash
            ? " · NEXT LOAD DRAFT DIFFERS"
            : ""}
        </p>{/if}
      {#if status.loaded && status.sources !== null}{@const runtimeReadoutA =
          currentlyPlayingReadout(status.sources.sourceA, loadedSourceA)}
        <div class="source-readout runtime-source-readout">
          <span>CURRENTLY PLAYING · {runtimeReadoutA.codecLabel}</span>
          <strong>{runtimeReadoutA.geometryLabel}</strong>
          <small>{runtimeReadoutA.frameLabel}</small>
          <small>{runtimeReadoutA.latentLabel}</small>
        </div>{:else if status.loaded}<div
          class="source-readout runtime-source-readout"
        >
          <span>CURRENTLY PLAYING</span>
          <strong>RUNTIME STATUS PENDING</strong>
          <small>SOURCE IDENTITY PENDING</small>
        </div>{:else}<div class="source-readout draft-primary-readout">
          <span>NEXT LOAD DRAFT · {sourceA?.codecProfile ?? "NO SOURCE"}</span>
          <strong
            >{sourceA === undefined
              ? "NO DRAFT SOURCE"
              : `${sourceA.decodedWidth}×${sourceA.decodedHeight}`}</strong
          >
          <small
            >{sourceA === undefined
              ? "SELECT A CARTRIDGE"
              : `${sourceA.decodedFrameCount} DECODED FRAMES`}</small
          >
          {#if sourceA !== undefined}<small
              >{describeIntrinsicFormat(sourceA).aspectLabel} · LATENT {describeIntrinsicFormat(
                sourceA,
              ).latentGrid ?? "N/A"}</small
            >{/if}
        </div>{/if}
      {#if status.loaded && status.sources !== null && shouldShowNextLoadDraftReadout(status.sources.sourceA, sourceAHash, sourceA)}<div
          class="draft-source-readout"
        >
          <span
            >NEXT LOAD DRAFT · {sourceA === undefined
              ? "UNRESOLVED"
              : "DIFFERS"}</span
          >
          <strong
            >{sourceA === undefined
              ? "DRAFT UNRESOLVED"
              : `${sourceA.decodedWidth}×${sourceA.decodedHeight}`}</strong
          >
          <small
            >{sourceA === undefined
              ? "CURRENTLY PLAYING IS UNCHANGED"
              : `${sourceA.decodedFrameCount} DECODED FRAMES · ${
                  describeIntrinsicFormat(sourceA).aspectLabel
                }`}</small
          >
        </div>{/if}
      <div class="source-transport">
        <button
          type="button"
          onclick={() => void togglePlaying("A")}
          disabled={slotDraftRequiresLoad("A")
            ? draftLoadBlocked()
            : !status.loaded || hostBusy || captureBusy || !captureUi.transport}
        >
          {slotDraftRequiresLoad("A")
            ? "Load + Play A"
            : !status.loaded
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
            disabled={!status.loaded ||
              hostBusy ||
              captureBusy ||
              !captureUi.transport}
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
            onclick={() => selectAlgorithm(algorithm)}
            disabled={!captureUi.realtimeControls}>{algorithm}</button
          >
        {/each}
      </div>

      <form
        class="control-form"
        oninput={controlsChanged}
        inert={!captureUi.realtimeControls}
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
                disabled={!captureUi.realtimeControls}
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
                disabled={!captureUi.realtimeControls}
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
                disabled={!captureUi.realtimeControls}
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
                disabled={!captureUi.realtimeControls}
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
                  disabled={!captureUi.realtimeControls}
                /> Hybridize</label
              >
              <label
                ><input
                  type="radio"
                  name="d2-mode"
                  value="INTERACT"
                  bind:group={controlsDraft.mode}
                  disabled={!captureUi.realtimeControls}
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
                  disabled={!captureUi.realtimeControls}
                /> Carrier A</label
              >
              <label
                ><input
                  type="radio"
                  name="d2-routing"
                  value="B"
                  bind:group={controlsDraft.routing}
                  disabled={!captureUi.realtimeControls}
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
                  disabled={!captureUi.realtimeControls}
                /></label
              >
              <label
                >Channel B<input
                  type="number"
                  min="0"
                  max="23"
                  step="1"
                  bind:value={controlsDraft.xs1ChannelB}
                  disabled={!captureUi.realtimeControls}
                /></label
              >
              <label
                >Angle °<input
                  type="number"
                  min="-180"
                  max="180"
                  step="1"
                  bind:value={controlsDraft.xs1AngleDegrees}
                  disabled={!captureUi.realtimeControls}
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
                  disabled={!captureUi.realtimeControls}
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
                  disabled={!captureUi.realtimeControls}
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
                  disabled={!captureUi.realtimeControls}
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
                  disabled={!captureUi.realtimeControls}
                /> TOPK</label
              >
              <label
                ><input
                  type="radio"
                  name="xs5-routing"
                  value="SINKHORN"
                  bind:group={controlsDraft.xs5Routing}
                  disabled={!captureUi.realtimeControls}
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
                  disabled={!captureUi.realtimeControls}
                /></label
              >
              <label
                >Top K<input
                  type="number"
                  min="1"
                  max="64"
                  step="1"
                  bind:value={controlsDraft.topK}
                  disabled={!captureUi.realtimeControls}
                /></label
              >
              <label
                >Iterations<input
                  type="number"
                  min="2"
                  max="12"
                  step="1"
                  bind:value={controlsDraft.sinkhornIterations}
                  disabled={!captureUi.realtimeControls}
                /></label
              >
            </div>
          {/if}
        </div>

        {#if controlsValidation !== null}
          <p class="control-validation-error">{controlsValidation}</p>
        {/if}

        <div class="control-commit">
          <span class:dirty={controlsDirty || hostState !== "ready"}
            >{controlsValidation !== null
              ? "INVALID DRAFT"
              : controlsDispatchRunning || controlsDispatchPending
                ? "REALTIME APPLYING"
                : hostState !== "ready"
                  ? "DRAFT · HOST PENDING"
                  : controlsDirty
                    ? "DRAFT CHANGED"
                    : "HOST ACKNOWLEDGED"}</span
          >
          <button
            type="submit"
            disabled={!status.loaded ||
              !controlsDirty ||
              controlsValidation !== null ||
              hostBusy ||
              !captureUi.realtimeControls}>Apply now</button
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
          disabled={!captureUi.seed || captureBusy}
        />
        <button
          type="button"
          onclick={() => void applySeed()}
          disabled={!status.loaded ||
            !seedDirty ||
            hostBusy ||
            captureBusy ||
            !captureUi.seed}>Set seed</button
        >
      </div>
    </section>

    <section class="source-module source-b" aria-labelledby="source-b-title">
      <header>
        <span>B</span>
        <div>
          <p>NEXT LOAD DRAFT</p>
          <h3 id="source-b-title">Cartridge B</h3>
        </div>
      </header>
      <label for="d2-source-b">Next load cartridge</label>
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
              isIncompatibleCandidate(
                cartridge,
                sourceAHash,
                compatibilityReady,
                compatibilityReasons,
              )}
            >{compatibilityLabel(
              cartridge,
              sourceAHash,
              compatibilityReasons,
            )}</option
          >
        {/each}
      </select>
      {#if status.loaded && status.sources !== null}<p
          class="loaded-source-label"
          title={status.sources.sourceB.archiveSha256}
        >
          CURRENTLY PLAYING · {describeCurrentlyPlayingSource(
            status.sources.sourceB,
            loadedSourceB,
          )}{status.sources.sourceB.archiveSha256 !== sourceBHash
            ? " · NEXT LOAD DRAFT DIFFERS"
            : ""}
        </p>{/if}
      {#if selectedCompatibilityReasons.length > 0}<p
          class="draft-compatibility-note"
          role="status"
        >
          NEXT LOAD DRAFT ONLY · B cannot mix with draft A: {selectedCompatibilityReasons.join(
            "; ",
          )}. The currently playing stream is unchanged. Use an explicit Toolkit
          Align/Crop node to create a compatible `.lc`.
        </p>{/if}
      {#if status.loaded && status.sources !== null}{@const runtimeReadoutB =
          currentlyPlayingReadout(status.sources.sourceB, loadedSourceB)}
        <div class="source-readout runtime-source-readout">
          <span>CURRENTLY PLAYING · {runtimeReadoutB.codecLabel}</span>
          <strong>{runtimeReadoutB.geometryLabel}</strong>
          <small>{runtimeReadoutB.frameLabel}</small>
          <small>{runtimeReadoutB.latentLabel}</small>
        </div>{:else if status.loaded}<div
          class="source-readout runtime-source-readout"
        >
          <span>CURRENTLY PLAYING</span>
          <strong>RUNTIME STATUS PENDING</strong>
          <small>SOURCE IDENTITY PENDING</small>
        </div>{:else}<div class="source-readout draft-primary-readout">
          <span>NEXT LOAD DRAFT · {sourceB?.codecProfile ?? "NO SOURCE"}</span>
          <strong
            >{sourceB === undefined
              ? "NO DRAFT SOURCE"
              : `${sourceB.decodedWidth}×${sourceB.decodedHeight}`}</strong
          >
          <small
            >{sourceB === undefined
              ? "SELECT A CARTRIDGE"
              : `${sourceB.decodedFrameCount} DECODED FRAMES`}</small
          >
          {#if sourceB !== undefined}<small
              >{describeIntrinsicFormat(sourceB).aspectLabel} · LATENT {describeIntrinsicFormat(
                sourceB,
              ).latentGrid ?? "N/A"}</small
            >{/if}
        </div>{/if}
      {#if status.loaded && status.sources !== null && shouldShowNextLoadDraftReadout(status.sources.sourceB, sourceBHash, sourceB)}<div
          class="draft-source-readout"
        >
          <span
            >NEXT LOAD DRAFT · {sourceB === undefined
              ? "UNRESOLVED"
              : "DIFFERS"}</span
          >
          <strong
            >{sourceB === undefined
              ? "DRAFT UNRESOLVED"
              : `${sourceB.decodedWidth}×${sourceB.decodedHeight}`}</strong
          >
          <small
            >{sourceB === undefined
              ? "CURRENTLY PLAYING IS UNCHANGED"
              : `${sourceB.decodedFrameCount} DECODED FRAMES · ${
                  describeIntrinsicFormat(sourceB).aspectLabel
                }`}</small
          >
        </div>{/if}
      <div class="source-transport">
        <button
          type="button"
          onclick={() => void togglePlaying("B")}
          disabled={slotDraftRequiresLoad("B")
            ? draftLoadBlocked()
            : !status.loaded || hostBusy || captureBusy || !captureUi.transport}
        >
          {slotDraftRequiresLoad("B")
            ? "Load + Play B"
            : !status.loaded
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
            disabled={!status.loaded ||
              hostBusy ||
              captureBusy ||
              !captureUi.transport}
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
          sourceReplaceBusy ||
          captureBusy ||
          !captureUi.load ||
          !viewportReady ||
          backend.state !== "ready" ||
          bankBusy ||
          sourceA === undefined ||
          sourceB === undefined ||
          !selectedSourcesCompatible ||
          controlsValidation !== null}>Load A + B</button
      >
    </div>
    <div class="restart-module">
      <span>CAUSAL TRANSPORT</span>
      <button
        type="button"
        onclick={() => void restart()}
        disabled={!status.loaded ||
          hostBusy ||
          captureBusy ||
          !captureUi.transport}>Restart both</button
      >
      <small>Restart and loop require a decoder reset barrier.</small>
    </div>
    <div class="capture-module" aria-label="Resampling status">
      <span>POST-OPERATOR RESAMPLE</span>
      <button
        type="button"
        onclick={() => void snapshotCapture()}
        disabled={!captureActions.snapshotEnabled ||
          recordingActive ||
          sourceReplaceBusy}
        title="Capture one complete structural-carrier cycle"
        >{capture.mode === "snapshot" && captureActive
          ? "Snapshot running…"
          : "Snapshot"}</button
      >
      <button
        type="button"
        onclick={() => void toggleLiveCapture()}
        disabled={captureActions.liveAction === null ||
          recordingActive ||
          sourceReplaceBusy}
        title="Record a bounded changing post-operator latent stream"
        >{captureActions.liveAction === "stop"
          ? "Stop Live Capture"
          : capture.mode === "live_capture" && captureActive
            ? "Live stopping…"
            : "Start Live Capture"}</button
      >
      <small
        class:error={capture.state === "error" || capture.state === "aborted"}
        >{captureStatusText()}</small
      >
      {#if capture.state === "finished" && capture.cartridgeId !== null && capture.archiveSha256 !== null}<div
          class="captured-source-actions"
        >
          <button
            type="button"
            onclick={() => void useCapturedSource("A")}
            disabled={hostBusy ||
              bankBusy ||
              presetBusy ||
              captureBusy ||
              sourceReplaceBusy}>Use capture in A</button
          >
          <button
            type="button"
            onclick={() => void useCapturedSource("B")}
            disabled={hostBusy ||
              bankBusy ||
              presetBusy ||
              captureBusy ||
              sourceReplaceBusy}>Use capture in B</button
          >
          <small
            >Explicit source insertion performs one bounded worker restart;
            other draft settings are retained and causal state restarts.</small
          >
        </div>{/if}
    </div>
    <div class="recording-module" aria-label="Decoded MP4 recording">
      <span>DECODED VIDEO</span>
      <button
        class:active={recordingControls.stop}
        type="button"
        onclick={() => void toggleDecodedRecording()}
        disabled={!recordingControls.start && !recordingControls.stop}
        >{recordingBusy
          ? recordingControls.stop
            ? "Finalizing…"
            : "Opening…"
          : recordingControls.stop
            ? "Stop MP4"
            : recording.state === "finalizing"
              ? "Finalizing…"
              : "Record MP4"}</button
      >
      <small class:error={recording.state === "failed"}
        >{recordingMessage || recordingStatusText}</small
      >
      <small>Video-only H.264 · 24 fps · intrinsic decoded pixels</small>
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
    <button
      class:active={outputFullscreen === true}
      aria-pressed={outputFullscreen ?? false}
      type="button"
      disabled={!canSetDeckFullscreen(
        {
          active,
          runtimeLoaded: status.loaded,
          viewportReady,
          busy: fullscreenBusy || fullscreenStatusPending,
          current: outputFullscreen,
        },
        !(outputFullscreen ?? false),
      )}
      onclick={() => void toggleFullscreen()}
      >{fullscreenBusy
        ? "Switching…"
        : outputFullscreen
          ? "Exit fullscreen"
          : "Fullscreen deck output"}</button
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

  .d2-output-monitor {
    position: sticky;
    z-index: 20;
    top: 0;
    display: grid;
    height: clamp(280px, 44vh, 560px);
    min-width: 0;
    min-height: 0;
    grid-template-rows: auto minmax(0, 1fr) auto;
    overflow: hidden;
    border-bottom: 1px solid var(--d2-line-bright);
    background: #030504;
  }

  .d2-output-monitor > header,
  .d2-output-monitor > footer {
    display: flex;
    min-width: 0;
    min-height: 31px;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    padding: 6px 12px;
    background: #0d120e;
    font-family: ui-monospace, "Cascadia Mono", Consolas, monospace;
  }

  .d2-output-monitor > header {
    border-bottom: 1px solid #29332b;
  }

  .d2-output-monitor > footer {
    border-top: 1px solid #29332b;
  }

  .d2-output-monitor > header > div {
    display: flex;
    min-width: 0;
    align-items: center;
    gap: 9px;
  }

  .d2-output-monitor > header span,
  .d2-output-monitor > footer span,
  .d2-output-monitor > header small,
  .d2-output-monitor > footer small {
    overflow: hidden;
    color: #77837a;
    font-size: 0.55rem;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .d2-output-monitor > header strong {
    color: var(--d2-green);
    font-size: 0.62rem;
    letter-spacing: 0.07em;
  }

  .d2-output-monitor > footer small.error {
    color: var(--d2-red);
  }

  .d2-viewport-frame {
    position: relative;
    min-width: 0;
    min-height: 0;
    overflow: hidden;
    background: #000;
    isolation: isolate;
  }

  .d2-native-viewport-anchor {
    position: absolute;
    inset: 0;
    min-width: 0;
    min-height: 0;
    overflow: hidden;
    contain: layout paint;
    background: #000;
  }

  .d2-output-placeholder {
    position: absolute;
    inset: 0;
    z-index: 1;
    display: grid;
    padding: 20px;
    place-content: center;
    place-items: center;
    color: #748178;
    background:
      linear-gradient(rgb(41 54 44 / 28%) 1px, transparent 1px),
      linear-gradient(90deg, rgb(41 54 44 / 28%) 1px, transparent 1px), #050806;
    background-size: 40px 40px;
    pointer-events: none;
    text-align: center;
  }

  .d2-output-placeholder span,
  .d2-output-placeholder small {
    font:
      700 0.56rem/1.35 ui-monospace,
      "Cascadia Mono",
      Consolas,
      monospace;
    letter-spacing: 0.1em;
  }

  .d2-output-placeholder strong {
    margin: 7px 0 5px;
    color: #c8d8ca;
    font-size: clamp(0.88rem, 1.6vw, 1.2rem);
  }

  .d2-faceplate.fullscreen-faceplate {
    position: fixed;
    inset: 0;
    z-index: 1000;
    display: grid;
    width: 100vw;
    height: 100dvh;
    min-height: 0;
    margin: 0;
    grid-template-rows: minmax(0, 1fr);
    overflow: hidden;
    border: 0;
    background: #000;
  }

  .fullscreen-faceplate > :not(.d2-output-monitor) {
    display: none;
  }

  .fullscreen-faceplate .d2-output-monitor {
    position: static;
    height: auto;
    min-height: 0;
    grid-template-rows: minmax(0, 1fr);
    border: 0;
  }

  .fullscreen-faceplate .d2-output-monitor > header,
  .fullscreen-faceplate .d2-output-monitor > footer {
    display: none;
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

  .loaded-source-label {
    min-height: 2.2em;
    margin: 0;
    color: var(--d2-amber);
    font:
      700 0.55rem ui-monospace,
      monospace;
  }

  .draft-compatibility-note {
    margin: 0;
    border-left: 2px solid var(--d2-amber);
    padding: 6px 8px;
    background: #211d10;
    color: #d8c989;
    font-size: 0.61rem;
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

  .d2-next-load-notice {
    grid-column: 1 / -1;
    margin: 0;
    border: 1px solid #665f30;
    padding: 7px 10px;
    background: #211d10;
    color: #d8c989;
    font-size: 0.62rem;
    font-weight: 750;
    letter-spacing: 0.04em;
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

  .source-readout > span {
    color: var(--d2-green);
    font-weight: 800;
    letter-spacing: 0.08em;
  }

  .source-readout strong {
    color: #c8d8ca;
    font-size: 1.05rem;
    font-weight: 500;
  }

  .runtime-source-readout {
    border-color: #4b7559;
    background:
      linear-gradient(135deg, rgb(70 168 102 / 8%), transparent 50%), #0a120d;
  }

  .draft-source-readout {
    display: grid;
    gap: 3px;
    border: 1px dashed #665f30;
    padding: 7px 8px;
    background: #17160e;
    color: #b8ad78;
    font-family: ui-monospace, monospace;
  }

  .draft-source-readout span {
    color: var(--d2-amber);
    font-size: 0.54rem;
    font-weight: 800;
    letter-spacing: 0.07em;
  }

  .draft-source-readout strong {
    color: #d2c993;
    font-size: 0.72rem;
  }

  .draft-source-readout small {
    color: #8c8665;
    font-size: 0.55rem;
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

  .control-validation-error {
    min-height: 30px;
    margin: 8px 0 0;
    border: 1px solid #7b4c45;
    padding: 6px 9px;
    background: #241715;
    color: #ed9a90;
    font-size: 0.63rem;
    line-height: 1.4;
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
    grid-template-columns:
      minmax(170px, 0.65fr) minmax(230px, 0.85fr)
      minmax(380px, 1.35fr) minmax(220px, 0.8fr);
    gap: 7px;
    border-top: 1px solid var(--d2-line-bright);
    padding: 7px;
    background: #151c17;
  }

  .load-module,
  .restart-module,
  .capture-module,
  .recording-module {
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

  .captured-source-actions {
    display: grid;
    grid-column: 1 / -1;
    grid-template-columns: 1fr 1fr;
    gap: 5px;
  }

  .captured-source-actions small {
    grid-column: 1 / -1;
  }

  .capture-module button:disabled {
    border-style: dashed;
    color: #7d807a;
    opacity: 0.72;
  }

  .recording-module {
    grid-template-columns: 1fr;
  }

  .recording-module > span,
  .recording-module > small {
    grid-column: 1 / -1;
  }

  .recording-module button.active {
    border-color: #c28672;
    background: linear-gradient(#6a3d32, #42251f);
  }

  .d2-spout-strip {
    display: grid;
    grid-template-columns:
      minmax(180px, 0.9fr) minmax(240px, 1.25fr)
      auto auto auto;
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
    .d2-bank-strip {
      grid-template-columns: minmax(0, 1fr) minmax(0, 1fr);
    }

    .d2-bank-strip > * {
      min-width: 0;
    }

    .d2-bank-strip select {
      width: 100%;
      min-width: 0;
    }

    .d2-preset-controls {
      grid-column: 1 / -1;
    }

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

    .recording-module {
      grid-column: 1 / -1;
    }

    .d2-spout-strip {
      grid-template-columns: 1fr 1fr;
    }
  }
</style>
