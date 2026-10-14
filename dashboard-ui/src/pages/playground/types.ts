// Typen fuer die Playground-Seite (reine Logik, keine Komponenten).

/** "default" = nichts senden, "on"/"off" = enable_thinking via chat_template_kwargs */
export type ReasoningMode = "default" | "on" | "off";
export type ReasoningEffort = "low" | "medium" | "high";

export interface ChatParams {
  model: string;
  systemPrompt: string;
  temperature?: number;
  maxTokens?: number;
  topP?: number;
  reasoning: ReasoningMode;
  /** nur relevant bei reasoning = "on" */
  effort?: ReasoningEffort;
  stream: boolean;
  /** rohes JSON (objekt) aus dem textfeld; leer = nichts */
  extraBody: string;
  /** provider-name fuer den x-llm-provider-header; leer = routing entscheidet */
  provider?: string;
}

export interface ChatMessage {
  role: "user" | "assistant";
  text: string;
  /** data-URLs (nur user) */
  images?: string[];
  /** nur anzeige — wird nicht zurueckgeschickt */
  reasoning?: string;
}

/** Aus den x-yalr-*-response-headern */
export interface RoutingInfo {
  requestId?: string;
  provider?: string;
  model?: string;
  upstreamModel?: string;
  fallback?: boolean;
  attempts?: number;
}

export interface Usage {
  prompt_tokens?: number;
  completion_tokens?: number;
  total_tokens?: number;
}

export type StreamEvent =
  | { type: "content"; text: string }
  | { type: "reasoning"; text: string }
  | { type: "usage"; usage: Usage }
  | { type: "error"; message: string }
  | { type: "done" };

export interface ChatResult {
  routing: RoutingInfo;
  status: number;
  /** geparste antwort bei stream=false */
  json?: unknown;
  /** stream ohne [DONE] beendet */
  truncated?: boolean;
  /** roher response-text (SSE oder JSON) fuer die "raw"-ansicht */
  rawText: string;
}

export interface SpeechResult {
  routing: RoutingInfo;
  blob: Blob;
  contentType: string;
}
