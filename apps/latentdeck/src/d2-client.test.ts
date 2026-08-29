import { describe, expect, it } from "vitest";
import {
  D2_COMMANDS,
  D2_EVENTS,
  createD2Client,
  type D2HostAdapter,
} from "./d2-client";
import {
  DEFAULT_D2_BACKEND,
  DEFAULT_D2_CAPTURE,
  DEFAULT_D2_CONTROLS,
  DEFAULT_D2_STATUS,
  DEFAULT_D2_TRANSPORT,
  type D2ErrorEvent,
} from "./d2-model";

describe("typed LD-D2 host client", () => {
  it("uses the planned host commands with exact bounded payloads", async () => {
    expect(D2_COMMANDS).toEqual({
      backendStatusGet: "deck_d2_backend_status_get",
      selectDecoder: "deck_d2_select_decoder",
      open: "deck_d2_open",
      controlsSet: "deck_d2_controls_set",
      transportSet: "deck_d2_transport_set",
      seedSet: "deck_d2_seed_set",
      restart: "deck_d2_restart",
      captureSnapshot: "deck_d2_capture_snapshot",
      captureLiveStart: "deck_d2_capture_live_start",
      captureLiveStop: "deck_d2_capture_live_stop",
      captureStatusGet: "deck_d2_capture_status_get",
      statusGet: "deck_d2_status_get",
    });
    const calls: Array<{ command: string; args: Record<string, unknown> }> = [];
    const host: D2HostAdapter = {
      invoke: async <T>(
        command: string,
        args: Record<string, unknown> = {},
      ) => {
        calls.push({ command, args });
        return DEFAULT_D2_STATUS as T;
      },
      listen: async () => () => undefined,
    };
    const client = createD2Client(host);
    const openRequest = {
      sourceA: { cartridgeId: "a", archiveSha256: "a".repeat(64) },
      sourceB: { cartridgeId: "b", archiveSha256: "b".repeat(64) },
      controls: DEFAULT_D2_CONTROLS,
      transport: DEFAULT_D2_TRANSPORT,
      seed: 9,
    };

    await client.backendStatusGet();
    await client.selectDecoder();
    await client.open(openRequest);
    await client.controlsSet(DEFAULT_D2_CONTROLS);
    await client.transportSet(DEFAULT_D2_TRANSPORT);
    await client.seedSet(9);
    await client.restart();
    await client.captureSnapshot();
    await client.captureLiveStart();
    await client.captureLiveStop();
    await client.captureStatusGet();
    await client.statusGet();

    expect(calls).toEqual([
      { command: "deck_d2_backend_status_get", args: {} },
      { command: "deck_d2_select_decoder", args: {} },
      { command: "deck_d2_open", args: openRequest },
      {
        command: "deck_d2_controls_set",
        args: { controls: DEFAULT_D2_CONTROLS },
      },
      {
        command: "deck_d2_transport_set",
        args: { transport: DEFAULT_D2_TRANSPORT },
      },
      { command: "deck_d2_seed_set", args: { seed: 9 } },
      { command: "deck_d2_restart", args: {} },
      { command: "deck_d2_capture_snapshot", args: {} },
      { command: "deck_d2_capture_live_start", args: {} },
      { command: "deck_d2_capture_live_stop", args: {} },
      { command: "deck_d2_capture_status_get", args: {} },
      { command: "deck_d2_status_get", args: {} },
    ]);
  });

  it("keeps decoder provenance typed and path-free", async () => {
    const host: D2HostAdapter = {
      invoke: async <T>() =>
        ({
          ...DEFAULT_D2_BACKEND,
          state: "ready",
          decoder: {
            assetId: "taeh3",
            variantId: "official",
            sha256: "a".repeat(64),
            byteLength: 42,
            sourceUrl: "https://example.invalid/weight",
            licenseLabel: "upstream license",
            licenseUrl: "https://example.invalid/license",
          },
        }) as T,
      listen: async () => () => undefined,
    };

    const backend = await createD2Client(host).backendStatusGet();
    expect(backend.decoder?.sha256).toBe("a".repeat(64));
    expect(JSON.stringify(backend)).not.toMatch(/[A-Z]:\\\\/);
  });

  it("subscribes to typed status and error event channels", async () => {
    expect(D2_EVENTS).toEqual({
      status: "deck-d2-status",
      error: "deck-d2-error",
      capture: "deck-d2-capture",
      captureError: "deck-d2-capture-error",
    });
    const listeners = new Map<string, (payload: unknown) => void>();
    const host: D2HostAdapter = {
      invoke: async <T>() => DEFAULT_D2_STATUS as T,
      listen: async <T>(event: string, handler: (payload: T) => void) => {
        listeners.set(event, handler as (payload: unknown) => void);
        return () => listeners.delete(event);
      },
    };
    const client = createD2Client(host);
    let status = null;
    let error: D2ErrorEvent | null = null;
    const stopStatus = await client.onStatus((incoming) => {
      status = incoming;
    });
    const stopError = await client.onError((incoming) => {
      error = incoming;
    });
    let capture = null;
    const stopCapture = await client.onCapture((incoming) => {
      capture = incoming;
    });

    listeners.get(D2_EVENTS.status)?.(DEFAULT_D2_STATUS);
    listeners.get(D2_EVENTS.error)?.({
      code: "deck.worker_missing",
      detail: "Codec host pending.",
    });
    listeners.get(D2_EVENTS.capture)?.(DEFAULT_D2_CAPTURE);
    expect(status).toBe(DEFAULT_D2_STATUS);
    expect(error).toEqual({
      code: "deck.worker_missing",
      detail: "Codec host pending.",
    });
    expect(capture).toBe(DEFAULT_D2_CAPTURE);

    stopStatus();
    stopError();
    stopCapture();
    expect(listeners.size).toBe(0);
  });
});
