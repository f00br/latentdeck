<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { open } from "@tauri-apps/plugin-dialog";
  import { onMount, tick } from "svelte";
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
    const snapshotTimer = globalThis.setInterval(() => {
      void refreshSnapshot();
    }, 100);
    const spoutTimer = globalThis.setInterval(() => {
      void refreshSpout();
      void refreshFullscreen();
    }, 250);
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
    };
  });
</script>

<svelte:window onkeydown={handleWindowKeydown} />

<svelte:head>
  <title>{product.name}</title>
</svelte:head>

<main
  class="player-shell"
  class:fullscreen-shell={fullscreen?.active && player.outputAvailable}
  aria-busy={busy}
>
  <header class="masthead">
    <div class="brand-lockup">
      <p class="eyebrow">Latent cartridge playback</p>
      <h1>{product.name}</h1>
    </div>
    <div class="masthead-actions">
      <span class="phase-badge">{player.phase}</span>
      <button class="open" disabled={!controls.open} onclick={openCartridge}
        >{operation === "open" ? "Opening…" : "Open cartridge"}</button
      >
      <p class="version">v{product.version}</p>
    </div>
  </header>

  <div class="player-workspace">
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

  <section class="control-dock" aria-label="Playback and position">
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

  <footer class="status-strip">
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
