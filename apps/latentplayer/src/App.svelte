<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { open } from "@tauri-apps/plugin-dialog";
  import { onMount } from "svelte";
  import {
    EMPTY_PLAYER_VIEW,
    acceptTrustedSnapshot,
    controlsFor,
    describeAudioAvailability,
    describeDiagnosticSaveResult,
    describePlayerOperation,
    describeRuntimeStatus,
    diagnosticSaveEnabled,
    formatFrameRate,
    formatFramePosition,
    fullscreenActionLabel,
    progressPercent,
    selectDisplayedError,
    spoutControlsFor,
    type DiagnosticSaveResult,
    type FullscreenStatus,
    type PlayerError,
    type PlayerOperation,
    type PlayerView,
    type SpoutStatus,
  } from "./player-model";
  import { product } from "./product";

  let player = $state<PlayerView>(EMPTY_PLAYER_VIEW);
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
      if (!snapshot.outputAvailable) {
        fullscreen = null;
      }
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
        fullscreen = null;
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
      fullscreen = null;
      if (reportError) {
        transientError = playerError(error, "output.fullscreen_status_failed");
      }
    } finally {
      fullscreenPending = false;
    }
  }

  async function setFullscreen(enabled: boolean): Promise<void> {
    busy = true;
    operation = enabled ? "fullscreen-enter" : "fullscreen-exit";
    transientError = null;
    try {
      fullscreen = await invoke<FullscreenStatus>("player_set_fullscreen", {
        enabled,
      });
    } catch (error) {
      fullscreen = null;
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

  onMount(() => {
    void refreshSnapshot(true);
    void refreshFullscreen();
    void refreshSpout();
    const snapshotTimer = globalThis.setInterval(() => {
      void refreshSnapshot();
    }, 100);
    const spoutTimer = globalThis.setInterval(() => {
      void refreshSpout();
      void refreshFullscreen();
    }, 250);
    return () => {
      globalThis.clearInterval(snapshotTimer);
      globalThis.clearInterval(spoutTimer);
    };
  });
</script>

<svelte:window onkeydown={handleWindowKeydown} />

<svelte:head>
  <title>{product.name}</title>
</svelte:head>

<main class="player-shell" aria-busy={busy}>
  <header class="masthead">
    <div>
      <p class="eyebrow">Latent cartridge playback</p>
      <h1>{product.name}</h1>
    </div>
    <p class="version">v{product.version}</p>
  </header>

  <section class="output-monitor" aria-label="Native output status">
    <div class="monitor-grid" aria-hidden="true"></div>
    <div class="monitor-copy">
      <p class="monitor-state">{player.phase}</p>
      <p class="monitor-title">
        {player.cartridge?.fileName ?? "No cartridge loaded"}
      </p>
      {#if player.cartridge}
        <p class="monitor-detail">
          {player.cartridge.width} × {player.cartridge.height} ·
          {player.cartridge.frameCount} frames · {formatFrameRate(player)}
        </p>
        <p class:preserved={player.cartridge.audioPresent} class="audio-notice">
          {audioAvailability}
        </p>
      {:else}
        <p class="monitor-detail">Open a validated .lc cartridge to begin.</p>
      {/if}
    </div>
  </section>

  <section class="transport" aria-label="Playback controls">
    <button class="open" disabled={!controls.open} onclick={openCartridge}
      >{operation === "open" ? "Opening…" : "Open"}</button
    >
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
      disabled={!controls.fullscreen || fullscreen === null}
      onclick={() => setFullscreen(!(fullscreen?.active ?? false))}
      >{fullscreenActionLabel(fullscreen)}</button
    >
  </section>

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

  <footer class="status-strip">
    <div>
      <span class:ready={player.codec.state === "ready"} class="status-dot"
      ></span>
      <span>Codec</span>
      <strong>{player.codec.displayName ?? player.codec.state}</strong>
    </div>
    <p>{describeRuntimeStatus(player)}</p>
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
        <span>Spout2 · GPU texture</span>
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

  {#if displayedError}
    <aside class="error-panel" role="alert">
      <strong>{displayedError.code}</strong>
      <span>{displayedError.message}</span>
    </aside>
  {/if}
</main>
