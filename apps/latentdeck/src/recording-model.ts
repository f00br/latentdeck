export type DecodedRecordingState =
  | "idle"
  | "armed"
  | "recording"
  | "finalizing"
  | "finished"
  | "cancelled"
  | "failed";

export interface DecodedRecordingStatus {
  state: DecodedRecordingState;
  framesAccepted: number;
  framesWritten: number;
  width: number | null;
  height: number | null;
  errorCode: string | null;
}

export interface DecodedRecordingControls {
  start: boolean;
  stop: boolean;
}

export const IDLE_DECODED_RECORDING: DecodedRecordingStatus = Object.freeze({
  state: "idle",
  framesAccepted: 0,
  framesWritten: 0,
  width: null,
  height: null,
  errorCode: null,
});

export function decodedRecordingControls(
  status: DecodedRecordingStatus,
  deckLoaded: boolean,
  busy: boolean,
): DecodedRecordingControls {
  if (busy) return { start: false, stop: false };
  if (status.state === "armed" || status.state === "recording") {
    return { start: false, stop: true };
  }
  if (status.state === "finalizing") {
    return { start: false, stop: false };
  }
  return { start: deckLoaded, stop: false };
}

export function describeDecodedRecording(
  status: DecodedRecordingStatus,
): string {
  switch (status.state) {
    case "idle":
      return "MP4 recorder ready";
    case "armed":
      return "Waiting for decoded frame · video-only H.264 MP4";
    case "recording": {
      const geometry =
        status.width === null || status.height === null
          ? "intrinsic"
          : `${status.width}×${status.height}`;
      return `Recording MP4 · ${status.framesWritten} frames · ${geometry}`;
    }
    case "finalizing":
      return `Finalizing MP4 · ${status.framesWritten} frames`;
    case "finished":
      return `MP4 saved · ${status.framesWritten} frames`;
    case "cancelled":
      return "MP4 recording cancelled before the first frame";
    case "failed":
      return status.errorCode ?? "recording.failed";
  }
}
