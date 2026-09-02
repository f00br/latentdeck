<script lang="ts">
  import type {
    Barycentric3Widget,
    NumericWidget,
    SelectWidget,
    ToggleWidget,
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
  }

  export let model: DeckUiModel;
  export let initialDraft: DeckUiDraft;
  export let sourceOptions: readonly SourceOption[] = [];
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
  export let captureActive = false;
  export let liveCaptureActive = false;
  export let captureUnavailableReason =
    "Latent capture is unavailable for this exact Deck and Codec profile.";
  export let outputFullscreen: boolean | null = null;
  export let onDraftChange: (draft: DeckUiDraft) => void = () => undefined;
  export let onLoad: (draft: DeckUiDraft) => void | Promise<void> = () =>
    undefined;
  export let onRestart: () => void | Promise<void> = () => undefined;
  export let onProcessOnce: () => void | Promise<void> = () => undefined;
  export let onControlsCommit: (
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
  export let onFullscreenToggle: () => void | Promise<void> = () => undefined;
  export let onMonitorAnchor: (element: HTMLDivElement | null) => void = () =>
    undefined;

  let draft = cloneDraft(initialDraft);
  let draftError = "";
  let sourceDraftReady = false;

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
    validateDraft();
    emitDraft();
  }

  function setToggleControl(widget: ToggleWidget, event: Event): void {
    draft.controls[widget.control_id] = (
      event.currentTarget as HTMLInputElement
    ).checked;
    validateDraft();
    emitDraft();
  }

  function setSelectControl(widget: SelectWidget, event: Event): void {
    draft.controls[widget.control_id] = (
      event.currentTarget as HTMLSelectElement
    ).value;
    validateDraft();
    emitDraft();
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
    validateDraft();
    emitDraft();
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
</script>

<section
  class="declarative-deck"
  class:fullscreen={active && outputFullscreen === true && runtimeLoaded}
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

  {#if !runtimeAvailable && !runtimeLoaded}
    <p class="runtime-unavailable" role="status">
      {runtimeUnavailableReason}
    </p>
  {/if}
  {#if runtimeAvailable && !loadAvailable}
    <p class="runtime-unavailable" role="status">
      {loadUnavailableReason}
    </p>
  {/if}
  {#if draftError !== ""}
    <p class="draft-error" role="alert">{draftError}</p>
  {/if}

  <div class="faceplate-sections">
    {#each model.faceplate.sections as section (section.section_id)}
      <section
        class="faceplate-section"
        class:monitor-section={section.widgets.some(
          (widget) => widget.kind === "monitor",
        )}
        aria-labelledby={`${model.exactKey}-${section.section_id}`}
      >
        <header>
          <span>{section.section_id}</span>
          <h3 id={`${model.exactKey}-${section.section_id}`}>
            {section.title}
          </h3>
        </header>
        <div class="widget-grid">
          {#each section.widgets as widget (widget.id)}
            <article
              class={`widget widget-${widget.kind}`}
              data-widget-kind={widget.kind}
            >
              {#if widget.kind === "source_picker"}
                <label>
                  <span>{widget.label}</span>
                  <select
                    value={draft.sourceArchiveSha256s[widget.slot_index] ?? ""}
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
                          >{draft.playing[slotIndex] ? "Pause" : "Play"}</button
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
              {:else if widget.kind === "capture"}
                <div class="capture-controls">
                  <strong>{widget.label}</strong>
                  {#if widget.modes.includes("snapshot")}
                    <button
                      type="button"
                      disabled={!captureAvailable ||
                        captureActive ||
                        !runtimeLoaded ||
                        runtimeBusy}
                      onclick={() => void onCapture("snapshot")}
                      >Snapshot</button
                    >
                  {/if}
                  {#if widget.modes.includes("live_capture")}
                    <button
                      type="button"
                      disabled={!captureAvailable ||
                        (captureActive && !liveCaptureActive) ||
                        !runtimeLoaded ||
                        runtimeBusy}
                      onclick={() => void onCapture("live_capture")}
                      >{liveCaptureActive
                        ? "Stop Live Capture"
                        : "Start Live Capture"}</button
                    >
                  {/if}
                  <small>{captureState}</small>
                  {#if !captureAvailable}
                    <small class="capability-unavailable"
                      >{captureUnavailableReason}</small
                    >
                  {/if}
                </div>
              {:else if widget.kind === "monitor"}
                <div class="monitor" class:live={runtimeLoaded}>
                  <header>
                    <span>{widget.label}</span>
                    <strong
                      >{runtimeLoaded
                        ? "POST-OPERATOR STREAM"
                        : "STANDBY"}</strong
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
                        <small
                          >Intrinsic signal only · no hidden conversion</small
                        >
                      </div>
                    {/if}
                  </div>
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
              {/if}
            </article>
          {/each}
        </div>
      </section>
    {/each}
  </div>

  <footer class="deck-actions">
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
      onclick={commitControls}>Apply controls</button
    >
    <button
      type="button"
      disabled={!runtimeLoaded || runtimeBusy}
      onclick={() => void onProcessOnce()}>Process once</button
    >
    <small>{model.summary}</small>
  </footer>
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
    background: #101416;
  }

  .deck-heading,
  .deck-actions,
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
  .faceplate-section h3,
  .faceplate-section header span {
    margin: 0;
  }

  .deck-heading p,
  .deck-heading small,
  .faceplate-section header span,
  .runtime-state,
  .deck-actions small {
    color: #879297;
    font:
      0.62rem/1.4 ui-monospace,
      "Cascadia Mono",
      Consolas,
      monospace;
  }

  .deck-heading h2,
  .faceplate-section h3 {
    font-family: "Arial Narrow", "Segoe UI", sans-serif;
    letter-spacing: 0.05em;
    text-transform: uppercase;
  }

  .runtime-state {
    display: grid;
    min-width: 190px;
    gap: 2px;
    border: 1px solid #4f6e5b;
    padding: 8px 10px;
  }

  .runtime-state strong {
    color: var(--accent);
  }

  .runtime-state.offline strong {
    color: var(--warning);
  }

  .runtime-unavailable,
  .draft-error,
  .capability-unavailable {
    margin: 0;
    border-bottom: 1px solid var(--line);
    padding: 8px 14px;
    color: var(--warning);
    background: #241e16;
    font-size: 0.72rem;
  }

  .draft-error {
    color: var(--error);
    background: #261719;
  }

  .capability-unavailable {
    color: var(--warning);
  }

  .faceplate-sections {
    display: grid;
  }

  .faceplate-section > header {
    justify-content: flex-start;
    padding-block: 7px;
  }

  .widget-grid {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(240px, 1fr));
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

  .monitor-section {
    position: sticky;
    z-index: 20;
    top: 0;
    order: -1;
    background: #050708;
  }

  .monitor-section > header {
    display: none;
  }

  .monitor-section .widget-grid,
  .widget-monitor {
    padding: 0;
  }

  .widget-monitor {
    grid-column: 1 / -1;
    border: 0;
  }

  .monitor {
    display: grid;
    height: clamp(280px, 44vh, 560px);
    grid-template-rows: auto minmax(0, 1fr) auto;
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

  .declarative-deck.fullscreen {
    position: fixed;
    inset: 0;
    z-index: 1000;
    width: 100vw;
    height: 100dvh;
    min-height: 0;
    overflow: hidden;
    border: 0;
    background: #000;
  }

  .fullscreen > :not(.faceplate-sections),
  .fullscreen .faceplate-section:not(.monitor-section) {
    display: none;
  }

  .fullscreen .faceplate-sections,
  .fullscreen .monitor-section,
  .fullscreen .widget-grid,
  .fullscreen .widget-monitor,
  .fullscreen .monitor {
    height: 100%;
    min-height: 0;
  }

  .fullscreen .monitor {
    grid-template-rows: minmax(0, 1fr);
  }

  .fullscreen .monitor > header,
  .fullscreen .monitor > button {
    display: none;
  }

  .deck-actions {
    justify-content: flex-start;
  }

  .deck-actions small {
    margin-left: auto;
  }
</style>
