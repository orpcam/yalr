import { describe, expect, it } from "vitest";
import { createMetrics } from "./metrics";

describe("createMetrics", () => {
  it("computes ttft, total and tokens/s from usage", () => {
    const m = createMetrics(1000);
    m.delta(5, 1200);
    m.delta(5, 1700);
    m.delta(5, 2200);
    expect(m.summary({ completion_tokens: 50 })).toEqual({
      ttftMs: 200,
      totalMs: 1200,
      completionTokens: 50,
      tokensPerSec: 50,
      estimated: false,
    });
  });

  it("estimates tokens from chars without usage", () => {
    const m = createMetrics(0);
    m.delta(40, 100);
    m.delta(40, 1100);
    const s = m.summary();
    expect(s.completionTokens).toBe(20);
    expect(s.estimated).toBe(true);
    expect(s.tokensPerSec).toBe(20);
  });

  it("ignores empty deltas", () => {
    const m = createMetrics(0);
    m.delta(0, 50);
    m.delta(4, 300);
    expect(m.summary(undefined, 400).ttftMs).toBe(300);
  });

  it("no tokens/s with a single delta", () => {
    const m = createMetrics(0);
    m.delta(10, 100);
    const s = m.summary({ completion_tokens: 3 });
    expect(s.tokensPerSec).toBeUndefined();
    expect(s.totalMs).toBe(100);
  });

  it("works without any delta (non-stream)", () => {
    const m = createMetrics(0);
    expect(m.summary({ completion_tokens: 7 }, 900)).toEqual({
      ttftMs: undefined,
      totalMs: 900,
      completionTokens: 7,
      tokensPerSec: undefined,
      estimated: false,
    });
    expect(createMetrics(0).summary(undefined, 10).completionTokens).toBeUndefined();
  });
});
