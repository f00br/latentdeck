import { describe, expect, it } from "vitest";
import faceplateRenderer from "./DeckFaceplateRenderer.svelte?raw";
import genericWorkspace from "./GenericDeckWorkspace.svelte?raw";
import recordingModel from "./recording-model.ts?raw";
import {
  decodedRecordingControls,
  describeDecodedRecording,
  type DecodedRecordingStatus,
} from "./recording-model";

const status = (
  state: DecodedRecordingStatus["state"],
): DecodedRecordingStatus => ({
  state,
  framesAccepted: state === "idle" ? 0 : 12,
  framesWritten: state === "finished" ? 12 : 10,
  width: state === "idle" ? null : 448,
  height: state === "idle" ? null : 800,
  errorCode: state === "failed" ? "recording.encode_failed" : null,
});

describe("decoded MP4 recording policy", () => {
  it("starts only from a loaded idle/terminal Deck and stops only active recording", () => {
    expect(decodedRecordingControls(status("idle"), true, false)).toEqual({
      start: true,
      stop: false,
    });
    expect(decodedRecordingControls(status("idle"), false, false).start).toBe(
      false,
    );
    expect(decodedRecordingControls(status("recording"), true, false)).toEqual({
      start: false,
      stop: true,
    });
    expect(decodedRecordingControls(status("finalizing"), true, false)).toEqual(
      { start: false, stop: false },
    );
    expect(
      decodedRecordingControls(status("finished"), true, false).start,
    ).toBe(true);
    expect(decodedRecordingControls(status("failed"), true, true)).toEqual({
      start: false,
      stop: false,
    });
  });

  it("describes a video-only intrinsic recording without calling it a cartridge", () => {
    expect(describeDecodedRecording(status("armed"))).toContain(
      "Waiting for decoded frame",
    );
    expect(describeDecodedRecording(status("recording"))).toBe(
      "Recording MP4 · 10 frames · 448×800",
    );
    expect(describeDecodedRecording(status("failed"))).toBe(
      "recording.encode_failed",
    );
  });

  it("wires one session-scoped recorder into the generic Deck workspace", () => {
    expect(genericWorkspace).toContain("recordingStatusGet(sessionId)");
    expect(genericWorkspace).toContain("recordingAction()");
    expect(faceplateRenderer).toContain("Record MP4");
    expect(genericWorkspace).toContain("onMp4Toggle={recordingAction}");
    expect(recordingModel).toContain("video-only H.264");
    expect(genericWorkspace).toContain(
      "$: recordingIsActive = recordingActive(recording)",
    );
    expect(genericWorkspace).toContain(
      "$: captureIsActive = captureActive(capture)",
    );
  });
});
