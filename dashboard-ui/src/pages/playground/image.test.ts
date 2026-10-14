import { describe, expect, it } from "vitest";
import { scaledSize } from "./image";

describe("scaledSize", () => {
  it("keeps small images", () => {
    expect(scaledSize(800, 600, 1568)).toEqual({ width: 800, height: 600 });
    expect(scaledSize(1568, 1568, 1568)).toEqual({ width: 1568, height: 1568 });
  });
  it("scales landscape by width", () => {
    expect(scaledSize(4000, 3000, 1568)).toEqual({ width: 1568, height: 1176 });
  });
  it("scales portrait by height", () => {
    expect(scaledSize(3000, 4000, 1568)).toEqual({ width: 1176, height: 1568 });
  });
  it("never goes below 1px", () => {
    expect(scaledSize(10000, 1, 100)).toEqual({ width: 100, height: 1 });
  });
  it("handles degenerate input", () => {
    expect(scaledSize(0, 100, 100)).toEqual({ width: 0, height: 0 });
  });
});
