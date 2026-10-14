import { describe, expect, it } from "vitest";
import { createSseParser } from "./sse";
import type { StreamEvent } from "./types";

const enc = new TextEncoder();

function run(chunks: (string | Uint8Array)[]) {
  const events: StreamEvent[] = [];
  const p = createSseParser((e) => events.push(e));
  for (const c of chunks) p.push(typeof c === "string" ? enc.encode(c) : c);
  const end = p.end();
  return { events, ...end };
}

const chunk = (delta: object) => `data: ${JSON.stringify({ choices: [{ index: 0, delta }] })}\n\n`;

describe("createSseParser", () => {
  it("parses content deltas and [DONE]", () => {
    const r = run([chunk({ content: "Hel" }), chunk({ content: "lo" }), "data: [DONE]\n\n"]);
    expect(r.events).toEqual([
      { type: "content", text: "Hel" },
      { type: "content", text: "lo" },
      { type: "done" },
    ]);
    expect(r.truncated).toBe(false);
  });

  it("handles CRLF line endings", () => {
    const r = run([chunk({ content: "a" }).replace(/\n/g, "\r\n"), "data: [DONE]\r\n\r\n"]);
    expect(r.events).toEqual([{ type: "content", text: "a" }, { type: "done" }]);
  });

  it("handles lines split across chunks", () => {
    const full = chunk({ content: "split" }) + "data: [DONE]\n\n";
    const parts = [full.slice(0, 7), full.slice(7, 20), full.slice(20)];
    const r = run(parts);
    expect(r.events[0]).toEqual({ type: "content", text: "split" });
    expect(r.truncated).toBe(false);
  });

  it("handles CRLF split between \\r and \\n", () => {
    const full = chunk({ content: "x" }).replace(/\n/g, "\r\n");
    const i = full.indexOf("\r") + 1;
    const r = run([full.slice(0, i), full.slice(i)]);
    expect(r.events).toEqual([{ type: "content", text: "x" }]);
  });

  it("handles multibyte UTF-8 split across chunks", () => {
    const bytes = enc.encode(chunk({ content: "Grüße 🚀" }));
    const idx = bytes.indexOf(0xf0) + 2; // mitten im emoji
    const r = run([bytes.slice(0, idx), bytes.slice(idx)]);
    expect(r.events).toEqual([{ type: "content", text: "Grüße 🚀" }]);
  });

  it("feeds byte-by-byte", () => {
    const bytes = enc.encode(chunk({ content: "äb" }) + "data: [DONE]\n\n");
    const r = run(Array.from(bytes, (b) => new Uint8Array([b])));
    expect(r.events).toEqual([{ type: "content", text: "äb" }, { type: "done" }]);
  });

  it("ignores comment / keepalive lines and event: lines", () => {
    const r = run([": keepalive\n\n", "event: message\n", chunk({ content: "a" }), ":ping\n"]);
    expect(r.events).toEqual([{ type: "content", text: "a" }]);
  });

  it("joins multiple data: lines with newline", () => {
    const r = run(['data: {"choices":[{"delta":\ndata: {"content":"m"}}]}\n\n']);
    expect(r.events).toEqual([{ type: "content", text: "m" }]);
  });

  it("accepts data: without space", () => {
    const r = run(['data:{"choices":[{"delta":{"content":"n"}}]}\n\n']);
    expect(r.events).toEqual([{ type: "content", text: "n" }]);
  });

  it("emits usage from usage-only chunk with empty choices", () => {
    const r = run([
      `data: ${JSON.stringify({ choices: [], usage: { prompt_tokens: 3, completion_tokens: 5, total_tokens: 8 } })}\n\n`,
      "data: [DONE]\n\n",
    ]);
    expect(r.events).toEqual([
      { type: "usage", usage: { prompt_tokens: 3, completion_tokens: 5, total_tokens: 8 } },
      { type: "done" },
    ]);
  });

  it("ignores usage: null", () => {
    const r = run([`data: ${JSON.stringify({ choices: [{ delta: { content: "a" } }], usage: null })}\n\n`]);
    expect(r.events).toEqual([{ type: "content", text: "a" }]);
  });

  it("supports delta.reasoning and delta.reasoning_content", () => {
    const r = run([chunk({ reasoning: "think1" }), chunk({ reasoning_content: "think2" })]);
    expect(r.events).toEqual([
      { type: "reasoning", text: "think1" },
      { type: "reasoning", text: "think2" },
    ]);
  });

  it("emits reasoning before content within one delta", () => {
    const r = run([chunk({ reasoning: "r", content: "c" })]);
    expect(r.events).toEqual([
      { type: "reasoning", text: "r" },
      { type: "content", text: "c" },
    ]);
  });

  it("skips null / empty content and role-only deltas", () => {
    const r = run([chunk({ role: "assistant", content: null }), chunk({ content: "" }), chunk({ reasoning: null })]);
    expect(r.events).toEqual([]);
  });

  it("turns error objects into error events", () => {
    const r = run([
      `data: ${JSON.stringify({ error: { message: "upstream died", type: "server_error" } })}\n\n`,
      `data: ${JSON.stringify({ error: "plain" })}\n\n`,
    ]);
    expect(r.events).toEqual([
      { type: "error", message: "upstream died" },
      { type: "error", message: "plain" },
    ]);
  });

  it("reports invalid JSON as error event", () => {
    const r = run(["data: {nope\n\n"]);
    expect(r.events[0].type).toBe("error");
  });

  it("marks stream without [DONE] as truncated", () => {
    const r = run([chunk({ content: "a" })]);
    expect(r.truncated).toBe(true);
  });

  it("flushes a final event without trailing blank line on end()", () => {
    const r = run(['data: {"choices":[{"delta":{"content":"last"}}]}']);
    expect(r.events).toEqual([{ type: "content", text: "last" }]);
    expect(r.truncated).toBe(true);
  });

  it("flushes [DONE] without trailing newline", () => {
    const r = run(["data: [DONE]"]);
    expect(r.events).toEqual([{ type: "done" }]);
    expect(r.truncated).toBe(false);
  });

  it("ignores data after [DONE]", () => {
    const r = run(["data: [DONE]\n\n", chunk({ content: "late" })]);
    expect(r.events).toEqual([{ type: "done" }]);
  });

  it("handles several events in one chunk", () => {
    const r = run([chunk({ content: "a" }) + chunk({ content: "b" }) + "data: [DONE]\n\n"]);
    expect(r.events.map((e) => e.type)).toEqual(["content", "content", "done"]);
  });

  it("falls back to reasoning_content when reasoning is an empty string", () => {
    const r = run([chunk({ reasoning: "", reasoning_content: "think" }), "data: [DONE]\n\n"]);
    expect(r.events[0]).toEqual({ type: "reasoning", text: "think" });
  });
});
