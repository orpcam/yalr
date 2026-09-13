import { createContext, useContext, useEffect, useState, ReactNode } from "react";
import { formatCost as formatCostPure, Currency } from "./format";

export type Language = "en" | "de";
export type { Currency };

const STORAGE_KEY = "llm-gw:lang";
const CURRENCY_STORAGE_KEY = "llm-gw:currency";

export const LANGUAGES: { value: Language; label: string }[] = [
  { value: "en", label: "English" },
  { value: "de", label: "Deutsch" },
];

export const CURRENCIES: { value: Currency; label: string }[] = [
  { value: "USD", label: "USD ($)" },
  { value: "EUR", label: "EUR (€)" },
];

function initialCurrency(): Currency {
  try {
    const stored = localStorage.getItem(CURRENCY_STORAGE_KEY);
    if (stored === "USD" || stored === "EUR") return stored;
  } catch {
    // localStorage nicht verfügbar
  }
  return "USD";
}

const en = {
  // AppLayout
  nav_overview: "Overview",
  nav_requests: "Requests",
  nav_keys: "API Keys",
  nav_providers: "Providers & Models",
  nav_settings: "Settings",
  loading: "Loading...",
  logout: "Log out",
  theme_light: "☀️ Light mode",
  theme_dark: "🌙 Dark mode",
  hide_sidebar: "Hide sidebar",
  show_sidebar: "Show sidebar",

  // Login
  login_title: "YALR",
  login_username: "Username",
  login_password: "Password",
  login_submit: "Log in",
  login_submitting: "Logging in...",
  login_failed: "Login failed",

  // Overview
  all_keys: "All keys",
  last_hour: "Last hour",
  last_24h: "24 hours",
  last_7d: "7 days",
  last_30d: "30 days",
  last_year: "1 year",
  all_time: "All time",
  requests: "Requests",
  cost: "Cost",
  tokens: "Tokens",
  prompt_tokens: "Prompt",
  completion_tokens: "Completion",
  total_tokens: "Total",
  prefill_tps: "Prefill t/s",
  decode_tps: "Decode t/s",
  kv_cache: "KV Cache",
  page_size: "{n} / page",
  pagination_range: "{from}–{to} of {total}",
  prev_page: "Prev",
  next_page: "Next",
  live_performance: "Live performance",
  in_flight: "In flight",
  requests_in_progress: "Requests in processing",
  queued: "Queued",
  ttft: "TTFT",
  avg_ttft: "Ø TTFT",
  running_for: "running for {d}",
  completed: "completed",
  prefill_tokens: "Prefill Tokens",
  decode_tokens: "Decode Tokens",
  first_token_pending: "…",
  live_window_note: "rolling window applies to latency, error rate & cost — decode/prefill, in-flight and queued are live (only shown while active)",
  na: "N/A",
  metrics_url: "Metrics URL",
  truncated_warning: "Truncated",
  view_raw: "Raw",
  view_text: "Text",
  edit: "Edit",
  capabilities: "Capabilities (JSON)",
  capabilities_placeholder: '{"attachment": true, "modalities": {"input": ["text", "image"]}, "max_content_length": 131072}',
  capabilities_invalid: "Capabilities must be a valid JSON object.",
  reasoning: "Reasoning",
  response_text: "Response text",
  tool_calls: "Tool calls",
  error_rate: "Error rate",
  window_30s: "30 s",
  window_60s: "60 s",
  window_5m: "5 min",
  last_seconds: "last {s} seconds",
  avg_latency: "Avg latency",
  success_rate: "{pct} successful",
  tokens_in_out: "{in} in / {out} out",
  requests_cost_over_time: "Requests & cost over time",
  cost_per_model: "Cost per model",
  no_data_range: "No data in the selected time range.",
  no_data: "No data.",
  model: "Model",

  // Requests
  columns: "Columns",
  reset: "Reset",
  all_providers: "All providers",
  all_status: "All statuses",
  model_placeholder: "Model...",
  time: "Time",
  key: "Key",
  provider: "Provider",
  type: "Type",
  status: "Status",
  stream: "Stream",
  duration: "Duration",
  no_requests: "No requests in the selected time range.",
  live: "Live",
  first_token: "first token after {ms}",

  // RequestDetail
  back: "← Back",
  request_details: "Request details",
  endpoint: "Endpoint",
  timestamp: "Timestamp",
  never: "never",
  error: "Error: {type}",
  request_body: "Request body",
  response_body: "Response body",
  retry: "Retry",
  load_failed: "Could not load data.",

  // Keys
  new_key_created: "New key created — visible only now!",
  copy: "Copy",
  show: "Show",
  hide: "Hide",
  copied: "Copied!",
  copy_failed: "Copy failed. Please copy the key manually.",
  reveal_unavailable: "This key cannot be displayed. It may be a legacy key or the APP_SECRET has changed.",
  reveal_rate_limited: "Too many reveal requests. Please wait a moment and try again.",
  reveal_legacy: "This key was created before the reveal feature and cannot be displayed. Only the hash was stored.",
  create_new_key: "Create new key",
  name: "Name",
  name_placeholder: "e.g. production-app",
  budget_usd_optional: "Budget (USD, optional)",
  budget_invalid: "Please enter a valid non-negative budget.",
  unlimited: "unlimited",
  create: "Create",
  budget: "Budget",
  state: "Status",
  active: "active",
  disabled: "disabled",
  last_used: "Last used",
  actions: "Actions",
  no_keys: "No keys yet.",
  confirm_delete_key: 'Really delete key "{name}"?',
  deactivate: "Deactivate",
  activate: "Activate",
  delete: "Delete",

  // Providers
  add: "Add",
  base_url: "Base URL",
  api_key: "API key",
  input_price: "Input $/1M",
  output_price: "Output $/1M",
  no_providers: "No providers configured.",
  confirm_delete_provider: 'Delete provider "{name}"? Associated models will be deleted as well.',
  confirm_delete_model: 'Delete model "{name}"?',
  confirm_delete_fallback: 'Delete fallback {model} → {fallback}?',
  select_placeholder: "Select...",
  model_name_gateway: "Model name (gateway)",
  upstream_model: "Upstream model",
  upstream_placeholder: "= model name",
  quantization: "Quantization",
  notes: "Notes",
  notes_placeholder: "Optional notes",
  link: "Link",
  link_placeholder: "https://… (model card, docs)",
  no_models: "No models configured.",
  fallback_chains: "Fallback chains",
  fallback_hint:
    "On errors (timeout, 5xx, rate limit) the gateway automatically tries the fallback models in the configured order.",
  fallback_to: "Fallback to",
  no_fallbacks: "No fallbacks configured.",

  // Settings
  change_admin_password: "Change admin password",
  current_password: "Current password",
  new_password: "New password (min. 8 characters)",
  save: "Save",
  rename: "Rename",
  cancel: "Cancel",
  sync_caps: "Sync capabilities",
  sync_caps_busy: "Syncing…",
  discover_metrics: "Discover",
  discover_metrics_hint:
    "Fills in the Metrics URL automatically if the provider exposes a /metrics endpoint.",
  metrics_not_found: "No /metrics endpoint found.",
  metrics_discovered: "Metrics URL discovered: {url}",
  api_key_unchanged: "unchanged (leave empty to keep)",
  password_changed: "Password changed.",
  using_gateway: "Using the gateway",
  gateway_compat: "The gateway is OpenAI-API-compatible:",
  gateway_extra:
    "Also available: {messages} (native Anthropic format), {embeddings}, {models}.",
  gateway_override: "Per-request provider override via header: ",

  // Language / Currency
  language: "Language",
  currency: "Currency",
  currency_description:
    "Display symbol only — no conversion. All amounts stay in USD internally.",
  currency_usd: "USD ($)",
  currency_eur: "EUR (€)",
  error_generic: "Error",
  // Toasts / feedback
  action_failed: "Action failed: {msg}",
  saved: "Saved.",
};

export type TranslationKey = keyof typeof en;

const de: Record<TranslationKey, string> = {
  nav_overview: "Übersicht",
  nav_requests: "Anfragen",
  nav_keys: "API-Keys",
  nav_providers: "Provider & Modelle",
  nav_settings: "Einstellungen",
  loading: "Lade...",
  logout: "Abmelden",
  theme_light: "☀️ Hell",
  theme_dark: "🌙 Dunkel",
  hide_sidebar: "Sidebar ausblenden",
  show_sidebar: "Sidebar einblenden",

  login_title: "YALR",
  login_username: "Benutzername",
  login_password: "Passwort",
  login_submit: "Anmelden",
  login_submitting: "Anmelden...",
  login_failed: "Login fehlgeschlagen",

  all_keys: "Alle Keys",
  last_hour: "Letzte Stunde",
  last_24h: "24 Stunden",
  last_7d: "7 Tage",
  last_30d: "30 Tage",
  last_year: "1 Jahr",
  all_time: "Seit Aufzeichnung",
  requests: "Anfragen",
  cost: "Kosten",
  tokens: "Tokens",
  prompt_tokens: "Prompt",
  completion_tokens: "Completion",
  total_tokens: "Total",
  prefill_tps: "Prefill t/s",
  decode_tps: "Decode t/s",
  kv_cache: "KV-Cache",
  page_size: "{n} / Seite",
  pagination_range: "{from}–{to} von {total}",
  prev_page: "Zurück",
  next_page: "Weiter",
  live_performance: "Live-Performance",
  in_flight: "In Bearbeitung",
  requests_in_progress: "Anfragen in Bearbeitung",
  queued: "Wartend",
  ttft: "TTFT",
  avg_ttft: "Ø TTFT",
  running_for: "läuft seit {d}",
  completed: "abgeschlossen",
  prefill_tokens: "Prefill-Tokens",
  decode_tokens: "Decode-Tokens",
  first_token_pending: "…",
  live_window_note: "Fenster gilt für Latenz, Fehlerrate & Kosten — Decode/Prefill, In-Flight und Wartend sind live (nur bei aktiver Anfrage sichtbar)",
  na: "N/A",
  metrics_url: "Metrics-URL",
  truncated_warning: "Abgeschnitten",
  view_raw: "Roh",
  view_text: "Text",
  edit: "Bearbeiten",
  capabilities: "Fähigkeiten (JSON)",
  capabilities_placeholder: '{"attachment": true, "modalities": {"input": ["text", "image"]}, "max_content_length": 131072}',
  capabilities_invalid: "Fähigkeiten müssen ein gültiges JSON-Objekt sein.",
  reasoning: "Reasoning",
  response_text: "Antworttext",
  tool_calls: "Tool-Calls",
  error_rate: "Fehlerrate",
  window_30s: "30 s",
  window_60s: "60 s",
  window_5m: "5 Min.",
  last_seconds: "letzte {s} Sekunden",
  avg_latency: "Ø Latenz",
  success_rate: "{pct} erfolgreich",
  tokens_in_out: "{in} in / {out} out",
  requests_cost_over_time: "Requests & Kosten über Zeit",
  cost_per_model: "Kosten pro Modell",
  no_data_range: "Keine Daten im gewählten Zeitraum.",
  no_data: "Keine Daten.",
  model: "Modell",

  columns: "Spalten",
  reset: "Zurücksetzen",
  all_providers: "Alle Provider",
  all_status: "Alle Status",
  model_placeholder: "Modell...",
  time: "Zeit",
  key: "Key",
  provider: "Provider",
  type: "Typ",
  status: "Status",
  stream: "Stream",
  duration: "Dauer",
  no_requests: "Keine Anfragen im gewählten Zeitraum.",
  live: "Live",
  first_token: "erstes Token nach {ms}",

  back: "← Zurück",
  request_details: "Anfrage-Details",
  endpoint: "Endpoint",
  timestamp: "Zeitpunkt",
  never: "nie",
  error: "Fehler: {type}",
  request_body: "Request-Body",
  response_body: "Response-Body",
  retry: "Erneut versuchen",
  load_failed: "Daten konnten nicht geladen werden.",

  new_key_created: "Neuer Key erstellt — nur jetzt sichtbar!",
  copy: "Kopieren",
  show: "Anzeigen",
  hide: "Verbergen",
  copied: "Kopiert!",
  copy_failed: "Kopieren fehlgeschlagen. Bitte Key manuell kopieren.",
  reveal_unavailable: "Dieser Key kann nicht angezeigt werden. Es handelt sich um einen Legacy-Key oder das APP_SECRET wurde geändert.",
  reveal_rate_limited: "Zu viele Anzeige-Anfragen. Bitte kurz warten und erneut versuchen.",
  reveal_legacy: "Dieser Key wurde vor der Anzeige-Funktion erstellt und kann nicht angezeigt werden – es wurde nur der Hash gespeichert.",
  create_new_key: "Neuen Key erstellen",
  name: "Name",
  name_placeholder: "z.B. production-app",
  budget_usd_optional: "Budget (USD, optional)",
  budget_invalid: "Bitte ein gültiges, nicht-negatives Budget eingeben.",
  unlimited: "unbegrenzt",
  create: "Erstellen",
  budget: "Budget",
  state: "Status",
  active: "aktiv",
  disabled: "deaktiviert",
  last_used: "Zuletzt genutzt",
  actions: "Aktionen",
  no_keys: "Noch keine Keys vorhanden.",
  confirm_delete_key: 'Key "{name}" wirklich löschen?',
  deactivate: "Deaktivieren",
  activate: "Aktivieren",
  delete: "Löschen",

  add: "Hinzufügen",
  base_url: "Base URL",
  api_key: "API-Key",
  input_price: "Input $/1M",
  output_price: "Output $/1M",
  no_providers: "Noch keine Provider konfiguriert.",
  confirm_delete_provider:
    'Provider "{name}" löschen? Zugehörige Modelle werden mitgelöscht.',
  confirm_delete_model: 'Modell "{name}" löschen?',
  confirm_delete_fallback: 'Fallback {model} → {fallback} löschen?',
  select_placeholder: "Auswählen...",
  model_name_gateway: "Modellname (Gateway)",
  upstream_model: "Upstream-Modell",
  upstream_placeholder: "= Modellname",
  quantization: "Quantisierung",
  notes: "Notizen",
  notes_placeholder: "Optionale Notizen",
  link: "Link",
  link_placeholder: "https://… (Model Card, Doku)",
  no_models: "Noch keine Modelle konfiguriert.",
  fallback_chains: "Fallback-Ketten",
  fallback_hint:
    "Bei Fehlern (Timeout, 5xx, Rate-Limit) versucht das Gateway automatisch die Fallback-Modelle in der konfigurierten Reihenfolge.",
  fallback_to: "Fallback auf",
  no_fallbacks: "Keine Fallbacks konfiguriert.",

  change_admin_password: "Admin-Passwort ändern",
  current_password: "Aktuelles Passwort",
  new_password: "Neues Passwort (min. 8 Zeichen)",
  save: "Speichern",
  rename: "Umbenennen",
  cancel: "Abbrechen",
  sync_caps: "Capabilities synchronisieren",
  sync_caps_busy: "Synchronisiere…",
  discover_metrics: "Discovern",
  discover_metrics_hint:
    "Füllt die Metrics-URL automatisch aus, falls der Provider einen /metrics-Endpunkt bereitstellt.",
  metrics_not_found: "Kein /metrics-Endpunkt gefunden.",
  metrics_discovered: "Metrics-URL entdeckt: {url}",
  api_key_unchanged: "unverändert (leer lassen, um zu behalten)",
  password_changed: "Passwort geändert.",
  using_gateway: "Gateway verwenden",
  gateway_compat: "Das Gateway ist OpenAI-API-kompatibel:",
  gateway_extra: "Zusätzlich verfügbar: {messages} (natives Anthropic-Format), {embeddings}, {models}.",
  gateway_override: "Provider-Override pro Request via Header: ",

  language: "Sprache",
  currency: "Währung",
  currency_description:
    "Nur Anzeigesymbol — keine Umrechnung. Alle Beträge bleiben intern in USD.",
  currency_usd: "USD ($)",
  currency_eur: "EUR (€)",
  error_generic: "Fehler",
  // Toasts / feedback
  action_failed: "Aktion fehlgeschlagen: {msg}",
  saved: "Gespeichert.",
};

const translations: Record<Language, Record<TranslationKey, string>> = { en, de };

const LOCALES: Record<Language, string> = { en: "en-US", de: "de-DE" };

function initialLanguage(): Language {
  try {
    const stored = localStorage.getItem(STORAGE_KEY);
    if (stored === "en" || stored === "de") return stored;
  } catch {
    // localStorage nicht verfügbar
  }
  return "en";
}

interface LanguageContextValue {
  language: Language;
  locale: string;
  setLanguage: (lang: Language) => void;
  currency: Currency;
  setCurrency: (c: Currency) => void;
  formatCost: (v: number) => string;
  t: (key: TranslationKey, params?: Record<string, string | number>) => string;
  formatNumber: (v: number) => string;
  formatDateTime: (date: Date | string) => string;
  formatTime: (date: Date | string) => string;
  formatDuration: (ms: number) => string;
}

const LanguageContext = createContext<LanguageContextValue | null>(null);

export function LanguageProvider({ children }: { children: ReactNode }) {
  const [language, setLanguageState] = useState<Language>(initialLanguage);
  const [currency, setCurrencyState] = useState<Currency>(initialCurrency);

  useEffect(() => {
    try {
      localStorage.setItem(STORAGE_KEY, language);
    } catch {
      // ignorieren
    }
  }, [language]);

  useEffect(() => {
    try {
      localStorage.setItem(CURRENCY_STORAGE_KEY, currency);
    } catch {
      // ignorieren
    }
  }, [currency]);

  const setLanguage = (lang: Language) => setLanguageState(lang);
  const setCurrency = (c: Currency) => setCurrencyState(c);

  const t = (
    key: TranslationKey,
    params?: Record<string, string | number>
  ): string => {
    let text: string = translations[language][key] ?? translations.en[key] ?? key;
    if (params) {
      for (const [k, v] of Object.entries(params)) {
        // replaceAll: mehrere vorkommen desselben parameters ersetzen
        text = text.replaceAll(`{${k}}`, String(v));
      }
    }
    return text;
  };

  const locale = LOCALES[language];

  const formatNumber = (v: number) => v.toLocaleString(locale);

  // zeiten: < 1s in millisekunden, darueber in sekunden (mit ms-reste)
  const formatDuration = (ms: number) => {
    if (ms < 1000) return `${Math.round(ms)} ms`;
    const secs = ms / 1000;
    return `${secs.toLocaleString(locale, { maximumFractionDigits: 2 })} s`;
  };

  const formatDateTime = (date: Date | string) =>
    new Date(date).toLocaleString(locale);

  const formatTime = (date: Date | string) =>
    new Date(date).toLocaleTimeString(locale, { hour: "2-digit", minute: "2-digit" });

  const formatCost = (v: number) => formatCostPure(v, currency, locale);

  return (
    <LanguageContext.Provider
      value={{ language, locale, setLanguage, currency, setCurrency, formatCost, t, formatNumber, formatDateTime, formatTime, formatDuration }}
    >
      {children}
    </LanguageContext.Provider>
  );
}

export function useLanguage() {
  const ctx = useContext(LanguageContext);
  if (!ctx) throw new Error("useLanguage must be used within LanguageProvider");
  return ctx;
}

export function useCurrency() {
  const ctx = useContext(LanguageContext);
  if (!ctx) throw new Error("useCurrency must be used within LanguageProvider");
  return { currency: ctx.currency, setCurrency: ctx.setCurrency, formatCost: ctx.formatCost };
}
