import { describe, expect, it } from "vitest";
import { product } from "./product";

describe("LatentDeck workspace identity", () => {
  it("uses the package version injected by the build", () => {
    expect(product).toEqual({ name: "LatentDeck", version: "0.1.0" });
  });
});
