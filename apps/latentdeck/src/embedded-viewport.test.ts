import { describe, expect, it } from "vitest";
import {
  buildEmbeddedViewportBounds,
  embeddedViewportFullyInsideClient,
  nextEmbeddedViewportRevision,
  observeEmbeddedViewportReflow,
  sameEmbeddedViewportGeometry,
} from "./embedded-viewport";

describe("shared embedded deck viewport", () => {
  it("allocates client revisions inside a host-issued epoch", () => {
    const first = nextEmbeddedViewportRevision(0);
    const second = nextEmbeddedViewportRevision(first!);
    expect(first).toBe(1);
    expect(second).toBe(2);
    expect(nextEmbeddedViewportRevision(Number.MAX_SAFE_INTEGER)).toBeNull();
  });

  it("keeps epoch, CSS geometry, and physical scaling explicit", () => {
    expect(
      buildEmbeddedViewportBounds(
        3,
        7,
        { left: 10.25, top: 20.5, width: 400.5, height: 300.25 },
        1.5,
        true,
      ),
    ).toEqual({
      epoch: 3,
      revision: 7,
      xCss: 10.25,
      yCss: 20.5,
      widthCss: 400.5,
      heightCss: 300.25,
      scaleFactor: 1.5,
      visible: true,
    });
  });

  it("coalesces unchanged geometry only inside the same epoch", () => {
    const visible = buildEmbeddedViewportBounds(
      1,
      1,
      { left: 0, top: 0, width: 640, height: 360 },
      1.25,
      true,
    );
    const same = buildEmbeddedViewportBounds(
      1,
      2,
      { left: 0, top: 0, width: 640, height: 360 },
      1.25,
      true,
    );
    const reloaded = buildEmbeddedViewportBounds(
      2,
      1,
      { left: 0, top: 0, width: 640, height: 360 },
      1.25,
      true,
    );
    const hidden = buildEmbeddedViewportBounds(
      1,
      3,
      { left: 0, top: 0, width: 640, height: 360 },
      1.25,
      false,
    );
    expect(visible).not.toBeNull();
    expect(same).not.toBeNull();
    expect(reloaded).not.toBeNull();
    expect(hidden).not.toBeNull();
    expect(sameEmbeddedViewportGeometry(visible, same!)).toBe(true);
    expect(sameEmbeddedViewportGeometry(visible, reloaded!)).toBe(false);
    expect(sameEmbeddedViewportGeometry(visible, hidden!)).toBe(false);
  });

  it("allows two physical pixels of fractional-DPI client-edge noise", () => {
    const fullscreenAt150Percent = {
      left: 0,
      top: 0,
      right: 1707.333_374_023_437_5,
      bottom: 960,
      width: 1707.333_374_023_437_5,
      height: 960,
    };
    expect(
      embeddedViewportFullyInsideClient(fullscreenAt150Percent, 1707, 960, 1.5),
    ).toBe(true);
    expect(
      embeddedViewportFullyInsideClient(
        { ...fullscreenAt150Percent, right: 1709 },
        1707,
        960,
        1.5,
      ),
    ).toBe(false);
    expect(
      embeddedViewportFullyInsideClient(
        { ...fullscreenAt150Percent, left: -0.01 },
        1707,
        960,
        1.5,
      ),
    ).toBe(false);
  });

  it("rejects unsafe measurements and suspends subpixel visibility", () => {
    expect(
      buildEmbeddedViewportBounds(
        1,
        1,
        { left: 0, top: 0, width: Number.NaN, height: 100 },
        1,
        true,
      ),
    ).toBeNull();
    expect(
      buildEmbeddedViewportBounds(
        1,
        2,
        { left: 0, top: 0, width: 0.5, height: 100 },
        1,
        true,
      )?.visible,
    ).toBe(false);
  });

  it("remeasures when a conditional status row moves the anchor without resizing it", async () => {
    const faceplate = document.createElement("section");
    const anchor = document.createElement("div");
    faceplate.append(anchor);
    let scheduled = 0;
    const disconnect = observeEmbeddedViewportReflow(faceplate, () => {
      scheduled += 1;
    });

    const resetMessage = document.createElement("p");
    resetMessage.textContent = "Restarting playback…";
    faceplate.insertBefore(resetMessage, anchor);
    await mutationDelivery();
    expect(scheduled).toBe(1);

    resetMessage.remove();
    await mutationDelivery();
    expect(scheduled).toBe(2);

    disconnect();
    faceplate.insertBefore(resetMessage, anchor);
    await mutationDelivery();
    expect(scheduled).toBe(2);
  });
});

function mutationDelivery(): Promise<void> {
  return new Promise((resolve) => globalThis.setTimeout(resolve, 0));
}
