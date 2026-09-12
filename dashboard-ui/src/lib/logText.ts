/**
 * Parser fuer request/response-bodies der request-log-detailansicht.
 * Extrahiert zur reinen json-ansicht menschenlesbaren text (messages,
 * stream-deltas, tool-calls) aus den verschiedenen log-formaten.
 */

export interface StreamText {
  content: string;
  reasoning: string;
  toolCalls: { name: string; arguments: string }[];
}

export function prettyJson(raw: string): string {
  // koennte ein SSE-mitschnitt sein: zeilenweise formatieren
  if (raw.startsWith("data:")) {
    return raw
      .split("\n")
      .filter((l) => l.startsWith("data:"))
      .map((l) => {
        const data = l.slice(5).trim();
        if (data === "[DONE]") return "[DONE]";
        try {
          return JSON.stringify(JSON.parse(data), null, 2);
        } catch {
          return data;
        }
      })
      .join("\n\n");
  }
  try {
    return JSON.stringify(JSON.parse(raw), null, 2);
  } catch {
    return raw;
  }
}

/** Extrahiert die menschenlesbaren teile aus einem chat-completions-request:
 *  die user/assistant/system-nachrichten, optional mitgelieferte tool-ergebnisse. */
export function extractRequestText(raw: string): StreamText | null {
  let parsed: Record<string, unknown>;
  try {
    parsed = JSON.parse(raw);
  } catch {
    return null;
  }
  const messages = parsed["messages"] as Record<string, unknown>[] | undefined;
  if (!Array.isArray(messages)) return null;

  const content: string[] = [];
  for (const msg of messages) {
    const role = String(msg["role"] ?? "");
    const body = msg["content"];
    let text = "";
    if (typeof body === "string") {
      text = body;
    } else if (Array.isArray(body)) {
      // multimodal: text-teile konkatenieren, sonstige teile andeuten
      const parts: string[] = [];
      for (const part of body as Record<string, unknown>[]) {
        if (typeof part["text"] === "string") parts.push(part["text"]);
        else if (part["type"]) parts.push(`[${part["type"]}]`);
      }
      text = parts.join("\n");
    }
    if (role || text) content.push(`${role.toUpperCase()}: ${text}`);
  }
  return { content: content.join("\n\n"), reasoning: "", toolCalls: [] };
}

/** Extrahiert den zusammenhaengenden text aus einem stream-response-body.
 *  Zwei formate: roher SSE-mitschnitt (aeltere logs) und das synthetische
 *  chat.completion-JSON (neuere logs). */
export function extractStreamText(raw: string): StreamText | null {
  // roher SSE-mitschnitt: deltas sammeln (openai-format; anthropic-rohstream
  // hat "delta": {"type": "text_delta", "text": ...})
  if (raw.startsWith("data:")) {
    const content: string[] = [];
    const reasoning: string[] = [];
    const toolArgs = new Map<number, { name: string; arguments: string }>();
    for (const line of raw.split("\n")) {
      if (!line.startsWith("data:")) continue;
      const data = line.slice(5).trim();
      if (data === "[DONE]") continue;
      let event: Record<string, unknown>;
      try {
        event = JSON.parse(data);
      } catch {
        continue;
      }
      // openai-delta
      const choice = (event["choices"] as Record<string, unknown>[] | undefined)?.[0];
      const delta = choice?.["delta"] as Record<string, unknown> | undefined;
      if (typeof delta?.["content"] === "string") content.push(delta["content"]);
      if (typeof delta?.["reasoning"] === "string") reasoning.push(delta["reasoning"]);
      // anthropic text_delta
      if (event["type"] === "content_block_delta") {
        const d = event["delta"] as Record<string, unknown> | undefined;
        if (d?.["type"] === "text_delta" && typeof d["text"] === "string") content.push(d["text"]);
      }
      // openai tool_call-deltas
      const calls = delta?.["tool_calls"] as Record<string, unknown>[] | undefined;
      for (const call of calls ?? []) {
        const idx = Number(call["index"] ?? 0);
        const fn = call["function"] as Record<string, unknown> | undefined;
        const entry = toolArgs.get(idx) ?? { name: "", arguments: "" };
        if (typeof fn?.["name"] === "string" && fn["name"]) entry.name = fn["name"];
        if (typeof fn?.["arguments"] === "string") entry.arguments += fn["arguments"];
        toolArgs.set(idx, entry);
      }
    }
    return {
      content: content.join(""),
      reasoning: reasoning.join(""),
      toolCalls: [...toolArgs.values()],
    };
  }

  // synthetisches completion-json: content/reasoning/tool_calls liegen fertig vor
  let parsed: unknown;
  try {
    parsed = JSON.parse(raw);
  } catch {
    return null;
  }
  const obj = parsed as Record<string, unknown>;
  if (obj["object"] !== "chat.completion") return null;

  const msg = (obj["choices"] as Record<string, unknown>[] | undefined)?.[0]?.["message"] as
    | Record<string, unknown>
    | undefined;
  const toolCalls = (msg?.["tool_calls"] as Record<string, unknown>[] | undefined)?.map((tc) => {
    const fn = tc["function"] as Record<string, unknown>;
    return { name: String(fn?.["name"] ?? ""), arguments: String(fn?.["arguments"] ?? "") };
  });
  return {
    content: typeof msg?.["content"] === "string" ? msg["content"] : "",
    reasoning: typeof msg?.["reasoning"] === "string" ? msg["reasoning"] : "",
    toolCalls: toolCalls ?? [],
  };
}
