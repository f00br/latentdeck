<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { open } from "@tauri-apps/plugin-dialog";
  import { onMount } from "svelte";
  import {
    EMPTY_PLAYER_VIEW,
    acceptTrustedSnapshot,
    controlsFor,
    formatFrameRate,
    formatFramePosition,
    progressPercent,
    spoutControlsFor,
    type PlayerView,
    type SpoutStatus,
  } from "./player-model";
  import { product } from "./product";

  let player = $state<PlayerView>(EMPTY_PLAYER_VIEW);
  let busy = $state(false);
  let transientError = $state<string | null>(null);
  let snapshotPending = false;
  let spout = $state<SpoutStatus | null>(null);
  let spoutBusy = $state(false);
  let spoutName = $state("LatentPlayer Output");
  let spoutNameDirty = $state(false);
  let spoutPending = false;

  const controls = $derived(controlsFor(player, busy));
  const progress = $derived(progressPercent(player));
  const spoutControls = $derived(spoutControlsFor(spout, busy || spoutBusy));
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

  async function command(
    name: string,
    arguments_: Record<string, unknown> = {},
  ): Promise<void> {
    busy = true;
    transientError = null;
    try {
      const snapshot = await invoke<PlayerView>(name, arguments_);
      player = acceptTrustedSnapshot(player, snapshot);
    } catch (error) {
      transientError = errorMessage(error);
    } finally {
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
        transientError = errorMessage(error);
      }
    } finally {
      snapshotPending = false;
    }
  }

  async function openCartridge(): Promise<void> {
    const path = await open({
      multiple: false,
      directory: false,
      filters: [{ name: "Latent Cartridge", extensions: ["lc"] }],
    });
    if (typeof path === "string") {
      await command("player_open", { path });
    }
  }

  async function selectDecoder(): Promise<void> {
    const path = await open({
      multiple: false,
      directory: false,
      filters: [{ name: "TAEH3 Safetensors", extensions: ["safetensors"] }],
    });
    if (typeof path === "string") {
      await command("player_select_decoder", { path });
    }
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
        transientError = errorMessage(error);
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
      transientError = errorMessage(error);
    } finally {
      spoutBusy = false;
    }
  }

  onMount(() => {
    void refreshSnapshot(true);
    void refreshSpout();
    const snapshotTimer = globalThis.setInterval(() => {
      void refreshSnapshot();
    }, 100);
    const spoutTimer = globalThis.setInterval(() => {
      void refreshSpout();
    }, 250);
    return () => {
      globalThis.clearInterval(snapshotTimer);
      globalThis.clearInterval(spoutTimer);
    };
  });
</script>

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
      {:else}
        <p class="monitor-detail">Open a validated .lc cartridge to begin.</p>
      {/if}
    </div>
  </section>

  <section class="transport" aria-label="Playback controls">
    <button class="open" disabled={!controls.open} onclick={openCartridge}
      >Open</button
    >
    <div class="transport-group">
      <button disabled={!controls.play} onclick={() => command("player_play")}
        >Play</button
      >
      <button disabled={!controls.pause} onclick={() => command("player_pause")}
        >Pause</button
      >
      <button
        class:active={player.loopEnabled}
        aria-pressed={player.loopEnabled}
        disabled={!controls.loop}
        onclick={() =>
          command("player_set_loop", { enabled: !player.loopEnabled })}
        >Loop</button
      >
      <button
        disabled={!controls.restart}
        onclick={() => command("player_restart")}>Restart</button
      >
    </div>
    <button
      disabled={!controls.fullscreen}
      onclick={() => command("player_fullscreen")}>Fullscreen</button
    >
  </section>

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
    <p>{player.codec.detail ?? "Runtime connected"}</p>
    {#if controls.configureCodec}
      <button class="codec-select" onclick={selectDecoder}
        >Select decoder</button
      >
    {/if}
  </footer>

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

  {#if player.error || transientError}
    <aside class="error-panel" role="alert">
      <strong>{player.error?.code ?? "player.command_failed"}</strong>
      <span>{player.error?.message ?? transientError}</span>
    </aside>
  {/if}
</main>
