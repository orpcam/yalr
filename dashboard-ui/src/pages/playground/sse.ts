// Inkrementeller parser fuer OpenAI-SSE (chat-completions, stream=true).
import type { StreamEvent, Usage } from "./types";

export interface SseParser {
  push(chunk: Uint8Array): void;
  /** flush; truncated = stream ohne [DONE] zu ende */
  end(): { truncated: boolean };
}

/** Holt eine fehlermeldung aus {"error": "..."} oder {"error": {"message": ...}} */
function errorText(err: unknown): string {
  if (typeof err === "string") return err;
  if (err && typeof err === "object") {
    const m = (err as { message?: unknown }).message;
    if (typeof m === "string" && m) return m;
    return JSON.stringify(err);
  }
  return String(err);
}

/** Wandelt ein geparstes chunk-objekt in events um */
export function chunkToEvents(obj: unknown): StreamEvent[] {
  if (!obj || typeof obj !== "object") return [];
  const o = obj as Record<string, unknown>;
  if (o.error !== undefined && o.error !== null) {
    return [{ type: "error", message: errorText(o.error) }];
  }
  const out: StreamEvent[] = [];
  const choices = Array.isArray(o.choices) ? o.choices : [];
  for (const c of choices) {
    const delta = (c as { delta?: Record<string, unknown> } | null)?.delta;
    if (!delta) continue;
    // leerer string zaehlt als fehlend
    const r = delta.reasoning || delta.reasoning_content;
    if (typeof r === "string" && r) out.push({ type: "reasoning", text: r });
    if (typeof delta.content === "string" && delta.content) {
      out.push({ type: "content", text: delta.content });
    }
  }
  if (o.usage && typeof o.usage === "object") {
    out.push({ type: "usage", usage: o.usage as Usage });
  }
  return out;
}

export function createSseParser(onEvent: (e: StreamEvent) => void): SseParser {
  const decoder = new TextDecoder("utf-8");
  let buf = "";
  let data: string[] = [];
  let done = false;

  const dispatch = () => {
    if (data.length === 0) return;
    const payload = data.join("\n");
    data = [];
    if (done) return;
    if (payload.trim() === "[DONE]") {
      done = true;
      onEvent({ type: "done" });
      return;
    }
    let obj: unknown;
    try {
      obj = JSON.parse(payload);
    } catch {
      onEvent({ type: "error", message: `invalid SSE data: ${payload.slice(0, 200)}` });
      return;
    }
    for (const e of chunkToEvents(obj)) onEvent(e);
  };

  const line = (raw: string) => {
    const l = raw.endsWith("\r") ? raw.slice(0, -1) : raw;
    if (l === "") return dispatch();
    if (l.startsWith(":")) return; // kommentar / keepalive
    const idx = l.indexOf(":");
    const field = idx === -1 ? l : l.slice(0, idx);
    let value = idx === -1 ? "" : l.slice(idx + 1);
    if (value.startsWith(" ")) value = value.slice(1);
    if (field === "data") data.push(value);
    // event:, id:, retry: werden ignoriert
  };

  const drain = () => {
    let nl: number;
    while ((nl = buf.indexOf("\n")) !== -1) {
      line(buf.slice(0, nl));
      buf = buf.slice(nl + 1);
    }
  };

  return {
    push(chunk) {
      buf += decoder.decode(chunk, { stream: true });
      drain();
    },
    end() {
      buf += decoder.decode();
      drain();
      if (buf) line(buf);
      buf = "";
      dispatch();
      return { truncated: !done };
    },
  };
}
