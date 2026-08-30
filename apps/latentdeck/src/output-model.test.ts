import { describe, expect, it } from "vitest";
import {
  describeSpout,
  spoutControlsFor,
  type SpoutStatus,
} from "./output-model";

const READY: SpoutStatus = {
  sdkBuilt: true,
  ready: true,
  enabled: false,
  published: false,
  requestedName: "LatentDeck LD-D2 Output",
  activeName: "LatentDeck LD-D2 Output",
  width: 800,
  height: 448,
  format: "rgba8_unorm",
  submittedFrames: 0,
  lastSequence: null,
  spoutFrame: null,
  lastErrorCode: null,
};

describe("native Spout UI contract", () => {
  it("gates controls on an opened real SDK surface", () => {
    expect(spoutControlsFor(null, false)).toEqual({
      rename: false,
      toggle: false,
    });
    expect(spoutControlsFor({ ...READY, ready: false }, false)).toEqual({
      rename: false,
      toggle: false,
    });
    expect(spoutControlsFor(READY, false)).toEqual({
      rename: true,
      toggle: true,
    });
    expect(spoutControlsFor(READY, true)).toEqual({
      rename: false,
      toggle: false,
    });
  });

  it("distinguishes ready, waiting, and published sender states", () => {
    expect(describeSpout(null)).toBe("Output inactive");
    expect(describeSpout(READY)).toBe("Ready / disabled");
    expect(describeSpout({ ...READY, enabled: true })).toBe(
      "Waiting for frame",
    );
    expect(describeSpout({ ...READY, enabled: true, published: true })).toBe(
      "Sending",
    );
  });
});
