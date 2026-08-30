<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { onMount, tick } from "svelte";
  import { q4Client, type StopQ4Listener } from "./q4-client";
  import {
    DEFAULT_Q4_BACKEND,
    DEFAULT_Q4_CAPTURE,
    DEFAULT_Q4_CONTROLS,
    DEFAULT_Q4_ROLES,
    DEFAULT_Q4_STATUS,
    DEFAULT_Q4_TRANSPORT,
    MAX_SAFE_Q4_SEED,
    Q4_SLOTS,
    buildQ4OpenRequest,
    chooseQ4Sources,
    copyQ4Controls,
    copyQ4Roles,
    copyQ4Transport,
    findQ4DuplicateSources,
    isQ4CaptureActive,
    parseQ4Seed,
    q4LiveCaptureAction,
    resolveQ4DonorWeights,
    setQ4SlotLoop,
    setQ4SlotPlaying,
    validateQ4Roles,
    type Q4BackendView,
    type Q4CaptureView,
    type Q4Controls,
    type Q4ErrorEvent,
    type Q4LiveCaptureAction,
    type Q4LoadedSources,
    type Q4Roles,
    type Q4Slot,
    type Q4SourceSelection,
    type Q4Status,
    type Q4Transport,
  } from "./q4-model";
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
    type SpoutControls,
    type SpoutStatus,
  } from "./output-model";
  import {
    buildQ4Preset,
    mergePresetSourceOptions,
    presetCollectionExists,
    q4ControlsFromPreset,
    q4RolesFromPreset,
    resolvePresetLoopDraft,
    resolvePresetSources,
    stagePresetLibraryLoad,
    transitionPresetLoopDraft,
    type DeckPreset,
    type PresetLoopDraft,
    type Q4DeckPreset,
  } from "./preset-model";

  type HostState = "checking" | "ready" | "pending" | "error";

  let bankView: LibraryView = EMPTY_LIBRARY_VIEW;
  let backend: Q4BackendView = { ...DEFAULT_Q4_BACKEND };
  let status: Q4Status = freshStatus();
  let controlsDraft: Q4Controls = copyQ4Controls(DEFAULT_Q4_CONTROLS);
  let rolesDraft: Q4Roles = copyQ4Roles(DEFAULT_Q4_ROLES);
  let sourceAHash = "";
  let sourceBHash = "";
  let sourceCHash = "";
  let sourceDHash = "";
  let seedDraft = "0";
  let hostState: HostState = "checking";
  let hostMessage = "Checking Q4 host contract…";
  let bankError = "";
  let resetMessage = "";
  let bankBusy = false;
  let backendBusy = false;
  let hostBusy = false;
  let captureBusy = false;
  let spoutBusy = false;
  let spoutPending = false;
  let controlsDirty = false;
  let rolesDirty = false;
  let seedDirty = false;
  let capture: Q4CaptureView = { ...DEFAULT_Q4_CAPTURE };
  let spout: SpoutStatus | null = null;
  let spoutName = "LatentDeck LD-Q4 Output";
  let spoutNameDirty = false;
  let spoutMessage = "";
  let presetBusy = false;
  let presetMessage = "";
  let presetResolvedSources: (CartridgeView | null)[] = [];
  let compatibilityReasons: ReadonlyMap<string, readonly string[]> = new Map();
  let compatibilityReady = false;
  let compatibilityRequest = 0;
  type Q4PresetLoops = Pick<
    Q4Transport,
    "loopA" | "loopB" | "loopC" | "loopD"
  >;
  let presetLoopDraft: PresetLoopDraft<Q4PresetLoops> | null = null;

  let activeBank: CollectionView | undefined;
  let sourceA: CartridgeView | undefined;
  let sourceB: CartridgeView | undefined;
  let sourceC: CartridgeView | undefined;
  let sourceD: CartridgeView | undefined;
  let sourceOptions: CartridgeView[] = [];
  let presentCount = 0;
  let captureActive = false;
  let liveCaptureAction: Q4LiveCaptureAction = "start";
  let rolesValid = true;
  let resolvedWeights: readonly [number, number, number] = [
    1 / 3,
    1 / 3,
    1 / 3,
  ];
  let spoutControls: SpoutControls = { rename: false, toggle: false };
  let spoutStateLabel = "Output inactive";
  let sourceSelection: Q4SourceSelection = {
    sourceAHash: "",
    sourceBHash: "",
    sourceCHash: "",
    sourceDHash: "",
  };
  let duplicateSources = findQ4DuplicateSources(sourceSelection);
  let distinctSourceCount = 0;
  let allSourcesReady = false;
  let loadedSourceSelection: Q4SourceSelection = {
    sourceAHash: "",
    sourceBHash: "",
    sourceCHash: "",
    sourceDHash: "",
  };
  let loadedDuplicateSources = findQ4DuplicateSources(loadedSourceSelection);
  let loadedDistinctSourceCount = 0;
  let incompatibleSelectedSlots: Q4Slot[] = [];
  let selectedSourcesCompatible = false;

  $: activeBank = bankView.collections.find(
    (collection) => collection.id === bankView.deckSession.activeCollectionId,
  );
  $: sourceOptions = mergePresetSourceOptions(
    bankView.cartridges,
    presetResolvedSources.filter(
      (source) =>
        source !== null &&
        [sourceAHash, sourceBHash, sourceCHash, sourceDHash].includes(
          source.archiveSha256,
        ),
    ),
  );
  $: sourceA = sourceFor(sourceAHash);
  $: sourceB = sourceFor(sourceBHash);
  $: sourceC = sourceFor(sourceCHash);
  $: sourceD = sourceFor(sourceDHash);
  $: sourceSelection = { sourceAHash, sourceBHash, sourceCHash, sourceDHash };
  $: duplicateSources = findQ4DuplicateSources(sourceSelection);
  $: distinctSourceCount = new Set(
    Object.values(sourceSelection).filter((hash) => hash !== ""),
  ).size;
  $: allSourcesReady = [sourceA, sourceB, sourceC, sourceD].every(
    (source) => source?.availability === "present",
  );
  $: incompatibleSelectedSlots = Q4_SLOTS.filter(
    (slot) =>
      slot !== rolesDraft.carrier &&
      (compatibilityReasons.get(sourceHash(slot))?.length ?? 0) > 0,
  );
  $: selectedSourcesCompatible =
    compatibilityReady &&
    allSourcesReady &&
    incompatibleSelectedSlots.length === 0;
  $: loadedSourceSelection =
    status.sources === null
      ? { sourceAHash: "", sourceBHash: "", sourceCHash: "", sourceDHash: "" }
      : {
          sourceAHash: status.sources.sourceA.archiveSha256,
          sourceBHash: status.sources.sourceB.archiveSha256,
          sourceCHash: status.sources.sourceC.archiveSha256,
          sourceDHash: status.sources.sourceD.archiveSha256,
        };
  $: loadedDuplicateSources = findQ4DuplicateSources(loadedSourceSelection);
  $: loadedDistinctSourceCount = new Set(
    Object.values(loadedSourceSelection).filter((hash) => hash !== ""),
  ).size;
  $: presentCount = bankView.cartridges.filter(
    (cartridge) => cartridge.availability === "present",
  ).length;
  $: captureActive = isQ4CaptureActive(capture.state);
  $: liveCaptureAction = q4LiveCaptureAction(capture);
  $: rolesValid = validateQ4Roles(rolesDraft);
  $: resolvedWeights = safeResolvedWeights(controlsDraft);
  $: spoutControls = spoutControlsFor(spout, spoutBusy || hostBusy);
  $: spoutStateLabel = describeSpout(spout);

  onMount(() => {
    let disposed = false;
    const stops: StopQ4Listener[] = [];
    let spoutPoll: ReturnType<typeof setInterval> | undefined;
    void (async () => {
      try {
        stops.push(
          ...(await Promise.all([
            q4Client.onStatus((incoming) => !disposed && applyStatus(incoming)),
            q4Client.onError((incoming) => !disposed && applyError(incoming)),
            q4Client.onCapture(
              (incoming) => !disposed && applyCapture(incoming),
            ),
            q4Client.onCaptureError(
              (incoming) => !disposed && applyError(incoming),
            ),
          ])),
        );
      } catch (error) {
        if (!disposed) markFailure(error);
      }
      if (!disposed) {
        await refreshBank();
        await refreshBackend();
        await refreshStatus();
        await refreshCapture();
        await refreshSpout();
        if (!disposed) {
          spoutPoll = setInterval(() => void refreshSpout(), 250);
        }
      }
    })();
    return () => {
      disposed = true;
      if (spoutPoll !== undefined) clearInterval(spoutPoll);
      for (const stop of stops) stop();
    };
  });

  function freshStatus(): Q4Status {
    return {
      ...DEFAULT_Q4_STATUS,
      controls: copyQ4Controls(DEFAULT_Q4_CONTROLS),
      roles: copyQ4Roles(DEFAULT_Q4_ROLES),
      transport: copyQ4Transport(DEFAULT_Q4_TRANSPORT),
      pendingResetReasons: [],
    };
  }

  function sourceFor(hash: string): CartridgeView | undefined {
    return sourceOptions.find(
      (cartridge) => cartridge.archiveSha256 === hash,
    );
  }

  function applySourceChoices(): void {
    const choices = chooseQ4Sources(
      bankView.cartridges,
      { sourceAHash, sourceBHash, sourceCHash, sourceDHash },
      { preserveExplicitDuplicates: true },
    );
    if (
      choices.sourceAHash !== sourceAHash ||
      choices.sourceBHash !== sourceBHash ||
      choices.sourceCHash !== sourceCHash ||
      choices.sourceDHash !== sourceDHash
    ) {
      discardPresetLoopDraft();
    }
    sourceAHash = choices.sourceAHash;
    sourceBHash = choices.sourceBHash;
    sourceCHash = choices.sourceCHash;
    sourceDHash = choices.sourceDHash;
  }

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
          [sourceAHash, sourceBHash, sourceCHash, sourceDHash].includes(
            source.archiveSha256,
          ),
      );
      applySourceChoices();
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
      discardPresetLoopDraft();
      applySourceChoices();
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
      sourceC?.availability !== "present" ||
      sourceD?.availability !== "present" ||
      !selectedSourcesCompatible
    ) {
      presetMessage =
        "Assign four compatible present cartridges and an active Bank before saving.";
      return;
    }
    const seed = parseQ4Seed(seedDraft);
    if (seed === null || !rolesValid) {
      presetMessage =
        seed === null
          ? `Seed must be 0…${MAX_SAFE_Q4_SEED}.`
          : "Carrier and donors must be an exact A/B/C/D permutation.";
      return;
    }
    presetBusy = true;
    presetMessage = "";
    try {
      const loops = resolvePresetLoopDraft(presetLoopDraft, status.transport);
      const preset = buildQ4Preset(
        activeBank.id,
        [sourceA, sourceB, sourceC, sourceD],
        controlsDraft,
        rolesDraft,
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
        result === null ? "Preset save cancelled." : "Q4 preset saved.";
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
      if (document.deck_type !== "LD-Q4") {
        presetMessage = `This is a ${document.deck_type} preset; LD-Q4 was not changed.`;
        return;
      }
      const preset: Q4DeckPreset = document;
      if (!presetCollectionExists(preset, bankView.collections)) {
        presetMessage =
          "The saved Collection is missing. The current Bank and sources were not changed.";
        return;
      }
      const identities = [
        preset.slots.a,
        preset.slots.b,
        preset.slots.c,
        preset.slots.d,
      ];
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
      [sourceAHash, sourceBHash, sourceCHash, sourceDHash] = resolution.hashes;
      // Protect the complete loaded draft before yielding: worker status events
      // may arrive during compatibility preflight and must not overwrite it.
      controlsDirty = true;
      rolesDirty = true;
      seedDirty = true;
      controlsDraft = q4ControlsFromPreset(preset);
      rolesDraft = q4RolesFromPreset(preset);
      seedDraft = String(preset.seed);
      presetLoopDraft = transitionPresetLoopDraft(presetLoopDraft, {
        type: "preset-loaded",
        loops: {
          loopA: preset.loops.loop_a,
          loopB: preset.loops.loop_b,
          loopC: preset.loops.loop_c,
          loopD: preset.loops.loop_d,
        },
      });
      await tick();
      await refreshSpatialCompatibility();
      presetMessage = [
        "Q4 preset loaded as a draft. Press Load Q4 to apply it.",
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
    const referenceArchiveSha256 = sourceHash(rolesDraft.carrier);
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

  async function rolesChanged(): Promise<void> {
    discardPresetLoopDraft();
    rolesDirty = true;
    await tick();
    await refreshSpatialCompatibility();
  }

  async function refreshBackend(): Promise<void> {
    if (backendBusy) return;
    backendBusy = true;
    try {
      backend = await q4Client.backendStatusGet();
    } catch (error) {
      backend = {
        ...DEFAULT_Q4_BACKEND,
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
      backend = await q4Client.selectDecoder();
      if (backend.state === "ready") {
        status = freshStatus();
        hostMessage = "Decoder validated · choose four cartridges.";
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

  async function refreshStatus(): Promise<void> {
    await runHostAction(async () => applyStatus(await q4Client.statusGet()));
  }

  async function refreshCapture(): Promise<void> {
    try {
      applyCapture(await q4Client.captureStatusGet());
    } catch (error) {
      capture = {
        ...DEFAULT_Q4_CAPTURE,
        state: "error",
        detail: describeCommandError(error),
      };
    }
  }

  async function refreshSpout(): Promise<void> {
    if (spoutPending) return;
    spoutPending = true;
    try {
      applySpoutStatus(await q4Client.spoutStatusGet());
      spoutMessage = "";
    } catch (error) {
      spoutMessage = describeCommandError(error);
    } finally {
      spoutPending = false;
    }
  }

  async function openDeck(): Promise<void> {
    if (presetBusy) return;
    if (backend.state !== "ready") {
      hostState = "error";
      hostMessage =
        backend.detail ?? "Select a compatible TAEH3 decoder first.";
      return;
    }
    if (
      sourceA === undefined ||
      sourceB === undefined ||
      sourceC === undefined ||
      sourceD === undefined
    ) {
      hostState = "error";
      hostMessage =
        "Assign a present cartridge to all four slots. Reused sources remain explicitly marked.";
      return;
    }
    if (!selectedSourcesCompatible) {
      hostState = "error";
      hostMessage = compatibilityReady
        ? `Donor slots ${incompatibleSelectedSlots.join("/")} are incompatible with carrier ${rolesDraft.carrier}. Use explicit Toolkit Align/Crop to create compatible cartridges.`
        : "Signal compatibility has not been verified; refresh the active Bank.";
      return;
    }
    const seed = parseQ4Seed(seedDraft);
    if (seed === null || !rolesValid) {
      hostState = "error";
      hostMessage =
        seed === null
          ? `Seed must be 0…${MAX_SAFE_Q4_SEED}.`
          : "Roles must be an A/B/C/D permutation.";
      return;
    }
    await runHostAction(async () => {
      const pendingLoops = presetLoopDraft?.loops;
      const transport =
        pendingLoops === undefined
          ? status.loaded
            ? status.transport
            : DEFAULT_Q4_TRANSPORT
          : {
              ...DEFAULT_Q4_TRANSPORT,
              loopA: pendingLoops.loopA,
              loopB: pendingLoops.loopB,
              loopC: pendingLoops.loopC,
              loopD: pendingLoops.loopD,
              playingA: false,
              playingB: false,
              playingC: false,
              playingD: false,
            };
      const request = buildQ4OpenRequest(
        [
          sourceA as CartridgeView,
          sourceB as CartridgeView,
          sourceC as CartridgeView,
          sourceD as CartridgeView,
        ],
        rolesDraft,
        controlsDraft,
        transport,
        seed,
      );
      applyStatus(await q4Client.open(request));
      presetLoopDraft = null;
      await refreshSpout();
      controlsDirty = false;
      rolesDirty = false;
      seedDirty = false;
    });
  }

  async function applyControls(): Promise<void> {
    await runHostAction(async () => {
      resolveQ4DonorWeights(controlsDraft);
      await q4Client.controlsSet(copyQ4Controls(controlsDraft));
      applyStatus(await q4Client.statusGet());
      controlsDirty = false;
    });
  }

  async function applyRoles(): Promise<void> {
    if (!rolesValid) return;
    await runHostAction(async () => {
      await q4Client.rolesSet(copyQ4Roles(rolesDraft));
      applyStatus(await q4Client.statusGet());
      rolesDirty = false;
    });
  }

  async function applySeed(): Promise<void> {
    const seed = parseQ4Seed(seedDraft);
    if (seed === null) {
      hostState = "error";
      hostMessage = `Seed must be 0…${MAX_SAFE_Q4_SEED}.`;
      return;
    }
    await runHostAction(async () => {
      await q4Client.seedSet(seed);
      applyStatus(await q4Client.statusGet());
      seedDirty = false;
    });
  }

  async function setTransport(transport: Q4Transport): Promise<void> {
    if (captureActive) return;
    await runHostAction(async () => {
      await q4Client.transportSet(transport);
      applyStatus(await q4Client.statusGet());
    });
  }

  async function togglePlay(slot: Q4Slot): Promise<void> {
    const playing = status.transport[`playing${slot}`];
    await setTransport(setQ4SlotPlaying(status.transport, slot, !playing));
  }

  async function toggleLoop(slot: Q4Slot, event: Event): Promise<void> {
    discardPresetLoopDraft();
    await setTransport(
      setQ4SlotLoop(
        status.transport,
        slot,
        (event.currentTarget as HTMLInputElement).checked,
      ),
    );
  }

  async function restart(): Promise<void> {
    if (captureActive) return;
    await runHostAction(async () => {
      resetMessage = "Restart requested · waiting for causal reset barrier.";
      applyStatus(await q4Client.restart());
    });
  }

  async function snapshot(): Promise<void> {
    if (!status.loaded || captureActive || captureBusy) return;
    captureBusy = true;
    try {
      const started = await q4Client.captureSnapshot();
      if (started !== null) applyCapture(started);
    } catch (error) {
      applyError({
        code: "capture.snapshot_failed",
        detail: describeCommandError(error),
      });
    } finally {
      captureBusy = false;
    }
  }

  async function toggleLiveCapture(): Promise<void> {
    if (!status.loaded || captureBusy || liveCaptureAction === null) return;
    captureBusy = true;
    try {
      const incoming =
        liveCaptureAction === "stop"
          ? await q4Client.captureLiveStop()
          : await q4Client.captureLiveStart();
      if (incoming !== null) applyCapture(incoming);
    } catch (error) {
      applyError({
        code: "capture.live_failed",
        detail: describeCommandError(error),
      });
    } finally {
      captureBusy = false;
    }
  }

  async function applySpoutName(): Promise<void> {
    const name = spoutName.trim();
    if (name.length === 0 || !spoutControls.rename) return;
    await configureSpout(name, null);
  }

  async function toggleSpout(): Promise<void> {
    if (spout === null || !spoutControls.toggle) return;
    await configureSpout(null, !spout.enabled);
  }

  async function configureSpout(
    name: string | null,
    enabled: boolean | null,
  ): Promise<void> {
    if (spoutBusy) return;
    spoutBusy = true;
    spoutMessage = "";
    try {
      const incoming = await q4Client.spoutConfigure({ name, enabled });
      spout = incoming;
      if (name !== null) {
        if (incoming.requestedName === name) {
          spoutName = incoming.requestedName;
          spoutNameDirty = false;
        } else {
          spoutMessage =
            incoming.lastErrorCode ?? "Sender name was not accepted.";
        }
      } else if (!spoutNameDirty) {
        spoutName = incoming.requestedName;
      }
      if (incoming.lastErrorCode !== null)
        spoutMessage = incoming.lastErrorCode;
    } catch (error) {
      spoutMessage = describeCommandError(error);
    } finally {
      spoutBusy = false;
    }
  }

  async function runHostAction(action: () => Promise<void>): Promise<void> {
    if (hostBusy || presetBusy) return;
    hostBusy = true;
    hostState = "pending";
    try {
      await action();
    } catch (error) {
      markFailure(error);
    } finally {
      hostBusy = false;
    }
  }

  function applyStatus(incoming: Q4Status): void {
    status = {
      ...incoming,
      controls: copyQ4Controls(incoming.controls),
      roles: copyQ4Roles(incoming.roles),
      transport: copyQ4Transport(incoming.transport),
      pendingResetReasons: [...incoming.pendingResetReasons],
    };
    if (incoming.loaded) {
      if (!controlsDirty) controlsDraft = copyQ4Controls(incoming.controls);
      if (!rolesDirty) rolesDraft = copyQ4Roles(incoming.roles);
      if (!seedDirty) seedDraft = String(incoming.seed);
      hostState = incoming.pendingReset ? "pending" : "ready";
      hostMessage = incoming.pendingReset
        ? "Reset barrier pending."
        : "Q4 worker acknowledged.";
      if (!incoming.pendingReset) resetMessage = "";
    } else {
      hostState = "ready";
      hostMessage = "Q4 worker is not loaded.";
    }
  }

  function applyCapture(incoming: Q4CaptureView): void {
    capture = { ...incoming };
  }

  function applySpoutStatus(incoming: SpoutStatus | null): void {
    spout = incoming;
    if (incoming !== null && !spoutNameDirty) {
      spoutName = incoming.requestedName;
    }
  }

  function applyError(incoming: Q4ErrorEvent): void {
    hostState = "error";
    hostMessage = `${incoming.code}: ${incoming.detail}`;
  }

  function markFailure(error: unknown): void {
    applyError({ code: "deck.q4.host", detail: describeCommandError(error) });
  }

  function cartridgeLabel(cartridge: CartridgeView): string {
    const file = cartridge.paths[0]?.fileName ?? "Unavailable";
    const format = describeIntrinsicFormat(cartridge);
    const latentGrid =
      format.latentGrid === null ? "" : ` · LATENT ${format.latentGrid}`;
    return `${file} · ${format.aspectLabel} · ${format.decodedGeometry}${latentGrid} · ${shortHash(cartridge.archiveSha256)}`;
  }

  function safeResolvedWeights(
    controls: Q4Controls,
  ): readonly [number, number, number] {
    try {
      return resolveQ4DonorWeights(controls);
    } catch {
      return [0, 0, 0];
    }
  }

  function sourceBySlot(slot: Q4Slot): CartridgeView | undefined {
    return { A: sourceA, B: sourceB, C: sourceC, D: sourceD }[slot];
  }

  function sourceHash(slot: Q4Slot): string {
    return { A: sourceAHash, B: sourceBHash, C: sourceCHash, D: sourceDHash }[
      slot
    ];
  }

  async function setSourceHash(slot: Q4Slot, value: string): Promise<void> {
    if (presetBusy) return;
    discardPresetLoopDraft();
    if (slot === "A") sourceAHash = value;
    if (slot === "B") sourceBHash = value;
    if (slot === "C") sourceCHash = value;
    if (slot === "D") sourceDHash = value;
    if (slot === rolesDraft.carrier) {
      await tick();
      await refreshSpatialCompatibility();
    }
  }

  function discardPresetLoopDraft(): void {
    if (presetLoopDraft === null) return;
    presetLoopDraft = transitionPresetLoopDraft(presetLoopDraft, {
      type: "manual-divergence",
    });
    presetMessage =
      "Preset loop draft discarded after a manual change; current Deck loop state will be used.";
  }

  function duplicateSlotsLabel(slot: Q4Slot): string {
    const duplicate = duplicateSources.find((group) =>
      group.slots.includes(slot),
    );
    return duplicate === undefined
      ? ""
      : `REUSED ARCHIVE · SLOTS ${duplicate.slots.join(" / ")}`;
  }

  function compatibilityReasonsFor(
    cartridge: CartridgeView,
  ): readonly string[] {
    return compatibilityReasons.get(cartridge.archiveSha256) ?? [];
  }

  function compatibilityLabel(
    slot: Q4Slot,
    cartridge: CartridgeView,
  ): string {
    const reasons = compatibilityReasonsFor(cartridge);
    return slot === rolesDraft.carrier || reasons.length === 0
      ? cartridgeLabel(cartridge)
      : `${cartridgeLabel(cartridge)} · INCOMPATIBLE: ${reasons.join("; ")}`;
  }

  function isIncompatibleCandidate(
    slot: Q4Slot,
    cartridge: CartridgeView,
  ): boolean {
    return (
      slot !== rolesDraft.carrier &&
      (!compatibilityReady || compatibilityReasonsFor(cartridge).length > 0)
    );
  }

  function loadedSourceBySlot(
    slot: Q4Slot,
  ): Q4LoadedSources[keyof Q4LoadedSources] | undefined {
    if (status.sources === null) return undefined;
    return {
      A: status.sources.sourceA,
      B: status.sources.sourceB,
      C: status.sources.sourceC,
      D: status.sources.sourceD,
    }[slot];
  }

  function formatBytes(bytes: number): string {
    if (!Number.isFinite(bytes) || bytes < 0) return "invalid size";
    if (bytes < 1024) return `${bytes} B`;
    const mebibytes = bytes / (1024 * 1024);
    return `${mebibytes.toFixed(mebibytes >= 100 ? 0 : 1)} MiB`;
  }
</script>

<section
  class="q4-faceplate"
  aria-labelledby="q4-title"
  aria-busy={presetBusy}
  inert={presetBusy}
>
  <header class="q4-header">
    <div>
      <p>Four-cartridge carrier · donor instrument</p>
      <h2 id="q4-title">LD-Q4</h2>
    </div>
    <div
      class:pending={hostState === "pending" || hostState === "checking"}
      class:error={hostState === "error"}
      class="host-meter"
    >
      <span></span><strong>{hostState}</strong><small
        >SEQ {status.streamSequence}</small
      >
    </div>
  </header>

  <div class:error={hostState === "error"} class="status-line">
    <span>{hostMessage}</span>
    <button
      type="button"
      onclick={() => void refreshStatus()}
      disabled={hostBusy}>Refresh</button
    >
  </div>
  {#if resetMessage}<p class="reset-line">{resetMessage}</p>{/if}

  <section class="codec-bank">
    <div>
      <span>CODEC PACK</span><strong
        >{backend.displayName ?? "NOT INSTALLED"}</strong
      ><small>{backend.packVersion ?? "—"}</small>
    </div>
    <div>
      <span>Q4 ENTRYPOINT</span><strong
        >{backend.q4EntrypointAvailable ? "DECLARED" : "UNAVAILABLE"}</strong
      ><small>{backend.state}</small>
    </div>
    <div>
      <span>DECODER</span><strong
        >{backend.decoder?.variantId ?? "SELECT EXPLICIT WEIGHT"}</strong
      ><small
        >{backend.decoder === null
          ? (backend.detail ?? "—")
          : `SHA-256 ${backend.decoder.sha256.slice(0, 12)}… · ${formatBytes(backend.decoder.byteLength)}`}</small
      >
      {#if backend.decoder !== null}
        <nav aria-label="Q4 decoder provenance">
          <a href={backend.decoder.sourceUrl} target="_blank" rel="noreferrer"
            >Source</a
          >
          <a href={backend.decoder.licenseUrl} target="_blank" rel="noreferrer"
            >{backend.decoder.licenseLabel}</a
          >
        </nav>
      {/if}
    </div>
    <button
      type="button"
      onclick={() => void selectDecoder()}
      disabled={backendBusy}>Select decoder</button
    >
  </section>

  <section class="spout-strip" aria-label="Spout2 native output">
    <div class="spout-state">
      <span>SPOUT2 · DX12 TEXTURE</span>
      <strong>{spoutStateLabel}</strong>
      <small
        >{spout === null
          ? "Q4 OUTPUT INACTIVE"
          : `${spout.width}×${spout.height} · ${spout.format} · ${spout.submittedFrames} FRAMES`}</small
      >
    </div>
    <label
      >Sender name
      <input
        aria-label="Q4 Spout sender name"
        value={spoutName}
        oninput={(event) => {
          spoutName = (event.currentTarget as HTMLInputElement).value;
          spoutNameDirty = true;
        }}
        disabled={!spoutControls.rename}
      />
    </label>
    <button
      type="button"
      onclick={() => void applySpoutName()}
      disabled={!spoutControls.rename ||
        !spoutNameDirty ||
        spoutName.trim().length === 0}>Apply name</button
    >
    <button
      class:active={spout?.enabled === true}
      type="button"
      onclick={() => void toggleSpout()}
      disabled={!spoutControls.toggle}
      >{spout?.enabled ? "Disable sender" : "Enable sender"}</button
    >
    <div class="spout-receiver">
      <span>RECEIVER NAME</span>
      <strong>{spout?.activeName || spout?.requestedName || "—"}</strong>
      <small
        class:error={spoutMessage.length > 0 ||
          (spout?.lastErrorCode ?? null) !== null}
        >{spoutMessage ||
          spout?.lastErrorCode ||
          (spout?.published
            ? `SEQUENCE ${spout.lastSequence ?? "—"}`
            : "NO FRAME PUBLISHED")}</small
      >
    </div>
  </section>

  <section class="bank-strip">
    <label
      >Active Bank
      <select
        value={bankView.deckSession.activeCollectionId}
        onchange={(event) => void changeBank(event)}
        disabled={bankBusy || presetBusy}
      >
        {#each bankView.collections as collection (collection.id)}
          <option value={collection.id}>{collection.name}</option>
        {/each}
      </select>
    </label>
    <div>
      <span>AVAILABLE</span><strong
        >{presentCount.toString().padStart(2, "0")}</strong
      ><small>{activeBank?.name ?? "No Bank"}</small>
    </div>
    <p>
      Bank scopes selection only. Changing it never unloads the running four
      slots.
    </p>
    <p>
      Release performance target: 448×800. Q4 TOPK and Sinkhorn remain pending
      until their final 30-minute receipts pass; larger intrinsic grids are not
      yet benchmark-certified and are never downscaled implicitly.
    </p>
    <div class="preset-controls">
      <span>DECK PRESET · JSON</span>
      <nav>
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
      </nav>
      <small
        >{presetMessage ||
          "Exact Bank, slot identities, controls, routing, loops and seed."}</small
      >
    </div>
  </section>
  {#if bankError}<p class="bank-error">{bankError}</p>{/if}

  <div class="slot-grid">
    {#each Q4_SLOTS as slot (slot)}
      {@const source = sourceBySlot(slot)}
      {@const loadedSource = loadedSourceBySlot(slot)}
      <article class:carrier={rolesDraft.carrier === slot} class="slot-module">
        <header>
          <span>{slot}</span>
          <div>
            <p>
              {rolesDraft.carrier === slot
                ? "STRUCTURAL CARRIER"
                : "SOURCE SLOT"}
            </p>
            <h3>Cartridge {slot}</h3>
          </div>
        </header>
        <select
          value={sourceHash(slot)}
          onchange={(event) =>
            void setSourceHash(
              slot,
              (event.currentTarget as HTMLSelectElement).value,
            )}
          disabled={bankBusy || presetBusy}
        >
          {#each sourceOptions as cartridge (cartridge.archiveSha256)}
            <option
              value={cartridge.archiveSha256}
              disabled={cartridge.availability !== "present" ||
                isIncompatibleCandidate(slot, cartridge)}
              >{compatibilityLabel(slot, cartridge)}</option
            >
          {/each}
        </select>
        {#if slot !== rolesDraft.carrier &&
        (compatibilityReasons.get(sourceHash(slot))?.length ?? 0) > 0}<p
            class="inline-error"
            role="status"
          >
            Slot {slot} cannot mix with carrier {rolesDraft.carrier}: {(
              compatibilityReasons.get(sourceHash(slot)) ?? []
            ).join("; ")}. Use an explicit Toolkit Align/Crop node.
          </p>{/if}
        {#if duplicateSlotsLabel(slot) !== ""}<p class="source-reuse-label">
            {duplicateSlotsLabel(slot)}
          </p>{/if}
        {#if status.loaded && loadedSource !== undefined}<p
            class="loaded-source-label"
          >
            LOADED {shortHash(loadedSource.archiveSha256)} · {loadedSource.latentSlotCount}
            LATENT SLOTS{loadedSource.archiveSha256 !== sourceHash(slot)
              ? " · DRAFT DIFFERS"
              : ""}
          </p>{/if}
        <div class="source-readout">
          <strong
            >{source === undefined
              ? "—"
              : `${source.decodedWidth}×${source.decodedHeight}`}</strong
          ><small>{source?.decodedFrameCount ?? 0} frames</small>
          {#if source !== undefined}<small
              >{describeIntrinsicFormat(source).aspectLabel} · LATENT {describeIntrinsicFormat(
                source,
              ).latentGrid ?? "N/A"}</small
            >{/if}
        </div>
        <div class="transport">
          <button
            type="button"
            onclick={() => void togglePlay(slot)}
            disabled={!status.loaded || hostBusy || captureActive}
            >{status.transport[`playing${slot}`] ? "Pause" : "Play"}</button
          >
          <label
            ><input
              type="checkbox"
              checked={status.transport[`loop${slot}`]}
              onchange={(event) => void toggleLoop(slot, event)}
              disabled={!status.loaded || hostBusy || captureActive}
            /> Loop</label
          >
          <small>HEAD {status[`playhead${slot}`]}</small>
        </div>
      </article>
    {/each}
  </div>

  {#if duplicateSources.length > 0}
    <section
      class="duplicate-source-warning"
      role="status"
      aria-label="Duplicate Q4 source disclosure"
    >
      <strong>DUPLICATE-SOURCE FUNCTIONAL MODE</strong>
      <p>
        {distinctSourceCount} distinct cartridge archives assigned across 4 slots.
        {duplicateSources
          .map(
            (group) =>
              `Slots ${group.slots.join("/")} share ${shortHash(group.archiveSha256)}`,
          )
          .join(" · ")}. This is valid for functional testing but does not
        satisfy four-independent-source release acceptance.
      </p>
    </section>
  {/if}
  {#if status.loaded && loadedDuplicateSources.length > 0}
    <section
      class="duplicate-source-warning loaded"
      role="status"
      aria-label="Loaded duplicate Q4 source disclosure"
    >
      <strong>LOADED DUPLICATE-SOURCE SESSION</strong>
      <p>
        {loadedDistinctSourceCount} distinct cartridge archives are loaded across
        4 runtime slots. {loadedDuplicateSources
          .map(
            (group) =>
              `Slots ${group.slots.join("/")} share ${shortHash(group.archiveSha256)}`,
          )
          .join(" · ")}. This session is functional evidence only, not
        four-independent-source acceptance.
      </p>
    </section>
  {/if}

  <section class="routing-panel">
    <header>
      <div>
        <p>Explicit full permutation</p>
        <h3>Carrier / Donor routing</h3>
      </div>
      <code>org.latentdeck.builtin.ld_q4@0.1.0</code>
    </header>
    <div
      class="role-grid"
      onchange={() => void rolesChanged()}
    >
      <label
        >Carrier<select bind:value={rolesDraft.carrier}
          >{#each Q4_SLOTS as slot}<option value={slot}>{slot}</option
            >{/each}</select
        ></label
      >
      <label
        >Donor B<select bind:value={rolesDraft.donorB}
          >{#each Q4_SLOTS as slot}<option value={slot}>{slot}</option
            >{/each}</select
        ></label
      >
      <label
        >Donor C<select bind:value={rolesDraft.donorC}
          >{#each Q4_SLOTS as slot}<option value={slot}>{slot}</option
            >{/each}</select
        ></label
      >
      <label
        >Donor D<select bind:value={rolesDraft.donorD}
          >{#each Q4_SLOTS as slot}<option value={slot}>{slot}</option
            >{/each}</select
        ></label
      >
      <button
        type="button"
        onclick={() => void applyRoles()}
        disabled={!status.loaded || !rolesDirty || !rolesValid || hostBusy}
        >Apply roles</button
      >
    </div>
    {#if !rolesValid}<p class="inline-error">
        Each physical slot must appear exactly once.
      </p>{/if}
  </section>

  <form
    class="operator-panel"
    oninput={() => {
      discardPresetLoopDraft();
      controlsDirty = true;
    }}
    onsubmit={(event) => {
      event.preventDefault();
      void applyControls();
    }}
  >
    <header>
      <div>
        <p>Post-operator latent synthesis</p>
        <h3>Q4 controls</h3>
      </div>
      <div class="algorithm-switch">
        <button
          type="button"
          class:active={controlsDraft.algorithm === "LINEAR"}
          onclick={() => {
            discardPresetLoopDraft();
            controlsDraft.algorithm = "LINEAR";
            controlsDirty = true;
          }}>LINEAR</button
        ><button
          type="button"
          class:active={controlsDraft.algorithm === "XS5"}
          onclick={() => {
            discardPresetLoopDraft();
            controlsDraft.algorithm = "XS5";
            controlsDirty = true;
          }}>XS5</button
        >
      </div>
    </header>
    <div class="control-grid">
      <label
        >Interaction <output>{controlsDraft.interaction.toFixed(2)}</output
        ><input
          type="range"
          min="0"
          max="1"
          step="0.01"
          bind:value={controlsDraft.interaction}
        /></label
      >
      <label
        >Preserve <output>{controlsDraft.preserve.toFixed(2)}</output><input
          type="range"
          min="0"
          max="1"
          step="0.01"
          bind:value={controlsDraft.preserve}
        /></label
      >
      <label
        >Chaos <output>{controlsDraft.chaos.toFixed(2)}</output><input
          type="range"
          min="0"
          max="1"
          step="0.01"
          bind:value={controlsDraft.chaos}
        /></label
      >
      <fieldset>
        <legend>Mode</legend><label
          ><input
            type="radio"
            value="HYBRIDIZE"
            bind:group={controlsDraft.mode}
          /> Hybridize</label
        ><label
          ><input
            type="radio"
            value="INTERACT"
            bind:group={controlsDraft.mode}
          /> Interact</label
        >
      </fieldset>
      <fieldset>
        <legend>Influence</legend><label
          ><input
            type="radio"
            value="MANUAL"
            bind:group={controlsDraft.influenceMode}
          /> Manual</label
        ><label
          ><input
            type="radio"
            value="TRIANGLE"
            bind:group={controlsDraft.influenceMode}
          /> Triangle</label
        >
      </fieldset>
    </div>
    {#if controlsDraft.influenceMode === "MANUAL"}
      <div class="donor-grid">
        <label
          >B<input
            type="number"
            min="0"
            max="1"
            step="0.01"
            bind:value={controlsDraft.donorWeightB}
          /></label
        ><label
          >C<input
            type="number"
            min="0"
            max="1"
            step="0.01"
            bind:value={controlsDraft.donorWeightC}
          /></label
        ><label
          >D<input
            type="number"
            min="0"
            max="1"
            step="0.01"
            bind:value={controlsDraft.donorWeightD}
          /></label
        >
      </div>
    {:else}
      <div class="donor-grid">
        <label
          >Triangle X<input
            type="number"
            min="0"
            max="1"
            step="0.01"
            bind:value={controlsDraft.triangleX}
          /></label
        ><label
          >Triangle Y<input
            type="number"
            min="0"
            max="1"
            step="0.01"
            bind:value={controlsDraft.triangleY}
          /></label
        >
      </div>
    {/if}
    <div class="weight-readout">
      <span>B {(resolvedWeights[0] * 100).toFixed(1)}%</span><span
        >C {(resolvedWeights[1] * 100).toFixed(1)}%</span
      ><span>D {(resolvedWeights[2] * 100).toFixed(1)}%</span>
    </div>
    {#if controlsDraft.algorithm === "XS5"}
      <div class="xs5-grid">
        <label
          >Routing<select bind:value={controlsDraft.xs5Routing}
            ><option value="TOPK">TOPK</option><option value="SINKHORN"
              >SINKHORN</option
            ></select
          ></label
        ><label
          >Temperature<input
            type="number"
            min="0.02"
            max="1"
            step="0.01"
            bind:value={controlsDraft.temperature}
          /></label
        ><label
          >Top K<input
            type="number"
            min="1"
            max="64"
            step="1"
            bind:value={controlsDraft.topK}
          /></label
        ><label
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
    <footer>
      <span>{controlsDirty ? "DRAFT CHANGED" : "HOST ACKNOWLEDGED"}</span
      ><button
        type="submit"
        disabled={!status.loaded || !controlsDirty || hostBusy}
        >Apply controls</button
      >
    </footer>
  </form>

  <footer class="master-strip">
    <div>
      <span>LOAD FOUR SLOTS</span><button
        class="load"
        type="button"
        onclick={() => void openDeck()}
        disabled={hostBusy ||
          backend.state !== "ready" ||
          !allSourcesReady ||
          !selectedSourcesCompatible}
        >Load Q4</button
      >
    </div>
    <div>
      <span>DETERMINISTIC SEED</span><input
        type="number"
        min="0"
        max={MAX_SAFE_Q4_SEED}
        value={seedDraft}
        oninput={(event) => {
          discardPresetLoopDraft();
          seedDraft = (event.currentTarget as HTMLInputElement).value;
          seedDirty = true;
        }}
      /><button
        type="button"
        onclick={() => void applySeed()}
        disabled={!status.loaded || !seedDirty || hostBusy}>Set</button
      >
    </div>
    <div>
      <span>CAUSAL TRANSPORT</span><button
        type="button"
        onclick={() => void restart()}
        disabled={!status.loaded || hostBusy || captureActive}
        >Restart all</button
      >
    </div>
    <div class="capture">
      <span>POST-OPERATOR RESAMPLE</span><button
        type="button"
        onclick={() => void snapshot()}
        disabled={!status.loaded || captureActive || captureBusy}
        >Snapshot</button
      ><button
        type="button"
        onclick={() => void toggleLiveCapture()}
        disabled={!status.loaded || captureBusy || liveCaptureAction === null}
        >{liveCaptureAction === "stop" ? "Stop Live" : "Start Live"}</button
      ><small
        >{capture.detail ??
          `${capture.state} · ${capture.latentSlots} slots`}</small
      >
    </div>
  </footer>
</section>

<style>
  .q4-faceplate {
    --line: #4f4f68;
    --panel: #171824;
    --raised: #202235;
    --ink: #e4e5ef;
    --blue: #83a9ff;
    --violet: #bc92ff;
    --amber: #e0bb6c;
    --red: #e77b86;
    min-height: calc(100vh - 132px);
    margin-top: 8px;
    border: 1px solid #737491;
    background:
      linear-gradient(135deg, rgb(255 255 255 / 3%), transparent 30%), #11121a;
    color: var(--ink);
    box-shadow: 0 16px 38px rgb(0 0 0 / 34%);
  }
  .q4-header,
  .routing-panel > header,
  .operator-panel > header {
    display: flex;
    align-items: center;
    justify-content: space-between;
  }
  .q4-header {
    min-height: 72px;
    padding: 10px 16px;
    border-bottom: 1px solid #737491;
    background: linear-gradient(90deg, #292a45, #181925 65%, #101119);
  }
  h2,
  h3,
  p {
    margin: 0;
  }
  h2,
  h3 {
    font-family: "Arial Narrow", "Segoe UI", sans-serif;
    text-transform: uppercase;
    letter-spacing: 0.07em;
  }
  h2 {
    font-size: 1.75rem;
  }
  .q4-header p,
  .slot-module p,
  .routing-panel p,
  .operator-panel p {
    color: #8d8ea6;
    font:
      700 0.57rem ui-monospace,
      monospace;
    letter-spacing: 0.11em;
    text-transform: uppercase;
  }
  .host-meter {
    display: grid;
    grid-template-columns: auto auto;
    gap: 2px 8px;
    align-items: center;
    min-width: 180px;
    border: 1px solid #526d9b;
    padding: 7px 10px;
    background: #11131d;
    font:
      700 0.6rem ui-monospace,
      monospace;
    text-transform: uppercase;
  }
  .host-meter > span {
    grid-row: 1/3;
    width: 9px;
    height: 9px;
    border-radius: 50%;
    background: var(--blue);
    box-shadow: 0 0 8px var(--blue);
  }
  .host-meter.pending > span {
    background: var(--amber);
    box-shadow: 0 0 8px var(--amber);
  }
  .host-meter.error > span {
    background: var(--red);
    box-shadow: 0 0 8px var(--red);
  }
  .host-meter small {
    color: #777991;
  }
  .status-line,
  .reset-line,
  .bank-error,
  .inline-error {
    min-height: 30px;
    padding: 5px 12px;
    border-bottom: 1px solid #343548;
    background: #0e0f16;
    color: #a4a6bb;
    font-size: 0.65rem;
  }
  .status-line {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .status-line button {
    margin-left: auto;
    min-height: 22px;
    padding: 2px 7px;
  }
  .status-line.error,
  .bank-error,
  .inline-error {
    color: #ec9aa2;
  }
  .reset-line {
    color: #d7c27d;
  }
  .codec-bank {
    display: grid;
    grid-template-columns: 1fr 0.7fr 1.5fr auto;
    gap: 1px;
    border-bottom: 1px solid var(--line);
    background: #3a3b52;
  }
  .codec-bank > div {
    display: grid;
    align-content: center;
    gap: 3px;
    min-height: 66px;
    padding: 8px 11px;
    background: #171824;
  }
  .codec-bank span,
  .codec-bank small,
  .bank-strip span,
  .bank-strip small,
  .master-strip span {
    color: #7f8199;
    font:
      700 0.54rem ui-monospace,
      monospace;
  }
  .codec-bank strong {
    overflow: hidden;
    color: #cfd2e3;
    font:
      700 0.65rem ui-monospace,
      monospace;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .codec-bank button {
    margin: 9px;
  }
  .codec-bank nav {
    display: flex;
    gap: 9px;
  }
  .codec-bank a {
    color: #9db8ef;
    font:
      700 0.54rem ui-monospace,
      monospace;
  }
  .spout-strip {
    display: grid;
    grid-template-columns:
      minmax(190px, 0.8fr) minmax(230px, 1fr)
      auto auto minmax(210px, 1fr);
    align-items: end;
    gap: 7px;
    padding: 8px 12px;
    border-bottom: 1px solid var(--line);
    background: #121722;
  }
  .spout-strip span,
  .spout-strip small {
    color: #7f8799;
    font:
      700 0.54rem ui-monospace,
      monospace;
  }
  .spout-strip strong {
    overflow: hidden;
    color: #cbd8e8;
    font:
      700 0.65rem ui-monospace,
      monospace;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .spout-strip label,
  .spout-state,
  .spout-receiver {
    display: grid;
    gap: 4px;
  }
  .spout-strip label {
    color: #969eaf;
    font-size: 0.58rem;
    font-weight: 800;
    text-transform: uppercase;
  }
  .spout-strip button.active {
    border-color: #7ea6df;
    background: #294564;
    box-shadow: inset 0 -2px #83b6ff;
  }
  .spout-receiver small.error {
    color: #ec9aa2;
  }
  .bank-strip {
    display: grid;
    grid-template-columns: minmax(250px, 420px) 180px minmax(190px, 1fr) minmax(
        240px,
        1fr
      );
    align-items: center;
    gap: 12px;
    padding: 10px 12px;
    border-bottom: 1px solid var(--line);
    background: #1b1d2b;
  }
  .bank-strip label {
    display: grid;
    gap: 4px;
    color: #9496aa;
    font-size: 0.6rem;
    font-weight: 800;
    text-transform: uppercase;
  }
  .bank-strip > div {
    display: grid;
    border-left: 1px solid var(--line);
    padding-left: 12px;
  }
  .bank-strip strong {
    color: var(--blue);
    font:
      700 0.75rem ui-monospace,
      monospace;
  }
  .bank-strip p {
    color: #85879d;
    font-size: 0.65rem;
  }
  .preset-controls {
    gap: 4px;
  }
  .preset-controls nav {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 5px;
  }
  .preset-controls small {
    min-height: 2.3em;
    line-height: 1.35;
  }
  .slot-grid {
    display: grid;
    grid-template-columns: repeat(4, minmax(190px, 1fr));
    gap: 7px;
    padding: 7px;
  }
  .slot-module {
    display: flex;
    min-width: 0;
    min-height: 265px;
    flex-direction: column;
    gap: 10px;
    padding: 11px;
    border: 1px solid var(--line);
    background:
      linear-gradient(135deg, rgb(255 255 255 / 2%), transparent 35%),
      var(--panel);
  }
  .slot-module.carrier {
    border-color: #789fff;
    box-shadow: inset 0 2px var(--blue);
  }
  .slot-module header {
    display: flex;
    align-items: center;
    gap: 8px;
    padding-bottom: 8px;
    border-bottom: 1px solid #3b3c51;
  }
  .slot-module header > span {
    display: grid;
    width: 32px;
    height: 32px;
    place-content: center;
    border: 1px solid #6d7197;
    background: #272a42;
    color: var(--blue);
    font:
      800 1rem ui-monospace,
      monospace;
  }
  .slot-module select {
    min-width: 0;
    width: 100%;
  }
  .source-readout {
    display: grid;
    min-height: 82px;
    place-content: center;
    gap: 4px;
    border: 1px solid #3b3e59;
    background: #0e1018;
    text-align: center;
  }
  .source-readout strong {
    font:
      500 1rem ui-monospace,
      monospace;
  }
  .source-readout small {
    color: #777a91;
  }
  .transport {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 5px;
    margin-top: auto;
  }
  .transport label {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 4px;
    border: 1px solid #45475d;
    background: #101119;
    font-size: 0.62rem;
  }
  .transport small {
    grid-column: 1/-1;
    color: #81839b;
    font:
      700 0.57rem ui-monospace,
      monospace;
    text-align: right;
  }
  .source-reuse-label,
  .loaded-source-label {
    color: var(--amber) !important;
    font-size: 0.55rem !important;
  }
  .loaded-source-label {
    min-height: 2.2em;
  }
  .duplicate-source-warning {
    margin: 0 7px 7px;
    border: 1px solid #8b6d35;
    padding: 9px 11px;
    background: #211b11;
    color: #d9c18a;
  }
  .duplicate-source-warning.loaded {
    border-color: #7554a5;
    background: #1d1728;
    color: #cbb4ef;
  }
  .duplicate-source-warning strong {
    font:
      800 0.63rem ui-monospace,
      monospace;
    letter-spacing: 0.1em;
  }
  .duplicate-source-warning p {
    margin-top: 5px;
    font-size: 0.66rem;
    line-height: 1.45;
  }
  .routing-panel,
  .operator-panel {
    margin: 0 7px 7px;
    padding: 11px;
    border: 1px solid var(--line);
    background: var(--panel);
  }
  .routing-panel > header,
  .operator-panel > header {
    padding-bottom: 9px;
    border-bottom: 1px solid #3b3d53;
  }
  .routing-panel code {
    color: #767991;
    font-size: 0.56rem;
  }
  .role-grid {
    display: grid;
    grid-template-columns: repeat(4, 1fr) auto;
    gap: 7px;
    align-items: end;
    margin-top: 9px;
  }
  .role-grid label,
  .donor-grid label,
  .xs5-grid label {
    display: grid;
    gap: 4px;
    color: #9698ae;
    font-size: 0.58rem;
    font-weight: 800;
    text-transform: uppercase;
  }
  .algorithm-switch {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 4px;
  }
  .algorithm-switch button.active {
    border-color: var(--violet);
    background: #443260;
    box-shadow: inset 0 -2px var(--violet);
  }
  .control-grid {
    display: grid;
    grid-template-columns: repeat(3, 1fr) 0.8fr 0.8fr;
    gap: 8px;
    margin-top: 10px;
  }
  .control-grid > label {
    display: grid;
    gap: 4px;
    color: #999bb0;
    font-size: 0.58rem;
    font-weight: 800;
    text-transform: uppercase;
  }
  .control-grid output {
    color: var(--violet);
    font-family: ui-monospace, monospace;
  }
  .control-grid input[type="range"] {
    width: 100%;
    accent-color: var(--violet);
  }
  fieldset {
    display: grid;
    gap: 3px;
    margin: 0;
    border: 1px solid #3f4158;
    padding: 5px;
  }
  legend {
    color: #7f8199;
    font-size: 0.54rem;
  }
  fieldset label {
    font-size: 0.57rem;
  }
  .donor-grid,
  .xs5-grid {
    display: grid;
    grid-template-columns: repeat(4, minmax(110px, 1fr));
    gap: 7px;
    margin-top: 9px;
  }
  .weight-readout {
    display: grid;
    grid-template-columns: repeat(3, 1fr);
    gap: 4px;
    margin-top: 7px;
  }
  .weight-readout span {
    padding: 5px;
    border: 1px solid #383a50;
    background: #11121b;
    color: #aeb0c3;
    font:
      700 0.58rem ui-monospace,
      monospace;
    text-align: center;
  }
  .operator-panel footer {
    display: flex;
    align-items: center;
    gap: 8px;
    margin-top: 9px;
    padding-top: 8px;
    border-top: 1px solid #383a50;
  }
  .operator-panel footer span {
    color: #85879b;
    font:
      700 0.55rem ui-monospace,
      monospace;
  }
  .operator-panel footer button {
    margin-left: auto;
  }
  .master-strip {
    display: grid;
    grid-template-columns: 0.8fr 1fr 0.7fr 1.5fr;
    gap: 7px;
    padding: 7px;
    border-top: 1px solid #737491;
    background: #151621;
  }
  .master-strip > div {
    display: grid;
    align-content: center;
    gap: 5px;
    min-height: 76px;
    padding: 8px;
    border: 1px solid #44465d;
    background: #0f1018;
  }
  .master-strip > div:nth-child(2) {
    grid-template-columns: 1fr auto;
  }
  .master-strip > div:nth-child(2) > span {
    grid-column: 1/-1;
  }
  .master-strip .load {
    border-color: #779eff;
    background: linear-gradient(#38558d, #28385d);
  }
  .capture {
    grid-template-columns: 1fr 1fr;
  }
  .capture > span,
  .capture > small {
    grid-column: 1/-1;
  }
  .capture small {
    color: #777a90;
    font-size: 0.57rem;
  }
  @media (max-width: 1180px) {
    .slot-grid {
      grid-template-columns: 1fr 1fr;
    }
    .role-grid {
      grid-template-columns: 1fr 1fr;
    }
    .control-grid {
      grid-template-columns: 1fr 1fr;
    }
    .master-strip {
      grid-template-columns: 1fr 1fr;
    }
    .codec-bank {
      grid-template-columns: 1fr 1fr;
    }
    .spout-strip {
      grid-template-columns: 1fr 1fr;
    }
    .spout-state,
    .spout-receiver {
      align-self: center;
    }
  }
</style>
