// Baut den chat-completions-body aus den UI-parametern und dem verlauf.
import type { ChatMessage, ChatParams } from "./types";

/** Schluessel, die das UI besitzt — extraBody darf sie nicht ueberschreiben */
const UI_OWNED = new Set(["model", "messages", "stream"]);

type Json = Record<string, unknown>;

function isPlainObject(v: unknown): v is Json {
  return typeof v === "object" && v !== null && !Array.isArray(v);
}

/** Tiefes merge; `extra` gewinnt, arrays werden ersetzt */
export function deepMerge(base: Json, extra: Json): Json {
  const out: Json = { ...base };
  for (const [k, v] of Object.entries(extra)) {
    out[k] = isPlainObject(v) && isPlainObject(out[k]) ? deepMerge(out[k] as Json, v) : v;
  }
  return out;
}

/** Parst das extraBody-feld; leer = {}, sonst muss es ein JSON-objekt sein */
export function parseExtraBody(raw: string): { value: Json; error?: string } {
  const t = raw.trim();
  if (!t) return { value: {} };
  let v: unknown;
  try {
    v = JSON.parse(t);
  } catch (e) {
    return { value: {}, error: `extra body: invalid JSON (${(e as Error).message})` };
  }
  if (!isPlainObject(v)) return { value: {}, error: "extra body: must be a JSON object" };
  return { value: v };
}

function toApiMessage(m: ChatMessage): Json {
  if (m.role === "user" && m.images && m.images.length > 0) {
    return {
      role: "user",
      content: [
        // leerer text-part wird weggelassen (manche upstreams lehnen ihn ab)
        ...(m.text.trim() ? [{ type: "text", text: m.text }] : []),
        ...m.images.map((url) => ({ type: "image_url", image_url: { url } })),
      ],
    };
  }
  // reasoning wird bewusst nicht zurueckgeschickt
  return { role: m.role, content: m.text };
}

export function buildChatRequest(
  params: ChatParams,
  history: ChatMessage[]
): { body: Json; error?: string } {
  const messages: Json[] = [];
  if (params.systemPrompt.trim()) messages.push({ role: "system", content: params.systemPrompt });
  for (const m of history) messages.push(toApiMessage(m));

  let body: Json = { model: params.model, messages, stream: params.stream };
  if (params.stream) body.stream_options = { include_usage: true };
  if (params.temperature !== undefined) body.temperature = params.temperature;
  if (params.topP !== undefined) body.top_p = params.topP;
  if (params.maxTokens !== undefined) body.max_tokens = params.maxTokens;

  // top-level reasoning_effort lehnen manche upstreams ab -> nur chat_template_kwargs
  if (params.reasoning === "on") {
    const kw: Json = { enable_thinking: true };
    if (params.effort) kw.reasoning_effort = params.effort;
    body.chat_template_kwargs = kw;
  } else if (params.reasoning === "off") {
    body.chat_template_kwargs = { enable_thinking: false };
  }

  const extra = parseExtraBody(params.extraBody);
  if (extra.error) return { body, error: extra.error };
  const filtered: Json = {};
  for (const [k, v] of Object.entries(extra.value)) if (!UI_OWNED.has(k)) filtered[k] = v;
  body = deepMerge(body, filtered);
  return { body };
}

/** Client-obergrenze fuer den request-body. Backend-limit sind 10 MB; 8 MB lassen
 *  luft fuer header und abweichungen zwischen schaetzung und tatsaechlichem body. */
export const MAX_BODY_BYTES = 8 * 1024 * 1024;

/** Groesse des serialisierten bodys in bytes (UTF-8) */
export function estimateBodySize(body: unknown): number {
  return new TextEncoder().encode(JSON.stringify(body)).length;
}
