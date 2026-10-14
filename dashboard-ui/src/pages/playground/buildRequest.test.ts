import { describe, expect, it } from "vitest";
import { MAX_BODY_BYTES, buildChatRequest, deepMerge, estimateBodySize, parseExtraBody } from "./buildRequest";
import type { ChatMessage, ChatParams } from "./types";

const base: ChatParams = {
  model: "m1",
  systemPrompt: "",
  reasoning: "default",
  stream: false,
  extraBody: "",
};
const hist: ChatMessage[] = [{ role: "user", text: "hi" }];

describe("buildChatRequest", () => {
  it("builds minimal body", () => {
    const { body, error } = buildChatRequest(base, hist);
    expect(error).toBeUndefined();
    expect(body).toEqual({ model: "m1", stream: false, messages: [{ role: "user", content: "hi" }] });
  });

  it("adds system prompt only if non-empty", () => {
    expect(buildChatRequest({ ...base, systemPrompt: "   " }, hist).body.messages).toHaveLength(1);
    const msgs = buildChatRequest({ ...base, systemPrompt: "be nice" }, hist).body.messages as unknown[];
    expect(msgs[0]).toEqual({ role: "system", content: "be nice" });
  });

  it("encodes images as content parts", () => {
    const { body } = buildChatRequest(base, [{ role: "user", text: "what?", images: ["data:a", "data:b"] }]);
    expect((body.messages as unknown[])[0]).toEqual({
      role: "user",
      content: [
        { type: "text", text: "what?" },
        { type: "image_url", image_url: { url: "data:a" } },
        { type: "image_url", image_url: { url: "data:b" } },
      ],
    });
  });

  it("omits the text part for image-only messages", () => {
    for (const text of ["", "  \n "]) {
      const { body } = buildChatRequest(base, [{ role: "user", text, images: ["data:a"] }]);
      expect((body.messages as unknown[])[0]).toEqual({
        role: "user",
        content: [{ type: "image_url", image_url: { url: "data:a" } }],
      });
    }
  });

  it("caps the body at 8 MB (below the 10 MB backend limit)", () => {
    expect(MAX_BODY_BYTES).toBeLessThan(10 * 1024 * 1024);
  });

  it("sends assistant messages as plain text without reasoning", () => {
    const { body } = buildChatRequest(base, [...hist, { role: "assistant", text: "ans", reasoning: "secret" }]);
    expect((body.messages as unknown[])[1]).toEqual({ role: "assistant", content: "ans" });
  });

  it("only includes sampling params when set (0 counts as set)", () => {
    expect(buildChatRequest(base, hist).body).not.toHaveProperty("temperature");
    const { body } = buildChatRequest({ ...base, temperature: 0, topP: 0.9, maxTokens: 100 }, hist);
    expect(body).toMatchObject({ temperature: 0, top_p: 0.9, max_tokens: 100 });
  });

  it("requests usage when streaming", () => {
    const { body } = buildChatRequest({ ...base, stream: true }, hist);
    expect(body).toMatchObject({ stream: true, stream_options: { include_usage: true } });
  });

  it("reasoning default sends nothing", () => {
    const { body } = buildChatRequest({ ...base, effort: "high" }, hist);
    expect(body).not.toHaveProperty("chat_template_kwargs");
    expect(body).not.toHaveProperty("reasoning_effort");
  });

  it("reasoning on/off uses chat_template_kwargs only", () => {
    expect(buildChatRequest({ ...base, reasoning: "on" }, hist).body.chat_template_kwargs).toEqual({
      enable_thinking: true,
    });
    const on = buildChatRequest({ ...base, reasoning: "on", effort: "low" }, hist).body;
    expect(on.chat_template_kwargs).toEqual({ enable_thinking: true, reasoning_effort: "low" });
    expect(on).not.toHaveProperty("reasoning_effort");
    const off = buildChatRequest({ ...base, reasoning: "off", effort: "high" }, hist).body;
    expect(off.chat_template_kwargs).toEqual({ enable_thinking: false });
  });

  it("deep-merges extraBody, extra wins", () => {
    const { body } = buildChatRequest(
      { ...base, reasoning: "on", temperature: 1, extraBody: '{"temperature":0.2,"chat_template_kwargs":{"x":1},"seed":7}' },
      hist
    );
    expect(body).toMatchObject({ temperature: 0.2, seed: 7, chat_template_kwargs: { enable_thinking: true, x: 1 } });
  });

  it("keeps model/messages/stream UI-owned", () => {
    const { body } = buildChatRequest(
      { ...base, extraBody: '{"model":"evil","messages":[],"stream":true,"n":2}' },
      hist
    );
    expect(body.model).toBe("m1");
    expect(body.stream).toBe(false);
    expect(body.messages).toHaveLength(1);
    expect(body.n).toBe(2);
  });

  it("reports invalid extraBody", () => {
    expect(buildChatRequest({ ...base, extraBody: "{bad" }, hist).error).toMatch(/invalid JSON/);
    expect(buildChatRequest({ ...base, extraBody: "[1]" }, hist).error).toMatch(/object/);
    expect(buildChatRequest({ ...base, extraBody: "42" }, hist).error).toMatch(/object/);
    expect(buildChatRequest({ ...base, extraBody: "null" }, hist).error).toMatch(/object/);
  });
});

describe("parseExtraBody / deepMerge", () => {
  it("treats whitespace as empty", () => {
    expect(parseExtraBody("  \n")).toEqual({ value: {} });
  });
  it("replaces arrays and nested scalars", () => {
    expect(deepMerge({ a: [1, 2], b: { c: 1, d: 2 } }, { a: [3], b: { d: 5 } })).toEqual({
      a: [3],
      b: { c: 1, d: 5 },
    });
  });
  it("does not mutate inputs", () => {
    const a = { x: { y: 1 } };
    deepMerge(a, { x: { y: 2 } });
    expect(a.x.y).toBe(1);
  });
});

describe("estimateBodySize", () => {
  it("counts UTF-8 bytes", () => {
    expect(estimateBodySize({ a: "ü" })).toBe(JSON.stringify({ a: "ü" }).length + 1);
  });
});
