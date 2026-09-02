<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { onMount } from "svelte";

  import DeckFaceplateRenderer from "./DeckFaceplateRenderer.svelte";
  import {
    buildEmbeddedViewportBounds,
    embeddedViewportFullyInsideClient,
    hiddenEmbeddedViewportBounds,
    nextEmbeddedViewportRevision,
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
    buildGenericDeckOpenDraft,
    buildGenericDeckPreset,
    codecOptionsForExactDeck,
    exactPackageKey,
    genericDeckDraftFromSessionSnapshot,
    genericDeckDraftFromPreset,
    retainExactSelection,
    sessionCapacityState,
    type GenericCodecOption,
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
    presetSlotIdentities,
    stagePresetLibraryLoad,
    type DeckPreset,
  } from "./preset-model";

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
  let outputFullscreen: boolean | null = null;
  let viewportAnchor: HTMLDivElement | null = null;
  let viewportEpoch: number | null = null;
  let viewportRevision = 0;
  let viewportFrame: number | null = null;
  let viewportSessionId = "";
  let viewportApplied: EmbeddedViewportBounds | null = null;
  let viewportBusy = false;
  let viewportResizeObserver: ResizeObserver | null = null;
  let observedModels = models;
  let publishedCaptureKey = "";
  let extensionsRefreshPending: Promise<void> | null = null;
  let extensionsRefreshRevision = 0;
  let extensionsAppliedRevision = 0;

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
    available: boolean;
    incompatibilityReason?: string;
  }> = [];
  let runtimeAvailable = false;
  let runtimeUnavailableReason = "Choose an exact compatible Codec version.";
  let loadAvailable = true;
  let captureAvailable = false;
  let captureUnavailableReason = "Load and foreground an exact session first.";
  let recordingAvailable = false;

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
  $: sourceOptions = library.cartridges.map((cartridge) =>
    sourceOption(cartridge),
  );
  $: runtimeAvailable = exactRuntimeAvailable();
  $: runtimeUnavailableReason = describeRuntimeAvailability();
  $: loadAvailable = sessionCapacityState(sessions.sessions.length).canOpen;
  $: captureAvailable =
    selectedSessionReady &&
    selectedSession?.foreground === true &&
    selectedSession.runtime.faultCode === null &&
    !recordingActive() &&
    model.requiredCapabilities.includes("snapshot_capture") &&
    model.requiredCapabilities.includes("live_capture");
  $: captureUnavailableReason = recordingActive()
    ? "MP4 recording pins the foreground output lease."
    : selectedSession?.foreground !== true
      ? "Capture requires this exact session to own the foreground output lease."
      : "This exact Deck and Codec profile does not expose both capture capabilities.";
  $: recordingAvailable =
    selectedSessionReady &&
    selectedSession?.foreground === true &&
    selectedSession.runtime.faultCode === null &&
    !captureActive();
  $: if (model.exactKey !== configuredDeckKey) resetForExactDeck();
  $: if (models !== observedModels) {
    observedModels = models;
    void refreshExtensions();
  }
  $: if (active && selectedSession?.sessionId !== viewportSessionId) {
    void establishViewport();
  }

  onMount(() => {
    registerLeave(leaveSurface);
    void refreshExtensions();
    void refreshSessions();
    viewportResizeObserver = new ResizeObserver(() => scheduleViewportSync());
    if (viewportAnchor !== null) viewportResizeObserver.observe(viewportAnchor);
    const resize = () => scheduleViewportSync();
    globalThis.addEventListener("resize", resize);
    globalThis.addEventListener("scroll", resize, true);
    const poll = globalThis.setInterval(() => {
      if (active) void pollForegroundState();
    }, 500);
    return () => {
      registerLeave(async () => undefined);
      globalThis.clearInterval(poll);
      globalThis.removeEventListener("resize", resize);
      globalThis.removeEventListener("scroll", resize, true);
      viewportResizeObserver?.disconnect();
      viewportResizeObserver = null;
      if (viewportFrame !== null)
        globalThis.cancelAnimationFrame(viewportFrame);
      void hideViewport();
    };
  });

  function resetForExactDeck(): void {
    configuredDeckKey = model.exactKey;
    selectedCodecKey = "";
    selectedDevice = "";
    selectedProfileKey = "";
    discovery = null;
    runtimeOptions = null;
    capture = null;
    recording = null;
    spout = null;
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
    selectedCodecKey = (event.currentTarget as HTMLSelectElement).value;
    selectedProfileKey = "";
    discovery = null;
    runtimeOptions = null;
    if (selectedCodec?.reason !== "compatible") {
      message =
        selectedCodec === undefined
          ? "Choose an exact Codec version."
          : compatibilityReasonLabel(selectedCodec.reason);
      return;
    }
    await discoverRuntime();
  }

  async function selectDevice(event: Event): Promise<void> {
    selectedDevice = (event.currentTarget as HTMLSelectElement).value as
      GenericDevice | "";
    selectedProfileKey = "";
    discovery = null;
    runtimeOptions = null;
    await discoverRuntime();
  }

  async function discoverRuntime(): Promise<void> {
    const codec = selectedCodec;
    const device = selectedDevice;
    if (codec?.reason !== "compatible" || device === "" || busy) {
      return;
    }
    await run(async () => {
      discovery = await genericDeckClient.runtimeOptions({
        deckId: model.deckId,
        deckVersion: model.deckVersion,
        codecId: codec.codecId,
        codecVersion: codec.codecVersion,
        profileKey: null,
        device,
        deviceOrdinal,
        sources: [],
      });
      selectedProfileKey = retainExactSelection(
        selectedProfileKey,
        discovery.profiles.map(profileKey),
      );
      runtimeOptions = null;
      message =
        discovery.reason === "compatible"
          ? "Choose one exact compatible Codec profile."
          : compatibilityReasonLabel(discovery.reason);
    });
  }

  async function selectProfile(event: Event): Promise<void> {
    selectedProfileKey = (event.currentTarget as HTMLSelectElement).value;
    await refreshSourceEligibility();
  }

  async function refreshSourceEligibility(): Promise<void> {
    const codec = selectedCodec;
    const profile = selectedProfile;
    const device = selectedDevice;
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
        sources: library.cartridges
          .filter((cartridge) => cartridge.availability === "present")
          .map((cartridge) => ({
            cartridgeId: cartridge.cartridgeId,
            archiveSha256: cartridge.archiveSha256,
          })),
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
    if (
      selectedCodec === undefined ||
      selectedProfile === undefined ||
      selectedDevice === ""
    ) {
      return;
    }
    runtimeOptions = await genericDeckClient.runtimeOptions({
      deckId: model.deckId,
      deckVersion: model.deckVersion,
      codecId: selectedCodec.codecId,
      codecVersion: selectedCodec.codecVersion,
      profileKey: selectedProfile,
      device: selectedDevice,
      deviceOrdinal,
      sources: library.cartridges
        .filter((cartridge) => cartridge.availability === "present")
        .map((cartridge) => ({
          cartridgeId: cartridge.cartridgeId,
          archiveSha256: cartridge.archiveSha256,
        })),
    });
  }

  function sourceOption(cartridge: CartridgeView) {
    const eligibility = runtimeOptions?.sources.find(
      (candidate) =>
        candidate.archiveSha256 === cartridge.archiveSha256 &&
        candidate.cartridgeId === cartridge.cartridgeId,
    );
    const incompatibilityReason =
      selectedProfile === undefined
        ? "Select an exact Codec profile"
        : eligibility === undefined
          ? "Host source preflight unavailable"
          : eligibility.reason === "compatible"
            ? undefined
            : compatibilityReasonLabel(eligibility.reason);
    return {
      archiveSha256: cartridge.archiveSha256,
      label: `${cartridge.paths[0]?.fileName ?? cartridge.cartridgeId} · ${shortHash(cartridge.archiveSha256)}`,
      available: cartridge.availability === "present",
      ...(incompatibilityReason === undefined ? {} : { incompatibilityReason }),
    };
  }

  function exactRuntimeAvailable(): boolean {
    if (
      selectedCodec?.reason !== "compatible" ||
      selectedProfile === undefined ||
      runtimeOptions?.reason !== "compatible"
    ) {
      return false;
    }
    return runtimeOptions.externalAssets.every(
      (asset) => !asset.required || asset.bound,
    );
  }

  function describeRuntimeAvailability(): string {
    if (selectedCodec === undefined) return "Choose an exact Codec version.";
    if (selectedCodec.reason !== "compatible") {
      return compatibilityReasonLabel(selectedCodec.reason);
    }
    if (selectedDevice === "") return "Choose the negotiated runtime device.";
    if (discovery === null) return "Exact Codec discovery has not completed.";
    if (discovery.reason !== "compatible") {
      return compatibilityReasonLabel(discovery.reason);
    }
    if (selectedProfile === undefined) return "Choose an exact Codec profile.";
    if (runtimeOptions === null)
      return "Exact source preflight has not completed.";
    if (runtimeOptions.reason !== "compatible") {
      return compatibilityReasonLabel(runtimeOptions.reason);
    }
    const missing = runtimeOptions.externalAssets.filter(
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
    draft = nextDraft;
    await run(async () => {
      const wire = buildGenericDeckOpenDraft(model, draft, library.cartridges);
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
      selectedSessionId = opened.sessionId;
      sessions = await genericDeckClient.foregroundSet(opened.sessionId);
      message = `Warm session ${opened.sessionId} owns foreground output.`;
      await establishViewport();
    });
  }

  async function refreshSessions(): Promise<void> {
    try {
      applySessions(await genericDeckClient.sessionsGet());
    } catch (error) {
      fail(error);
    }
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
      selectedSessionId = foreground?.sessionId ?? "";
      hydratedSessionId = "";
      sessionSnapshotValid = false;
    }
  }

  function hydrateSelectedSession(session: GenericDeckSessionView): void {
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
      outputFullscreen = null;
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
    sessions = {
      ...sessions,
      sessions: sessions.sessions.map((session) =>
        session.sessionId === selectedSessionId
          ? { ...session, runtime: next }
          : session,
      ),
    };
  }

  async function foregroundSession(
    session: GenericDeckSessionView,
  ): Promise<void> {
    await run(async () => {
      sessions = await genericDeckClient.foregroundSet(session.sessionId);
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
    await run(async () => {
      await genericDeckClient.close(sessionId);
      applySessions(await genericDeckClient.sessionsGet());
      message = `Warm session ${sessionId} closed explicitly.`;
    });
  }

  async function runSessionAction(
    operation: (sessionId: string) => Promise<GenericDeckRuntimeView>,
  ): Promise<void> {
    if (selectedSession === undefined || !selectedSessionReady) return;
    await run(async () => {
      applyRuntime(await operation(selectedSession!.sessionId));
    });
  }

  async function commitControls(
    _controls: Record<string, DeckUiScalar>,
  ): Promise<void> {
    const wire = buildGenericDeckOpenDraft(model, draft, library.cartridges);
    await runSessionAction((sessionId) =>
      genericDeckClient.controlsSet(sessionId, wire.controls),
    );
  }

  async function commitRoles(_roles: Record<string, number>): Promise<void> {
    const wire = buildGenericDeckOpenDraft(model, draft, library.cartridges);
    await runSessionAction((sessionId) =>
      genericDeckClient.rolesSet(sessionId, wire.roles),
    );
  }

  async function commitTransport(
    _playing: readonly boolean[],
    _loops: readonly boolean[],
  ): Promise<void> {
    const wire = buildGenericDeckOpenDraft(model, draft, library.cartridges);
    await runSessionAction((sessionId) =>
      genericDeckClient.transportSet(sessionId, wire.sourceTransport),
    );
  }

  async function commitSeed(_seed: number): Promise<void> {
    const wire = buildGenericDeckOpenDraft(model, draft, library.cartridges);
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

  async function pollForegroundState(): Promise<void> {
    const sessionId = selectedSessionId;
    if (sessionId === "" || busy) return;
    try {
      applySession(await genericDeckClient.statusGet(sessionId));
      capture = await genericDeckClient.captureStatusGet(sessionId);
      await publishCompletedCapture(capture);
      recording = await genericDeckClient.recordingStatusGet(sessionId);
      spout = await genericDeckClient.spoutStatusGet(sessionId);
      outputFullscreen = await genericDeckClient.fullscreenStatusGet(sessionId);
      if (spout !== null && !spoutNameDirty) spoutName = spout.requestedName;
    } catch (error) {
      fail(error);
    }
  }

  async function captureAction(
    mode: "snapshot" | "live_capture",
  ): Promise<void> {
    if (!captureAvailable || selectedSession === undefined) return;
    await run(async () => {
      if (
        mode === "live_capture" &&
        capture?.mode === "live_capture" &&
        capture.state === "capturing"
      ) {
        capture = await genericDeckClient.captureStop(
          selectedSession!.sessionId,
        );
      } else {
        capture = await genericDeckClient.captureStart(
          selectedSession!.sessionId,
          mode,
        );
      }
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
    publishedCaptureKey = key;
    onLibraryChanged(await librarySnapshot());
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
        return;
      }
      const started = await genericDeckClient.recordingStart(
        selectedSession!.sessionId,
      );
      if (started !== null) recording = started;
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
        library.cartridges,
        library.deckSession.activeCollectionId,
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
      onLibraryChanged(incoming);
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
    if (element !== null) viewportResizeObserver?.observe(element);
    scheduleViewportSync();
  }

  async function establishViewport(): Promise<void> {
    const sessionId = selectedSession?.sessionId;
    if (!active || sessionId === undefined || viewportBusy) return;
    if (viewportSessionId === sessionId && viewportEpoch !== null) {
      scheduleViewportSync();
      return;
    }
    viewportBusy = true;
    const requested = sessionId;
    try {
      const viewport = await genericDeckClient.viewportSessionBegin(requested);
      if (selectedSession?.sessionId !== requested) return;
      viewportSessionId = requested;
      viewportEpoch = viewport.epoch;
      viewportRevision = 0;
      viewportApplied = null;
      scheduleViewportSync();
    } catch (error) {
      fail(error);
    } finally {
      viewportBusy = false;
    }
  }

  function scheduleViewportSync(): void {
    if (viewportFrame !== null) return;
    viewportFrame = globalThis.requestAnimationFrame(() => {
      viewportFrame = null;
      void syncViewport();
    });
  }

  async function syncViewport(): Promise<void> {
    const epoch = viewportEpoch;
    const sessionId = selectedSession?.sessionId;
    if (epoch === null || sessionId === undefined || viewportAnchor === null)
      return;
    const revision = nextEmbeddedViewportRevision(viewportRevision);
    if (revision === null) return;
    const rect = viewportAnchor.getBoundingClientRect();
    const scaleFactor = globalThis.devicePixelRatio;
    const inside = embeddedViewportFullyInsideClient(
      rect,
      document.documentElement.clientWidth,
      document.documentElement.clientHeight,
      scaleFactor,
    );
    const visible = active && selectedSession?.foreground === true && inside;
    const bounds = visible
      ? buildEmbeddedViewportBounds(epoch, revision, rect, scaleFactor, true)
      : hiddenEmbeddedViewportBounds(epoch, revision, scaleFactor);
    if (bounds === null) return;
    viewportRevision = revision;
    try {
      await genericDeckClient.viewportSetBounds(sessionId, bounds);
      viewportApplied = bounds;
    } catch (error) {
      fail(error);
    }
  }

  async function hideViewport(): Promise<void> {
    const epoch = viewportEpoch;
    const sessionId = viewportSessionId;
    const revision = nextEmbeddedViewportRevision(viewportRevision);
    if (epoch === null || sessionId === "" || revision === null) return;
    const bounds = hiddenEmbeddedViewportBounds(
      epoch,
      revision,
      globalThis.devicePixelRatio,
    );
    if (bounds === null) return;
    viewportRevision = revision;
    try {
      await genericDeckClient.viewportSetBounds(sessionId, bounds);
      viewportApplied = bounds;
    } catch {
      // Best-effort teardown; the host destroys the child with the session.
    }
  }

  async function leaveSurface(): Promise<void> {
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
      outputFullscreen = await genericDeckClient.fullscreenSet(
        selectedSession!.sessionId,
        !outputFullscreen,
      );
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

  function captureActive(): boolean {
    return (
      capture?.state === "starting" ||
      capture?.state === "capturing" ||
      capture?.state === "finalizing"
    );
  }

  function recordingActive(): boolean {
    return (
      recording?.state === "armed" ||
      recording?.state === "recording" ||
      recording?.state === "finalizing"
    );
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
      <div>
        <span>New warm-session negotiation</span>
        <strong>{model.deckId}@{model.deckVersion}</strong>
      </div>
      <small>{message}</small>
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
          onchange={() => void discoverRuntime()}
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
    {/if}
    {#if errorCode !== ""}
      <p class="runtime-error" role="alert">
        <strong>{errorCode}</strong> · {message}
      </p>
    {/if}
  </section>

  <section class="session-rail" aria-label="Warm generic Deck sessions">
    <header>
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
    </header>
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
            disabled={busy}>Close</button
          >
        </article>
      {/each}
      {#if sessions.sessions.length === 0}<p>
          No warm Protocol 2 sessions.
        </p>{/if}
    </div>
    {#if !loadAvailable}
      <p class="capacity-note">
        {SESSION_CAPACITY_CODE} · close one session explicitly; no LRU eviction occurs.
      </p>
    {/if}
    {#if errorCode === SESSION_PINNED_CODE}
      <p class="capacity-note">
        {SESSION_PINNED_CODE} · capture or MP4 owns the foreground lease.
      </p>
    {/if}
  </section>

  <section class="host-tools" aria-label="Generic Deck host tools">
    <div class="preset-tools">
      <strong>Preset v2</strong>
      <button
        type="button"
        disabled={presetBusy || busy}
        onclick={() => void loadPreset()}>Load preset</button
      >
      <button
        type="button"
        disabled={presetBusy || busy}
        onclick={() => void savePreset()}>Save preset</button
      >
      <small>{presetMessage}</small>
    </div>
    <div class="spout-tools">
      <strong>Spout2 · {describeSpout(spout)}</strong>
      <input
        value={spoutName}
        maxlength="255"
        disabled={!spoutControlsFor(spout, busy).rename}
        oninput={(event) => {
          spoutName = event.currentTarget.value;
          spoutNameDirty = true;
        }}
      />
      <button
        type="button"
        disabled={!spoutControlsFor(spout, busy).rename ||
          !spoutNameDirty ||
          spoutName.trim() === ""}
        onclick={() => void configureSpout(spoutName.trim(), null)}
        >Apply name</button
      >
      <button
        type="button"
        disabled={!spoutControlsFor(spout, busy).toggle}
        onclick={() => void configureSpout(null, !(spout?.enabled ?? false))}
        >{spout?.enabled ? "Disable sender" : "Enable sender"}</button
      >
      <small
        >{spout === null
          ? "No foreground output"
          : `${spout.width}×${spout.height} · ${spout.submittedFrames} frames`}</small
      >
    </div>
    <div class="recording-tools">
      <strong>Decoded MP4 · video-only H.264</strong>
      <button
        type="button"
        disabled={!recordingAvailable ||
          (!decodedRecordingControls(
            recording ?? IDLE_DECODED_RECORDING,
            selectedSession?.foreground === true,
            busy,
          ).start &&
            !decodedRecordingControls(
              recording ?? IDLE_DECODED_RECORDING,
              selectedSession?.foreground === true,
              busy,
            ).stop)}
        onclick={() => void recordingAction()}
        >{decodedRecordingControls(
          recording ?? IDLE_DECODED_RECORDING,
          selectedSession?.foreground === true,
          busy,
        ).stop
          ? "Stop MP4"
          : "Record MP4…"}</button
      >
      <small
        >{captureActive()
          ? "Latent capture pins the foreground output lease."
          : describeDecodedRecording(
              recording ?? IDLE_DECODED_RECORDING,
            )}</small
      >
    </div>
  </section>

  {#key `${model.exactKey}:${draftRevision}`}
    <DeckFaceplateRenderer
      {model}
      initialDraft={draft}
      {sourceOptions}
      {active}
      {runtimeAvailable}
      {runtimeUnavailableReason}
      {loadAvailable}
      loadUnavailableReason={`${SESSION_CAPACITY_CODE}: close one of the four warm sessions explicitly.`}
      runtimeLoaded={selectedSessionReady}
      runtimeBusy={busy}
      statusMessage={statusMessage()}
      {playheads}
      captureState={captureState()}
      {captureAvailable}
      captureActive={captureActive()}
      liveCaptureActive={capture?.mode === "live_capture" &&
        capture.state === "capturing"}
      {captureUnavailableReason}
      {outputFullscreen}
      onDraftChange={(next) => {
        draft = next;
      }}
      onLoad={openDeck}
      onRestart={restart}
      onProcessOnce={processOnce}
      onControlsCommit={commitControls}
      onRolesCommit={commitRoles}
      onTransportCommit={commitTransport}
      onSeedCommit={commitSeed}
      onCapture={captureAction}
      onFullscreenToggle={toggleFullscreen}
      onMonitorAnchor={monitorAnchor}
    />
  {/key}
</section>

<style>
  .generic-workspace {
    display: grid;
    gap: 8px;
  }

  .runtime-config,
  .session-rail,
  .host-tools {
    border: 1px solid #465149;
    color: #dce4de;
    background: #111713;
  }

  .runtime-config > header,
  .session-rail > header,
  .host-tools,
  .config-grid,
  .asset-grid,
  .session-list {
    display: flex;
    gap: 8px;
    padding: 8px 10px;
  }

  .runtime-config > header,
  .session-rail > header {
    align-items: center;
    justify-content: space-between;
    border-bottom: 1px solid #39443d;
  }

  .runtime-config > header div,
  .runtime-config label,
  .asset-grid article > div,
  .preset-tools,
  .spout-tools,
  .recording-tools {
    display: grid;
    gap: 4px;
  }

  .runtime-config span,
  .runtime-config small,
  .session-rail span,
  .session-rail small,
  .host-tools small {
    color: #88948c;
    font-size: 0.62rem;
  }

  .config-grid {
    display: grid;
    grid-template-columns: repeat(4, minmax(160px, 1fr));
  }

  .asset-grid,
  .session-list {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(270px, 1fr));
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

  .host-tools {
    display: grid;
    grid-template-columns: 1fr 1fr 1fr;
  }

  .preset-tools,
  .spout-tools,
  .recording-tools {
    grid-template-columns: auto auto auto minmax(0, 1fr);
    align-items: center;
  }

  @media (max-width: 1180px) {
    .config-grid,
    .host-tools {
      grid-template-columns: 1fr 1fr;
    }

    .preset-tools,
    .spout-tools,
    .recording-tools {
      grid-template-columns: 1fr 1fr;
    }
  }
</style>
