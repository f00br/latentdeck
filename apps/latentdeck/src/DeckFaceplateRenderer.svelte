<script lang="ts">
  import { onDestroy } from "svelte";
  import {
    isFaceplateWidgetVisible,
    type Barycentric3Widget,
    type CaptureWidget,
    type FaceplateSection,
    type FaceplateWidget,
    type MonitorWidget,
    type NumericWidget,
    type SelectWidget,
    type ToggleWidget,
  } from "./faceplate-model";
  import {
    serializeDeckControls,
    serializeRoleBindings,
    type DeckUiDraft,
    type DeckUiModel,
    type DeckUiScalar,
  } from "./deck-ui-model";

  interface SourceOption {
    archiveSha256: string;
    label: string;
    available: boolean;
    incompatibilityReason?: string;
    detail?: string;
  }

  export let model: DeckUiModel;
  export let initialDraft: DeckUiDraft;
  export let sourceOptions: readonly SourceOption[] = [];
  export let sourceGeometryWarning = "";
  export let active = false;
  export let runtimeAvailable = false;
  export let runtimeUnavailableReason =
    "This exact Deck version has no active host runtime.";
  export let loadAvailable = true;
  export let loadUnavailableReason = "A new warm session cannot be opened.";
  export let runtimeLoaded = false;
  export let runtimeBusy = false;
  export let statusMessage = "Deck UI ready.";
  export let playheads: readonly number[] = [];
  export let captureState = "idle";
  export let captureAvailable = false;
  export let captureStartAvailable = true;
  export let captureActive = false;
  export let liveCaptureActive = false;
  export let capturedSourceAvailable = false;
  export let captureReuseAvailable = true;
  export let captureUnavailableReason =
    "Latent capture is unavailable for this exact Deck and Codec profile.";
  export let mp4Available = false;
  export let mp4Active = false;
  export let mp4Status = "Decoded MP4 output is unavailable.";
  export let spoutName = "LatentDeck";
  export let spoutStatus = "Spout2 output is unavailable.";
  export let spoutEnabled = false;
  export let spoutRenameAvailable = false;
  export let spoutToggleAvailable = false;
  export let outputFullscreen: boolean | null = null;
  export let onDraftChange: (draft: DeckUiDraft) => void = () => undefined;
  export let onLoad: (draft: DeckUiDraft) => void | Promise<void> = () =>
    undefined;
  export let onRestart: () => void | Promise<void> = () => undefined;
  export let onProcessOnce: () => void | Promise<void> = () => undefined;
  export let onControlsCommit: (
    controls: Record<string, DeckUiScalar>,
  ) => void | Promise<void> = () => undefined;
  export let onControlsChange: (
    controls: Record<string, DeckUiScalar>,
  ) => void | Promise<void> = () => undefined;
  export let onRolesCommit: (
    roles: Record<string, number>,
  ) => void | Promise<void> = () => undefined;
  export let onTransportCommit: (
    playing: readonly boolean[],
    loops: readonly boolean[],
  ) => void | Promise<void> = () => undefined;
  export let onSeedCommit: (seed: number) => void | Promise<void> = () =>
    undefined;
  export let onCapture: (
    mode: "snapshot" | "live_capture",
  ) => void | Promise<void> = () => undefined;
  export let onUseCapture: (slotIndex: number) => void | Promise<void> = () =>
    undefined;
  export let onMp4Toggle: () => void | Promise<void> = () => undefined;
  export let onSpoutNameChange: (name: string) => void = () => undefined;
  export let onSpoutNameCommit: (name: string) => void | Promise<void> = () =>
    undefined;
  export let onSpoutToggle: (enabled: boolean) => void | Promise<void> = () =>
    undefined;
  export let onFullscreenToggle: () => void | Promise<void> = () => undefined;
  export let onMonitorAnchor: (element: HTMLDivElement | null) => void = () =>
    undefined;

  let draft = cloneDraft(initialDraft);
  let draftError = "";
  let sourceDraftReady = false;
  let fullscreenActive = false;
  let monitorWidget: MonitorWidget | undefined;
  let captureWidget: CaptureWidget | undefined;
  let controlSections: Array<{
    section: FaceplateSection;
    widgets: readonly FaceplateWidget[];
  }> = [];
  let surfaceNotice = "";
  let captureReasonVisible = false;

  $: fullscreenActive = active && outputFullscreen === true && runtimeLoaded;
  $: syncFullscreenDocument(fullscreenActive);
  $: monitorWidget = model.faceplate.sections
    .flatMap((section) => section.widgets)
    .find(isMonitorWidget);
  $: captureWidget = model.faceplate.sections
    .flatMap((section) => section.widgets)
    .find(isCaptureWidget);
  $: controlSections = model.faceplate.sections
    .map((section) => ({
      section,
      widgets: section.widgets.filter(
        (widget) =>
          !isMonitorWidget(widget) &&
          !isCaptureWidget(widget) &&
          isFaceplateWidgetVisible(widget, draft.controls),
      ),
    }))
    .filter(({ widgets }) => widgets.length > 0);
  $: surfaceNotice =
    draftError !== ""
      ? draftError
      : !runtimeAvailable && !runtimeLoaded
        ? runtimeUnavailableReason
        : runtimeAvailable && !loadAvailable
          ? loadUnavailableReason
          : "";
  // Realtime-control acknowledgements are intentionally silent. Capture
  // buttons may be disabled for a few milliseconds, but that transient state
  // must not flash text or move the professional workbench around.
  $: captureReasonVisible = !captureAvailable;

  $: sourceDraftReady =
    draft.sourceArchiveSha256s.length === model.slots &&
    draft.sourceArchiveSha256s.every((archiveSha256) => {
      const option = sourceOptions.find(
        (candidate) => candidate.archiveSha256 === archiveSha256,
      );
      return (
        archiveSha256 !== "" &&
        option?.available === true &&
        option.incompatibilityReason === undefined
      );
    });

  function cloneDraft(value: DeckUiDraft): DeckUiDraft {
    return {
      sourceArchiveSha256s: [...value.sourceArchiveSha256s],
      controls: { ...value.controls },
      roleBindings: { ...value.roleBindings },
      playing: [...value.playing],
      loops: [...value.loops],
      seed: value.seed,
    };
  }

  function emitDraft(): void {
    draft = cloneDraft(draft);
    onDraftChange(cloneDraft(draft));
  }

  function setSource(slotIndex: number, archiveSha256: string): void {
    draft.sourceArchiveSha256s[slotIndex] = archiveSha256;
    emitDraft();
  }

  function setNumericControl(widget: NumericWidget, event: Event): void {
    const value = (event.currentTarget as HTMLInputElement).valueAsNumber;
    draft.controls[widget.control_id] = value;
    const valid = validateDraft();
    emitDraft();
    if (runtimeLoaded && valid) emitRealtimeControls();
  }

  function setToggleControl(widget: ToggleWidget, event: Event): void {
    draft.controls[widget.control_id] = (
      event.currentTarget as HTMLInputElement
    ).checked;
    const valid = validateDraft();
    emitDraft();
    if (runtimeLoaded && valid) emitRealtimeControls();
  }

  function setSelectControl(widget: SelectWidget, event: Event): void {
    draft.controls[widget.control_id] = (
      event.currentTarget as HTMLSelectElement
    ).value;
    const valid = validateDraft();
    emitDraft();
    if (runtimeLoaded && valid) emitRealtimeControls();
  }

  function setBarycentricControl(
    widget: Barycentric3Widget,
    axis: "x" | "y",
    event: Event,
  ): void {
    const controlId = axis === "x" ? widget.x_control_id : widget.y_control_id;
    draft.controls[controlId] = (
      event.currentTarget as HTMLInputElement
    ).valueAsNumber;
    const valid = validateDraft();
    emitDraft();
    if (runtimeLoaded && valid) emitRealtimeControls();
  }

  function setRole(roleId: string, event: Event): void {
    const nextSlot = Number((event.currentTarget as HTMLSelectElement).value);
    const previousSlot = draft.roleBindings[roleId];
    const displacedRole = model.roles.find(
      (role) => draft.roleBindings[role.roleId] === nextSlot,
    )?.roleId;
    draft.roleBindings[roleId] = nextSlot;
    if (displacedRole !== undefined && displacedRole !== roleId) {
      draft.roleBindings[displacedRole] = previousSlot;
    }
    validateDraft();
    emitDraft();
  }

  function togglePlaying(slotIndex: number): void {
    draft.playing[slotIndex] = !draft.playing[slotIndex];
    emitDraft();
    if (runtimeLoaded) {
      void onTransportCommit([...draft.playing], [...draft.loops]);
    }
  }

  function setLoop(slotIndex: number, event: Event): void {
    draft.loops[slotIndex] = (event.currentTarget as HTMLInputElement).checked;
    emitDraft();
    if (runtimeLoaded) {
      void onTransportCommit([...draft.playing], [...draft.loops]);
    }
  }

  function setSeed(event: Event): void {
    draft.seed = (event.currentTarget as HTMLInputElement).valueAsNumber;
    validateDraft();
    emitDraft();
  }

  function validateDraft(): boolean {
    try {
      serializeDeckControls(model, draft.controls);
      serializeRoleBindings(model, draft.roleBindings);
      if (!Number.isSafeInteger(draft.seed) || draft.seed < 0) {
        throw new Error("Seed must be a non-negative safe integer.");
      }
      draftError = "";
      return true;
    } catch (error) {
      draftError =
        error instanceof Error ? error.message : "Deck draft is invalid.";
      return false;
    }
  }

  function commitControls(): void {
    if (!runtimeLoaded || !validateDraft()) return;
    void onControlsCommit(serializeDeckControls(model, draft.controls));
  }

  function emitRealtimeControls(): void {
    void onControlsChange(serializeDeckControls(model, draft.controls));
  }

  function commitRoles(): void {
    if (!runtimeLoaded || !validateDraft()) return;
    void onRolesCommit(serializeRoleBindings(model, draft.roleBindings));
  }

  function commitSeed(): void {
    if (!runtimeLoaded || !validateDraft()) return;
    void onSeedCommit(draft.seed);
  }

  function loadDeck(): void {
    if (!runtimeAvailable || !validateDraft()) return;
    void onLoad(cloneDraft(draft));
  }

  function numericContract(controlId: string): {
    minimum: number;
    maximum: number;
    step: number;
  } {
    const control = model.controls.find(
      (candidate) => candidate.controlId === controlId,
    );
    if (
      control === undefined ||
      (control.valueType !== "number" && control.valueType !== "integer")
    ) {
      return { minimum: 0, maximum: 1, step: 0.01 };
    }
    return {
      minimum: control.minimum,
      maximum: control.maximum,
      step: control.step,
    };
  }

  function barycentricXMinimum(value: DeckUiScalar | undefined): number {
    const y = Number(value);
    return Number.isFinite(y) ? 0.5 * y : 0;
  }

  function barycentricXMaximum(value: DeckUiScalar | undefined): number {
    const y = Number(value);
    return Number.isFinite(y) ? 1 - 0.5 * y : 1;
  }

  function barycentricYMaximum(value: DeckUiScalar | undefined): number {
    const x = Number(value);
    return Number.isFinite(x) ? 2 * Math.min(x, 1 - x) : 1;
  }

  function monitorAnchor(element: HTMLDivElement): { destroy(): void } {
    onMonitorAnchor(element);
    return { destroy: () => onMonitorAnchor(null) };
  }

  function isMonitorWidget(widget: FaceplateWidget): widget is MonitorWidget {
    return widget.kind === "monitor";
  }

  function isCaptureWidget(widget: FaceplateWidget): widget is CaptureWidget {
    return widget.kind === "capture";
  }

  function slotLabel(slotIndex: number): string {
    return String.fromCharCode("A".charCodeAt(0) + slotIndex);
  }

  function selectedSourceDetail(slotIndex: number): string {
    const selectedHash = draft.sourceArchiveSha256s[slotIndex];
    return (
      sourceOptions.find((option) => option.archiveSha256 === selectedHash)
        ?.detail ?? ""
    );
  }

  function syncFullscreenDocument(enabled: boolean): void {
    if (typeof document === "undefined") return;
    document.documentElement.classList.toggle(
      "deck-output-fullscreen",
      enabled,
    );
    document.body?.classList.toggle("deck-output-fullscreen", enabled);
  }

  onDestroy(() => syncFullscreenDocument(false));
</script>

<section
  class="declarative-deck"
  class:fullscreen={fullscreenActive}
  data-deck-exact-key={model.exactKey}
  aria-labelledby={`deck-title-${model.exactKey}`}
  aria-busy={runtimeBusy}
>
  <header class="deck-heading">
    <div>
      <p>{model.operatorId} · host-rendered faceplate</p>
      <h2 id={`deck-title-${model.exactKey}`}>{model.displayName}</h2>
      <small>{model.deckId} · {model.deckVersion}</small>
    </div>
    <div
      class:offline={!runtimeAvailable && !runtimeLoaded}
      class="runtime-state"
    >
      <strong>{runtimeLoaded ? "STREAM ACTIVE" : "NO STREAM"}</strong>
      <span>{statusMessage}</span>
    </div>
  </header>

  <p
    class="surface-notice"
    class:error={draftError !== ""}
    role={draftError === "" ? "status" : "alert"}
    title={surfaceNotice}
  >
    {surfaceNotice || "\u00a0"}
  </p>

  <div class="deck-workbench">
    <aside class="output-column">
      <section
        class="output-stage"
        data-workbench-region="output"
        aria-label="Native Deck output"
      >
        {#if monitorWidget !== undefined}
          <div class="monitor" class:live={runtimeLoaded}>
            <header>
              <span>{monitorWidget.label}</span>
              <strong
                >{runtimeLoaded ? "POST-OPERATOR STREAM" : "STANDBY"}</strong
              >
            </header>
            <div class="monitor-frame">
              <div
                use:monitorAnchor
                class="native-monitor-anchor"
                data-native-viewport={model.exactKey}
                aria-hidden="true"
              ></div>
              {#if !runtimeLoaded}
                <div class="monitor-placeholder">
                  <strong>No active output</strong>
                  <small>Intrinsic signal only · no hidden conversion</small>
                </div>
              {/if}
            </div>
          </div>
        {/if}
      </section>

      <section
        class="output-actions"
        data-workbench-region="output-actions"
        aria-label="Output and capture actions"
      >
        <header>
          <span>OUTPUT</span>
          <strong>Host actions</strong>
        </header>
        <div class="primary-actions">
          <button
            type="button"
            class="primary"
            disabled={!runtimeAvailable ||
              !loadAvailable ||
              runtimeBusy ||
              draftError !== "" ||
              !sourceDraftReady}
            onclick={loadDeck}>Load exact Deck draft</button
          >
          <button
            type="button"
            disabled={!runtimeLoaded || runtimeBusy || draftError !== ""}
            onclick={commitControls}>Apply now</button
          >
          <button
            type="button"
            disabled={!runtimeLoaded || runtimeBusy}
            onclick={() => void onProcessOnce()}>Process once</button
          >
          <button
            type="button"
            disabled={!runtimeLoaded || runtimeBusy}
            onclick={() => void onRestart()}>Restart all</button
          >
          <button
            type="button"
            class:active={outputFullscreen === true}
            disabled={!runtimeLoaded ||
              runtimeBusy ||
              outputFullscreen === null}
            onclick={() => void onFullscreenToggle()}
            >{outputFullscreen === true
              ? "Exit fullscreen"
              : "Fullscreen output"}</button
          >
        </div>

        {#if captureWidget !== undefined}
          <article class="capture-module" data-widget-kind="capture">
            <div class="capture-controls">
              <strong>{captureWidget.label}</strong>
              <div class="capture-buttons">
                {#if captureWidget.modes.includes("snapshot")}
                  <button
                    type="button"
                    disabled={!captureAvailable ||
                      !captureStartAvailable ||
                      captureActive ||
                      !runtimeLoaded ||
                      runtimeBusy}
                    onclick={() => void onCapture("snapshot")}>Snapshot</button
                  >
                {/if}
                {#if captureWidget.modes.includes("live_capture")}
                  <button
                    type="button"
                    disabled={!captureAvailable ||
                      (!liveCaptureActive && !captureStartAvailable) ||
                      (captureActive && !liveCaptureActive) ||
                      !runtimeLoaded ||
                      runtimeBusy}
                    onclick={() => void onCapture("live_capture")}
                    >{liveCaptureActive
                      ? "Stop Live Capture"
                      : "Start Live Capture"}</button
                  >
                {/if}
              </div>
              <div class="capture-status" aria-live="polite">
                <small>{captureState}</small>
                <small
                  class="capture-reason"
                  class:quiet={!captureReasonVisible}
                  title={captureReasonVisible ? captureUnavailableReason : ""}
                  >{captureReasonVisible
                    ? captureUnavailableReason
                    : "\u00a0"}</small
                >
              </div>
              {#if capturedSourceAvailable}
                <div class="captured-source-actions">
                  {#each Array.from({ length: model.slots }, (_, slotIndex) => slotIndex) as slotIndex}
                    <button
                      type="button"
                      disabled={runtimeBusy || !captureReuseAvailable}
                      onclick={() => void onUseCapture(slotIndex)}
                      >Use capture in {slotLabel(slotIndex)}</button
                    >
                  {/each}
                </div>
              {/if}
            </div>
          </article>
        {/if}

        <div class="output-connectors" aria-label="Host output connectors">
          <div class="mp4-output">
            <strong>Decoded MP4</strong>
            <button
              type="button"
              class:active={mp4Active}
              disabled={!mp4Available || runtimeBusy}
              onclick={() => void onMp4Toggle()}
              >{mp4Active ? "Stop MP4" : "Record MP4…"}</button
            >
            <small title={mp4Status}>{mp4Status}</small>
          </div>
          <div class="spout-output">
            <strong>Spout2</strong>
            <input
              value={spoutName}
              maxlength="255"
              aria-label="Spout sender name"
              disabled={!spoutRenameAvailable || runtimeBusy}
              oninput={(event) => onSpoutNameChange(event.currentTarget.value)}
            />
            <button
              type="button"
              disabled={!spoutRenameAvailable ||
                runtimeBusy ||
                spoutName.trim() === ""}
              onclick={() => void onSpoutNameCommit(spoutName.trim())}
              >Apply name</button
            >
            <button
              type="button"
              class:active={spoutEnabled}
              disabled={!spoutToggleAvailable || runtimeBusy}
              onclick={() => void onSpoutToggle(!spoutEnabled)}
              >{spoutEnabled ? "Disable sender" : "Enable sender"}</button
            >
            <small title={spoutStatus}>{spoutStatus}</small>
          </div>
        </div>
        <small class="deck-summary">{model.summary}</small>
      </section>
    </aside>

    <div class="control-column" data-workbench-region="controls">
      {#if sourceGeometryWarning !== ""}
        <p class="geometry-warning" role="status">
          <strong>SOURCE GEOMETRY</strong>
          <span>{sourceGeometryWarning}</span>
        </p>
      {/if}
      {#each controlSections as { section, widgets } (section.section_id)}
        <section
          class="faceplate-section"
          aria-labelledby={`${model.exactKey}-${section.section_id}`}
          style={`--section-columns:${section.columns ?? 2}`}
        >
          <header>
            <span>{section.section_id}</span>
            <h3 id={`${model.exactKey}-${section.section_id}`}>
              {section.title}
            </h3>
          </header>
          <div class="widget-grid">
            {#each widgets as widget (widget.id)}
              <article
                class={`widget widget-${widget.kind}`}
                data-widget-kind={widget.kind}
              >
                {#if widget.kind === "source_picker"}
                  <label>
                    <span>{widget.label}</span>
                    <select
                      value={draft.sourceArchiveSha256s[widget.slot_index] ??
                        ""}
                      disabled={runtimeBusy}
                      onchange={(event) =>
                        setSource(widget.slot_index, event.currentTarget.value)}
                    >
                      <option value="">No source selected</option>
                      {#each sourceOptions as option (option.archiveSha256)}
                        <option
                          value={option.archiveSha256}
                          disabled={!option.available ||
                            option.incompatibilityReason !== undefined}
                          >{option.label}{option.incompatibilityReason ===
                          undefined
                            ? ""
                            : ` · INCOMPATIBLE: ${option.incompatibilityReason}`}</option
                        >
                      {/each}
                    </select>
                    {#if selectedSourceDetail(widget.slot_index) !== ""}
                      <small class="source-detail"
                        >{selectedSourceDetail(widget.slot_index)}</small
                      >
                    {/if}
                  </label>
                {:else if widget.kind === "slider" || widget.kind === "number"}
                  <label>
                    <span>{widget.label}</span>
                    <input
                      type={widget.kind === "slider" ? "range" : "number"}
                      min={widget.minimum}
                      max={widget.maximum}
                      step={widget.step}
                      value={String(draft.controls[widget.control_id])}
                      data-control-id={widget.control_id}
                      disabled={runtimeBusy}
                      oninput={(event) => setNumericControl(widget, event)}
                    />
                    <output>{String(draft.controls[widget.control_id])}</output>
                  </label>
                {:else if widget.kind === "toggle"}
                  <label class="toggle">
                    <input
                      type="checkbox"
                      checked={draft.controls[widget.control_id] === true}
                      data-control-id={widget.control_id}
                      disabled={runtimeBusy}
                      onchange={(event) => setToggleControl(widget, event)}
                    />
                    <span>{widget.label}</span>
                  </label>
                {:else if widget.kind === "select"}
                  <label>
                    <span>{widget.label}</span>
                    <select
                      value={String(draft.controls[widget.control_id])}
                      data-control-id={widget.control_id}
                      disabled={runtimeBusy}
                      onchange={(event) => setSelectControl(widget, event)}
                    >
                      {#each widget.options as option (option.value)}
                        <option value={option.value}>{option.label}</option>
                      {/each}
                    </select>
                  </label>
                {:else if widget.kind === "role_editor"}
                  <fieldset>
                    <legend>{widget.label}</legend>
                    <div class="role-grid">
                      {#each widget.role_ids as roleId (roleId)}
                        <label>
                          <span
                            >{model.roles.find((role) => role.roleId === roleId)
                              ?.displayName ?? roleId}</span
                          >
                          <select
                            value={String(draft.roleBindings[roleId])}
                            disabled={runtimeBusy}
                            onchange={(event) => setRole(roleId, event)}
                          >
                            {#each Array.from({ length: model.slots }, (_, slot) => slot) as slot}
                              <option value={slot}
                                >Physical slot {slot + 1}</option
                              >
                            {/each}
                          </select>
                        </label>
                      {/each}
                    </div>
                    <button
                      type="button"
                      disabled={!runtimeLoaded ||
                        runtimeBusy ||
                        draftError !== ""}
                      onclick={commitRoles}>Apply role permutation</button
                    >
                  </fieldset>
                {:else if widget.kind === "barycentric3"}
                  {@const xContract = numericContract(widget.x_control_id)}
                  {@const yContract = numericContract(widget.y_control_id)}
                  <fieldset>
                    <legend>{widget.label}</legend>
                    <div class="triangle" aria-hidden="true">
                      <span>{widget.vertex_role_ids[0]}</span>
                      <span>{widget.vertex_role_ids[1]}</span>
                      <span>{widget.vertex_role_ids[2]}</span>
                      <i
                        style={`left:${Number(draft.controls[widget.x_control_id]) * 100}%;top:${(1 - Number(draft.controls[widget.y_control_id])) * 100}%`}
                      ></i>
                    </div>
                    <label>
                      X
                      <input
                        type="range"
                        min={Math.max(
                          xContract.minimum,
                          barycentricXMinimum(
                            draft.controls[widget.y_control_id],
                          ),
                        )}
                        max={Math.min(
                          xContract.maximum,
                          barycentricXMaximum(
                            draft.controls[widget.y_control_id],
                          ),
                        )}
                        step={xContract.step}
                        value={String(draft.controls[widget.x_control_id])}
                        data-control-id={widget.x_control_id}
                        disabled={runtimeBusy}
                        oninput={(event) =>
                          setBarycentricControl(widget, "x", event)}
                      />
                    </label>
                    <label>
                      Y
                      <input
                        type="range"
                        min={yContract.minimum}
                        max={Math.min(
                          yContract.maximum,
                          barycentricYMaximum(
                            draft.controls[widget.x_control_id],
                          ),
                        )}
                        step={yContract.step}
                        value={String(draft.controls[widget.y_control_id])}
                        data-control-id={widget.y_control_id}
                        disabled={runtimeBusy}
                        oninput={(event) =>
                          setBarycentricControl(widget, "y", event)}
                      />
                    </label>
                  </fieldset>
                {:else if widget.kind === "transport"}
                  <fieldset>
                    <legend>{widget.label}</legend>
                    <div class="transport-grid">
                      {#each widget.slot_indices as slotIndex (slotIndex)}
                        <div>
                          <strong>Slot {slotIndex + 1}</strong>
                          <button
                            type="button"
                            disabled={!runtimeLoaded || runtimeBusy}
                            onclick={() => togglePlaying(slotIndex)}
                            >{draft.playing[slotIndex]
                              ? "Pause"
                              : "Play"}</button
                          >
                          <label class="toggle">
                            <input
                              type="checkbox"
                              checked={draft.loops[slotIndex]}
                              disabled={!runtimeLoaded || runtimeBusy}
                              onchange={(event) => setLoop(slotIndex, event)}
                            />
                            Loop
                          </label>
                          <small>HEAD {playheads[slotIndex] ?? 0}</small>
                        </div>
                      {/each}
                    </div>
                    <button
                      type="button"
                      disabled={!runtimeLoaded || runtimeBusy}
                      onclick={() => void onRestart()}>Restart all</button
                    >
                  </fieldset>
                {:else if widget.kind === "seed"}
                  <label>
                    <span>{widget.label}</span>
                    <input
                      type="number"
                      min="0"
                      max={Number.MAX_SAFE_INTEGER}
                      step="1"
                      value={String(draft.seed)}
                      disabled={runtimeBusy}
                      oninput={setSeed}
                    />
                    <button
                      type="button"
                      disabled={!runtimeLoaded ||
                        runtimeBusy ||
                        draftError !== ""}
                      onclick={commitSeed}>Set seed</button
                    >
                  </label>
                {/if}
              </article>
            {/each}
          </div>
        </section>
      {/each}
    </div>
  </div>
</section>

<style>
  .declarative-deck {
    --line: #4f5960;
    --panel: #151a1d;
    --raised: #20272b;
    --accent: #93d7ab;
    --warning: #e0b477;
    --error: #ed8f98;
    min-height: calc(100vh - 132px);
    border: 1px solid var(--line);
    color: #dce4e6;
    background:
      linear-gradient(135deg, rgb(255 255 255 / 2%), transparent 32%), #101416;
  }

  .deck-heading,
  .output-actions > header,
  .faceplate-section > header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    border-bottom: 1px solid var(--line);
    padding: 10px 14px;
    background: var(--raised);
  }

  .deck-heading p,
  .deck-heading h2,
  .deck-heading small,
  .output-actions > header strong,
  .faceplate-section h3,
  .faceplate-section header span {
    margin: 0;
  }

  .deck-heading p,
  .deck-heading small,
  .faceplate-section header span,
  .output-actions > header span,
  .runtime-state,
  .deck-summary {
    color: #879297;
    font:
      0.62rem/1.4 ui-monospace,
      "Cascadia Mono",
      Consolas,
      monospace;
  }

  .deck-heading h2,
  .output-actions > header strong,
  .faceplate-section h3 {
    font-family: "Arial Narrow", "Segoe UI", sans-serif;
    letter-spacing: 0.05em;
    text-transform: uppercase;
  }

  .deck-heading {
    height: 72px;
    overflow: hidden;
  }

  .deck-heading > div:first-child,
  .runtime-state {
    min-width: 0;
  }

  .runtime-state {
    display: grid;
    min-width: 190px;
    gap: 2px;
    border: 1px solid #4f6e5b;
    padding: 8px 10px;
  }

  .runtime-state span {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .runtime-state strong {
    color: var(--accent);
  }

  .runtime-state.offline strong {
    color: var(--warning);
  }

  .surface-notice {
    height: 34px;
    margin: 0;
    border-bottom: 1px solid var(--line);
    overflow: hidden;
    padding: 8px 14px;
    color: var(--warning);
    background: #241e16;
    font-size: 0.72rem;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .surface-notice.error {
    color: var(--error);
    background: #261719;
  }

  .deck-workbench {
    display: grid;
    grid-template-columns: minmax(440px, 1.15fr) minmax(360px, 0.85fr);
    align-items: start;
    gap: 10px;
    padding: 10px;
  }

  .output-column {
    position: sticky;
    z-index: 20;
    top: 0;
    display: grid;
    min-width: 0;
    gap: 8px;
    align-self: start;
  }

  .output-stage,
  .output-actions,
  .faceplate-section {
    min-width: 0;
    border: 1px solid #3d484d;
    background: #0d1214;
  }

  .output-stage {
    overflow: hidden;
    background: #000;
  }

  .output-actions {
    display: grid;
    gap: 8px;
    padding-bottom: 10px;
  }

  .output-actions > header,
  .faceplate-section > header {
    min-height: 38px;
    padding-block: 7px;
  }

  .primary-actions,
  .capture-buttons,
  .captured-source-actions {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(132px, 1fr));
    gap: 6px;
    padding-inline: 10px;
  }

  .capture-module,
  .output-connectors {
    margin-inline: 10px;
    border: 1px solid #354046;
    padding: 9px;
    background: var(--panel);
  }

  .capture-controls {
    display: grid;
    min-width: 0;
    gap: 7px;
  }

  .capture-buttons,
  .captured-source-actions {
    padding-inline: 0;
  }

  .capture-buttons button {
    min-height: 34px;
    white-space: nowrap;
  }

  .capture-status {
    display: grid;
    grid-template-rows: 1.25rem 2.5rem;
    min-height: 3.75rem;
    color: #879297;
    font:
      0.62rem/1.25 ui-monospace,
      "Cascadia Mono",
      Consolas,
      monospace;
  }

  .capture-status small {
    overflow: hidden;
  }

  .capture-reason {
    min-height: 2.5rem;
    color: var(--warning);
  }

  .capture-reason.quiet {
    visibility: hidden;
  }

  .output-connectors {
    display: grid;
    grid-template-columns: minmax(0, 0.72fr) minmax(0, 1.28fr);
    gap: 8px;
  }

  .mp4-output,
  .spout-output {
    display: grid;
    min-width: 0;
    align-content: start;
    gap: 6px;
  }

  .spout-output {
    grid-template-columns: minmax(0, 1fr) auto;
  }

  .spout-output strong,
  .spout-output small {
    grid-column: 1 / -1;
  }

  .output-connectors small,
  .deck-summary {
    overflow: hidden;
    color: #7f8a8f;
    font-size: 0.61rem;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .deck-summary {
    padding-inline: 10px;
  }

  .control-column {
    display: grid;
    min-width: 0;
    gap: 8px;
  }

  .geometry-warning {
    display: grid;
    gap: 3px;
    margin: 0;
    border: 1px solid #80672f;
    padding: 9px 11px;
    color: #d9c18a;
    background: #211b11;
    font-size: 0.68rem;
  }

  .faceplate-section > header {
    justify-content: flex-start;
  }

  .widget-grid {
    display: grid;
    grid-template-columns: repeat(var(--section-columns, 1), minmax(0, 1fr));
    gap: 8px;
    padding: 10px;
  }

  .widget {
    min-width: 0;
    border: 1px solid #384247;
    padding: 9px;
    background: var(--panel);
  }

  .widget label,
  .widget fieldset,
  .capture-controls {
    display: grid;
    min-width: 0;
    gap: 6px;
    margin: 0;
  }

  .widget label > span,
  .widget legend,
  .capture-controls strong {
    color: #9da8ad;
    font-size: 0.68rem;
    font-weight: 700;
    text-transform: uppercase;
  }

  .source-detail {
    min-height: 1.1rem;
    overflow: hidden;
    color: #879297;
    font-size: 0.62rem;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  select,
  input,
  button {
    min-width: 0;
    border: 1px solid #58636a;
    border-radius: 2px;
    padding: 6px 8px;
    color: #e2e7e9;
    background: #0e1214;
    font: inherit;
  }

  button:not(:disabled) {
    cursor: pointer;
  }

  button.primary,
  button.active {
    border-color: #7db695;
    background: #294536;
  }

  button:disabled,
  input:disabled,
  select:disabled {
    opacity: 0.5;
  }

  .toggle {
    display: flex !important;
    align-items: center;
    grid-template-columns: auto 1fr;
  }

  .role-grid,
  .transport-grid {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(130px, 1fr));
    gap: 6px;
  }

  .transport-grid > div {
    display: grid;
    gap: 5px;
    border: 1px solid #343d42;
    padding: 7px;
  }

  .triangle {
    position: relative;
    height: 130px;
    clip-path: polygon(50% 0, 100% 100%, 0 100%);
    background:
      linear-gradient(150deg, #67536c, transparent 65%),
      linear-gradient(30deg, #345d5c, #2a383d);
  }

  .triangle span {
    position: absolute;
    z-index: 1;
    font-size: 0.55rem;
  }

  .triangle span:nth-child(1) {
    top: 8px;
    left: 50%;
  }

  .triangle span:nth-child(2) {
    right: 8px;
    bottom: 8px;
  }

  .triangle span:nth-child(3) {
    bottom: 8px;
    left: 8px;
  }

  .triangle i {
    position: absolute;
    width: 9px;
    height: 9px;
    border: 2px solid #fff;
    border-radius: 50%;
    background: #111;
    transform: translate(-50%, -50%);
  }

  .monitor {
    display: grid;
    height: clamp(280px, 44vh, 560px);
    min-height: 0;
    grid-template-rows: auto minmax(0, 1fr);
    background: #000;
  }

  .monitor > header {
    display: flex;
    justify-content: space-between;
    padding: 7px 10px;
    color: #899496;
    background: #0b0f10;
    font-size: 0.62rem;
  }

  .monitor-frame {
    position: relative;
    min-height: 0;
    overflow: hidden;
  }

  .native-monitor-anchor,
  .monitor-placeholder {
    position: absolute;
    inset: 0;
  }

  .native-monitor-anchor {
    contain: layout paint;
    background: #000;
  }

  .monitor-placeholder {
    display: grid;
    place-content: center;
    place-items: center;
    color: #778185;
    background:
      linear-gradient(rgb(50 61 65 / 25%) 1px, transparent 1px),
      linear-gradient(90deg, rgb(50 61 65 / 25%) 1px, transparent 1px), #06090a;
    background-size: 40px 40px;
  }

  :global(html.deck-output-fullscreen),
  :global(body.deck-output-fullscreen) {
    overflow: hidden !important;
    overscroll-behavior: none;
  }

  .declarative-deck.fullscreen {
    position: fixed;
    inset: 0;
    z-index: 2147483647;
    width: 100vw;
    height: 100dvh;
    min-height: 0;
    overflow: hidden;
    border: 0;
    background: #000;
  }

  .fullscreen > :not(.deck-workbench),
  .fullscreen .control-column,
  .fullscreen .output-actions {
    display: none;
  }

  .fullscreen .deck-workbench {
    display: block;
    width: 100%;
    height: 100%;
    padding: 0;
  }

  .fullscreen .output-column {
    position: static;
    display: block;
    width: 100%;
    height: 100%;
  }

  .fullscreen .output-stage,
  .fullscreen .monitor {
    width: 100%;
    height: 100%;
    min-height: 0;
    border: 0;
  }

  .fullscreen .monitor {
    grid-template-rows: minmax(0, 1fr);
  }

  .fullscreen .monitor > header {
    display: none;
  }

  .fullscreen .monitor-frame {
    height: 100%;
  }

  @media (max-width: 1120px) {
    .deck-workbench {
      grid-template-columns: minmax(0, 1fr);
    }

    .output-column {
      position: relative;
      top: auto;
    }

    .widget-grid {
      grid-template-columns: minmax(0, 1fr);
    }

    .output-connectors {
      grid-template-columns: minmax(0, 1fr);
    }
  }
</style>
