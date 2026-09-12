-- Virtual Keys: benannte API-Keys, über die Clients das Gateway nutzen.
CREATE TABLE IF NOT EXISTS virtual_keys (
    id UUID PRIMARY KEY,
    name TEXT NOT NULL,
    key_hash TEXT NOT NULL UNIQUE,       -- sha256 des klartext-keys
    key_prefix TEXT NOT NULL,            -- "sk-llm-abc..." fuer Anzeige im Dashboard
    budget_cents BIGINT,                 -- optional: budget in cent (usd), NULL = unbegrenzt
    enabled BOOLEAN NOT NULL DEFAULT TRUE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_used_at TIMESTAMPTZ
);

-- Provider-Credentials (im Dashboard verwaltet).
CREATE TABLE IF NOT EXISTS providers (
    id UUID PRIMARY KEY,
    name TEXT NOT NULL,                  -- anzeigename, z.b. "openai-main"
    kind TEXT NOT NULL,                  -- 'openai' | 'anthropic' | 'gemini' | 'openai_compat'
    base_url TEXT NOT NULL,
    api_key_encrypted TEXT NOT NULL,     -- verschluesselt mit APP_SECRET
    enabled BOOLEAN NOT NULL DEFAULT TRUE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Modelle: ordnet modellnamen einem provider zu (routing) und haelt pricing vor.
CREATE TABLE IF NOT EXISTS models (
    id UUID PRIMARY KEY,
    provider_id UUID NOT NULL REFERENCES providers(id) ON DELETE CASCADE,
    model_name TEXT NOT NULL,            -- name, den der client nennt, z.b. "gpt-4o"
    upstream_model TEXT NOT NULL,        -- name beim provider, z.b. "gpt-4o-2024-08-06"
    input_price_per_million NUMERIC(12, 4) NOT NULL DEFAULT 0,   -- USD pro 1M input tokens
    output_price_per_million NUMERIC(12, 4) NOT NULL DEFAULT 0,  -- USD pro 1M output tokens
    enabled BOOLEAN NOT NULL DEFAULT TRUE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (model_name, provider_id)
);

-- Fallback-Ketten: geordnete alternative modelle pro model_name.
CREATE TABLE IF NOT EXISTS fallbacks (
    id UUID PRIMARY KEY,
    model_name TEXT NOT NULL,            -- model, dessen requests failover erhalten
    fallback_model_name TEXT NOT NULL,   -- alternative (muss in models existieren)
    priority INT NOT NULL DEFAULT 0,     -- kleinere priority = wird zuerst versucht
    enabled BOOLEAN NOT NULL DEFAULT TRUE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (model_name, fallback_model_name)
);

-- Admin-User fuer das Dashboard.
CREATE TABLE IF NOT EXISTS admin_users (
    id UUID PRIMARY KEY,
    username TEXT NOT NULL UNIQUE,
    password_hash TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Dashboard-Sessions.
CREATE TABLE IF NOT EXISTS sessions (
    token_hash TEXT PRIMARY KEY,         -- sha256 des session-cookies
    admin_user_id UUID NOT NULL REFERENCES admin_users(id) ON DELETE CASCADE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at TIMESTAMPTZ NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_models_model_name ON models(model_name);
CREATE INDEX IF NOT EXISTS idx_fallbacks_model_name ON fallbacks(model_name);
