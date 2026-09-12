import { describe, expect, it } from "vitest";
import { formatCost, formatPercent, formatTps, tps } from "./format";

describe("formatCost", () => {
  it("formats USD with $ prefix (en-US, default locale)", () => {
    expect(formatCost(1.5, "USD")).toBe("$1.50");
    expect(formatCost(0.45, "USD")).toBe("$0.4500");
  });

  it("formats EUR with € prefix", () => {
    expect(formatCost(1.5, "EUR")).toBe("€1.50");
    expect(formatCost(0.45, "EUR")).toBe("€0.4500");
  });

  it("uses 4 decimals for |v| < 1", () => {
    expect(formatCost(0, "USD")).toBe("$0.0000");
    expect(formatCost(0.99999, "USD")).toBe("$1.0000");
    expect(formatCost(0.000042, "USD")).toBe("$0.0000");
  });

  it("uses 2 decimals for |v| >= 1", () => {
    expect(formatCost(1, "USD")).toBe("$1.00");
    expect(formatCost(123.456, "USD")).toBe("$123.46");
  });

  it("handles negative values", () => {
    expect(formatCost(-0.45, "EUR")).toBe("€-0.4500");
    expect(formatCost(-2, "USD")).toBe("$-2.00");
  });

  it("formats with comma decimal separator for de-DE", () => {
    expect(formatCost(0.45, "EUR", "de-DE")).toBe("€0,4500");
    expect(formatCost(1.5, "EUR", "de-DE")).toBe("€1,50");
    expect(formatCost(1.5, "USD", "de-DE")).toBe("$1,50");
  });

  it("does not add grouping separators", () => {
    expect(formatCost(1234.5, "USD")).toBe("$1234.50");
    expect(formatCost(1e6, "USD")).toBe("$1000000.00");
    expect(formatCost(1234.5, "EUR", "de-DE")).toBe("€1234,50");
  });

  it("normalizes -0 to 0", () => {
    expect(formatCost(-0, "USD")).toBe("$0.0000");
    expect(formatCost(-0, "EUR", "de-DE")).toBe("€0,0000");
  });

  it("returns dash for non-finite values", () => {
    expect(formatCost(NaN, "USD")).toBe("–");
    expect(formatCost(Infinity, "EUR")).toBe("–");
  });

  it("formats negative values in de-DE", () => {
    expect(formatCost(-1.5, "EUR", "de-DE")).toBe("€-1,50");
  });
});

describe("formatCost (USD)", () => {
  it("formats USD values", () => {
    expect(formatCost(1.5, "USD")).toBe("$1.50");
    expect(formatCost(0.45, "USD")).toBe("$0.4500");
  });
});

describe("formatTps", () => {
  it("returns dash for null/NaN", () => {
    expect(formatTps(null)).toBe("–");
    expect(formatTps(NaN)).toBe("–");
    expect(formatTps(null, "de-DE")).toBe("–");
  });

  it("rounds large values without decimals", () => {
    expect(formatTps(123.4)).toBe("123 t/s");
    expect(formatTps(123.4, "de-DE")).toBe("123 t/s");
  });

  it("keeps one decimal below 100", () => {
    expect(formatTps(42.24)).toBe("42.2 t/s");
    expect(formatTps(42.24, "de-DE")).toBe("42,2 t/s");
  });

  it("uses 0 decimals at exactly 100", () => {
    expect(formatTps(100)).toBe("100 t/s");
  });

  it("rounds 99.96 to 100.0 (1 decimal below 100)", () => {
    expect(formatTps(99.96)).toBe("100.0 t/s");
  });

  it("normalizes -0 to 0", () => {
    expect(formatTps(-0)).toBe("0.0 t/s");
  });

  it("returns dash for Infinity", () => {
    expect(formatTps(Infinity)).toBe("–");
  });
});

describe("tps", () => {
  it("computes tokens per second", () => {
    expect(tps(100, 1000)).toBe("100 t/s");
    expect(tps(150, 1000)).toBe("150 t/s");
  });

  it("returns zero for zero tokens", () => {
    expect(tps(0, 1000)).toBe("0.0 t/s");
  });

  it("returns infinity symbol for zero milliseconds", () => {
    expect(tps(100, 0)).toBe("∞");
  });

  it("localizes decimal separator", () => {
    expect(tps(42, 1000, "de-DE")).toBe("42,0 t/s");
    expect(tps(105, 1000, "de-DE")).toBe("105 t/s");
  });
});

describe("formatPercent", () => {
  it("formats with 1 decimal and % suffix for en-US", () => {
    expect(formatPercent(2.3)).toBe("2.3%");
    expect(formatPercent(0)).toBe("0.0%");
    expect(formatPercent(100)).toBe("100.0%");
  });

  it("formats with comma decimal and space before % for de-DE", () => {
    expect(formatPercent(2.3, "de-DE")).toBe("2,3 %");
    expect(formatPercent(0, "de-DE")).toBe("0,0 %");
    expect(formatPercent(100, "de-DE")).toBe("100,0 %");
  });

  it("normalizes -0 to 0", () => {
    expect(formatPercent(-0)).toBe("0.0%");
    expect(formatPercent(-0, "de-DE")).toBe("0,0 %");
  });

  it("returns dash for non-finite values", () => {
    expect(formatPercent(NaN)).toBe("–");
    expect(formatPercent(Infinity)).toBe("–");
  });
});
