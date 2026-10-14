import { describe, expect, it } from "vitest";
import type { Model, Redirect } from "@/lib/api";
import {
  DEFAULT_PARAMS,
  applyParamsPatch,
  buildModelOptions,
  formatBytes,
  imageRejection,
  parseOptionalNumber,
  prettyRaw,
  providersForModel,
  providersForSelection,
  routedModelName,
  routingHeaders,
  toChatParams,
  toHistory,
  type UiMessage,
} from "./helpers";

const model = (model_name: string, provider_name: string, enabled = true): Model => ({
  id: `${provider_name}/${model_name}`,
  model_name,
  upstream_model: model_name,
  input_price_per_million: 0,
  output_price_per_million: 0,
  enabled,
  provider_id: provider_name,
  provider_name,
});
const redirect = (model_name: string, redirect_model_name: string): Redirect => ({
  id: model_name,
  model_name,
  redirect_model_name,
  note: "",
  created_at: "",
  target_has_route: true,
});

describe("buildModelOptions", () => {
  it("dedupes, drops disabled, adds redirect sources, sorts", () => {
    const opts = buildModelOptions(
      [model("b", "p1"), model("a", "p1"), model("b", "p2"), model("off", "p1", false)],
      [redirect("c-old", "b"), redirect("a", "b")]
    );
    // "a" hat eigene modelle, wird aber trotzdem umgeleitet
    expect(opts).toEqual([
      { name: "a", redirectTo: "b" },
      { name: "b" },
      { name: "c-old", redirectTo: "b" },
    ]);
  });
});

describe("redirect-aware provider options", () => {
  const ms = [model("src", "own"), model("tgt", "t1"), model("tgt", "t2"), model("tgt", "t3", false)];
  const rs = [redirect("src", "tgt"), redirect("ghost", "tgt")];
  it("resolves the routed model", () => {
    expect(routedModelName(rs, "src")).toBe("tgt");
    expect(routedModelName(rs, "tgt")).toBe("tgt");
  });
  it("lists the redirect target's enabled providers, even if the source has its own", () => {
    expect(providersForSelection(ms, rs, "src")).toEqual(["t1", "t2"]);
    expect(providersForSelection(ms, rs, "ghost")).toEqual(["t1", "t2"]);
    expect(providersForSelection(ms, rs, "tgt")).toEqual(["t1", "t2"]);
  });
});

describe("applyParamsPatch", () => {
  const ms = [model("a", "p1"), model("a", "p2"), model("b", "p2"), model("c", "p1", false), model("t", "p3")];
  const rs = [redirect("r", "t")];
  const base = { ...DEFAULT_PARAMS, model: "a", provider: "p1" };
  it("clears the provider when the new model does not offer it", () => {
    expect(applyParamsPatch(base, { model: "b" }, ms, rs)).toMatchObject({ model: "b", provider: "" });
    expect(applyParamsPatch(base, { model: "c" }, ms, rs).provider).toBe("");
    expect(applyParamsPatch(base, { model: "r" }, ms, rs).provider).toBe("");
  });
  it("keeps the provider when offered (incl. via redirect target)", () => {
    expect(applyParamsPatch({ ...base, provider: "p2" }, { model: "b" }, ms, rs).provider).toBe("p2");
    expect(applyParamsPatch({ ...base, provider: "p3" }, { model: "r" }, ms, rs).provider).toBe("p3");
  });
  it("leaves the provider alone for non-model patches", () => {
    expect(applyParamsPatch({ ...base, provider: "zzz" }, { temperature: "1" }, ms, rs)).toMatchObject({
      provider: "zzz",
      temperature: "1",
    });
    expect(applyParamsPatch(base, { model: "b", provider: "p2" }, ms, rs).provider).toBe("p2");
  });
});

describe("providersForModel", () => {
  it("lists distinct enabled providers", () => {
    const ms = [model("m", "z"), model("m", "a"), model("m", "a"), model("m", "x", false), model("n", "q")];
    expect(providersForModel(ms, "m")).toEqual(["a", "z"]);
    expect(providersForModel(ms, "none")).toEqual([]);
  });
});

describe("parseOptionalNumber", () => {
  it("handles empty, valid, invalid", () => {
    expect(parseOptionalNumber("  ")).toBeUndefined();
    expect(parseOptionalNumber("0")).toBe(0);
    expect(parseOptionalNumber("0,7")).toBe(0.7);
    expect(parseOptionalNumber("abc")).toBeNull();
    expect(parseOptionalNumber("1.5", true)).toBeNull();
    expect(parseOptionalNumber("256", true)).toBe(256);
  });
});

describe("toChatParams", () => {
  it("maps form state", () => {
    const { params, invalid } = toChatParams({
      ...DEFAULT_PARAMS,
      model: "m",
      provider: "p",
      temperature: "0.2",
      maxTokens: "x",
      reasoning: "off",
      effort: "high",
    });
    expect(invalid).toEqual(["maxTokens"]);
    expect(params).toMatchObject({ model: "m", provider: "p", temperature: 0.2, stream: true });
    expect(params.maxTokens).toBeUndefined();
    expect(params.topP).toBeUndefined();
    expect(params.effort).toBeUndefined();
  });
  it("keeps effort only when reasoning on; empty provider undefined", () => {
    const { params } = toChatParams({ ...DEFAULT_PARAMS, reasoning: "on", effort: "low" });
    expect(params.effort).toBe("low");
    expect(params.provider).toBeUndefined();
  });
});

describe("toHistory", () => {
  it("drops failed turns together with their user message; keeps stopped replies with text", () => {
    const msgs: UiMessage[] = [
      { id: 1, role: "user", text: "q1", images: ["data:x"] },
      { id: 2, role: "assistant", text: "", status: "error", error: { message: "x" } },
      { id: 3, role: "user", text: "q2" },
      { id: 4, role: "assistant", text: "partial", reasoning: "r", status: "stopped" },
      { id: 5, role: "user", text: "q3" },
      { id: 6, role: "assistant", text: "", status: "stopped" },
      { id: 7, role: "user", text: "q4" },
    ];
    expect(toHistory(msgs)).toEqual([
      { role: "user", text: "q2", images: undefined },
      { role: "assistant", text: "partial" },
      { role: "user", text: "q4", images: undefined },
    ]);
  });
  it("handles consecutive failures, errors with partial text, empty done and streaming", () => {
    const msgs: UiMessage[] = [
      { id: 1, role: "user", text: "ok" },
      { id: 2, role: "assistant", text: "fine", status: "done" },
      { id: 3, role: "user", text: "f1", images: ["data:a"] },
      { id: 4, role: "assistant", text: "half", status: "error", error: { message: "cut" } },
      { id: 5, role: "user", text: "f2" },
      { id: 6, role: "assistant", text: "", status: "done" },
      { id: 7, role: "user", text: "f3" },
      { id: 8, role: "assistant", text: "busy", status: "streaming" },
      { id: 9, role: "user", text: "next" },
    ];
    const h = toHistory(msgs);
    expect(h).toEqual([
      { role: "user", text: "ok", images: undefined },
      { role: "assistant", text: "fine" },
      { role: "user", text: "next", images: undefined },
    ]);
    for (let i = 1; i < h.length; i++) expect(h[i].role).not.toBe(h[i - 1].role);
  });
});

describe("prettyRaw / formatBytes / routingHeaders / imageRejection", () => {
  it("pretty prints JSON, leaves text", () => {
    expect(prettyRaw('{"a":1}')).toBe('{\n  "a": 1\n}');
    expect(prettyRaw("data: x")).toBe("data: x");
    expect(prettyRaw("")).toBe("");
  });
  it("formats bytes", () => {
    expect(formatBytes(512)).toBe("512 B");
    expect(formatBytes(2048)).toBe("2.0 KB");
    expect(formatBytes(8 * 1024 * 1024)).toBe("8.0 MB");
  });
  it("lists set routing headers", () => {
    expect(routingHeaders({ requestId: "r", fallback: false, attempts: 2 })).toEqual([
      ["x-yalr-request-id", "r"],
      ["x-yalr-fallback", "0"],
      ["x-yalr-attempts", "2"],
    ]);
  });
  it("rejects gif and non-images", () => {
    expect(imageRejection("image/png")).toBeNull();
    expect(imageRejection("image/gif")).toBe("gif");
    expect(imageRejection("text/plain")).toBe("not_image");
  });
});
