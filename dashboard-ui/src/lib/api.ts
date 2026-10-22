export class ApiError extends Error {
  status: number;
  constructor(status: number, message: string) {
    super(message);
    this.status = status;
  }
}

interface ApiErrorResponse {
  error?: string;
  /** konkrete Meldung des Backends (Validierung/Konflikt) — bevorzugt in ApiError.message */
  detail?: string;
}

async function request<T>(path: string, options?: RequestInit): Promise<T> {
  const res = await fetch(`/dashboard-api${path}`, {
    credentials: "same-origin",
    headers: {
      "Content-Type": "application/json",
      ...(options?.headers ?? {}),
    },
    ...options,
  });
  if (res.status === 401) {
    // Beim login ist eine 401 die antwort auf falsche zugangsdaten und darf
    // *keinen* reload/redirect ausloesen — die seite selbst reagiert darauf.
    if (!path.startsWith("/auth/")) {
      window.location.href = "/login";
    }
    throw new ApiError(401, "unauthorized");
  }
  const body = await res.json().catch(() => ({} as ApiErrorResponse));
  if (!res.ok) {
    const b = body as ApiErrorResponse;
    throw new ApiError(res.status, b.detail ?? b.error ?? res.statusText);
  }
  return body as T;
}

export const api = {
  get: <T>(path: string, options?: RequestInit) => request<T>(path, options),
  post: <T>(path: string, body?: unknown) =>
    request<T>(path, { method: "POST", body: body !== undefined ? JSON.stringify(body) : undefined }),
  put: <T>(path: string, body?: unknown) =>
    request<T>(path, { method: "PUT", body: body !== undefined ? JSON.stringify(body) : undefined }),
  del: <T>(path: string) => request<T>(path, { method: "DELETE" }),
};

// ===== Typen =====

export interface VirtualKey {
  id: string;
  name: string;
  key_prefix: string;
  budget_cents: number | null;
  enabled: boolean;
  created_at: string;
  last_used_at: string | null;
  key_hint?: string;
  /** Allow-Liste: leer = unbeschränkt. Alt-Daten haben das Feld evtl. nicht. */
  allowed_providers?: { id: string; name: string; kind: string }[];
  allowed_models?: string[];
}

export interface Provider {
  id: string;
  name: string;
  kind: string;
  base_url: string;
  enabled: boolean;
  created_at: string;
  metrics_url?: string | null;
}

export interface DiscoverMetricsResponse {
  metrics_url: string | null;
}

export interface RefreshCapsResponse {
  ok: boolean;
  provider_id: string;
  fetched_models: number;
  updated: { model_name: string; added: Record<string, unknown> }[];
  unchanged: string[];
  not_found_upstream: string[];
  metrics_url_discovered?: string | null;
}

export interface Model {
  id: string;
  model_name: string;
  upstream_model: string;
  input_price_per_million: number;
  output_price_per_million: number;
  enabled: boolean;
  provider_id: string;
  provider_name: string;
  quantization?: string | null;
  notes?: string | null;
  link?: string | null;
  capabilities?: Record<string, unknown> | null;
}

export interface Fallback {
  id: string;
  model_name: string;
  fallback_model_name: string;
  priority: number;
  enabled: boolean;
}

export interface Redirect {
  id: string;
  model_name: string;
  redirect_model_name: string;
  note: string;
  created_at: string;
  target_has_route: boolean;
}

/** Denkstufe und Stellschrauben einer Anfrage (vom Gateway ausgelesen). */
export interface RequestConfig {
  /** vereinheitlicht: none/low/medium/high/max, on, adaptive, "budget 8000" … */
  reasoning_effort?: string;
  /** true: die Stufe hat das Gateway aus den Modell-Faehigkeiten gesetzt */
  effort_by_gateway?: boolean;
  temperature?: number;
  top_p?: number;
  max_tokens?: number;
  /** Anzahl mitgeschickter Werkzeuge */
  tools?: number;
  json_mode?: boolean;
}

export interface LogEntry {
  id: string;
  request_id: string;
  timestamp: string;
  key_name: string;
  provider: string;
  provider_name: string;
  provider_id?: string | null;
  model: string;
  upstream_model: string;
  endpoint: string;
  status: number;
  error_type: string;
  is_stream: boolean;
  prompt_tokens: number;
  completion_tokens: number;
  cost_usd: number;
  duration_ms: number;
  first_byte_ms: number;
  /** Fallback-Infos (Teil 3) — optional: Alt-Daten haben die Felder nicht */
  is_fallback?: boolean;
  original_model?: string;
  attempts_made?: number;
  /** Redirect-Info — optional: Alt-Daten haben das Feld nicht */
  is_redirect?: boolean;
  /** Denkstufe ("" = ohne Angabe) und Konfiguration — optional: Alt-Daten haben sie nicht */
  reasoning_effort?: string;
  request_config?: RequestConfig | null;
  /** davon Nachdenken (in completion_tokens enthalten) */
  reasoning_tokens?: number;
  reasoning_exact?: boolean;
}

// ===== Live-Events (SSE /dashboard-api/live) =====

export interface LiveLog extends LogEntry {
  error_message?: string;
}

/** Phase eines laufenden Streams: denkt nach / schreibt die Antwort. */
export type StreamPhase = "thinking" | "answering";

export type LiveEvent =
  | {
      type: "request_started";
      request_id: string;
      timestamp: string;
      key_name: string;
      provider: string;
      provider_name: string;
      model: string;
      endpoint: string;
      is_stream: boolean;
      /** Redirect-Infos. Wire: vom Backend IMMER gesendet (is_redirect bool,
       *  redirect_to = "" bei Nicht-Redirect, nie null/abwesend).
       *  `?` nur als Alt-Daten-Toleranz. */
      is_redirect?: boolean;
      /** Redirect-Ziel ("" bei Nicht-Redirect). `?` nur als Alt-Daten-Toleranz. */
      redirect_to?: string;
      /** Prefill-Tokens, aus der Anfrage geschaetzt (0 = unbekannt). `?` nur als Alt-Daten-Toleranz. */
      prompt_tokens?: number;
      /** Denkstufe und Stellschrauben. `?` nur als Alt-Daten-Toleranz. */
      config?: RequestConfig;
    }
  | { type: "first_byte"; request_id: string; first_byte_ms: number }
  | {
      /** Zwischenstand eines laufenden Streams (hoechstens ~1/s je Request). */
      type: "progress";
      request_id: string;
      generated_tokens: number;
      reasoning_tokens: number;
      /** true: Zahl vom Anbieter, sonst aus der Textlaenge geschaetzt */
      tokens_exact: boolean;
      /** Tokens/s seit dem ersten erzeugten Zeichen (null in der ersten Sekunde) */
      tokens_per_s: number | null;
      /** Prefill-Tokens, sobald der Anbieter sie meldet (sonst null). `?` nur als Alt-Daten-Toleranz. */
      prompt_tokens?: number | null;
      /** true: reasoning_tokens vom Anbieter gemeldet */
      reasoning_exact?: boolean;
      /** woran das Modell gerade schreibt */
      phase?: StreamPhase | null;
    }
  | { type: "completed"; log: LiveLog };

export interface Stats {
  total_requests: number;
  success_requests: number;
  error_requests: number;
  total_cost: number;
  total_prompt_tokens: number;
  total_completion_tokens: number;
  avg_duration_ms: number;
  /** Fallback-Stats (Teil 3) — optional-tolerant behandeln */
  fallback_count?: number;
  /** 0..1, anteil der erfolgreichen requests */
  fallback_rate?: number;
  /** Redirect-Stats — optional-tolerant behandeln */
  redirect_count?: number;
  /** 0..1, anteil der erfolgreichen requests */
  redirect_rate?: number;
}

export interface TimeseriesPoint {
  bucket: string;
  requests: number;
  cost: number;
  avg_duration_ms: number;
}

export interface BreakdownItem {
  group: string;
  provider_name?: string | null;
  /** Denkstufe (nur bei group_by=model_provider_effort; "" = ohne Angabe) */
  effort?: string;
  requests: number;
  cost: number;
  tokens: number;
  completion_tokens?: number;
  reasoning_tokens?: number;
  duration_sum_ms?: number;
}

// ===== Live Provider Stats (/dashboard-api/live/stats) =====

export interface LiveProviderStats {
  provider: string;
  provider_name: string;
  reqs: number;
  errors: number;
  avg_ttft: number | null;
  p50: number;
  p95: number;
  decode_tps: number | null;
  decode_tps_own: number | null;
  prefill_tps: number | null;
  decode_tps_live: number | null;
  prefill_tps_live: number | null;
  cost_usd: number;
  metrics_url: string | null;
  queued: number | null;
  running: number | null;
  kv_cache_usage: number | null;
}

export interface InFlightRequest {
  request_id: string;
  /** provider-kind-slug (z. B. "openai_compat"); die Zuordnung zu provider-zeilen
   *  erfolgt ueber `provider_name` (instanz-name), da sich der slug ueber
   *  mehrere provider-instanzen teilt */
  provider: string;
  provider_name: string;
  model: string;
  key_name: string;
  /** epoch ms */
  started_at_ms: number;
  first_byte_ms: number | null;
  /** Redirect-Infos. Wire: vom Backend IMMER gesendet (`redirect_to` = ""
   *  bei Nicht-Redirect, nie null/abwesend). `?` nur als Alt-Daten-Toleranz.
   *  `model` ist das ANGEFRAGTE Modell, `redirect_to` das Ziel. */
  is_redirect?: boolean;
  redirect_to?: string;
  /** Bisher erzeugte Tokens (nur Streams, null vor dem ersten Zwischenstand).
   *  `?` nur als Alt-Daten-Toleranz. */
  generated_tokens?: number | null;
  reasoning_tokens?: number | null;
  tokens_exact?: boolean;
  tokens_per_s?: number | null;
  /** Prefill-Tokens: beim Start aus der Anfrage geschaetzt, spaeter vom Anbieter
   *  (`prompt_tokens_exact`). `?` nur als Alt-Daten-Toleranz. */
  prompt_tokens?: number | null;
  prompt_tokens_exact?: boolean;
  reasoning_exact?: boolean;
  phase?: StreamPhase | null;
  config?: RequestConfig;
}

export interface LiveStats {
  window: number;
  providers: LiveProviderStats[];
  in_flight: InFlightRequest[];
}
