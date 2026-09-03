import { flushSync, mount, tick, unmount } from "svelte";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import d2DeckPack from "../../../operators/builtin/d2/package/deck-pack.json";
import d2Faceplate from "../../../operators/builtin/d2/package/faceplate.json";
import d2Operator from "../../../operators/builtin/d2/package/operator.json";
import { createDeckUiDraft, parseDeckUiCatalog } from "./deck-ui-model";
import { genericDeckDraftFromSessionSnapshot } from "./generic-deck-model";
import type {
  GenericCaptureView,
  GenericDeckSessionView,
  GenericDeckSessionsView,
  GenericOutputPin,
  GenericRecordingView,
} from "./generic-deck-client";
import GenericDeckWorkspace from "./GenericDeckWorkspace.svelte";
import {
  EMPTY_LIBRARY_VIEW,
  type CartridgeView,
  type LibraryView,
} from "./library-model";

const { invokeMock } = vi.hoisted(() => ({ invokeMock: vi.fn() }));

vi.mock("@tauri-apps/api/core", () => ({ invoke: invokeMock }));

const SESSION_ID = "session-output-pin";

function model() {
  return parseDeckUiCatalog({
    decks: [
      {
        package: {
          kind: "deck_pack",
          packageId: d2DeckPack.deck_id,
          packageVersion: d2DeckPack.deck_version,
        },
        deck: {
          deckId: d2DeckPack.deck_id,
          deckVersion: d2DeckPack.deck_version,
          displayName: d2DeckPack.display_name,
          summary: d2DeckPack.summary,
          slots: d2DeckPack.signal.slots,
          roles: d2DeckPack.signal.roles.map((role) => ({
            roleId: role.role_id,
            displayName: role.display_name,
          })),
          defaultPermutation: d2DeckPack.signal.default_permutation,
          structuralCarrierRole: d2DeckPack.signal.structural_carrier_role,
          requiredCapabilities: d2DeckPack.signal.required_capabilities,
        },
        operator: {
          operatorId: d2Operator.operator_id,
          controls: d2Operator.controls,
        },
        faceplate: d2Faceplate,
      },
    ],
    issues: [],
  }).decks[0];
}

function session(): GenericDeckSessionView {
  const deck = model();
  const draft = createDeckUiDraft(deck);
  const view: GenericDeckSessionView = {
    sessionId: SESSION_ID,
    workerId: "worker-output-pin",
    deck: {
      packageId: deck.deckId,
      packageVersion: deck.deckVersion,
    },
    codec: {
      packageId: "org.example.codec",
      packageVersion: "2.0.0",
    },
    profileKey: {
      codecFamily: "test",
      profile: "latent",
      profileVersion: "1.0.0",
    },
    device: "cuda",
    deviceOrdinal: 0,
    externalAssets: [],
    sources: [
      { cartridgeId: "source-a", archiveSha256: "a".repeat(64) },
      { cartridgeId: "source-b", archiveSha256: "b".repeat(64) },
    ],
    runtime: {
      status: {
        deck_session_id: SESSION_ID,
        state: "playing",
        deck_revision: 1,
        stream_generation: 1,
        stream_sequence: 1,
        playheads: [
          {
            physical_slot: 1,
            latent_slot: 1,
            loop_enabled: true,
            end_of_stream: false,
          },
          {
            physical_slot: 2,
            latent_slot: 1,
            loop_enabled: true,
            end_of_stream: false,
          },
        ],
        roles: deck.defaultPermutation.map((role, index) => ({
          role,
          physical_slot: index + 1,
        })),
        controls: deck.controls.map((control) => ({
          name: control.controlId,
          value:
            control.valueType === "enum"
              ? { kind: "text" as const, value: control.defaultValue }
              : control.valueType === "boolean"
                ? { kind: "boolean" as const, value: control.defaultValue }
                : control.valueType === "integer"
                  ? { kind: "integer" as const, value: control.defaultValue }
                  : { kind: "number" as const, value: control.defaultValue },
        })),
        source_transport: draft.playing.map((playing, index) => ({
          physical_slot: index + 1,
          playing,
          loop_enabled: draft.loops[index],
        })),
        seed: draft.seed,
        capture_state: "idle",
      },
      outputVisible: true,
      faultCode: null,
    },
    foreground: true,
  };
  genericDeckDraftFromSessionSnapshot(deck, {
    sources: view.sources,
    roles: view.runtime.status.roles,
    controls: view.runtime.status.controls,
    sourceTransport: view.runtime.status.source_transport,
    seed: view.runtime.status.seed,
  });
  return view;
}

function capture(state: GenericCaptureView["state"]): GenericCaptureView {
  const finished = state === "finished";
  return {
    sessionId: SESSION_ID,
    captureId: "capture-1",
    mode: "live_capture",
    state,
    latentSlots: finished ? "12" : "4",
    resetEvents: 0,
    cartridgeId: finished ? "captured-cartridge" : null,
    archiveSha256: finished ? "c".repeat(64) : null,
    detail: null,
  };
}

function recording(state: GenericRecordingView["state"]): GenericRecordingView {
  return {
    sessionId: SESSION_ID,
    state,
    framesAccepted: state === "finished" ? 12 : 4,
    framesWritten: state === "finished" ? 12 : 4,
    width: 448,
    height: 800,
    errorCode: null,
  };
}

function idleCapture(): GenericCaptureView {
  return {
    sessionId: SESSION_ID,
    captureId: null,
    mode: null,
    state: "idle",
    latentSlots: "0",
    resetEvents: 0,
    cartridgeId: null,
    archiveSha256: null,
    detail: null,
  };
}

function idleRecording(): GenericRecordingView {
  return {
    sessionId: SESSION_ID,
    state: "idle",
    framesAccepted: 0,
    framesWritten: 0,
    width: null,
    height: null,
    errorCode: null,
  };
}

function sourceCartridge(marker: string, cartridgeId: string): CartridgeView {
  return {
    archiveSha256: marker.repeat(64),
    cartridgeId,
    codecFamily: "test",
    codecProfile: "latent",
    codecProfileVersion: "1.0.0",
    timingContract: "test_24fps",
    timingContractVersion: "1.0.0",
    frameRateNumerator: 24,
    frameRateDenominator: 1,
    decodedWidth: 64,
    decodedHeight: 64,
    decodedFrameCount: 24,
    durationNumerator: 1,
    durationDenominator: 1,
    signalGeometry: {
      codec_family: "test",
      profile: "latent",
      profile_version: "1.0.0",
      runtime_dtype: "F16",
      batch: 1,
      latent_channels: 24,
      latent_slots: 24,
      latent_height: 30,
      latent_width: 45,
      decoded_frame_count: 24,
      decoded_height: 64,
      decoded_width: 64,
      timing_contract: "test_24fps",
      timing_contract_version: "1.0.0",
      frame_rate: { numerator: 24, denominator: 1 },
    },
    signalPresentation: {
      orientation: "square",
      aspect_ratio: { width: 1, height: 1 },
      decoded_width: 64,
      decoded_height: 64,
    },
    signalWorkload: {
      latent_sites_per_slot: null,
      latent_values_per_slot: null,
      latent_values_per_clip: null,
      decoded_pixels_per_frame: null,
    },
    favorite: false,
    tags: [],
    availability: "present",
    paths: [
      {
        path: `${cartridgeId}.lc`,
        fileName: `${cartridgeId}.lc`,
        state: "present",
        warningCode: null,
      },
    ],
  };
}

function sourceLibrary(): LibraryView {
  const cartridges = [
    sourceCartridge("a", "source-a"),
    sourceCartridge("b", "source-b"),
    sourceCartridge("c", "captured-cartridge"),
  ];
  return {
    ...EMPTY_LIBRARY_VIEW,
    cartridges,
    recent: cartridges,
    totalIndexed: cartridges.length,
    activeMemberCount: cartridges.length,
  };
}

function clickButton(target: HTMLElement, label: string): void {
  const button = Array.from(target.querySelectorAll("button")).find(
    (candidate) => candidate.textContent?.trim() === label,
  );
  if (!(button instanceof HTMLButtonElement) || button.disabled) {
    const buttons = Array.from(target.querySelectorAll("button")).map(
      (candidate) =>
        `${candidate.textContent?.trim() ?? ""}:${candidate.disabled ? "disabled" : "enabled"}`,
    );
    throw new Error(
      `Missing enabled button: ${label}; sessionClass=${target.querySelector(".session-list article")?.className}; buttons=${buttons.join("|")}; text=${target.textContent}`,
    );
  }
  button.click();
  flushSync();
}

function findButton(target: HTMLElement, label: string): HTMLButtonElement {
  const button = Array.from(target.querySelectorAll("button")).find(
    (candidate) => candidate.textContent?.trim() === label,
  );
  if (!(button instanceof HTMLButtonElement)) {
    throw new Error(`Missing button: ${label}`);
  }
  return button;
}

function deferred<T>(): {
  promise: Promise<T>;
  resolve(value: T): void;
  reject(error: unknown): void;
} {
  let resolve!: (value: T) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<T>((accept, deny) => {
    resolve = accept;
    reject = deny;
  });
  return { promise, resolve, reject };
}

async function settleUi(): Promise<void> {
  for (let index = 0; index < 8; index += 1) {
    await Promise.resolve();
    await tick();
    flushSync();
  }
}

function installHost(state: {
  outputPin: GenericOutputPin | null;
  capture: GenericCaptureView;
  recording: GenericRecordingView;
  controlsSet?: (
    args: Record<string, unknown>,
  ) => Promise<GenericDeckSessionView["runtime"]>;
  replaceSources?: (
    args: Record<string, unknown>,
  ) => Promise<GenericDeckSessionView>;
}) {
  const runtimeSession = session();
  const snapshot = (): GenericDeckSessionsView => ({
    sessions: [runtimeSession],
    foregroundOutput: { sessionId: SESSION_ID, generation: 1 },
    outputPin: state.outputPin,
    recentFaults: [],
  });
  invokeMock.mockImplementation(
    (command: string, args: Record<string, unknown> = {}) => {
      switch (command) {
        case "extensions_snapshot":
          return Promise.resolve({ packages: [], matrix: [] });
        case "deck_generic_sessions_get":
          return Promise.resolve(snapshot());
        case "deck_generic_status_get":
          return Promise.resolve(runtimeSession);
        case "deck_generic_viewport_session_begin":
          return Promise.resolve({ epoch: 1 });
        case "deck_generic_viewport_set_bounds":
          return Promise.resolve();
        case "deck_generic_capture_status_get":
          return Promise.resolve(state.capture);
        case "deck_generic_recording_status_get":
          return Promise.resolve(state.recording);
        case "deck_generic_spout_status_get":
          return Promise.resolve(null);
        case "deck_generic_fullscreen_status_get":
          return Promise.resolve(false);
        case "deck_generic_controls_set":
          return (
            state.controlsSet?.(args) ?? Promise.resolve(runtimeSession.runtime)
          );
        case "deck_generic_runtime_options":
          return Promise.resolve({
            deck: runtimeSession.deck,
            codec: runtimeSession.codec,
            reason: "compatible",
            profiles: [runtimeSession.profileKey],
            device: runtimeSession.device,
            slots: 2,
            externalAssets: [],
            sources: sourceLibrary().cartridges.map((source) => ({
              cartridgeId: source.cartridgeId,
              archiveSha256: source.archiveSha256,
              reason: "compatible",
            })),
          });
        case "deck_generic_sources_replace":
          return (
            state.replaceSources?.(args) ?? Promise.resolve(runtimeSession)
          );
        case "deck_generic_capture_start":
          state.capture = capture("capturing");
          state.outputPin = {
            session_id: SESSION_ID,
            lease_generation: 1,
            pin_generation: 1,
            kind: "capture",
          };
          return Promise.resolve(state.capture);
        case "deck_generic_capture_stop":
          state.capture = capture("finished");
          return Promise.resolve(state.capture);
        case "deck_generic_recording_start":
          state.recording = recording("recording");
          state.outputPin = {
            session_id: SESSION_ID,
            lease_generation: 1,
            pin_generation: 2,
            kind: "mp4",
          };
          return Promise.resolve(state.recording);
        case "deck_generic_recording_stop":
          state.recording = recording("finished");
          return Promise.resolve(state.recording);
        case "library_resolve_preset_sources":
          return Promise.resolve([sourceLibrary().cartridges[2]]);
        case "library_snapshot":
          return Promise.resolve(sourceLibrary());
        default:
          return Promise.reject(new Error(`unexpected command: ${command}`));
      }
    },
  );
}

async function mountWorkspace(target: HTMLElement) {
  const deck = model();
  const component = mount(GenericDeckWorkspace, {
    target,
    props: {
      model: deck,
      models: [deck],
      library: EMPTY_LIBRARY_VIEW,
      active: true,
      registerLeave: () => undefined,
    },
  });
  await settleUi();
  await vi.advanceTimersByTimeAsync(500);
  await settleUi();
  return component;
}

function sessionsGetCount(): number {
  return invokeMock.mock.calls.filter(
    ([command]) => command === "deck_generic_sessions_get",
  ).length;
}

describe("generic Deck foreground output pin refresh", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    invokeMock.mockReset();
    vi.stubGlobal(
      "ResizeObserver",
      class {
        observe(): void {}
        disconnect(): void {}
      },
    );
    vi.stubGlobal("requestAnimationFrame", () => 1);
    vi.stubGlobal("cancelAnimationFrame", () => undefined);
  });

  afterEach(() => {
    vi.useRealTimers();
    vi.unstubAllGlobals();
  });

  it("dispatches realtime controls latest-only without blocking live-capture stop", async () => {
    const first = deferred<GenericDeckSessionView["runtime"]>();
    const third = deferred<GenericDeckSessionView["runtime"]>();
    const controlRequests: Record<string, unknown>[] = [];
    const state = {
      outputPin: null as GenericOutputPin | null,
      capture: idleCapture(),
      recording: idleRecording(),
      controlsSet: async (args: Record<string, unknown>) => {
        controlRequests.push(args);
        if (controlRequests.length === 1) return first.promise;
        if (controlRequests.length === 3) return third.promise;
        return session().runtime;
      },
    };
    installHost(state);
    const target = document.createElement("div");
    document.body.append(target);
    const component = await mountWorkspace(target);
    const mix = target.querySelector<HTMLInputElement>(
      'input[data-control-id="mix"]',
    );
    expect(mix).toBeInstanceOf(HTMLInputElement);

    mix!.value = "0.2";
    mix!.dispatchEvent(new Event("input", { bubbles: true }));
    flushSync();
    expect(findButton(target, "Start Live Capture").disabled).toBe(true);
    await vi.advanceTimersByTimeAsync(0);
    await settleUi();
    expect(controlRequests).toHaveLength(1);
    expect(mix!.disabled).toBe(false);

    for (const value of ["0.4", "0.7"]) {
      mix!.value = value;
      mix!.dispatchEvent(new Event("input", { bubbles: true }));
      flushSync();
    }
    expect(controlRequests).toHaveLength(1);
    first.resolve(session().runtime);
    await settleUi();
    await vi.advanceTimersByTimeAsync(75);
    await settleUi();

    expect(controlRequests).toHaveLength(2);
    const latestBindings = controlRequests[1].controls as Array<{
      name: string;
      value: { value: unknown };
    }>;
    expect(
      latestBindings.find((binding) => binding.name === "mix")?.value,
    ).toEqual({ kind: "number", value: 0.7 });
    expect(findButton(target, "Start Live Capture").disabled).toBe(false);

    clickButton(target, "Start Live Capture");
    await settleUi();
    mix!.value = "0.9";
    mix!.dispatchEvent(new Event("input", { bubbles: true }));
    flushSync();
    await vi.advanceTimersByTimeAsync(75);
    await settleUi();
    expect(controlRequests).toHaveLength(3);
    expect(mix!.disabled).toBe(false);
    expect(findButton(target, "Stop Live Capture").disabled).toBe(false);
    clickButton(target, "Stop Live Capture");
    await settleUi();

    third.resolve(session().runtime);
    await settleUi();
    await unmount(component);
    target.remove();
  });

  it("shows capture pin/unpin after actions and refreshes a terminal transition once", async () => {
    const state = {
      outputPin: null as GenericOutputPin | null,
      capture: idleCapture(),
      recording: idleRecording(),
    };
    installHost(state);
    const target = document.createElement("div");
    document.body.append(target);
    const component = await mountWorkspace(target);

    expect(target.textContent).toContain("Output lease unpinned");
    expect(sessionsGetCount()).toBe(1);

    clickButton(target, "Start Live Capture");
    await settleUi();
    expect(target.textContent).toContain("Pinned by capture");
    expect(findButton(target, "Record MP4…").disabled).toBe(true);
    expect(target.textContent).toContain(
      "Latent capture pins the foreground output lease.",
    );
    expect(sessionsGetCount()).toBe(2);

    await vi.advanceTimersByTimeAsync(500);
    await settleUi();
    expect(sessionsGetCount()).toBe(2);

    clickButton(target, "Stop Live Capture");
    await settleUi();
    expect(target.textContent).toContain("Pinned by capture");
    expect(sessionsGetCount()).toBe(3);

    state.outputPin = null;
    await vi.advanceTimersByTimeAsync(500);
    await settleUi();
    expect(target.textContent).toContain("Output lease unpinned");
    expect(sessionsGetCount()).toBe(4);

    clickButton(target, "Start Live Capture");
    await settleUi();
    expect(target.textContent).toContain("Pinned by capture");
    expect(sessionsGetCount()).toBe(5);

    state.capture = capture("finished");
    state.outputPin = null;
    await vi.advanceTimersByTimeAsync(500);
    await settleUi();
    expect(target.textContent).toContain("Output lease unpinned");
    expect(findButton(target, "Record MP4…").disabled).toBe(false);
    expect(sessionsGetCount()).toBe(6);

    await vi.advanceTimersByTimeAsync(500);
    await settleUi();
    expect(sessionsGetCount()).toBe(6);

    await unmount(component);
    target.remove();
  });

  it("makes a finished capture immediately usable in either D2 slot without opening a fifth session", async () => {
    const replacements: Record<string, unknown>[] = [];
    const state = {
      outputPin: null as GenericOutputPin | null,
      capture: capture("finished"),
      recording: idleRecording(),
      replaceSources: async (args: Record<string, unknown>) => {
        replacements.push(args);
        const request = args.request as {
          sources: GenericDeckSessionView["sources"];
        };
        return { ...session(), sources: request.sources };
      },
    };
    installHost(state);
    const target = document.createElement("div");
    document.body.append(target);
    const component = await mountWorkspace(target);

    expect(findButton(target, "Use capture in A").disabled).toBe(false);
    expect(findButton(target, "Use capture in B").disabled).toBe(false);

    clickButton(target, "Start Live Capture");
    await settleUi();
    expect(findButton(target, "Use capture in A").disabled).toBe(true);
    clickButton(target, "Stop Live Capture");
    await settleUi();
    expect(findButton(target, "Use capture in A").disabled).toBe(true);
    state.outputPin = null;
    await vi.advanceTimersByTimeAsync(1_000);
    await settleUi();
    expect(target.textContent).toContain("Output lease unpinned");
    expect(findButton(target, "Snapshot").disabled).toBe(false);
    expect(findButton(target, "Use capture in A").disabled).toBe(false);

    const sourceSelects = target.querySelectorAll<HTMLSelectElement>(
      '[data-widget-kind="source_picker"] select',
    );
    expect(sourceSelects).toHaveLength(2);
    sourceSelects[1].value = "a".repeat(64);
    sourceSelects[1].dispatchEvent(new Event("change", { bubbles: true }));
    await settleUi();

    clickButton(target, "Use capture in A");
    await settleUi();

    expect(replacements).toHaveLength(1);
    expect(replacements[0].sessionId).toBe(SESSION_ID);
    expect(
      (
        replacements[0].request as {
          sources: GenericDeckSessionView["sources"];
        }
      ).sources,
    ).toEqual([
      {
        cartridgeId: "captured-cartridge",
        archiveSha256: "c".repeat(64),
      },
      { cartridgeId: "source-b", archiveSha256: "b".repeat(64) },
    ]);
    expect(
      invokeMock.mock.calls.filter(
        ([command]) => command === "deck_generic_open",
      ),
    ).toHaveLength(0);

    await unmount(component);
    target.remove();
  });

  it("shows MP4 pin/unpin after actions and refreshes a terminal transition once", async () => {
    const state = {
      outputPin: null as GenericOutputPin | null,
      capture: idleCapture(),
      recording: idleRecording(),
    };
    installHost(state);
    const target = document.createElement("div");
    document.body.append(target);
    const component = await mountWorkspace(target);

    clickButton(target, "Record MP4…");
    await settleUi();
    expect(target.textContent).toContain("Pinned by mp4");
    expect(findButton(target, "Snapshot").disabled).toBe(true);
    expect(target.textContent).toContain(
      "MP4 recording pins the foreground output lease.",
    );
    expect(sessionsGetCount()).toBe(2);

    await vi.advanceTimersByTimeAsync(500);
    await settleUi();
    expect(sessionsGetCount()).toBe(2);

    clickButton(target, "Stop MP4");
    await settleUi();
    expect(target.textContent).toContain("Pinned by mp4");
    expect(sessionsGetCount()).toBe(3);

    state.outputPin = null;
    await vi.advanceTimersByTimeAsync(500);
    await settleUi();
    expect(target.textContent).toContain("Output lease unpinned");
    expect(sessionsGetCount()).toBe(4);

    clickButton(target, "Record MP4…");
    await settleUi();
    expect(target.textContent).toContain("Pinned by mp4");
    expect(sessionsGetCount()).toBe(5);

    state.recording = recording("finished");
    state.outputPin = null;
    await vi.advanceTimersByTimeAsync(500);
    await settleUi();
    expect(target.textContent).toContain("Output lease unpinned");
    expect(findButton(target, "Snapshot").disabled).toBe(false);
    expect(sessionsGetCount()).toBe(6);

    await vi.advanceTimersByTimeAsync(500);
    await settleUi();
    expect(sessionsGetCount()).toBe(6);

    await unmount(component);
    target.remove();
  });

  it("reconciles Close immediately and ignores a late stale session.not_found poll", async () => {
    const state = {
      outputPin: null as GenericOutputPin | null,
      capture: idleCapture(),
      recording: idleRecording(),
    };
    installHost(state);
    const target = document.createElement("div");
    document.body.append(target);
    const component = await mountWorkspace(target);
    const normalHost = invokeMock.getMockImplementation();
    const lateStatus = deferred<GenericDeckSessionView>();
    let closed = false;
    invokeMock.mockImplementation(
      (command: string, args: Record<string, unknown> = {}) => {
        if (command === "deck_generic_status_get" && !closed) {
          return lateStatus.promise;
        }
        if (command === "deck_generic_close") {
          closed = true;
          return Promise.resolve();
        }
        if (command === "deck_generic_sessions_get" && closed) {
          return Promise.resolve({
            sessions: [],
            foregroundOutput: null,
            outputPin: null,
            recentFaults: [],
          });
        }
        return normalHost?.(command, args);
      },
    );

    await vi.advanceTimersByTimeAsync(500);
    await settleUi();
    const statusCallsWhilePending = invokeMock.mock.calls.filter(
      ([command]) => command === "deck_generic_status_get",
    ).length;
    await vi.advanceTimersByTimeAsync(500);
    await settleUi();
    expect(
      invokeMock.mock.calls.filter(
        ([command]) => command === "deck_generic_status_get",
      ),
    ).toHaveLength(statusCallsWhilePending);
    clickButton(target, "Close");
    await settleUi();
    expect(target.textContent).toContain("No warm Protocol 2 sessions.");

    lateStatus.reject({
      code: "session.not_found",
      message: "The exact generic Deck session is not active.",
    });
    await settleUi();
    expect(target.querySelector('[role="alert"]')).toBeNull();
    expect(target.textContent).not.toContain("session.not_found");
    expect(target.textContent).toContain("No warm Protocol 2 sessions.");

    await unmount(component);
    target.remove();
  });
});
