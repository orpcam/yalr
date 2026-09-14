# YALR — Yet Another LLM Router

YALR is a self-hosted LLM gateway (similar to Helicone/LiteLLM) written in Rust: a unified OpenAI-compatible API for multiple providers, with request logging, cost tracking, virtual keys, fallbacks, temporary redirects, and an embedded web dashboard.

## Features

- **Unified API**: OpenAI-compatible endpoints (`/v1/chat/completions`,
  `/v1/embeddings`, `/v1/models`) plus the native Anthropic format (`/v1/messages`)
- **Providers**: OpenAI, Anthropic, Google Gemini, and any OpenAI-compatible
  service (Ollama, Groq, DeepSeek, Mistral, Together, vLLM, ...) — including
  transparent format translation in both directions
- **Streaming**: SSE streaming is passed through 1:1 (OpenAI) or translated
  live into the target format (Anthropic/Gemini → OpenAI chunks)
- **Virtual Keys**: any number of named API keys with optional budgets;
  requests can be filtered by key name in the dashboard
- **Fallbacks & Retries**: per-model fallback chains, automatic retries on
  transient errors (429/5xx/network)
- **Temporary Redirects**: deliberate model reroutes (A → B) that always
  apply, even while A is healthy — single-hop, at most one per model, with
  an optional note. Unlike fallbacks, a redirect takes precedence over the
  model's own route; the target's fallback chains still apply
- **Logging & Costs**: every request is logged asynchronously (without blocking
  the hot path) to ClickHouse, including token usage and costs; 90-day TTL
- **Dashboard**: React UI with overview (KPIs, charts), request log with
  request/response details, key management, provider/model/fallback/redirect
  setup
  - Dark/light mode, English/German UI (switchable in settings)
- **Routing Visibility**: served-by-fallback and served-by-redirect markers
  on overview, live requests, and the request log (including in-flight rows)

## Architecture

```
Public ──► Caddy Edge :8081 (only /v1/*, /health, /metrics)
                │
                ▼
Client ──► Gateway :8080 ──► OpenAI / Anthropic / Gemini / OpenAI-compatible
              │  Auth (virtual keys, sha256 + cache)
              │  Retry/Fallback (provider chain)
              ├─► Postgres   (keys, provider credentials, models, fallbacks, redirects, sessions)
              ├─► ClickHouse (request logs, batched via channel)
              └─► /dashboard-api + React UI (session cookie, bcrypt) — private
```

## Quickstart

```bash
cp .env.example .env        # adjust values (especially ADMIN_PASSWORD, SESSION_SECRET)!
docker compose up --build
```

- Dashboard: http://localhost:8080/ (log in with `ADMIN_USERNAME`/`ADMIN_PASSWORD` from `.env`)
- Initial setup in the dashboard under "Providers & Models": add provider
  credentials, map models (with prices), optionally define fallback chains
  or temporary redirects
- Create a named key under "API Keys"

## Usage (Gateway)

```bash
curl http://localhost:8080/v1/chat/completions \
  -H "Authorization: Bearer sk-llm-..." \
  -H "Content-Type: application/json" \
  -d '{
    "model": "gpt-4o",
    "messages": [{"role": "user", "content": "Hello!"}],
    "stream": true
  }'
```

Any OpenAI client works — just point its `base_url` at the gateway and use the
virtual key as the API key.

| Endpoint | Description |
|---|---|
| `POST /v1/chat/completions` | OpenAI format (streaming + non-streaming) |
| `POST /v1/messages` | Native Anthropic format (passthrough to Anthropic) |
| `POST /v1/embeddings` | Embeddings (OpenAI-compatible providers) |
| `GET /v1/models` | Available (mapped) models |
| `GET /health` | Healthcheck (liveness) |

Hybrid routing: the gateway picks the provider automatically based on the model
name. Can be overridden per request via the header `x-llm-provider: openai |
anthropic | gemini | <provider-name>`.

Budgets: a virtual key with a budget set receives `402 Payment Required` once
the budget is exceeded. Spend is aggregated from ClickHouse and cached.

## Development (without Docker)

```bash
# Rust
cargo check && cargo test

# Dashboard with hot reload (Vite on :5173, proxying to :8080)
cd dashboard-ui && npm install && npm run dev
```

For local Postgres/ClickHouse instances, set the `POSTGRES_*`/`CLICKHOUSE_*`
variables accordingly (see `.env.example`).

## Project Structure

```
crates/
├── common/     # AppState (pools, key cache, routing table), crypto, auth
├── providers/  # Provider adapters + format translation (OpenAI/Anthropic/Gemini)
├── ingest/     # Async log pipeline: channel → ClickHouse batch inserts
├── yalr/       # Axum server: proxy, retry/fallback, redirects, SSE streaming
└── dashboard/  # Dashboard API + embedded React UI
dashboard-ui/   # React (Vite + Tailwind, shadcn-style)
migrations/     # Postgres migrations
```

## Notes

- Provider API keys are stored encrypted in Postgres
  (XOR keystream; swap for an AEAD library if you need stronger guarantees)
- Logs contain request/response bodies (max. 32 KB, configurable truncation) —
  adjust the ingest pipeline if you deal with sensitive content
- ClickHouse TTL: logs are automatically deleted after 90 days
