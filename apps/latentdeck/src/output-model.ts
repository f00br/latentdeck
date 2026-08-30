export interface SpoutStatus {
  sdkBuilt: boolean;
  ready: boolean;
  enabled: boolean;
  published: boolean;
  requestedName: string;
  activeName: string;
  width: number;
  height: number;
  format: "rgba8_unorm";
  submittedFrames: number;
  lastSequence: number | null;
  spoutFrame: number | null;
  lastErrorCode: string | null;
}

export interface SpoutConfigure {
  name: string | null;
  enabled: boolean | null;
}

export interface SpoutControls {
  rename: boolean;
  toggle: boolean;
}

export function spoutControlsFor(
  status: SpoutStatus | null,
  busy: boolean,
): SpoutControls {
  const ready = !busy && status?.sdkBuilt === true && status.ready;
  return { rename: ready, toggle: ready };
}

export function describeSpout(status: SpoutStatus | null): string {
  if (status === null) return "Output inactive";
  if (!status.sdkBuilt) return "SDK not built";
  if (!status.ready) return "SDK unavailable";
  if (status.published) return "Sending";
  if (status.enabled) return "Waiting for frame";
  return "Ready / disabled";
}
