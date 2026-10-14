// Reine UI-hilfsfunktionen der playground-seite (ohne react).
import type { Model, Redirect } from "@/lib/api";
import type { MetricsSummary } from "./metrics";
import type { ChatMessage, ChatParams, ReasoningEffort, ReasoningMode, RoutingInfo } from "./types";

export interface ModelOption {
  name: string;
  /** gesetzt, wenn der name eine redirect-quelle ist (ziel-modellname) */
  redirectTo?: string;
}

/**
 * Eindeutige, sortierte modellnamen (nur enabled) plus redirect-quellen. Ein redirect
 * greift im backend IMMER — auch wenn die quelle eigene modelle hat — daher wird
 * redirectTo auch dann gesetzt.
 */
export function buildModelOptions(models: Model[], redirects: Redirect[]): ModelOption[] {
  const map = new Map<string, ModelOption>();
  for (const m of models) {
    if (m.enabled && m.model_name) map.set(m.model_name, { name: m.model_name });
  }
  for (const r of redirects) {
    if (r.model_name && r.redirect_model_name) {
      map.set(r.model_name, { name: r.model_name, redirectTo: r.redirect_model_name });
    }
  }
  return [...map.values()].sort((a, b) => a.name.localeCompare(b.name));
}

/** Modellname, den das backend tatsaechlich routet (redirect-ziel, sonst der name selbst) */
export function routedModelName(redirects: Redirect[], modelName: string): string {
  const r = redirects.find((x) => x.model_name === modelName && x.redirect_model_name);
  return r ? r.redirect_model_name : modelName;
}

/** Provider-namen (eindeutig, sortiert), die diesen modellnamen enabled anbieten */
export function providersForModel(models: Model[], modelName: string): string[] {
  const set = new Set<string>();
  for (const m of models) {
    if (m.enabled && m.model_name === modelName && m.provider_name) set.add(m.provider_name);
  }
  return [...set].sort((a, b) => a.localeCompare(b));
}

/** Provider-auswahl fuer das gewaehlte modell — bei redirect die des ziels */
export function providersForSelection(models: Model[], redirects: Redirect[], modelName: string): string[] {
  return providersForModel(models, routedModelName(redirects, modelName));
}

/** "" -> undefined, gueltige zahl -> zahl, sonst null (= ungueltig) */
export function parseOptionalNumber(raw: string, integer = false): number | undefined | null {
  const t = raw.trim().replace(",", ".");
  if (!t) return undefined;
  const n = Number(t);
  if (!Number.isFinite(n)) return null;
  if (integer && !Number.isInteger(n)) return null;
  return n;
}

/** Formular-zustand des params-panels (zahlen als rohtext) */
export interface ParamsState {
  model: string;
  provider: string;
  systemPrompt: string;
  temperature: string;
  maxTokens: string;
  topP: string;
  stream: boolean;
  reasoning: ReasoningMode;
  effort: ReasoningEffort | "";
  extraBody: string;
}

export const DEFAULT_PARAMS: ParamsState = {
  model: "",
  provider: "",
  systemPrompt: "",
  temperature: "",
  maxTokens: "",
  topP: "",
  stream: true,
  reasoning: "default",
  effort: "",
  extraBody: "",
};

/** Wendet ein formular-patch an; provider wird geleert, wenn das neue modell ihn nicht anbietet */
export function applyParamsPatch(
  prev: ParamsState,
  patch: Partial<ParamsState>,
  models: Model[],
  redirects: Redirect[]
): ParamsState {
  const next = { ...prev, ...patch };
  if (
    patch.model !== undefined &&
    next.provider &&
    !providersForSelection(models, redirects, next.model).includes(next.provider)
  ) {
    next.provider = "";
  }
  return next;
}

export type NumberField = "temperature" | "maxTokens" | "topP";

/** Formular -> ChatParams; invalid listet felder mit ungueltiger zahl */
export function toChatParams(s: ParamsState): { params: ChatParams; invalid: NumberField[] } {
  const invalid: NumberField[] = [];
  const num = (field: NumberField, integer = false) => {
    const v = parseOptionalNumber(s[field], integer);
    if (v === null) {
      invalid.push(field);
      return undefined;
    }
    return v;
  };
  const params: ChatParams = {
    model: s.model,
    systemPrompt: s.systemPrompt,
    temperature: num("temperature"),
    maxTokens: num("maxTokens", true),
    topP: num("topP"),
    reasoning: s.reasoning,
    effort: s.reasoning === "on" && s.effort ? s.effort : undefined,
    stream: s.stream,
    extraBody: s.extraBody,
    provider: s.provider || undefined,
  };
  return { params, invalid };
}

export type MessageStatus = "streaming" | "done" | "stopped" | "error";

/** Anzeige-nachricht im chat (erweitert ChatMessage um laufzeit-infos) */
export interface UiMessage extends ChatMessage {
  id: number;
  status?: MessageStatus;
  error?: { status?: number; message: string };
  routing?: RoutingInfo;
  metrics?: MetricsSummary;
  truncated?: boolean;
  streamErrors?: string[];
  /** antwort ohne darstellbaren text (tool_calls / content-parts): hinweis + JSON */
  unsupported?: { reason: "tool_calls" | "non_string_content"; json: string };
}

/**
 * Verlauf fuer den naechsten request. Fehlgeschlagene, laufende oder leere
 * antworten fallen raus — zusammen mit der user-nachricht, die sie ausgeloest hat,
 * damit die rollen alternieren und fehlgeschlagene bilder nicht erneut gesendet
 * werden. Abgebrochene antworten mit text bleiben (samt frage).
 */
export function toHistory(messages: UiMessage[]): ChatMessage[] {
  const out: ChatMessage[] = [];
  for (const m of messages) {
    if (m.role === "assistant") {
      if (m.status === "error" || m.status === "streaming" || !m.text) {
        if (out.length > 0 && out[out.length - 1].role === "user") out.pop();
        continue;
      }
      out.push({ role: "assistant", text: m.text });
    } else {
      out.push({ role: "user", text: m.text, images: m.images });
    }
  }
  return out;
}

/** JSON-text huebsch formatieren; kein JSON -> unveraendert */
export function prettyRaw(text: string): string {
  const t = text.trim();
  if (!t) return text;
  try {
    return JSON.stringify(JSON.parse(t), null, 2);
  } catch {
    return text;
  }
}

/** Bytes lesbar (KB/MB, 1024er-basis) */
export function formatBytes(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
  return `${(n / (1024 * 1024)).toFixed(1)} MB`;
}

/** Routing-info als header-liste fuer die raw-ansicht (nur gesetzte werte) */
export function routingHeaders(r: RoutingInfo): [string, string][] {
  const rows: [string, string | undefined][] = [
    ["x-yalr-request-id", r.requestId],
    ["x-yalr-provider", r.provider],
    ["x-yalr-model", r.model],
    ["x-yalr-upstream-model", r.upstreamModel],
    ["x-yalr-fallback", r.fallback === undefined ? undefined : r.fallback ? "1" : "0"],
    ["x-yalr-attempts", r.attempts === undefined ? undefined : String(r.attempts)],
  ];
  return rows.filter((x): x is [string, string] => x[1] !== undefined);
}

/** Pruefung vor resizeImageFile, damit die meldung uebersetzt werden kann */
export function imageRejection(type: string): "not_image" | "gif" | null {
  if (!type.startsWith("image/")) return "not_image";
  if (type === "image/gif") return "gif";
  return null;
}

/** Tailwind-klassen fuer textareas (passend zu components/ui/input) */
export const TEXTAREA_CLASS =
  "flex w-full rounded-md border border-border bg-transparent px-3 py-2 text-sm shadow-sm placeholder:text-muted-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:cursor-not-allowed disabled:opacity-50";
