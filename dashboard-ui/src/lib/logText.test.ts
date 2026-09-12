import { describe, expect, it } from "vitest";
import { extractRequestText, extractStreamText, prettyJson } from "./logText";

describe("prettyJson", () => {
  it("pretty-prints valid json", () => {
    expect(prettyJson('{"a":1}')).toBe('{\n  "a": 1\n}');
  });

  it("returns raw text for invalid json", () => {
    expect(prettyJson("not json")).toBe("not json");
  });

  it("formats SSE captures line by line", () => {
    const sse = [
      'data: {"a":1}',
      "data: [DONE]",
      "data: not-json",
    ].join("\n");
    expect(prettyJson(sse)).toBe('{\n  "a": 1\n}\n\n[DONE]\n\nnot-json');
  });
});

describe("extractRequestText", () => {
  it("extracts messages with roles", () => {
    const raw = JSON.stringify({
      messages: [
        { role: "system", content: "be brief" },
        { role: "user", content: "hi" },
      ],
    });
    const result = extractRequestText(raw);
    expect(result).not.toBeNull();
    expect(result!.content).toBe("SYSTEM: be brief\n\nUSER: hi");
  });

  it("concatenates multimodal text parts and hints at others", () => {
    const raw = JSON.stringify({
      messages: [
        {
          role: "user",
          content: [
            { type: "text", text: "look at this" },
            { type: "image_url" },
          ],
        },
      ],
    });
    const result = extractRequestText(raw)!;
    expect(result.content).toBe("USER: look at this\n[image_url]");
  });

  it("returns null for non-message bodies", () => {
    expect(extractRequestText('{"prompt":"x"}')).toBeNull();
    expect(extractRequestText("invalid")).toBeNull();
  });
});

describe("extractStreamText", () => {
  it("reassembles openai content deltas from a raw SSE capture", () => {
    const sse = [
      'data: {"choices":[{"delta":{"content":"Hel"}}]}',
      'data: {"choices":[{"delta":{"content":"lo"}}]}',
      "data: [DONE]",
    ].join("\n");
    const result = extractStreamText(sse)!;
    expect(result.content).toBe("Hello");
    expect(result.reasoning).toBe("");
    expect(result.toolCalls).toEqual([]);
  });

  it("collects reasoning and tool call deltas", () => {
    const sse = [
      'data: {"choices":[{"delta":{"reasoning":"think"}}]}',
      'data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"name":"get_weather","arguments":"{\\"ci"}}]}}]}',
      'data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"ty\\":\\"berlin\\"}"}}]}}]}',
    ].join("\n");
    const result = extractStreamText(sse)!;
    expect(result.reasoning).toBe("think");
    expect(result.toolCalls).toEqual([
      { name: "get_weather", arguments: '{"city":"berlin"}' },
    ]);
  });

  it("understands anthropic text_delta events", () => {
    const sse = 'data: {"type":"content_block_delta","delta":{"type":"text_delta","text":"hi"}}';
    const result = extractStreamText(sse)!;
    expect(result.content).toBe("hi");
  });

  it("parses synthetic chat.completion json", () => {
    const raw = JSON.stringify({
      object: "chat.completion",
      choices: [
        {
          message: {
            content: "done",
            reasoning: "thoughts",
            tool_calls: [
              { function: { name: "fn", arguments: "{}" } },
            ],
          },
        },
      ],
    });
    const result = extractStreamText(raw)!;
    expect(result.content).toBe("done");
    expect(result.reasoning).toBe("thoughts");
    expect(result.toolCalls).toEqual([{ name: "fn", arguments: "{}" }]);
  });

  it("returns null for unrelated json", () => {
    expect(extractStreamText('{"object":"list"}')).toBeNull();
    expect(extractStreamText("nope")).toBeNull();
  });
});
