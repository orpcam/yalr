// Fetch-wrapper fuer /dashboard-api/playground/* (chat + speech).
import { createSseParser } from "./sse";
import type { ChatResult, RoutingInfo, SpeechResult, StreamEvent, Usage } from "./types";

const BASE = "/dashboard-api/playground";

export class PlaygroundError extends Error {
  status: number;
  routing: RoutingInfo;
  constructor(status: number, message: string, routing: RoutingInfo) {
    super(message);
    this.status = status;
    this.routing = routing;
  }
}

/** Antwort-body brach nach den headern ab (netzwerk oder abort) — routing + bisheriger rohtext bleiben */
export class StreamInterruptedError extends PlaygroundError {
  rawText: string;
  constructor(status: number, message: string, routing: RoutingInfo, rawText: string) {
    super(status, message, routing);
    this.rawText = rawText;
  }
}

/** fetch scheiterte, bevor eine antwort kam (verbindung, DNS, CORS …) */
export class NetworkError extends Error {}

/** error.type aus einem JSON-fehler-body, sonst undefined */
export function extractErrorType(text: string): string | undefined {
  try {
    const v = JSON.parse(text) as { error?: { type?: unknown } } | null;
    const ty = v && typeof v === "object" && v.error && typeof v.error === "object" ? v.error.type : undefined;
    return typeof ty === "string" ? ty : undefined;
  } catch {
    return undefined;
  }
}

/**
 * Nur die eigene dashboard-session ist abgelaufen -> /login. Ein upstream-401 hat
 * immer eine x-yalr-request-id (request erreichte den proxy-kern) und wird
 * wie jeder andere fehler angezeigt.
 */
export function isSessionExpired(status: number, errorType: string | undefined, requestId: string | undefined): boolean {
  return status === 401 && (errorType === "auth" || !requestId);
}

export interface SendOptions {
  signal?: AbortSignal;
  /** provider-name -> x-llm-provider */
  provider?: string;
}

/** Liest die x-yalr-*-header; fehlende header bleiben undefined */
export function readRouting(headers: Headers): RoutingInfo {
  const get = (k: string) => headers.get(k) ?? undefined;
  const fb = get("x-yalr-fallback");
  const att = get("x-yalr-attempts");
  const n = att !== undefined ? Number.parseInt(att, 10) : NaN;
  return {
    requestId: get("x-yalr-request-id"),
    provider: get("x-yalr-provider"),
    model: get("x-yalr-model"),
    upstreamModel: get("x-yalr-upstream-model"),
    fallback: fb === undefined ? undefined : fb === "1" || fb.toLowerCase() === "true",
    attempts: Number.isFinite(n) ? n : undefined,
  };
}

/** Fehlermeldung aus einem fehler-body: {"error":{"message"}}, {"error":"..."}, {"detail"}, sonst text */
export function extractErrorMessage(status: number, text: string): string {
  const t = text.trim();
  if (t) {
    try {
      const v = JSON.parse(t) as Record<string, unknown> | null;
      if (v && typeof v === "object") {
        const err = v.error;
        if (err && typeof err === "object") {
          const m = (err as { message?: unknown }).message;
          if (typeof m === "string" && m) return m;
        }
        if (typeof err === "string" && err) return err;
        if (typeof v.detail === "string" && v.detail) return v.detail;
        if (typeof v.message === "string" && v.message) return v.message;
      }
    } catch {
      // kein JSON -> rohtext
    }
    return t.length > 500 ? `${t.slice(0, 500)}…` : t;
  }
  return `HTTP ${status}`;
}

async function post(path: string, body: unknown, opts: SendOptions): Promise<Response> {
  const headers: Record<string, string> = { "Content-Type": "application/json" };
  if (opts.provider) headers["x-llm-provider"] = opts.provider;
  let res: Response;
  try {
    res = await fetch(`${BASE}${path}`, {
      method: "POST",
      credentials: "same-origin",
      headers,
      body: JSON.stringify(body),
      signal: opts.signal,
    });
  } catch (e) {
    // abort (DOMException) unveraendert weiterreichen; TypeError = netzwerkfehler
    if (e instanceof TypeError) throw new NetworkError(e.message);
    throw e;
  }
  const routing = readRouting(res.headers);
  if (!res.ok) {
    const text = await res.text().catch(() => "");
    if (isSessionExpired(res.status, extractErrorType(text), routing.requestId)) {
      window.location.assign("/login");
      throw new PlaygroundError(401, "unauthorized", routing);
    }
    throw new PlaygroundError(res.status, extractErrorMessage(res.status, text), routing);
  }
  return res;
}

export async function sendChat(
  body: unknown,
  opts: SendOptions & { onEvent: (e: StreamEvent) => void }
): Promise<ChatResult> {
  const res = await post("/chat", body, opts);
  const routing = readRouting(res.headers);
  const ct = res.headers.get("content-type") ?? "";

  if (ct.includes("text/event-stream") && res.body) {
    const parser = createSseParser(opts.onEvent);
    const raw = new TextDecoder();
    let rawText = "";
    const reader = res.body.getReader();
    try {
      for (;;) {
        const { done, value } = await reader.read();
        if (done) break;
        rawText += raw.decode(value, { stream: true });
        parser.push(value);
      }
    } catch (e) {
      rawText += raw.decode();
      throw new StreamInterruptedError(res.status, e instanceof Error ? e.message : String(e), routing, rawText);
    } finally {
      reader.releaseLock();
    }
    rawText += raw.decode();
    const { truncated } = parser.end();
    return { routing, status: res.status, truncated, rawText };
  }

  let rawText: string;
  try {
    rawText = await res.text();
  } catch (e) {
    throw new StreamInterruptedError(res.status, e instanceof Error ? e.message : String(e), routing, "");
  }
  let json: unknown;
  try {
    json = JSON.parse(rawText);
  } catch {
    throw new PlaygroundError(res.status, `invalid JSON response: ${rawText.slice(0, 200)}`, routing);
  }
  return { routing, status: res.status, json, rawText };
}

export async function sendSpeech(body: unknown, opts: SendOptions = {}): Promise<SpeechResult> {
  const res = await post("/speech", body, opts);
  const routing = readRouting(res.headers);
  const contentType = (res.headers.get("content-type") ?? "").trim();
  if (!isAudioContentType(contentType)) {
    // z.B. JSON-fehler mit status 200 -> als fehler anzeigen, keine blob-URL
    const text = await res.text().catch(() => "");
    const detail = text.trim() ? extractErrorMessage(res.status, text) : "";
    const msg = `unexpected content-type "${contentType || "none"}"${detail ? `: ${detail}` : ""}`;
    throw new PlaygroundError(res.status, msg, routing);
  }
  // neu verpacken, damit der blob-typ sicher dem audio-typ entspricht
  const blob = new Blob([await res.blob()], { type: contentType });
  return { routing, blob, contentType };
}

export function isAudioContentType(ct: string): boolean {
  return ct.trim().toLowerCase().startsWith("audio/");
}

const AUDIO_EXT: Record<string, string> = {
  "audio/mpeg": "mp3",
  "audio/mp3": "mp3",
  "audio/wav": "wav",
  "audio/x-wav": "wav",
  "audio/wave": "wav",
  "audio/vnd.wave": "wav",
  "audio/opus": "opus",
  "audio/ogg": "ogg",
  "audio/flac": "flac",
  "audio/x-flac": "flac",
  "audio/aac": "aac",
  "audio/mp4": "m4a",
  "audio/webm": "webm",
  "audio/pcm": "pcm",
  "audio/l16": "pcm",
};

/** Download-endung aus dem tatsaechlichen content-type; unbekannt -> angefragtes format */
export function audioExtension(contentType: string, fallback: string): string {
  const base = contentType.split(";")[0].trim().toLowerCase();
  return AUDIO_EXT[base] ?? fallback;
}

export type CompletionOutcome =
  | { kind: "ok"; content: string; reasoning: string; usage?: Usage }
  | { kind: "error"; message: string }
  /** antwort ohne darstellbaren text (tool_calls / content-parts) -> hinweis + JSON */
  | { kind: "unsupported"; reason: "tool_calls" | "non_string_content"; json: string; reasoning: string; usage?: Usage };

/** Bewertet eine nicht-gestreamte 200-antwort */
export function classifyCompletion(json: unknown): CompletionOutcome {
  const o = json && typeof json === "object" ? (json as Record<string, unknown>) : null;
  if (o && o.error !== undefined && o.error !== null) {
    const err = o.error;
    const m = err && typeof err === "object" ? (err as { message?: unknown }).message : undefined;
    const message = typeof err === "string" ? err : typeof m === "string" && m ? m : JSON.stringify(err);
    return { kind: "error", message };
  }
  const choices = o && Array.isArray(o.choices) ? o.choices : [];
  const msg = (choices[0] as { message?: unknown } | undefined)?.message;
  if (!msg || typeof msg !== "object") {
    return { kind: "error", message: "response has no choices[0].message" };
  }
  const { content, reasoning, usage } = parseCompletion(json);
  const m = msg as { content?: unknown; tool_calls?: unknown };
  const pretty = () => JSON.stringify(msg, null, 2);
  if (Array.isArray(m.tool_calls) && m.tool_calls.length > 0) {
    return { kind: "unsupported", reason: "tool_calls", json: pretty(), reasoning, usage };
  }
  if (m.content !== undefined && m.content !== null && typeof m.content !== "string") {
    return { kind: "unsupported", reason: "non_string_content", json: pretty(), reasoning, usage };
  }
  return { kind: "ok", content, reasoning, usage };
}

/** Holt text/reasoning/usage aus einer nicht-gestreamten chat-completion */
export function parseCompletion(json: unknown): {
  content: string;
  reasoning: string;
  usage?: Usage;
} {
  const o = (json ?? {}) as {
    choices?: { message?: { content?: unknown; reasoning?: unknown; reasoning_content?: unknown } }[];
    usage?: Usage;
  };
  const msg = o.choices?.[0]?.message ?? {};
  const str = (v: unknown) => (typeof v === "string" ? v : "");
  return {
    content: str(msg.content),
    reasoning: str(msg.reasoning) || str(msg.reasoning_content),
    usage: o.usage,
  };
}
