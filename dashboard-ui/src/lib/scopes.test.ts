import { describe, expect, it } from "vitest";
import type { Model } from "./api";
import { buildModelOptions, orphanValues, providerLabel, toggleValue } from "./scopes";

const model = (model_name: string, provider_name: string): Model => ({
  id: `${provider_name}/${model_name}`,
  model_name,
  upstream_model: model_name,
  input_price_per_million: 1,
  output_price_per_million: 2,
  enabled: true,
  provider_id: provider_name,
  provider_name,
});

describe("buildModelOptions", () => {
  it("deduplicates model names across providers and merges providers", () => {
    const options = buildModelOptions([
      model("gpt-4o", "openai-a"),
      model("gpt-4o", "openai-b"),
      model("gpt-4o", "openai-a"), // Doppelzeile desselben Providers
    ]);
    expect(options).toEqual([
      { name: "gpt-4o", providers: ["openai-a", "openai-b"] },
    ]);
  });

  it("returns one option per model name with its single provider", () => {
    const options = buildModelOptions([
      model("b", "p1"),
      model("a", "p2"),
    ]);
    expect(options).toEqual([
      { name: "a", providers: ["p2"] },
      { name: "b", providers: ["p1"] },
    ]);
  });

  it("sorts options by model name", () => {
    const options = buildModelOptions([
      model("zeta", "p1"),
      model("alpha", "p1"),
      model("mid", "p1"),
    ]);
    expect(options.map((o) => o.name)).toEqual(["alpha", "mid", "zeta"]);
  });

  it("returns an empty list for no models", () => {
    expect(buildModelOptions([])).toEqual([]);
  });
});

describe("toggleValue", () => {
  it("adds a missing value", () => {
    expect(toggleValue(["a"], "b")).toEqual(["a", "b"]);
  });

  it("removes a present value", () => {
    expect(toggleValue(["a", "b", "c"], "b")).toEqual(["a", "c"]);
  });

  it("does not mutate the input list", () => {
    const input = ["a"];
    toggleValue(input, "b");
    expect(input).toEqual(["a"]);
  });

  it("handles an empty list", () => {
    expect(toggleValue([], "x")).toEqual(["x"]);
  });
});

describe("providerLabel", () => {
  it("combines name and kind", () => {
    expect(providerLabel({ name: "openai", kind: "openai" })).toBe("openai (openai)");
  });
});

describe("orphanValues", () => {
  it("returns selected values that are not in the options", () => {
    expect(orphanValues(["a", "gone", "b"], ["a", "b", "c"])).toEqual(["gone"]);
  });

  it("returns an empty list when everything is known", () => {
    expect(orphanValues(["a", "b"], ["a", "b"])).toEqual([]);
  });

  it("keeps order of the selected list", () => {
    expect(orphanValues(["z1", "a1", "z2"], [])).toEqual(["z1", "a1", "z2"]);
  });
});
