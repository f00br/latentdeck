import { describe, expect, it } from "vitest";
import {
  Q4_COMMANDS,
  Q4_EVENTS,
  createQ4Client,
  selectQ4DecoderAndStatus,
  type Q4HostAdapter,
} from "./q4-client";
import {
  DEFAULT_Q4_BACKEND,
  DEFAULT_Q4_CONTROLS,
  DEFAULT_Q4_ROLES,
  DEFAULT_Q4_STATUS,
  DEFAULT_Q4_TRANSPORT,
} from "./q4-model";

describe("Q4 client boundary", () => {
  it("uses path-free typed Tauri commands", async () => {
    const calls: Array<{
      command: string;
      args: Record<string, unknown> | undefined;
    }> = [];
    const host: Q4HostAdapter = {
      invoke: async <T>(command: string, args?: Record<string, unknown>) => {
        calls.push({ command, args });
        return {} as T;
      },
      listen: async () => () => undefined,
    };
    const client = createQ4Client(host);

    await client.backendRediscover();
    await client.open({
      sourceA: { cartridgeId: "a", archiveSha256: "1" },
      sourceB: { cartridgeId: "b", archiveSha256: "2" },
      sourceC: { cartridgeId: "c", archiveSha256: "3" },
      sourceD: { cartridgeId: "d", archiveSha256: "4" },
      roles: DEFAULT_Q4_ROLES,
      controls: DEFAULT_Q4_CONTROLS,
      transport: DEFAULT_Q4_TRANSPORT,
      seed: 7,
    });
    await client.rolesSet(DEFAULT_Q4_ROLES);
    await client.captureSnapshot();
    await client.recordingStart();
    await client.recordingStatusGet();
    await client.recordingStop();
    await client.viewportSessionBegin();
    await client.viewportSetBounds({
      epoch: 4,
      revision: 7,
      xCss: 10,
      yCss: 20,
      widthCss: 640,
      heightCss: 360,
      scaleFactor: 1.5,
      visible: true,
    });
    await client.fullscreenStatusGet();
    await client.fullscreenSet(true);
    await client.spoutStatusGet();
    await client.spoutConfigure({ name: "LatentDeck Q4", enabled: true });

    expect(calls.map((call) => call.command)).toEqual([
      Q4_COMMANDS.backendRediscover,
      Q4_COMMANDS.open,
      Q4_COMMANDS.rolesSet,
      Q4_COMMANDS.captureSnapshot,
      Q4_COMMANDS.recordingStart,
      Q4_COMMANDS.recordingStatusGet,
      Q4_COMMANDS.recordingStop,
      Q4_COMMANDS.viewportSessionBegin,
      Q4_COMMANDS.viewportSetBounds,
      Q4_COMMANDS.fullscreenStatusGet,
      Q4_COMMANDS.fullscreenSet,
      Q4_COMMANDS.spoutStatusGet,
      Q4_COMMANDS.spoutConfigure,
    ]);
    expect(calls.at(-1)?.args).toEqual({
      name: "LatentDeck Q4",
      enabled: true,
    });
    expect(
      calls.find((call) => call.command === Q4_COMMANDS.viewportSetBounds)
        ?.args,
    ).toEqual({
      bounds: {
        epoch: 4,
        revision: 7,
        xCss: 10,
        yCss: 20,
        widthCss: 640,
        heightCss: 360,
        scaleFactor: 1.5,
        visible: true,
      },
    });
    expect(JSON.stringify(calls)).not.toContain("path");
  });

  it("subscribes to the four closed Q4 event names", async () => {
    const events: string[] = [];
    const host: Q4HostAdapter = {
      invoke: async <T>() => ({}) as T,
      listen: async (event) => {
        events.push(event);
        return () => undefined;
      },
    };
    const client = createQ4Client(host);
    await Promise.all([
      client.onStatus(() => undefined),
      client.onError(() => undefined),
      client.onCapture(() => undefined),
      client.onCaptureError(() => undefined),
    ]);
    expect(events).toEqual([
      Q4_EVENTS.status,
      Q4_EVENTS.error,
      Q4_EVENTS.capture,
      Q4_EVENTS.captureError,
    ]);
  });

  it("queries authoritative Q4 runtime status after the native decoder picker", async () => {
    const calls: string[] = [];
    const host: Q4HostAdapter = {
      invoke: async <T>(command: string) => {
        calls.push(command);
        if (command === Q4_COMMANDS.selectDecoder) {
          return { ...DEFAULT_Q4_BACKEND, state: "ready" } as T;
        }
        if (command === Q4_COMMANDS.statusGet) {
          return { ...DEFAULT_Q4_STATUS, loaded: true } as T;
        }
        throw new Error(`Unexpected command ${command}`);
      },
      listen: async () => () => undefined,
    };

    const selection = await selectQ4DecoderAndStatus(createQ4Client(host));

    expect(selection.status.loaded).toBe(true);
    expect(calls).toEqual([Q4_COMMANDS.selectDecoder, Q4_COMMANDS.statusGet]);
  });
});
