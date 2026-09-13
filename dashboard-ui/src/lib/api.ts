export class ApiError extends Error {
  status: number;
  constructor(status: number, message: string) {
    super(message);
    this.status = status;
  }
}

interface ApiErrorResponse {
  error?: string;
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
    throw new ApiError(res.status, (body as ApiErrorResponse).error ?? res.statusText);
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
}

// ===== Live-Events (SSE /dashboard-api/live) =====

export interface LiveLog extends LogEntry {
  error_message?: string;
}

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
    }
  | { type: "first_byte"; request_id: string; first_byte_ms: number }
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
  requests: number;
  cost: number;
  tokens: number;
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
}

export interface LiveStats {
  window: number;
  providers: LiveProviderStats[];
  in_flight: InFlightRequest[];
}
