import { describe, expect, it } from "vitest";

import {
  GENERIC_DECK_COMMANDS,
  createGenericDeckClient,
  parseGenericCaptureView,
  parseGenericRecordingView,
  type GenericDeckHostAdapter,
  type GenericDeckOpenRequest,
  type GenericRuntimeOptionsRequest,
} from "./generic-deck-client";

describe("generic Deck exact host client", () => {
  it("sends discovery with a null profile and never invents an exact profile", async () => {
    const calls: Array<[string, Record<string, unknown>]> = [];
    const client = createGenericDeckClient(host(calls));
    const request: GenericRuntimeOptionsRequest = {
      deckId: "org.example.deck",
      deckVersion: "1.2.3",
      codecId: "org.example.codec",
      codecVersion: "4.5.6",
      profileKey: null,
      device: "cpu",
      deviceOrdinal: 0,
      sources: [],
    };

    await client.runtimeOptions(request);

    expect(calls).toEqual([
      [GENERIC_DECK_COMMANDS.runtimeOptions, { request }],
    ]);
  });

  it("passes exact versions/profile and immutable Library identities to open", async () => {
    const calls: Array<[string, Record<string, unknown>]> = [];
    const client = createGenericDeckClient(host(calls));
    const request: GenericDeckOpenRequest = {
      deckId: "org.example.deck",
      deckVersion: "1.2.3",
      codecId: "org.example.codec",
      codecVersion: "4.5.6",
      profileKey: {
        codecFamily: "synthetic",
        profile: "grid",
        profileVersion: "1.0.0",
      },
      device: "cpu",
      deviceOrdinal: 0,
      sources: [
        {
          cartridgeId: "10000000-0000-4000-8000-000000000001",
          archiveSha256: "a".repeat(64),
        },
      ],
      roles: [{ role: "carrier", physical_slot: 1 }],
      controls: [{ name: "mix", value: { kind: "number", value: 0.5 } }],
      sourceTransport: [
        { physical_slot: 1, playing: true, loop_enabled: false },
      ],
      seed: 9,
    };

    await client.open(request);

    expect(calls).toEqual([[GENERIC_DECK_COMMANDS.open, { request }]]);
  });

  it("keeps session operations scoped while matching the global viewport host contract", async () => {
    const calls: Array<[string, Record<string, unknown>]> = [];
    const client = createGenericDeckClient(host(calls));
    const bounds = {
      epoch: 1,
      revision: 2,
      xCss: 3,
      yCss: 4,
      widthCss: 640,
      heightCss: 360,
      scaleFactor: 1,
      visible: true,
    };

    await client.sessionsGet();
    await client.statusGet("session-1");
    await client.processOnce("session-1");
    await client.controlsSet("session-1", []);
    await client.rolesSet("session-1", []);
    await client.transportSet("session-1", []);
    await client.seedSet("session-1", 4);
    await client.reset("session-1", false);
    await client.foregroundSet("session-1");
    await client.foregroundClear();
    await client.close("session-1");
    await client.viewportSessionBegin();
    await client.viewportSetBounds(bounds);
    await client.fullscreenStatusGet("session-1");
    await client.fullscreenSet("session-1", false);
    await client.spoutStatusGet("session-1");
    await client.spoutConfigure("session-1", { name: null, enabled: true });
    await client.captureStart("session-1", "snapshot");
    await client.captureStop("session-1");
    await client.captureStatusGet("session-1");
    await client.recordingStart("session-1");
    await client.recordingStop("session-1");
    await client.recordingStatusGet("session-1");

    expect(calls).toEqual([
      [GENERIC_DECK_COMMANDS.sessionsGet, {}],
      [GENERIC_DECK_COMMANDS.statusGet, { sessionId: "session-1" }],
      [GENERIC_DECK_COMMANDS.processOnce, { sessionId: "session-1" }],
      [
        GENERIC_DECK_COMMANDS.controlsSet,
        { sessionId: "session-1", controls: [] },
      ],
      [GENERIC_DECK_COMMANDS.rolesSet, { sessionId: "session-1", roles: [] }],
      [
        GENERIC_DECK_COMMANDS.transportSet,
        { sessionId: "session-1", sourceTransport: [] },
      ],
      [GENERIC_DECK_COMMANDS.seedSet, { sessionId: "session-1", seed: 4 }],
      [
        GENERIC_DECK_COMMANDS.reset,
        { sessionId: "session-1", preservePlayheads: false },
      ],
      [GENERIC_DECK_COMMANDS.foregroundSet, { sessionId: "session-1" }],
      [GENERIC_DECK_COMMANDS.foregroundClear, {}],
      [GENERIC_DECK_COMMANDS.close, { sessionId: "session-1" }],
      [GENERIC_DECK_COMMANDS.viewportSessionBegin, {}],
      [GENERIC_DECK_COMMANDS.viewportSetBounds, { bounds }],
      [GENERIC_DECK_COMMANDS.fullscreenStatusGet, { sessionId: "session-1" }],
      [
        GENERIC_DECK_COMMANDS.fullscreenSet,
        { sessionId: "session-1", enabled: false },
      ],
      [GENERIC_DECK_COMMANDS.spoutStatusGet, { sessionId: "session-1" }],
      [
        GENERIC_DECK_COMMANDS.spoutConfigure,
        { sessionId: "session-1", name: null, enabled: true },
      ],
      [
        GENERIC_DECK_COMMANDS.captureStart,
        { sessionId: "session-1", mode: "snapshot" },
      ],
      [GENERIC_DECK_COMMANDS.captureStop, { sessionId: "session-1" }],
      [GENERIC_DECK_COMMANDS.captureStatusGet, { sessionId: "session-1" }],
      [GENERIC_DECK_COMMANDS.recordingStart, { sessionId: "session-1" }],
      [GENERIC_DECK_COMMANDS.recordingStop, { sessionId: "session-1" }],
      [GENERIC_DECK_COMMANDS.recordingStatusGet, { sessionId: "session-1" }],
    ]);
  });

  it("lets only the native host picker bind an exact declared external asset", async () => {
    const calls: Array<[string, Record<string, unknown>]> = [];
    const client = createGenericDeckClient(host(calls));

    await client.externalAssetSelect("org.example.codec", "4.5.6", "decoder");
    await client.externalAssetClear("org.example.codec", "4.5.6", "decoder");

    expect(calls).toEqual([
      [
        GENERIC_DECK_COMMANDS.externalAssetSelect,
        {
          codecId: "org.example.codec",
          codecVersion: "4.5.6",
          assetId: "decoder",
        },
      ],
      [
        GENERIC_DECK_COMMANDS.externalAssetClear,
        {
          codecId: "org.example.codec",
          codecVersion: "4.5.6",
          assetId: "decoder",
        },
      ],
    ]);
  });

  it("parses the session-scoped capture wrapper with decimal-string latent slots", () => {
    const capture = {
      sessionId: "session-1",
      captureId: "20000000-0000-4000-8000-000000000001",
      mode: "live_capture",
      state: "finished",
      latentSlots: "9007199254740993",
      resetEvents: 2,
      cartridgeId: "30000000-0000-4000-8000-000000000001",
      archiveSha256: "a".repeat(64),
      detail: null,
    };

    expect(parseGenericCaptureView(capture)).toEqual(capture);
    expect(() =>
      parseGenericCaptureView({ ...capture, latentSlots: 9 }),
    ).toThrow(/wire contract/i);
    expect(() =>
      parseGenericCaptureView({ ...capture, state: "done" }),
    ).toThrow(/wire contract/i);
    for (const nonHostState of [
      "awaiting_reset",
      "stop_armed",
      "completed",
      "faulted",
    ]) {
      expect(() =>
        parseGenericCaptureView({ ...capture, state: nonHostState }),
      ).toThrow(/wire contract/i);
    }
  });

  it("parses bounded path-free MP4 status and rejects unsafe counters", () => {
    const recording = {
      sessionId: "session-1",
      state: "recording",
      framesAccepted: 24,
      framesWritten: 23,
      width: 800,
      height: 448,
      errorCode: null,
    };

    expect(parseGenericRecordingView(recording)).toEqual(recording);
    expect(() =>
      parseGenericRecordingView({
        ...recording,
        framesWritten: Number.MAX_SAFE_INTEGER + 1,
      }),
    ).toThrow(/wire contract/i);
    expect(Object.keys(recording)).not.toContain("path");
  });

  it("preserves native MP4 picker cancellation as null", async () => {
    const client = createGenericDeckClient({
      invoke: async <T>() => null as T,
    });

    await expect(client.recordingStart("session-1")).resolves.toBeNull();
  });
});

function host(
  calls: Array<[string, Record<string, unknown>]>,
): GenericDeckHostAdapter {
  return {
    invoke: async <T>(command: string, args: Record<string, unknown> = {}) => {
      calls.push([command, args]);
      if (command.includes("capture")) {
        return {
          sessionId: "session-1",
          captureId: null,
          mode: null,
          state: "idle",
          latentSlots: "0",
          resetEvents: 0,
          cartridgeId: null,
          archiveSha256: null,
          detail: null,
        } as T;
      }
      if (command.includes("recording")) {
        return {
          sessionId: "session-1",
          state: "idle",
          framesAccepted: 0,
          framesWritten: 0,
          width: null,
          height: null,
          errorCode: null,
        } as T;
      }
      return {} as T;
    },
  };
}
