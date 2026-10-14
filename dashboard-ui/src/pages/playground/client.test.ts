import { afterEach, describe, expect, it, vi } from "vitest";
import {
  NetworkError,
  PlaygroundError,
  StreamInterruptedError,
  audioExtension,
  classifyCompletion,
  extractErrorMessage,
  extractErrorType,
  isAudioContentType,
  isSessionExpired,
  parseCompletion,
  readRouting,
  sendChat,
  sendSpeech,
} from "./client";

describe("readRouting", () => {
  it("reads all headers", () => {
    const h = new Headers({
      "x-yalr-request-id": "r1",
      "x-yalr-provider": "p",
      "x-yalr-model": "m",
      "x-yalr-upstream-model": "um",
      "x-yalr-fallback": "1",
      "x-yalr-attempts": "2",
    });
    expect(readRouting(h)).toEqual({
      requestId: "r1",
      provider: "p",
      model: "m",
      upstreamModel: "um",
      fallback: true,
      attempts: 2,
    });
  });
  it("leaves missing headers undefined", () => {
    expect(readRouting(new Headers())).toEqual({
      requestId: undefined,
      provider: undefined,
      model: undefined,
      upstreamModel: undefined,
      fallback: undefined,
      attempts: undefined,
    });
  });
  it("parses fallback 0 and bad attempts", () => {
    const r = readRouting(new Headers({ "x-yalr-fallback": "0", "x-yalr-attempts": "x" }));
    expect(r.fallback).toBe(false);
    expect(r.attempts).toBeUndefined();
  });
});

describe("extractErrorMessage", () => {
  it("prefers nested error.message", () => {
    expect(extractErrorMessage(400, '{"error":{"message":"bad model","type":"invalid_request_error"}}')).toBe(
      "bad model"
    );
  });
  it("accepts string error and detail", () => {
    expect(extractErrorMessage(500, '{"error":"boom"}')).toBe("boom");
    expect(extractErrorMessage(422, '{"error":"x","detail":"y"}')).toBe("x");
    expect(extractErrorMessage(422, '{"detail":"y"}')).toBe("y");
  });
  it("falls back to text", () => {
    expect(extractErrorMessage(502, "Bad Gateway")).toBe("Bad Gateway");
    expect(extractErrorMessage(500, '{"foo":1}')).toBe('{"foo":1}');
  });
  it("falls back to status when empty", () => {
    expect(extractErrorMessage(503, "  ")).toBe("HTTP 503");
  });
  it("truncates long text", () => {
    expect(extractErrorMessage(500, "a".repeat(1000)).length).toBe(501);
  });
});

describe("parseCompletion", () => {
  it("extracts content, reasoning and usage", () => {
    expect(
      parseCompletion({
        choices: [{ message: { content: "c", reasoning_content: "r" } }],
        usage: { completion_tokens: 1 },
      })
    ).toEqual({ content: "c", reasoning: "r", usage: { completion_tokens: 1 } });
  });
  it("handles null content and missing choices", () => {
    expect(parseCompletion({ choices: [{ message: { content: null, reasoning: "x" } }] })).toEqual({
      content: "",
      reasoning: "x",
      usage: undefined,
    });
    expect(parseCompletion(null)).toEqual({ content: "", reasoning: "", usage: undefined });
  });
});

describe("parseCompletion reasoning fallback", () => {
  it("treats empty reasoning as missing", () => {
    expect(parseCompletion({ choices: [{ message: { content: "c", reasoning: "", reasoning_content: "rc" } }] }).reasoning).toBe(
      "rc"
    );
  });
});

describe("isSessionExpired / extractErrorType", () => {
  it("reads error.type", () => {
    expect(extractErrorType('{"error":{"type":"auth","message":"x"}}')).toBe("auth");
    expect(extractErrorType('{"error":"x"}')).toBeUndefined();
    expect(extractErrorType("nope")).toBeUndefined();
  });
  it("logs out on dashboard auth errors", () => {
    expect(isSessionExpired(401, "auth", undefined)).toBe(true);
    expect(isSessionExpired(401, "auth", "r1")).toBe(true);
    expect(isSessionExpired(401, undefined, undefined)).toBe(true);
  });
  it("keeps upstream 401s and other statuses as normal errors", () => {
    expect(isSessionExpired(401, "invalid_request_error", "r1")).toBe(false);
    expect(isSessionExpired(401, undefined, "r1")).toBe(false);
    expect(isSessionExpired(500, "internal", undefined)).toBe(false);
    expect(isSessionExpired(403, "auth", undefined)).toBe(false);
  });
});

describe("classifyCompletion", () => {
  it("ok for plain text", () => {
    expect(classifyCompletion({ choices: [{ message: { content: "hi" } }], usage: { total_tokens: 3 } })).toEqual({
      kind: "ok",
      content: "hi",
      reasoning: "",
      usage: { total_tokens: 3 },
    });
  });
  it("error field wins", () => {
    expect(classifyCompletion({ error: { message: "boom" } })).toEqual({ kind: "error", message: "boom" });
    expect(classifyCompletion({ error: "bad", choices: [{ message: { content: "x" } }] })).toEqual({
      kind: "error",
      message: "bad",
    });
    expect(classifyCompletion({ error: { code: 1 } })).toEqual({ kind: "error", message: '{"code":1}' });
  });
  it("error without choices[0].message", () => {
    expect(classifyCompletion({ choices: [] }).kind).toBe("error");
    expect(classifyCompletion({}).kind).toBe("error");
    expect(classifyCompletion(null).kind).toBe("error");
    expect(classifyCompletion({ choices: [{ delta: {} }] }).kind).toBe("error");
  });
  it("tool_calls and non-string content are unsupported", () => {
    const tc = classifyCompletion({
      choices: [{ message: { content: null, tool_calls: [{ id: "t1" }] } }],
    });
    expect(tc).toMatchObject({ kind: "unsupported", reason: "tool_calls" });
    expect(tc.kind === "unsupported" && JSON.parse(tc.json)).toEqual({ content: null, tool_calls: [{ id: "t1" }] });
    expect(classifyCompletion({ choices: [{ message: { content: [{ type: "text", text: "a" }] } }] })).toMatchObject({
      kind: "unsupported",
      reason: "non_string_content",
    });
  });
  it("null content without tool calls is an (empty) ok", () => {
    expect(classifyCompletion({ choices: [{ message: { content: null, tool_calls: [] } }] }).kind).toBe("ok");
  });
});

describe("speech content-type", () => {
  it("accepts only audio/*", () => {
    expect(isAudioContentType("audio/mpeg")).toBe(true);
    expect(isAudioContentType(" Audio/WAV ")).toBe(true);
    expect(isAudioContentType("application/json")).toBe(false);
    expect(isAudioContentType("")).toBe(false);
  });
  it("derives extension from content-type, falls back to format", () => {
    expect(audioExtension("audio/mpeg", "wav")).toBe("mp3");
    expect(audioExtension("audio/wav; codecs=1", "mp3")).toBe("wav");
    expect(audioExtension("audio/ogg", "opus")).toBe("ogg");
    expect(audioExtension("audio/x-unknown", "flac")).toBe("flac");
  });
});

describe("sendChat / sendSpeech with mocked fetch", () => {
  afterEach(() => vi.unstubAllGlobals());
  const enc = new TextEncoder();

  it("keeps routing and partial raw text when the stream breaks", async () => {
    const body = new ReadableStream<Uint8Array>({
      start(c) {
        c.enqueue(enc.encode('data: {"choices":[{"delta":{"content":"hi"}}]}\n\n'));
      },
      pull(c) {
        c.error(new TypeError("network lost"));
      },
    });
    vi.stubGlobal(
      "fetch",
      vi.fn(async () =>
        new Response(body, {
          status: 200,
          headers: { "content-type": "text/event-stream", "x-yalr-request-id": "r9", "x-yalr-provider": "p" },
        })
      )
    );
    const events: unknown[] = [];
    const err = await sendChat({}, { onEvent: (e) => events.push(e) }).catch((e: unknown) => e);
    expect(err).toBeInstanceOf(StreamInterruptedError);
    const si = err as StreamInterruptedError;
    expect(si.status).toBe(200);
    expect(si.routing).toMatchObject({ requestId: "r9", provider: "p" });
    expect(si.rawText).toContain('"hi"');
    expect(events).toEqual([{ type: "content", text: "hi" }]);
  });

  it("maps fetch TypeError to NetworkError", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async () => {
        throw new TypeError("Failed to fetch");
      })
    );
    const err = await sendChat({}, { onEvent: () => {} }).catch((e: unknown) => e);
    expect(err).toBeInstanceOf(NetworkError);
    expect((err as Error).message).toBe("Failed to fetch");
  });

  it("treats an upstream 401 (with request id) as a normal error", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(
        async () =>
          new Response('{"error":{"message":"bad key","type":"invalid_request_error"}}', {
            status: 401,
            headers: { "x-yalr-request-id": "r1" },
          })
      )
    );
    const err = await sendChat({}, { onEvent: () => {} }).catch((e: unknown) => e);
    expect(err).toBeInstanceOf(PlaygroundError);
    expect(err).toMatchObject({ status: 401, message: "bad key" });
  });

  it("rejects non-audio speech responses with the body as message", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(
        async () => new Response('{"error":"no tts"}', { status: 200, headers: { "content-type": "application/json" } })
      )
    );
    const err = await sendSpeech({}).catch((e: unknown) => e);
    expect(err).toBeInstanceOf(PlaygroundError);
    expect((err as Error).message).toContain("no tts");
  });

  it("re-wraps audio blobs with the response content-type", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async () => new Response(new Uint8Array([1, 2, 3]), { status: 200, headers: { "content-type": "audio/wav" } }))
    );
    const res = await sendSpeech({});
    expect(res.contentType).toBe("audio/wav");
    expect(res.blob.type).toBe("audio/wav");
    expect(res.blob.size).toBe(3);
  });
});
