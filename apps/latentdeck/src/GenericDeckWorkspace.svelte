<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { onMount } from "svelte";

  import DeckFaceplateRenderer from "./DeckFaceplateRenderer.svelte";
  import {
    buildEmbeddedViewportBounds,
    embeddedViewportFullyInsideClient,
    hiddenEmbeddedViewportBounds,
    nextEmbeddedViewportRevision,
    sameEmbeddedViewportGeometry,
    type EmbeddedViewportBounds,
  } from "./embedded-viewport";
  import {
    EMPTY_EXTENSIONS_SNAPSHOT,
    compatibilityReasonLabel,
    type ExtensionsSnapshot,
  } from "./extension-manager-model";
  import {
    genericDeckClient,
    type GenericCaptureView,
    type GenericDeckRuntimeView,
    type GenericDeckSessionView,
    type GenericDeckSessionsView,
    type GenericDevice,
    type GenericProfileKey,
    type GenericRecordingView,
    type GenericRuntimeOptions,
  } from "./generic-deck-client";
  import {
    MAX_WARM_DECK_SESSIONS,
    buildGenericControlBindings,
    buildGenericDeckOpenDraft,
    buildGenericDeckPreset,
    codecOptionsForExactDeck,
    exactPackageKey,
    genericDeckDraftFromSessionSnapshot,
    genericDeckDraftFromPreset,
    retainExactSelection,
    sessionCapacityState,
    type GenericCodecOption,
    type GenericControlBinding,
    type GenericDeckSourceIdentity,
  } from "./generic-deck-model";
  import {
    createDeckUiDraft,
    type DeckUiDraft,
    type DeckUiModel,
    type DeckUiScalar,
  } from "./deck-ui-model";
  import {
    describeCommandError,
    shortHash,
    type CartridgeView,
    type LibraryView,
  } from "./library-model";
  import {
    describeSpout,
    spoutControlsFor,
    type SpoutStatus,
  } from "./output-model";
  import { handleDeckFullscreenKeydown } from "./deck-fullscreen-policy";
  import {
    IDLE_DECODED_RECORDING,
    decodedRecordingControls,
    describeDecodedRecording,
  } from "./recording-model";
  import {
    LatestValueDispatcher,
    sameControlSnapshot,
    type LatestValueDispatchState,
  } from "./realtime-controls";
  import {
    presetSlotIdentities,
    stagePresetLibraryLoad,
    type DeckPreset,
  } from "./preset-model";
  import {
    replaceDraftSource,
    retainDraftSourceOptions,
    selectedSourceAspectWarning,
  } from "./source-replacement";

  export let model: DeckUiModel;
  export let models: readonly DeckUiModel[];
  export let library: LibraryView;
  export let active = false;
  export let onSelectDeck: (exactKey: string) => void | Promise<void> = () =>
    undefined;
  export let onLibraryChanged: (next: LibraryView) => void = () => undefined;
  export let registerLeave: (leave: () => Promise<void>) => void = () =>
    undefined;

  const EMPTY_SESSIONS: GenericDeckSessionsView = {
    sessions: [],
    foregroundOutput: null,
    outputPin: null,
    recentFaults: [],
  };
  const PROFILE_SEPARATOR = "\u0000";
  const VIEWPORT_RETRY_DELAYS_MS = [100, 250, 500] as const;
  const REALTIME_CONTROL_THROTTLE_MS = 75;

  interface RealtimeControlsCommand {
    sessionId: string;
    epoch: number;
    controls: GenericControlBinding[];
    snapshot: Record<string, DeckUiScalar>;
  }

  let extensions: ExtensionsSnapshot = EMPTY_EXTENSIONS_SNAPSHOT;
  let selectedCodecKey = "";
  let selectedDevice: GenericDevice | "" = "";
  let deviceOrdinal = 0;
  let selectedProfileKey = "";
  let discovery: GenericRuntimeOptions | null = null;
  let runtimeOptions: GenericRuntimeOptions | null = null;
  let sessions: GenericDeckSessionsView = EMPTY_SESSIONS;
  let selectedSessionId = "";
  let hydratedSessionId = "";
  let sessionSnapshotValid = false;
  let draft = createDeckUiDraft(model);
  let observedLibrary = library;
  let libraryView = library;
  let sourceCartridges: CartridgeView[] = [...library.cartridges];
  let completedCaptureSource: CartridgeView | null = null;
  let draftRevision = 0;
  let configuredDeckKey = model.exactKey;
  let busy = false;
  let matrixBusy = false;
  let message = "Choose an exact Codec version, device, and profile.";
  let errorCode = "";
  let presetBusy = false;
  let presetMessage = "Preset v2 uses a native open/save dialog.";
  let capture: GenericCaptureView | null = null;
  let recording: GenericRecordingView | null = null;
  let spout: SpoutStatus | null = null;
  let spoutName = "LatentDeck Generic Output";
  let spoutNameDirty = false;
  let spoutStatusKnown = false;
  let outputFullscreen: boolean | null = null;
  let viewportAnchor: HTMLDivElement | null = null;
  let viewportEpoch: number | null = null;
  let viewportRevision = 0;
  let viewportFrame: number | null = null;
  let viewportApplied: EmbeddedViewportBounds | null = null;
  let viewportBusy = false;
  let viewportSyncRunning = false;
  let viewportSyncPending = false;
  let viewportResizeObserver: ResizeObserver | null = null;
  let viewportRetryAttempt = 0;
  let viewportRetryTimer: ReturnType<typeof globalThis.setTimeout> | null =
    null;
  let viewportRetryExhausted = false;
  let viewportFailureCode = "";
  let viewportSuspended = false;
  let viewportObservedActive = active;
  let observedModels = models;
  let extensionsRefreshDeferred = false;
  let publishedCaptureKey = "";
  let extensionsRefreshPending: Promise<void> | null = null;
  let extensionsRefreshRevision = 0;
  let extensionsAppliedRevision = 0;
  let controlsEpoch = 0;
  let controlsDispatchRunning = false;
  let controlsDispatchPending = false;
  let acknowledgedControls: Record<string, DeckUiScalar> | null = null;
  let controlsDispatcher = createControlsDispatcher(controlsEpoch);
  let sessionLifecycleEpoch = 0;
  let sessionPollToken: {
    epoch: number;
    sessionId: string;
    deckKey: string;
  } | null = null;
  let sessionsRequestRevision = 0;
  let closingSessionIds = new Set<string>();

  let codecOptions: GenericCodecOption[] = [];
  let selectedCodec: GenericCodecOption | undefined;
  let profiles: readonly GenericProfileKey[] = [];
  let selectedProfile: GenericProfileKey | undefined;
  let selectedSession: GenericDeckSessionView | undefined;
  let selectedSessionReady = false;
  let playheads: number[] = [];
  let sourceOptions: Array<{
    archiveSha256: string;
    label: string;
    detail?: string;
    available: boolean;
    incompatibilityReason?: string;
  }> = [];
  let runtimeAvailable = false;
  let runtimeUnavailableReason = "Choose an exact compatible Codec version.";
  let sessionCapacityAvailable = true;
  let loadAvailable = false;
  let loadUnavailableReason = "The native output viewport is not ready.";
  let captureAvailable = false;
  let captureStartAvailable = false;
  let captureUnavailableReason = "Load and foreground an exact session first.";
  let recordingAvailable = false;
  let captureIsActive = false;
  let recordingIsActive = false;
  let controlsDraftDirty = false;
  let controlsUnsettled = false;
  let capturedSourceAvailable = false;
  let captureReuseAvailable = false;
  let sourceGeometryWarning = "";

  $: codecOptions = codecOptionsForExactDeck(model.exactKey, extensions.matrix);
  $: selectedCodec = codecOptions.find(
    (option) => option.exactKey === selectedCodecKey,
  );
  $: profiles = discovery?.profiles ?? [];
  $: selectedProfile = profiles.find(
    (profile) => profileKey(profile) === selectedProfileKey,
  );
  $: selectedSession = sessions.sessions.find(
    (session) => session.sessionId === selectedSessionId,
  );
  $: selectedSessionReady =
    selectedSession !== undefined &&
    selectedSession.sessionId === hydratedSessionId &&
    sessionSnapshotValid;
  $: if (
    selectedSession !== undefined &&
    selectedSession.sessionId !== hydratedSessionId &&
    exactPackageKey(
      selectedSession.deck.packageId,
      selectedSession.deck.packageVersion,
    ) === model.exactKey
  ) {
    hydrateSelectedSession(selectedSession);
  }
  $: playheads = playheadsFor(model, selectedSession);
  $: sourceOptions = sourceCartridges.map((cartridge) =>
    sourceOption(cartridge, selectedProfile, runtimeOptions),
  );
  $: sourceGeometryWarning = selectedSourceAspectWarning(
    draft.sourceArchiveSha256s,
    sourceCartridges,
  );
  $: runtimeAvailable = exactRuntimeAvailable(
    selectedCodec,
    selectedProfile,
    runtimeOptions,
  );
  $: runtimeUnavailableReason = describeRuntimeAvailability(
    selectedCodec,
    selectedDevice,
    discovery,
    selectedProfile,
    runtimeOptions,
  );
  $: sessionCapacityAvailable = sessionCapacityState(
    sessions.sessions.length,
  ).canOpen;
  $: loadAvailable =
    sessionCapacityAvailable && viewportApplied?.visible === true;
  $: loadUnavailableReason = !sessionCapacityAvailable
    ? `${SESSION_CAPACITY_CODE}: close one of the four warm sessions explicitly.`
    : "LatentDeck is waiting for the visible embedded video area.";
  $: captureIsActive = captureActive(capture);
  $: recordingIsActive = recordingActive(recording);
  $: captureAvailable =
    selectedSessionReady &&
    selectedSession?.foreground === true &&
    selectedSession.runtime.faultCode === null &&
    !recordingIsActive &&
    model.requiredCapabilities.includes("snapshot_capture") &&
    model.requiredCapabilities.includes("live_capture");
  $: controlsDraftDirty =
    selectedSessionReady &&
    (acknowledgedControls === null ||
      !sameControlSnapshot(draft.controls, acknowledgedControls));
  $: controlsUnsettled =
    controlsDraftDirty || controlsDispatchRunning || controlsDispatchPending;
  $: capturedSourceAvailable =
    completedCaptureSource?.availability === "present" &&
    selectedSessionReady &&
    selectedSession?.foreground === true;
  $: captureReuseAvailable =
    capturedSourceAvailable &&
    !captureIsActive &&
    (sessions.outputPin?.session_id !== selectedSessionId ||
      sessions.outputPin.kind !== "capture") &&
    !controlsUnsettled;
  $: captureStartAvailable = captureAvailable && !controlsUnsettled;
  $: captureUnavailableReason = recordingIsActive
    ? "MP4 recording pins the foreground output lease."
    : controlsUnsettled
      ? "Wait for the latest realtime controls to reach the runtime before starting capture."
      : selectedSession?.foreground !== true
        ? "Capture requires this exact session to own the foreground output lease."
        : "This exact Deck and Codec profile does not expose both capture capabilities.";
  $: recordingAvailable =
    selectedSessionReady &&
    selectedSession?.foreground === true &&
    selectedSession.runtime.faultCode === null &&
    !captureIsActive;
  $: if (model.exactKey !== configuredDeckKey) resetForExactDeck();
  $: if (library !== observedLibrary) syncLibraryView(library);
  $: if (models !== observedModels) {
    observedModels = models;
    if (active) {
      extensionsRefreshDeferred = false;
      void refreshExtensions();
    } else extensionsRefreshDeferred = true;
  }
  $: if (active !== viewportObservedActive) {
    viewportObservedActive = active;
    if (active) {
      if (extensionsRefreshDeferred) {
        extensionsRefreshDeferred = false;
        void refreshExtensions();
      }
      viewportSuspended = false;
      viewportRetryAttempt = 0;
      viewportRetryExhausted = false;
    } else invalidateSessionLifecycle();
  }
  $: if (active && !viewportSuspended && viewportAnchor !== null) {
    if (viewportEpoch === null) {
      if (viewportRetryTimer === null && !viewportRetryExhausted)
        void establishViewport();
    } else scheduleViewportSync();
  }

  onMount(() => {
    registerLeave(leaveSurface);
    void refreshExtensions();
    void refreshSessions();
    viewportResizeObserver = new ResizeObserver(() =>
      requestViewportRecovery(),
    );
    if (viewportAnchor !== null) viewportResizeObserver.observe(viewportAnchor);
    void establishViewport();
    const resize = () => requestViewportRecovery();
    globalThis.addEventListener("resize", resize);
    globalThis.addEventListener("scroll", resize, true);
    const poll = globalThis.setInterval(() => {
      if (active) void pollForegroundState();
    }, 500);
    return () => {
      disposeControlsDispatcher();
      invalidateSessionLifecycle();
      viewportSuspended = true;
      registerLeave(async () => undefined);
      globalThis.clearInterval(poll);
      globalThis.removeEventListener("resize", resize);
      globalThis.removeEventListener("scroll", resize, true);
      viewportResizeObserver?.disconnect();
      viewportResizeObserver = null;
      cancelViewportRetry();
      cancelViewportSyncSchedule();
      void hideViewport();
    };
  });

  function resetForExactDeck(): void {
    invalidateSessionLifecycle();
    resetControlsScope();
    viewportSuspended = false;
    viewportRetryAttempt = 0;
    viewportRetryExhausted = false;
    configuredDeckKey = model.exactKey;
    selectedCodecKey = "";
    selectedDevice = "";
    selectedProfileKey = "";
    discovery = null;
    runtimeOptions = null;
    capture = null;
    recording = null;
    spout = null;
    spoutStatusKnown = false;
    outputFullscreen = null;
    draft = createDeckUiDraft(model);
    draftRevision += 1;
    hydratedSessionId = "";
    sessionSnapshotValid = false;
    const foreground = sessions.sessions.find(
      (session) =>
        session.foreground &&
        exactPackageKey(session.deck.packageId, session.deck.packageVersion) ===
          model.exactKey,
    );
    selectedSessionId = foreground?.sessionId ?? "";
    message = "Choose an exact Codec version, device, and profile.";
    errorCode = "";
    void hideViewport();
  }

  function syncLibraryView(next: LibraryView): void {
    observedLibrary = next;
    libraryView = next;
    const retainedHashes = [
      ...draft.sourceArchiveSha256s,
      ...(completedCaptureSource === null
        ? []
        : [completedCaptureSource.archiveSha256]),
    ];
    sourceCartridges = retainDraftSourceOptions(
      next.cartridges,
      [
        ...sourceCartridges,
        ...(completedCaptureSource === null ? [] : [completedCaptureSource]),
      ],
      retainedHashes,
    );
  }

  function acceptLibrarySnapshot(
    next: LibraryView,
    resolvedSources: readonly (CartridgeView | null)[] = [],
    additionallyRetained: readonly string[] = [],
  ): void {
    observedLibrary = next;
    libraryView = next;
    sourceCartridges = retainDraftSourceOptions(
      next.cartridges,
      [...sourceCartridges, ...resolvedSources],
      [...draft.sourceArchiveSha256s, ...additionallyRetained],
    );
    onLibraryChanged(next);
  }

  function availableSourceIdentities(
    cartridges: readonly CartridgeView[] = sourceCartridges,
  ): GenericDeckSourceIdentity[] {
    return cartridges
      .filter((cartridge) => cartridge.availability === "present")
      .map((cartridge) => ({
        cartridgeId: cartridge.cartridgeId,
        archiveSha256: cartridge.archiveSha256,
      }));
  }

  function refreshExtensions(): Promise<void> {
    extensionsRefreshRevision += 1;
    if (extensionsRefreshPending !== null) return extensionsRefreshPending;
    matrixBusy = true;
    const refresh = (async () => {
      while (extensionsAppliedRevision < extensionsRefreshRevision) {
        const revision = extensionsRefreshRevision;
        try {
          const next = await invoke<ExtensionsSnapshot>("extensions_snapshot");
          extensionsAppliedRevision = revision;
          if (revision !== extensionsRefreshRevision) continue;
          extensions = next;
          selectedCodecKey = retainExactSelection(
            selectedCodecKey,
            codecOptionsForExactDeck(model.exactKey, extensions.matrix).map(
              (option) => option.exactKey,
            ),
          );
        } catch (error) {
          extensionsAppliedRevision = revision;
          if (revision === extensionsRefreshRevision) fail(error);
        }
      }
    })();
    extensionsRefreshPending = refresh.finally(() => {
      extensionsRefreshPending = null;
      matrixBusy = false;
    });
    return extensionsRefreshPending;
  }

  async function selectCodec(event: Event): Promise<void> {
    const codecKey = (event.currentTarget as HTMLSelectElement).value;
    selectedCodecKey = codecKey;
    selectedProfileKey = "";
    discovery = null;
    runtimeOptions = null;
    const codec = codecOptions.find((option) => option.exactKey === codecKey);
    if (codec?.reason !== "compatible") {
      message =
        codec === undefined
          ? "Choose an exact Codec version."
          : compatibilityReasonLabel(codec.reason);
      return;
    }
    await discoverRuntime(codec, selectedDevice);
  }

  async function selectDevice(event: Event): Promise<void> {
    const device = (event.currentTarget as HTMLSelectElement).value as
      GenericDevice | "";
    selectedDevice = device;
    selectedProfileKey = "";
    discovery = null;
    runtimeOptions = null;
    await discoverRuntime(selectedCodec, device);
  }

  async function rediscoverRuntimeAfterOrdinalChange(): Promise<void> {
    const codec = selectedCodec;
    const device = selectedDevice;
    const retainedProfileKey = selectedProfileKey;
    discovery = null;
    runtimeOptions = null;
    const nextDiscovery = await discoverRuntime(codec, device);
    if (retainedProfileKey === "" || nextDiscovery === null) return;
    const retainedProfile = nextDiscovery.profiles.find(
      (candidate) => profileKey(candidate) === retainedProfileKey,
    );
    if (retainedProfile !== undefined) {
      await refreshSourceEligibility(codec, retainedProfile, device);
    }
  }

  async function discoverRuntime(
    codec: GenericCodecOption | undefined = selectedCodec,
    device: GenericDevice | "" = selectedDevice,
  ): Promise<GenericRuntimeOptions | null> {
    if (codec?.reason !== "compatible" || device === "" || busy) {
      return null;
    }
    let nextDiscovery: GenericRuntimeOptions | null = null;
    await run(async () => {
      const next = await genericDeckClient.runtimeOptions({
        deckId: model.deckId,
        deckVersion: model.deckVersion,
        codecId: codec.codecId,
        codecVersion: codec.codecVersion,
        profileKey: null,
        device,
        deviceOrdinal,
        sources: [],
        selectedSources: [],
      });
      nextDiscovery = next;
      discovery = next;
      selectedProfileKey = retainExactSelection(
        selectedProfileKey,
        next.profiles.map(profileKey),
      );
      runtimeOptions = null;
      message =
        next.reason === "compatible"
          ? "Choose one exact compatible Codec profile."
          : compatibilityReasonLabel(next.reason);
    });
    return nextDiscovery;
  }

  async function selectProfile(event: Event): Promise<void> {
    const nextProfileKey = (event.currentTarget as HTMLSelectElement).value;
    selectedProfileKey = nextProfileKey;
    const profile = profiles.find(
      (candidate) => profileKey(candidate) === nextProfileKey,
    );
    await refreshSourceEligibility(selectedCodec, profile, selectedDevice);
  }

  async function refreshSourceEligibility(
    codec: GenericCodecOption | undefined = selectedCodec,
    profile: GenericProfileKey | undefined = selectedProfile,
    device: GenericDevice | "" = selectedDevice,
    selectedSources: GenericDeckSourceIdentity[] = selectedSourceSet(draft),
  ): Promise<void> {
    if (codec === undefined || profile === undefined || device === "" || busy) {
      runtimeOptions = null;
      return;
    }
    await run(async () => {
      runtimeOptions = await genericDeckClient.runtimeOptions({
        deckId: model.deckId,
        deckVersion: model.deckVersion,
        codecId: codec.codecId,
        codecVersion: codec.codecVersion,
        profileKey: profile,
        device,
        deviceOrdinal,
        sources: availableSourceIdentities(),
        selectedSources,
      });
      message =
        runtimeOptions.reason === "compatible"
          ? "Exact runtime preflight complete."
          : compatibilityReasonLabel(runtimeOptions.reason);
    });
  }

  async function selectExternalAsset(assetId: string): Promise<void> {
    if (selectedCodec === undefined) return;
    await run(async () => {
      await genericDeckClient.externalAssetSelect(
        selectedCodec.codecId,
        selectedCodec.codecVersion,
        assetId,
      );
      await refreshRuntimeOptionsInsideAction();
    });
  }

  async function clearExternalAsset(assetId: string): Promise<void> {
    if (selectedCodec === undefined) return;
    await run(async () => {
      await genericDeckClient.externalAssetClear(
        selectedCodec.codecId,
        selectedCodec.codecVersion,
        assetId,
      );
      await refreshRuntimeOptionsInsideAction();
    });
  }

  async function refreshRuntimeOptionsInsideAction(): Promise<void> {
    const codec = selectedCodec;
    const profile = selectedProfile;
    const device = selectedDevice;
    if (codec === undefined || device === "") return;
    const next = await genericDeckClient.runtimeOptions({
      deckId: model.deckId,
      deckVersion: model.deckVersion,
      codecId: codec.codecId,
      codecVersion: codec.codecVersion,
      profileKey: profile ?? null,
      device,
      deviceOrdinal,
      sources: profile === undefined ? [] : availableSourceIdentities(),
      selectedSources: profile === undefined ? [] : selectedSourceSet(draft),
    });
    if (profile === undefined) {
      discovery = next;
      runtimeOptions = null;
      message =
        next.reason === "compatible"
          ? "Choose one exact compatible Codec profile."
          : compatibilityReasonLabel(next.reason);
      return;
    }
    runtimeOptions = next;
    message =
      runtimeOptions.reason === "compatible"
        ? "Exact runtime preflight complete."
        : compatibilityReasonLabel(runtimeOptions.reason);
  }

  function sourceOption(
    cartridge: CartridgeView,
    profile: GenericProfileKey | undefined,
    options: GenericRuntimeOptions | null,
  ) {
    const eligibility = options?.sources.find(
      (candidate) =>
        candidate.archiveSha256 === cartridge.archiveSha256 &&
        candidate.cartridgeId === cartridge.cartridgeId,
    );
    const incompatibilityReason =
      profile === undefined
        ? "Select an exact Codec profile"
        : eligibility === undefined
          ? "Host source preflight unavailable"
          : eligibility.reason === "compatible"
            ? undefined
            : compatibilityReasonLabel(eligibility.reason);
    return {
      archiveSha256: cartridge.archiveSha256,
      label: `${cartridge.paths[0]?.fileName ?? cartridge.cartridgeId} · ${shortHash(cartridge.archiveSha256)}`,
      detail: `${cartridge.signalPresentation.decoded_width}×${cartridge.signalPresentation.decoded_height} · ${cartridge.signalPresentation.aspect_ratio.width}:${cartridge.signalPresentation.aspect_ratio.height}`,
      available: cartridge.availability === "present",
      ...(incompatibilityReason === undefined ? {} : { incompatibilityReason }),
    };
  }

  function selectedSourceSet(
    value: DeckUiDraft,
    cartridges: readonly CartridgeView[] = sourceCartridges,
  ): GenericDeckSourceIdentity[] {
    if (
      value.sourceArchiveSha256s.length !== model.slots ||
      value.sourceArchiveSha256s.some((archiveSha256) => archiveSha256 === "")
    ) {
      return [];
    }
    const selected = value.sourceArchiveSha256s.map((archiveSha256) =>
      cartridges.find(
        (cartridge) =>
          cartridge.archiveSha256 === archiveSha256 &&
          cartridge.availability === "present",
      ),
    );
    if (selected.some((cartridge) => cartridge === undefined)) return [];
    return selected.map((cartridge) => ({
      cartridgeId: cartridge!.cartridgeId,
      archiveSha256: cartridge!.archiveSha256,
    }));
  }

  function createControlsDispatcher(
    epoch: number,
  ): LatestValueDispatcher<RealtimeControlsCommand> {
    return new LatestValueDispatcher<RealtimeControlsCommand>({
      throttleMs: REALTIME_CONTROL_THROTTLE_MS,
      apply: applyRealtimeControls,
      onError: fail,
      onStateChange: (state: LatestValueDispatchState) => {
        if (epoch !== controlsEpoch) return;
        controlsDispatchRunning = state.running;
        controlsDispatchPending = state.pending;
      },
    });
  }

  function resetControlsScope(): void {
    controlsEpoch += 1;
    controlsDispatcher.dispose();
    controlsDispatchRunning = false;
    controlsDispatchPending = false;
    acknowledgedControls = null;
    controlsDispatcher = createControlsDispatcher(controlsEpoch);
  }

  function disposeControlsDispatcher(): void {
    controlsEpoch += 1;
    controlsDispatcher.dispose();
    controlsDispatchRunning = false;
    controlsDispatchPending = false;
    acknowledgedControls = null;
  }

  function controlsCommandIsCurrent(command: RealtimeControlsCommand): boolean {
    return (
      command.epoch === controlsEpoch &&
      command.sessionId === selectedSessionId &&
      command.sessionId === hydratedSessionId &&
      sessionSnapshotValid
    );
  }

  async function applyRealtimeControls(
    command: RealtimeControlsCommand,
  ): Promise<void> {
    try {
      const next = await genericDeckClient.controlsSet(
        command.sessionId,
        command.controls,
      );
      if (!controlsCommandIsCurrent(command)) return;
      applyRuntimeForSession(command.sessionId, next);
      acknowledgedControls = { ...command.snapshot };
    } catch (error) {
      if (controlsCommandIsCurrent(command)) throw error;
    }
  }

  function queueControls(
    controls: Record<string, DeckUiScalar>,
    immediate: boolean,
  ): void {
    const sessionId = selectedSession?.sessionId;
    if (!selectedSessionReady || sessionId === undefined) return;
    controlsDispatcher.push(
      {
        sessionId,
        epoch: controlsEpoch,
        controls: buildGenericControlBindings(model, controls),
        snapshot: { ...controls },
      },
      immediate,
    );
  }

  function updateDraft(next: DeckUiDraft): void {
    const sourcesChanged = next.sourceArchiveSha256s.some(
      (archiveSha256, index) =>
        archiveSha256 !== draft.sourceArchiveSha256s[index],
    );
    draft = next;
    if (sourcesChanged) {
      void refreshSourceEligibility(
        selectedCodec,
        selectedProfile,
        selectedDevice,
        selectedSourceSet(next),
      );
    }
  }

  function exactRuntimeAvailable(
    codec: GenericCodecOption | undefined,
    profile: GenericProfileKey | undefined,
    options: GenericRuntimeOptions | null,
  ): boolean {
    if (
      codec?.reason !== "compatible" ||
      profile === undefined ||
      options?.reason !== "compatible"
    ) {
      return false;
    }
    return options.externalAssets.every(
      (asset) => !asset.required || asset.bound,
    );
  }

  function describeRuntimeAvailability(
    codec: GenericCodecOption | undefined,
    device: GenericDevice | "",
    runtimeDiscovery: GenericRuntimeOptions | null,
    profile: GenericProfileKey | undefined,
    options: GenericRuntimeOptions | null,
  ): string {
    if (codec === undefined) return "Choose an exact Codec version.";
    if (codec.reason !== "compatible") {
      return compatibilityReasonLabel(codec.reason);
    }
    if (device === "") return "Choose the negotiated runtime device.";
    if (runtimeDiscovery === null)
      return "Exact Codec discovery has not completed.";
    if (profile === undefined) {
      return runtimeDiscovery.reason === "compatible"
        ? "Choose an exact Codec profile."
        : compatibilityReasonLabel(runtimeDiscovery.reason);
    }
    if (options === null) return "Exact source preflight has not completed.";
    if (options.reason !== "compatible") {
      return compatibilityReasonLabel(options.reason);
    }
    const missing = options.externalAssets.filter(
      (asset) => asset.required && !asset.bound,
    );
    return missing.length === 0
      ? "Exact runtime ready."
      : `Bind required external asset: ${missing[0].displayName}.`;
  }

  async function openDeck(nextDraft: DeckUiDraft): Promise<void> {
    const codec = selectedCodec;
    const profile = selectedProfile;
    const device = selectedDevice;
    if (
      !runtimeAvailable ||
      codec === undefined ||
      profile === undefined ||
      device === ""
    ) {
      return;
    }
    if (
      viewportApplied?.visible !== true ||
      viewportAnchor === null ||
      !viewportAnchor.isConnected
    ) {
      errorCode = "output.viewport_not_ready";
      message = "LatentDeck is waiting for the visible embedded video area.";
      scheduleViewportSync();
      return;
    }
    draft = nextDraft;
    await run(async () => {
      const wire = buildGenericDeckOpenDraft(model, draft, sourceCartridges);
      const opened = await genericDeckClient.open({
        deckId: model.deckId,
        deckVersion: model.deckVersion,
        codecId: codec.codecId,
        codecVersion: codec.codecVersion,
        profileKey: profile,
        device,
        deviceOrdinal,
        ...wire,
      });
      if (opened.sessionId !== selectedSessionId) resetControlsScope();
      invalidateSessionLifecycle();
      selectedSessionId = opened.sessionId;
      const revision = beginSessionsRequest();
      applySessionsResponse(
        await genericDeckClient.foregroundSet(opened.sessionId),
        revision,
      );
      message = `Warm session ${opened.sessionId} owns foreground output.`;
      await establishViewport();
    });
  }

  async function refreshSessions(): Promise<void> {
    const revision = beginSessionsRequest();
    try {
      applySessionsResponse(await genericDeckClient.sessionsGet(), revision);
    } catch (error) {
      if (revision === sessionsRequestRevision) fail(error);
    }
  }

  function beginSessionsRequest(): number {
    sessionsRequestRevision += 1;
    return sessionsRequestRevision;
  }

  function applySessionsResponse(
    next: GenericDeckSessionsView,
    revision: number,
  ): boolean {
    if (revision !== sessionsRequestRevision) return false;
    applySessions(next);
    return true;
  }

  function invalidateSessionLifecycle(): void {
    sessionLifecycleEpoch += 1;
    sessionPollToken = null;
  }

  function sessionPollIsCurrent(token: {
    epoch: number;
    sessionId: string;
    deckKey: string;
  }): boolean {
    return (
      sessionPollToken === token &&
      token.epoch === sessionLifecycleEpoch &&
      token.sessionId === selectedSessionId &&
      token.deckKey === model.exactKey &&
      active
    );
  }

  function applySessions(next: GenericDeckSessionsView): void {
    sessions = next;
    if (
      !next.sessions.some((session) => session.sessionId === selectedSessionId)
    ) {
      const foreground = next.sessions.find(
        (session) =>
          session.foreground &&
          exactPackageKey(
            session.deck.packageId,
            session.deck.packageVersion,
          ) === model.exactKey,
      );
      const nextSelectedSessionId = foreground?.sessionId ?? "";
      if (nextSelectedSessionId !== selectedSessionId) {
        invalidateSessionLifecycle();
        resetControlsScope();
      }
      selectedSessionId = nextSelectedSessionId;
      hydratedSessionId = "";
      sessionSnapshotValid = false;
      capture = null;
      recording = null;
      spout = null;
      spoutStatusKnown = false;
      outputFullscreen = null;
    }
  }

  function hydrateSelectedSession(session: GenericDeckSessionView): void {
    resetControlsScope();
    hydratedSessionId = session.sessionId;
    sessionSnapshotValid = false;
    try {
      draft = genericDeckDraftFromSessionSnapshot(model, {
        sources: session.sources,
        roles: session.runtime.status.roles,
        controls: session.runtime.status.controls,
        sourceTransport: session.runtime.status.source_transport,
        seed: session.runtime.status.seed,
      });
      draftRevision += 1;
      capture = null;
      recording = null;
      spout = null;
      spoutStatusKnown = false;
      outputFullscreen = null;
      acknowledgedControls = { ...draft.controls };
      sessionSnapshotValid = true;
    } catch (error) {
      draft = createDeckUiDraft(model);
      draftRevision += 1;
      errorCode = "deck.session_snapshot_invalid";
      message = describeCommandError(error);
    }
  }

  function applySession(next: GenericDeckSessionView): void {
    sessions = {
      ...sessions,
      sessions: sessions.sessions.map((session) =>
        session.sessionId === next.sessionId ? next : session,
      ),
    };
  }

  function applyRuntime(next: GenericDeckRuntimeView): void {
    if (selectedSessionId === "") return;
    applyRuntimeForSession(selectedSessionId, next);
  }

  function applyRuntimeForSession(
    sessionId: string,
    next: GenericDeckRuntimeView,
  ): void {
    sessions = {
      ...sessions,
      sessions: sessions.sessions.map((session) =>
        session.sessionId === sessionId
          ? { ...session, runtime: next }
          : session,
      ),
    };
  }

  async function foregroundSession(
    session: GenericDeckSessionView,
  ): Promise<void> {
    await run(async () => {
      const revision = beginSessionsRequest();
      const next = await genericDeckClient.foregroundSet(session.sessionId);
      if (!applySessionsResponse(next, revision)) return;
      if (session.sessionId !== selectedSessionId) {
        invalidateSessionLifecycle();
        resetControlsScope();
      }
      selectedSessionId = session.sessionId;
      hydratedSessionId = "";
      sessionSnapshotValid = false;
      const exactKey = exactPackageKey(
        session.deck.packageId,
        session.deck.packageVersion,
      );
      if (exactKey !== model.exactKey) await onSelectDeck(exactKey);
      message = `Foreground output switched to ${session.sessionId}.`;
    });
  }

  async function closeSession(sessionId: string): Promise<void> {
    if (closingSessionIds.has(sessionId)) return;
    closingSessionIds = new Set([...closingSessionIds, sessionId]);
    errorCode = "";
    if (sessionId === selectedSessionId) {
      invalidateSessionLifecycle();
      resetControlsScope();
    }
    let closeError: unknown = null;
    try {
      await genericDeckClient.close(sessionId);
    } catch (error) {
      closeError = error;
    }
    const revision = beginSessionsRequest();
    try {
      const next = await genericDeckClient.sessionsGet();
      if (!applySessionsResponse(next, revision)) return;
      if (!next.sessions.some((session) => session.sessionId === sessionId)) {
        message = `Warm session ${sessionId} closed explicitly.`;
        errorCode = "";
        return;
      }
      if (closeError !== null) fail(closeError);
    } catch (error) {
      if (revision === sessionsRequestRevision) fail(closeError ?? error);
    } finally {
      const nextClosing = new Set(closingSessionIds);
      nextClosing.delete(sessionId);
      closingSessionIds = nextClosing;
    }
  }

  async function runSessionAction(
    operation: (sessionId: string) => Promise<GenericDeckRuntimeView>,
  ): Promise<void> {
    if (selectedSession === undefined || !selectedSessionReady) return;
    await run(async () => {
      applyRuntime(await operation(selectedSession!.sessionId));
    });
  }

  function changeControls(controls: Record<string, DeckUiScalar>): void {
    queueControls(controls, false);
  }

  function commitControls(controls: Record<string, DeckUiScalar>): void {
    queueControls(controls, true);
  }

  async function commitRoles(_roles: Record<string, number>): Promise<void> {
    const wire = buildGenericDeckOpenDraft(model, draft, sourceCartridges);
    await runSessionAction((sessionId) =>
      genericDeckClient.rolesSet(sessionId, wire.roles),
    );
  }

  async function commitTransport(
    _playing: readonly boolean[],
    _loops: readonly boolean[],
  ): Promise<void> {
    const wire = buildGenericDeckOpenDraft(model, draft, sourceCartridges);
    await runSessionAction((sessionId) =>
      genericDeckClient.transportSet(sessionId, wire.sourceTransport),
    );
  }

  async function commitSeed(_seed: number): Promise<void> {
    const wire = buildGenericDeckOpenDraft(model, draft, sourceCartridges);
    await runSessionAction((sessionId) =>
      genericDeckClient.seedSet(sessionId, wire.seed),
    );
  }

  async function processOnce(): Promise<void> {
    await runSessionAction((sessionId) =>
      genericDeckClient.processOnce(sessionId),
    );
  }

  async function restart(): Promise<void> {
    await runSessionAction((sessionId) =>
      genericDeckClient.reset(sessionId, false),
    );
  }

  function captureStatusNeedsPolling(): boolean {
    if (captureActive(capture)) return true;
    return (
      capture === null &&
      selectedSession !== undefined &&
      selectedSession.runtime.status.capture_state !== "idle"
    );
  }

  function recordingStatusNeedsPolling(): boolean {
    return (
      recordingActive(recording) ||
      (recording === null && selectedSessionOutputPinKind() === "mp4")
    );
  }

  function spoutStatusNeedsPolling(): boolean {
    return (
      !spoutStatusKnown || spout?.enabled === true || spout?.published === true
    );
  }

  async function pollForegroundState(): Promise<void> {
    const sessionId = selectedSessionId;
    if (sessionId === "" || busy || sessionPollToken !== null) return;
    const token = {
      epoch: sessionLifecycleEpoch,
      sessionId,
      deckKey: model.exactKey,
    };
    sessionPollToken = token;
    try {
      const outputPinKindBeforePoll = activeOutputPinKind();
      const pollCapture = captureStatusNeedsPolling();
      const pollRecording = recordingStatusNeedsPolling();
      const pollSpout = spoutStatusNeedsPolling();
      const pollFullscreen =
        outputFullscreen === null || outputFullscreen === true;
      const [
        nextSession,
        nextCapture,
        nextRecording,
        nextSpout,
        nextFullscreen,
      ] = await Promise.all([
        genericDeckClient.statusGet(sessionId),
        pollCapture
          ? genericDeckClient.captureStatusGet(sessionId)
          : Promise.resolve(undefined),
        pollRecording
          ? genericDeckClient.recordingStatusGet(sessionId)
          : Promise.resolve(undefined),
        pollSpout
          ? genericDeckClient.spoutStatusGet(sessionId)
          : Promise.resolve(undefined),
        pollFullscreen
          ? genericDeckClient.fullscreenStatusGet(sessionId)
          : Promise.resolve(undefined),
      ]);
      if (!sessionPollIsCurrent(token)) return;

      // Commit one staged polling snapshot so Svelte performs one UI flush.
      applySession(nextSession);
      if (nextCapture !== undefined) capture = nextCapture;
      if (nextRecording !== undefined) recording = nextRecording;
      if (nextSpout !== undefined) {
        spout = nextSpout;
        spoutStatusKnown = true;
      }
      if (nextFullscreen !== undefined) outputFullscreen = nextFullscreen;
      if (spout !== null && !spoutNameDirty) spoutName = spout.requestedName;

      const outputPinKindAfterPoll = activeOutputPinKind();
      if (
        outputPinKindBeforePoll !== outputPinKindAfterPoll ||
        selectedSessionOutputPinKind() !== outputPinKindAfterPoll
      ) {
        await refreshSessions();
        if (!sessionPollIsCurrent(token)) return;
      }
      await publishCompletedCapture(capture);
      if (!sessionPollIsCurrent(token)) return;
    } catch (error) {
      if (!sessionPollIsCurrent(token)) return;
      if (commandErrorCode(error) === "session.not_found") {
        invalidateSessionLifecycle();
        await refreshSessions();
        if (errorCode === "session.not_found") errorCode = "";
        return;
      }
      fail(error);
    } finally {
      if (sessionPollToken === token) sessionPollToken = null;
    }
  }

  async function captureAction(
    mode: "snapshot" | "live_capture",
  ): Promise<void> {
    if (!captureAvailable || selectedSession === undefined) return;
    const sessionId = selectedSession.sessionId;
    const stoppingLiveCapture =
      mode === "live_capture" &&
      capture?.mode === "live_capture" &&
      capture.state === "capturing";
    if (!stoppingLiveCapture && controlsUnsettled) return;
    await run(async () => {
      if (stoppingLiveCapture) {
        capture = await genericDeckClient.captureStop(sessionId);
      } else {
        capture = await genericDeckClient.captureStart(sessionId, mode);
      }
      if (capture !== null) await refreshSessions();
      await publishCompletedCapture(capture);
    });
  }

  async function publishCompletedCapture(
    completed: GenericCaptureView | null,
  ): Promise<void> {
    if (
      completed === null ||
      completed.state !== "finished" ||
      completed.cartridgeId === null ||
      completed.archiveSha256 === null
    ) {
      return;
    }
    const key = `${completed.captureId ?? completed.cartridgeId}:${completed.archiveSha256}`;
    if (key === publishedCaptureKey) return;
    const resolved = await invoke<(CartridgeView | null)[]>(
      "library_resolve_preset_sources",
      {
        identities: [
          {
            cartridge_id: completed.cartridgeId,
            archive_sha256: completed.archiveSha256,
          },
        ],
      },
    );
    const capturedSource = resolved[0];
    if (
      capturedSource === null ||
      capturedSource === undefined ||
      capturedSource.cartridgeId !== completed.cartridgeId ||
      capturedSource.archiveSha256 !== completed.archiveSha256
    ) {
      throw new Error(
        "The completed capture is not yet available under its exact Library identity.",
      );
    }
    const incoming = await librarySnapshot();
    completedCaptureSource = capturedSource;
    acceptLibrarySnapshot(
      incoming,
      [capturedSource],
      [capturedSource.archiveSha256],
    );
    if (selectedCodec !== undefined && selectedProfile !== undefined) {
      await refreshRuntimeOptionsInsideAction();
    }
    publishedCaptureKey = key;
  }

  async function useCompletedCapture(slotIndex: number): Promise<void> {
    const capturedSource = completedCaptureSource;
    const session = selectedSession;
    if (
      capturedSource === null ||
      session === undefined ||
      !selectedSessionReady ||
      !captureReuseAvailable ||
      !Number.isSafeInteger(slotIndex) ||
      slotIndex < 0 ||
      slotIndex >= model.slots
    ) {
      return;
    }
    await run(async () => {
      invalidateSessionLifecycle();
      const authoritative = await genericDeckClient.statusGet(
        session.sessionId,
      );
      if (authoritative.sessionId !== session.sessionId) {
        throw new Error(
          "Source replacement status returned a different generic Deck session.",
        );
      }
      const loadedDraft = genericDeckDraftFromSessionSnapshot(model, {
        sources: authoritative.sources,
        roles: authoritative.runtime.status.roles,
        controls: authoritative.runtime.status.controls,
        sourceTransport: authoritative.runtime.status.source_transport,
        seed: authoritative.runtime.status.seed,
      });
      const nextDraft: DeckUiDraft = {
        ...loadedDraft,
        sourceArchiveSha256s: replaceDraftSource(
          loadedDraft.sourceArchiveSha256s,
          slotIndex,
          capturedSource.archiveSha256,
        ),
      };
      const selectedSources = selectedSourceSet(nextDraft);
      if (selectedSources.length !== model.slots) {
        throw new Error(
          "Every Deck source slot must resolve to an exact available Library identity.",
        );
      }
      const nextOptions = await genericDeckClient.runtimeOptions({
        deckId: authoritative.deck.packageId,
        deckVersion: authoritative.deck.packageVersion,
        codecId: authoritative.codec.packageId,
        codecVersion: authoritative.codec.packageVersion,
        profileKey: authoritative.profileKey,
        device: authoritative.device,
        deviceOrdinal: authoritative.deviceOrdinal,
        sources: availableSourceIdentities(),
        selectedSources,
      });
      runtimeOptions = nextOptions;
      if (nextOptions.reason !== "compatible") {
        message = compatibilityReasonLabel(nextOptions.reason);
        return;
      }
      const wire = buildGenericDeckOpenDraft(
        model,
        nextDraft,
        sourceCartridges,
      );
      const replaced = await genericDeckClient.replaceSources(
        authoritative.sessionId,
        {
          deckId: authoritative.deck.packageId,
          deckVersion: authoritative.deck.packageVersion,
          codecId: authoritative.codec.packageId,
          codecVersion: authoritative.codec.packageVersion,
          profileKey: authoritative.profileKey,
          device: authoritative.device,
          deviceOrdinal: authoritative.deviceOrdinal,
          ...wire,
        },
      );
      if (replaced.sessionId !== authoritative.sessionId) {
        throw new Error(
          "Source replacement returned a different generic Deck session.",
        );
      }
      resetControlsScope();
      draft = genericDeckDraftFromSessionSnapshot(model, {
        sources: replaced.sources,
        roles: replaced.runtime.status.roles,
        controls: replaced.runtime.status.controls,
        sourceTransport: replaced.runtime.status.source_transport,
        seed: replaced.runtime.status.seed,
      });
      draftRevision += 1;
      hydratedSessionId = replaced.sessionId;
      sessionSnapshotValid = true;
      acknowledgedControls = { ...draft.controls };
      applySession(replaced);
      message = `Captured cartridge loaded into slot ${String.fromCharCode(65 + slotIndex)} without closing the warm session.`;
    });
  }

  async function recordingAction(): Promise<void> {
    if (!recordingAvailable || selectedSession === undefined) return;
    const controls = decodedRecordingControls(
      recording ?? IDLE_DECODED_RECORDING,
      true,
      busy,
    );
    if (!controls.start && !controls.stop) return;
    await run(async () => {
      if (controls.stop) {
        recording = await genericDeckClient.recordingStop(
          selectedSession!.sessionId,
        );
        await refreshSessions();
        return;
      }
      const started = await genericDeckClient.recordingStart(
        selectedSession!.sessionId,
      );
      if (started !== null) {
        recording = started;
        await refreshSessions();
      }
    });
  }

  async function configureSpout(name: string | null, enabled: boolean | null) {
    if (selectedSession === undefined) return;
    await run(async () => {
      spout = await genericDeckClient.spoutConfigure(
        selectedSession!.sessionId,
        {
          name,
          enabled,
        },
      );
      spoutStatusKnown = true;
      if (name !== null) {
        spoutName = spout.requestedName;
        spoutNameDirty = false;
      }
    });
  }

  async function savePreset(): Promise<void> {
    if (presetBusy) return;
    presetBusy = true;
    try {
      const preset = buildGenericDeckPreset(
        model,
        draft,
        sourceCartridges,
        libraryView.deckSession.activeCollectionId,
      );
      const saved = await invoke<{ saved: boolean } | null>(
        "deck_generic_preset_save",
        {
          preset,
        },
      );
      presetMessage =
        saved === null ? "Preset save cancelled." : "Exact preset v2 saved.";
    } catch (error) {
      presetMessage = describeCommandError(error);
    } finally {
      presetBusy = false;
    }
  }

  async function loadPreset(): Promise<void> {
    if (presetBusy) return;
    presetBusy = true;
    try {
      const preset = await invoke<DeckPreset | null>(
        "deck_generic_preset_load",
      );
      if (preset === null) {
        presetMessage = "Preset load cancelled.";
        return;
      }
      const exactKey = exactPackageKey(preset.deck_id, preset.deck_version);
      if (!models.some((candidate) => candidate.exactKey === exactKey)) {
        presetMessage = `Exact Deck ${exactKey} is not enabled; no substitute was selected.`;
        return;
      }
      if (exactKey !== model.exactKey) {
        presetMessage = `Preset targets ${exactKey}. Select that exact Deck and load again.`;
        await onSelectDeck(exactKey);
        return;
      }
      const identities = presetSlotIdentities(preset);
      const { sources, library: incoming } = await stagePresetLibraryLoad(
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
      if (sources.some((source) => source === null)) {
        throw new Error(
          "One or more exact preset cartridge identities are unavailable.",
        );
      }
      draft = genericDeckDraftFromPreset(
        model,
        preset,
        sources as CartridgeView[],
      );
      draftRevision += 1;
      acceptLibrarySnapshot(incoming, sources, draft.sourceArchiveSha256s);
      presetMessage =
        "Exact preset v2 loaded as a draft; press Load exact Deck draft.";
      if (selectedProfile !== undefined) await refreshSourceEligibility();
    } catch (error) {
      presetMessage = describeCommandError(error);
    } finally {
      presetBusy = false;
    }
  }

  function monitorAnchor(element: HTMLDivElement | null): void {
    viewportAnchor = element;
    viewportResizeObserver?.disconnect();
    if (element === null) {
      cancelViewportRetry();
      cancelViewportSyncSchedule();
      viewportRetryAttempt = 0;
      viewportRetryExhausted = false;
      viewportApplied = null;
      return;
    }
    viewportResizeObserver?.observe(element);
    if (active && !viewportSuspended && viewportRetryTimer === null)
      void establishViewport();
  }

  async function establishViewport(): Promise<void> {
    if (!active || viewportSuspended || viewportAnchor === null || viewportBusy)
      return;
    if (viewportEpoch !== null) {
      scheduleViewportSync();
      return;
    }
    const anchor = viewportAnchor;
    viewportBusy = true;
    try {
      const viewport = await genericDeckClient.viewportSessionBegin();
      if (
        !active ||
        viewportSuspended ||
        viewportAnchor !== anchor ||
        !anchor.isConnected
      ) {
        resetViewportBootstrap();
        if (active && !viewportSuspended && viewportAnchor !== null)
          scheduleViewportRetry();
        return;
      }
      viewportEpoch = viewport.epoch;
      viewportRevision = 0;
      viewportApplied = null;
      scheduleViewportSync();
    } catch (error) {
      failViewportBootstrap(error);
    } finally {
      viewportBusy = false;
    }
  }

  function scheduleViewportRetry(): void {
    if (
      !active ||
      viewportSuspended ||
      viewportAnchor === null ||
      viewportRetryTimer !== null
    )
      return;
    const delay = VIEWPORT_RETRY_DELAYS_MS[viewportRetryAttempt];
    if (delay === undefined) {
      viewportRetryExhausted = true;
      return;
    }
    viewportRetryAttempt += 1;
    viewportRetryTimer = globalThis.setTimeout(() => {
      viewportRetryTimer = null;
      if (active && !viewportSuspended && viewportAnchor !== null)
        void establishViewport();
    }, delay);
  }

  function cancelViewportRetry(): void {
    if (viewportRetryTimer === null) return;
    globalThis.clearTimeout(viewportRetryTimer);
    viewportRetryTimer = null;
  }

  function requestViewportRecovery(): void {
    if (!active || viewportSuspended || viewportAnchor === null) return;
    if (viewportEpoch !== null) {
      scheduleViewportSync();
      return;
    }
    if (viewportBusy || viewportRetryTimer !== null) return;
    viewportRetryAttempt = 0;
    viewportRetryExhausted = false;
    void establishViewport();
  }

  function resetViewportBootstrap(): void {
    cancelViewportSyncSchedule();
    viewportEpoch = null;
    viewportRevision = 0;
    viewportApplied = null;
  }

  function failViewportBootstrap(error: unknown): void {
    resetViewportBootstrap();
    viewportFailureCode = commandErrorCode(error);
    fail(error);
    scheduleViewportRetry();
  }

  function confirmViewportBounds(bounds: EmbeddedViewportBounds): void {
    cancelViewportRetry();
    viewportRetryAttempt = 0;
    viewportRetryExhausted = false;
    viewportApplied = bounds;
    if (viewportFailureCode !== "" && errorCode === viewportFailureCode) {
      errorCode = "";
      message = "Native output viewport ready.";
    }
    viewportFailureCode = "";
  }

  function cancelViewportSyncSchedule(): void {
    viewportSyncPending = false;
    if (viewportFrame === null) return;
    globalThis.cancelAnimationFrame(viewportFrame);
    viewportFrame = null;
  }

  function scheduleViewportSync(): void {
    viewportSyncPending = true;
    if (viewportSyncRunning || viewportFrame !== null) return;
    viewportFrame = globalThis.requestAnimationFrame(() => {
      viewportFrame = null;
      if (!viewportSyncPending) return;
      viewportSyncPending = false;
      void syncViewport();
    });
  }

  async function syncViewport(): Promise<void> {
    if (viewportSyncRunning) {
      viewportSyncPending = true;
      return;
    }
    viewportSyncRunning = true;
    const epoch = viewportEpoch;
    const anchor = viewportAnchor;
    let revision: number | null = null;
    try {
      if (epoch === null || anchor === null || !anchor.isConnected) return;
      revision = nextEmbeddedViewportRevision(viewportRevision);
      if (revision === null) return;
      const rect = anchor.getBoundingClientRect();
      const scaleFactor = globalThis.devicePixelRatio;
      const inside = embeddedViewportFullyInsideClient(
        rect,
        document.documentElement.clientWidth,
        document.documentElement.clientHeight,
        scaleFactor,
      );
      const visible = active && !viewportSuspended && inside;
      const bounds = visible
        ? buildEmbeddedViewportBounds(epoch, revision, rect, scaleFactor, true)
        : hiddenEmbeddedViewportBounds(epoch, revision, scaleFactor);
      if (
        bounds === null ||
        sameEmbeddedViewportGeometry(viewportApplied, bounds)
      )
        return;
      viewportRevision = revision;
      await genericDeckClient.viewportSetBounds(bounds);
      if (
        viewportEpoch === epoch &&
        viewportRevision === revision &&
        viewportAnchor === anchor
      )
        confirmViewportBounds(bounds);
    } catch (error) {
      if (
        viewportEpoch === epoch &&
        viewportRevision === revision &&
        viewportAnchor === anchor
      )
        failViewportBootstrap(error);
    } finally {
      viewportSyncRunning = false;
      if (
        viewportSyncPending &&
        active &&
        !viewportSuspended &&
        viewportAnchor !== null &&
        viewportEpoch !== null
      ) {
        scheduleViewportSync();
      } else {
        viewportSyncPending = false;
      }
    }
  }

  async function hideViewport(): Promise<void> {
    const epoch = viewportEpoch;
    const revision = nextEmbeddedViewportRevision(viewportRevision);
    if (epoch === null || revision === null) return;
    const bounds = hiddenEmbeddedViewportBounds(
      epoch,
      revision,
      globalThis.devicePixelRatio,
    );
    if (bounds === null) return;
    viewportRevision = revision;
    try {
      await genericDeckClient.viewportSetBounds(bounds);
      viewportApplied = bounds;
    } catch {
      // Best-effort teardown; the host destroys the child with the session.
    }
  }

  async function leaveSurface(): Promise<void> {
    viewportSuspended = true;
    cancelViewportRetry();
    const sessionId = selectedSession?.sessionId;
    if (sessionId !== undefined && outputFullscreen === true) {
      outputFullscreen = await genericDeckClient.fullscreenSet(
        sessionId,
        false,
      );
    }
    await hideViewport();
  }

  async function toggleFullscreen(): Promise<void> {
    if (selectedSession === undefined || outputFullscreen === null) return;
    await run(async () => {
      try {
        outputFullscreen = await genericDeckClient.fullscreenSet(
          selectedSession!.sessionId,
          !outputFullscreen,
        );
      } catch (error) {
        outputFullscreen = null;
        throw error;
      }
      scheduleViewportSync();
    });
  }

  function handleWindowKeydown(event: KeyboardEvent): void {
    handleDeckFullscreenKeydown(
      event,
      {
        active,
        runtimeLoaded: selectedSessionReady,
        viewportReady: viewportApplied?.visible === true,
        busy,
        current: outputFullscreen,
      },
      () => void toggleFullscreen(),
    );
  }

  async function run(operation: () => Promise<void>): Promise<void> {
    if (busy) return;
    busy = true;
    errorCode = "";
    try {
      await operation();
    } catch (error) {
      fail(error);
    } finally {
      busy = false;
    }
  }

  function fail(error: unknown): void {
    errorCode = commandErrorCode(error);
    message = describeCommandError(error);
  }

  function commandErrorCode(error: unknown): string {
    if (
      typeof error === "object" &&
      error !== null &&
      "code" in error &&
      typeof error.code === "string"
    ) {
      return error.code;
    }
    return "deck.command_failed";
  }

  function profileKey(profile: GenericProfileKey): string {
    return [profile.codecFamily, profile.profile, profile.profileVersion].join(
      PROFILE_SEPARATOR,
    );
  }

  function profileLabel(profile: GenericProfileKey): string {
    return `${profile.codecFamily}/${profile.profile}@${profile.profileVersion}`;
  }

  function playheadsFor(
    deck: DeckUiModel,
    session: GenericDeckSessionView | undefined,
  ): number[] {
    const values = Array.from({ length: deck.slots }, () => 0);
    for (const playhead of session?.runtime.status.playheads ?? []) {
      if (playhead.physical_slot >= 1 && playhead.physical_slot <= deck.slots) {
        values[playhead.physical_slot - 1] = playhead.latent_slot;
      }
    }
    return values;
  }

  function statusMessage(): string {
    if (selectedSession === undefined) return message;
    const status = selectedSession.runtime.status;
    return [
      status.state,
      `generation ${status.stream_generation}`,
      `sequence ${status.stream_sequence}`,
      selectedSession.runtime.faultCode,
    ]
      .filter((part) => part !== null)
      .join(" · ");
  }

  function captureState(): string {
    return capture === null
      ? (selectedSession?.runtime.status.capture_state ?? "idle")
      : `${capture.mode ?? "capture"} · ${capture.state} · ${capture.latentSlots} slots`;
  }

  function captureActive(value: GenericCaptureView | null): boolean {
    return (
      value?.state === "starting" ||
      value?.state === "capturing" ||
      value?.state === "finalizing"
    );
  }

  function recordingActive(value: GenericRecordingView | null): boolean {
    return (
      value?.state === "armed" ||
      value?.state === "recording" ||
      value?.state === "finalizing"
    );
  }

  function activeOutputPinKind(): "capture" | "mp4" | null {
    if (captureActive(capture)) return "capture";
    if (recordingActive(recording)) return "mp4";
    return null;
  }

  function selectedSessionOutputPinKind(): "capture" | "mp4" | null {
    const pin = sessions.outputPin;
    return pin?.session_id === selectedSessionId ? pin.kind : null;
  }

  async function librarySnapshot(): Promise<LibraryView> {
    return invoke<LibraryView>("library_snapshot", { search: null });
  }

  // Stable host codes are rendered verbatim so capacity and pin refusals never
  // look like an implicit eviction or a successful foreground switch.
  const SESSION_CAPACITY_CODE = "session.capacity_exceeded";
  const SESSION_PINNED_CODE = "session.output_lease_pinned";
</script>

<svelte:window onkeydown={handleWindowKeydown} />

<section class="generic-workspace" aria-busy={busy || matrixBusy}>
  <section class="runtime-config" aria-label="Exact Deck and Codec selection">
    <header>
      <div class="runtime-identity">
        <span>New warm-session negotiation</span>
        <strong>{model.deckId}@{model.deckVersion}</strong>
      </div>
      <small class="runtime-message" title={message}>{message}</small>
      <div class="preset-tools">
        <strong>Preset v2</strong>
        <button
          type="button"
          disabled={presetBusy || busy}
          onclick={() => void loadPreset()}>Load</button
        >
        <button
          type="button"
          disabled={presetBusy || busy}
          onclick={() => void savePreset()}>Save</button
        >
        <small title={presetMessage}>{presetMessage}</small>
      </div>
    </header>
    <div class="config-grid">
      <label>
        <span>Codec Pack version</span>
        <select value={selectedCodecKey} onchange={selectCodec} disabled={busy}>
          <option value="">Choose exact Codec version…</option>
          {#each codecOptions as option (option.exactKey)}
            <option
              value={option.exactKey}
              disabled={option.reason !== "compatible"}
              >{option.codecId}@{option.codecVersion} · {compatibilityReasonLabel(
                option.reason,
              )}</option
            >
          {/each}
        </select>
      </label>
      <label>
        <span>Negotiated device</span>
        <select value={selectedDevice} onchange={selectDevice} disabled={busy}>
          <option value="">Choose device…</option>
          <option value="cpu">CPU</option>
          <option value="cuda">CUDA</option>
        </select>
      </label>
      <label>
        <span>Device ordinal</span>
        <input
          type="number"
          min="0"
          max="15"
          step="1"
          bind:value={deviceOrdinal}
          disabled={busy || selectedDevice === ""}
          onchange={() => void rediscoverRuntimeAfterOrdinalChange()}
        />
      </label>
      <label>
        <span>Codec profile</span>
        <select
          value={selectedProfileKey}
          onchange={selectProfile}
          disabled={busy || discovery === null}
        >
          <option value="">Choose exact profile…</option>
          {#each profiles as profile (profileKey(profile))}
            <option value={profileKey(profile)}>{profileLabel(profile)}</option>
          {/each}
        </select>
      </label>
    </div>
    {#if (runtimeOptions ?? discovery)?.externalAssets.length}
      <details
        class="asset-drawer"
        open={(runtimeOptions ?? discovery)?.externalAssets.some(
          (asset) => asset.required && !asset.bound,
        )}
      >
        <summary>
          Codec assets · {(runtimeOptions ?? discovery)?.externalAssets.filter(
            (asset) => asset.bound,
          ).length ?? 0}/{(runtimeOptions ?? discovery)?.externalAssets
            .length ?? 0} bound
        </summary>
        <div class="asset-grid" aria-label="External Codec assets">
          {#each (runtimeOptions ?? discovery)?.externalAssets ?? [] as asset (asset.assetId)}
            <article class:bound={asset.bound}>
              <div>
                <strong>{asset.displayName}</strong>
                <small
                  >{asset.assetId} · {asset.required
                    ? "required"
                    : "optional"}</small
                >
                <small
                  >{asset.bound
                    ? `SHA-256 ${shortHash(asset.boundSha256 ?? "")}`
                    : "Not bound"}</small
                >
              </div>
              <button
                type="button"
                disabled={busy}
                onclick={() => void selectExternalAsset(asset.assetId)}
                >Choose file…</button
              >
              <button
                type="button"
                disabled={busy || !asset.bound}
                onclick={() => void clearExternalAsset(asset.assetId)}
                >Clear</button
              >
            </article>
          {/each}
        </div>
      </details>
    {/if}
    {#if errorCode !== ""}
      <p class="runtime-error" role="alert">
        <strong>{errorCode}</strong> · {message}
      </p>
    {/if}
  </section>

  <details class="session-rail" aria-label="Warm generic Deck sessions">
    <summary>
      <strong
        >Warm sessions {sessions.sessions
          .length}/{MAX_WARM_DECK_SESSIONS}</strong
      >
      <span
        >{sessionCapacityState(sessions.sessions.length).remaining} available</span
      >
      <small
        >{sessions.outputPin === null
          ? "Output lease unpinned"
          : `Pinned by ${sessions.outputPin.kind}`}</small
      >
    </summary>
    <div class="session-list">
      {#each sessions.sessions as session (session.sessionId)}
        <article
          class:foreground={session.foreground}
          class:selected={session.sessionId === selectedSessionId}
        >
          <button
            type="button"
            class="session-select"
            onclick={() => void foregroundSession(session)}
            disabled={busy}
          >
            <strong
              >{session.deck.packageId}@{session.deck.packageVersion}</strong
            >
            <small
              >{session.codec.packageId}@{session.codec.packageVersion}</small
            >
            <small
              >{profileLabel(session.profileKey)} · {session.device}:{session.deviceOrdinal}</small
            >
            <small
              >{session.externalAssets.length === 0
                ? "No external asset receipts"
                : session.externalAssets
                    .map(
                      (asset) =>
                        `${asset.assetId} ${shortHash(asset.sha256)} ${asset.byteLength} B`,
                    )
                    .join(" · ")}</small
            >
            <small>{session.sessionId} · {session.runtime.status.state}</small>
          </button>
          <button
            type="button"
            onclick={() => void closeSession(session.sessionId)}
            disabled={busy || closingSessionIds.has(session.sessionId)}
            >{closingSessionIds.has(session.sessionId)
              ? "Closing…"
              : "Close"}</button
          >
        </article>
      {/each}
      {#if sessions.sessions.length === 0}<p>
          No warm Protocol 2 sessions.
        </p>{/if}
    </div>
    {#if !sessionCapacityAvailable}
      <p class="capacity-note">
        {SESSION_CAPACITY_CODE} · close one session explicitly; no LRU eviction occurs.
      </p>
    {/if}
    {#if errorCode === SESSION_PINNED_CODE}
      <p class="capacity-note">
        {SESSION_PINNED_CODE} · capture or MP4 owns the foreground lease.
      </p>
    {/if}
  </details>

  {#key `${model.exactKey}:${draftRevision}`}
    <DeckFaceplateRenderer
      {model}
      initialDraft={draft}
      {sourceOptions}
      {active}
      {runtimeAvailable}
      {runtimeUnavailableReason}
      {loadAvailable}
      {loadUnavailableReason}
      runtimeLoaded={selectedSessionReady}
      runtimeBusy={busy}
      statusMessage={statusMessage()}
      {playheads}
      captureState={captureState()}
      {captureAvailable}
      {captureStartAvailable}
      captureActive={captureIsActive}
      liveCaptureActive={capture?.mode === "live_capture" &&
        capture.state === "capturing"}
      {captureUnavailableReason}
      {capturedSourceAvailable}
      {captureReuseAvailable}
      {sourceGeometryWarning}
      mp4Available={recordingAvailable &&
        (decodedRecordingControls(
          recording ?? IDLE_DECODED_RECORDING,
          selectedSession?.foreground === true,
          busy,
        ).start ||
          decodedRecordingControls(
            recording ?? IDLE_DECODED_RECORDING,
            selectedSession?.foreground === true,
            busy,
          ).stop)}
      mp4Active={decodedRecordingControls(
        recording ?? IDLE_DECODED_RECORDING,
        selectedSession?.foreground === true,
        busy,
      ).stop}
      mp4Status={captureIsActive
        ? "Latent capture pins the foreground output lease."
        : describeDecodedRecording(recording ?? IDLE_DECODED_RECORDING)}
      {spoutName}
      spoutStatus={describeSpout(spout)}
      spoutEnabled={spout?.enabled ?? false}
      spoutRenameAvailable={spoutControlsFor(spout, busy).rename}
      spoutToggleAvailable={spoutControlsFor(spout, busy).toggle}
      {outputFullscreen}
      onDraftChange={updateDraft}
      onLoad={openDeck}
      onRestart={restart}
      onProcessOnce={processOnce}
      onControlsCommit={commitControls}
      onControlsChange={changeControls}
      onRolesCommit={commitRoles}
      onTransportCommit={commitTransport}
      onSeedCommit={commitSeed}
      onCapture={captureAction}
      onUseCapture={useCompletedCapture}
      onMp4Toggle={recordingAction}
      onSpoutNameChange={(name) => {
        spoutName = name;
        spoutNameDirty = true;
      }}
      onSpoutNameCommit={(name) => configureSpout(name.trim(), null)}
      onSpoutToggle={(enabled) => configureSpout(null, enabled)}
      onFullscreenToggle={toggleFullscreen}
      onMonitorAnchor={monitorAnchor}
    />
  {/key}
</section>

<style>
  .generic-workspace {
    display: grid;
    gap: 6px;
  }

  .runtime-config,
  .session-rail {
    border: 1px solid #3e4a43;
    color: #dce4de;
    background: #0e1411;
    box-shadow: inset 0 1px rgb(255 255 255 / 3%);
  }

  .runtime-config > header,
  .config-grid,
  .asset-grid,
  .session-list {
    display: grid;
    gap: 8px;
    padding: 7px 9px;
  }

  .runtime-config > header {
    grid-template-columns: minmax(230px, auto) minmax(180px, 1fr) auto;
    align-items: center;
    border-bottom: 1px solid #39443d;
  }

  .runtime-identity,
  .runtime-config label,
  .asset-grid article > div {
    display: grid;
    min-width: 0;
    gap: 3px;
  }

  .runtime-identity strong {
    overflow: hidden;
    color: #b9d7c0;
    font-size: 0.72rem;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .runtime-config span,
  .runtime-config small,
  .session-rail span,
  .session-rail small {
    color: #88948c;
    font-size: 0.62rem;
  }

  .runtime-message {
    min-width: 0;
    overflow: hidden;
    text-align: center;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .preset-tools {
    display: grid;
    grid-template-columns: auto auto auto;
    align-items: center;
    gap: 5px;
  }

  .preset-tools small {
    grid-column: 1 / -1;
    max-width: 240px;
    overflow: hidden;
    text-align: right;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .config-grid {
    grid-template-columns: repeat(4, minmax(160px, 1fr));
  }

  .config-grid label > span {
    overflow: hidden;
    letter-spacing: 0.04em;
    text-overflow: ellipsis;
    text-transform: uppercase;
    white-space: nowrap;
  }

  .config-grid select,
  .config-grid input {
    min-width: 0;
    height: 30px;
    border: 1px solid #4c5951;
    border-radius: 2px;
    padding: 4px 7px;
    color: #dce4de;
    background: #0a0f0c;
    font-size: 0.68rem;
  }

  .asset-drawer,
  .session-rail {
    min-width: 0;
  }

  .asset-drawer > summary,
  .session-rail > summary {
    min-height: 32px;
    padding: 7px 10px;
    color: #9eaaa1;
    background: #141b16;
    cursor: pointer;
    font-size: 0.67rem;
    user-select: none;
  }

  .session-rail > summary {
    display: flex;
    align-items: center;
    gap: 14px;
  }

  .session-rail > summary small {
    margin-left: auto;
  }

  .asset-grid,
  .session-list {
    grid-template-columns: repeat(auto-fit, minmax(270px, 1fr));
    border-top: 1px solid #303a34;
  }

  .asset-grid article,
  .session-list article {
    display: grid;
    grid-template-columns: minmax(0, 1fr) auto auto;
    gap: 6px;
    border: 1px solid #364139;
    padding: 7px;
    background: #0d120f;
  }

  .asset-grid article small,
  .session-select small {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .asset-grid article.bound,
  .session-list article.foreground {
    border-color: #6a9876;
  }

  .session-list article.selected {
    box-shadow: inset 0 -2px #88c596;
  }

  .session-select {
    display: grid;
    min-width: 0;
    gap: 2px;
    text-align: left;
  }

  .runtime-error,
  .capacity-note {
    margin: 0;
    border-top: 1px solid #6d4b47;
    padding: 7px 10px;
    color: #e3a49a;
    background: #251817;
    font-size: 0.68rem;
  }

  @media (max-width: 1180px) {
    .runtime-config > header {
      grid-template-columns: minmax(220px, 1fr) auto;
    }

    .runtime-message {
      display: none;
    }

    .config-grid {
      grid-template-columns: 1fr 1fr;
    }
  }
</style>
