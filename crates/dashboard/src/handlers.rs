//! Dashboard-API-Handler: Keys, Providers, Models, Fallbacks, Logs/Stats.

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::auth;
use common::state::AppState;
use chrono::{DateTime, Utc};
use std::collections::HashMap;

fn unauthorized() -> Response {
    (StatusCode::UNAUTHORIZED, Json(json!({"error": "unauthorized"}))).into_response()
}

fn server_error(e: impl std::fmt::Display) -> Response {
    tracing::error!("database error: {e}");
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({"error": "database error", "detail": e.to_string()})),
    )
        .into_response()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

// ============================================================
// Auth-Endpunkte
// ============================================================

#[derive(Deserialize)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
}

pub async fn login(
    State(state): State<AppState>,
    Json(req): Json<LoginRequest>,
) -> Response {
    #[derive(sqlx::FromRow)]
    struct PasswordHashRow {
        password_hash: String,
    }
    let user = sqlx::query_as::<_, PasswordHashRow>(
        "SELECT password_hash FROM admin_users WHERE username = $1",
    )
    .bind(&req.username)
    .fetch_optional(&state.pg)
    .await;

    let password_hash = match user {
        Ok(Some(u)) => u.password_hash,
        Ok(None) => {
            return (StatusCode::UNAUTHORIZED, Json(json!({"error": "invalid credentials"}))).into_response();
        }
        Err(e) => return server_error(e),
    };

    let valid = bcrypt::verify(&req.password, &password_hash).unwrap_or(false);
    if !valid {
        return (StatusCode::UNAUTHORIZED, Json(json!({"error": "invalid credentials"}))).into_response();
    }

    match auth::create_session(&state.pg, &req.username).await {
        Ok(token) => {
            let cookie = format!(
                "{}={}; Path=/; HttpOnly; SameSite=Strict; Secure; Max-Age={}",
                auth::SESSION_COOKIE,
                token,
                7 * 24 * 3600
            );
            (
                StatusCode::OK,
                [("set-cookie", cookie)],
                Json(json!({"ok": true})),
            )
                .into_response()
        }
        Err(e) => {
            tracing::error!("session creation failed: {e}");
            (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": "internal"}))).into_response()
        }
    }
}

pub async fn logout(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let _ = auth::delete_session(&state.pg, &headers).await;
    let cookie = format!("{}=; Path=/; HttpOnly; Secure; Max-Age=0", auth::SESSION_COOKIE);
    (
        StatusCode::OK,
        [("set-cookie", cookie)],
        Json(json!({"ok": true})),
    )
        .into_response()
}

pub async fn me(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if auth::require_session(&state, &headers).await.is_err() {
        return unauthorized();
    }
    Json(json!({"authenticated": true})).into_response()
}

// ============================================================
// Virtual Keys
// ============================================================

#[derive(sqlx::FromRow)]
struct KeyRow {
    id: Uuid,
    name: String,
    key_prefix: String,
    budget_cents: Option<i64>,
    enabled: bool,
    created_at: chrono::DateTime<chrono::Utc>,
    last_used_at: Option<chrono::DateTime<chrono::Utc>>,
    key_hint: Option<String>,
}

pub async fn list_keys(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if auth::require_session(&state, &headers).await.is_err() {
        return unauthorized();
    }
    let rows = sqlx::query_as::<_, KeyRow>(
        r#"
        SELECT id, name, key_prefix, budget_cents, enabled, created_at, last_used_at, key_hint
        FROM virtual_keys
        ORDER BY created_at DESC
        "#,
    )
    .fetch_all(&state.pg)
    .await;

    match rows {
        Ok(rows) => {
            let keys: Vec<_> = rows
                .into_iter()
                .map(|r| {
                    json!({
                        "id": r.id,
                        "name": r.name,
                        "key_prefix": r.key_prefix,
                        "budget_cents": r.budget_cents,
                        "enabled": r.enabled,
                        "created_at": r.created_at,
                        "last_used_at": r.last_used_at,
                        "key_hint": r.key_hint.unwrap_or_default(),
                    })
                })
                .collect();
            Json(json!({ "keys": keys })).into_response()
        }
        Err(e) => server_error(e),
    }
}

#[derive(Deserialize)]
pub struct CreateKeyRequest {
    pub name: String,
    pub budget_cents: Option<i64>,
}

pub async fn create_key(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<CreateKeyRequest>,
) -> Response {
    if auth::require_session(&state, &headers).await.is_err() {
        return unauthorized();
    }
    if req.name.trim().is_empty() {
        return (StatusCode::BAD_REQUEST, Json(json!({"error": "name required"}))).into_response();
    }

    // key generieren
    let mut raw = [0u8; 24];
    use rand::RngCore;
    rand::thread_rng().fill_bytes(&mut raw);
    let key = format!("sk-llm-{}", hex(&raw));
    let key_hash = common::auth::hash_key(&key);
    let key_prefix = format!("{}...", &key[..14]);
    let key_hint = key_hash[..8].to_string();
    // klartext verschluesselt persistieren (v2 AEAD mit APP_SECRET) fuer reveal
    let key_encrypted = common::crypto::encrypt(&key, &state.encryption_key);

    let res = sqlx::query(
        r#"
        INSERT INTO virtual_keys (id, name, key_hash, key_prefix, budget_cents, key_hint, key_encrypted)
        VALUES ($1, $2, $3, $4, $5, $6, $7)
        "#,
    )
    .bind(Uuid::new_v4())
    .bind(req.name.trim())
    .bind(&key_hash)
    .bind(&key_prefix)
    .bind(req.budget_cents)
    .bind(&key_hint)
    .bind(&key_encrypted)
    .execute(&state.pg)
    .await;

    match res {
        Ok(_) => Json(json!({ "key": key, "name": req.name.trim() })).into_response(),
        Err(e) => server_error(e),
    }
}

#[derive(Deserialize)]
pub struct UpdateKeyRequest {
    pub name: Option<String>,
    pub budget_cents: Option<Option<i64>>,
    pub enabled: Option<bool>,
}

pub async fn update_key(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(req): Json<UpdateKeyRequest>,
) -> Response {
    if auth::require_session(&state, &headers).await.is_err() {
        return unauthorized();
    }

    let existing = sqlx::query_as::<_, KeyRow>(
        "SELECT id, name, key_prefix, budget_cents, enabled, created_at, last_used_at, key_hint FROM virtual_keys WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(&state.pg)
    .await;
    let existing = match existing {
        Ok(Some(e)) => e,
        Ok(None) => return (StatusCode::NOT_FOUND, Json(json!({"error": "not found"}))).into_response(),
        Err(e) => return server_error(e),
    };

    let name = req.name.unwrap_or(existing.name);
    let enabled = req.enabled.unwrap_or(existing.enabled);
    let budget = match req.budget_cents {
        Some(b) => b,
        None => existing.budget_cents,
    };

    let res = sqlx::query(
        "UPDATE virtual_keys SET name = $1, budget_cents = $2, enabled = $3 WHERE id = $4",
    )
    .bind(&name)
    .bind(budget)
    .bind(enabled)
    .bind(id)
    .execute(&state.pg)
    .await;

    match res {
        Ok(_) => {
            // key-cache invalidieren (einfach: ganz leeren)
            state.key_cache.write().await.clear();
            Json(json!({"ok": true})).into_response()
        }
        Err(e) => server_error(e),
    }
}

pub async fn delete_key(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Response {
    if auth::require_session(&state, &headers).await.is_err() {
        return unauthorized();
    }
    let res = sqlx::query("DELETE FROM virtual_keys WHERE id = $1")
        .bind(id)
        .execute(&state.pg)
        .await;
    match res {
        Ok(_) => {
            state.key_cache.write().await.clear();
            Json(json!({"ok": true})).into_response()
        }
        Err(e) => server_error(e),
    }
}

#[derive(sqlx::FromRow)]
struct KeyHintRow {
    key_hint: String,
    key_encrypted: Option<String>,
}

#[derive(Deserialize)]
pub struct RevealKeyRequest {
    /// erster teil des key-hashes (aus der liste), als schutz gegen CSRF/versehentliches reveal
    pub key_hint: String,
}

/// Zeigt den klartext-key an. Der key liegt verschluesselt (v2-AEAD mit
/// APP_SECRET) in Postgres; alte keys ohne key_encrypted sind nicht anzeigbar.
/// Rate-limit: max. 10 reveals pro minute (global), sonst 429.
pub async fn reveal_key(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(req): Json<RevealKeyRequest>,
) -> Response {
    if auth::require_session(&state, &headers).await.is_err() {
        return unauthorized();
    }

    // simples rate-limit (fixed window, global)
    {
        const MAX_REVEALS_PER_MINUTE: u32 = 10;
        let mut count = REVEAL_COUNTER.lock().await;
        let now = std::time::Instant::now();
        if now.duration_since(count.window_start) >= std::time::Duration::from_secs(60) {
            count.window_start = now;
            count.count = 0;
        }
        if count.count >= MAX_REVEALS_PER_MINUTE {
            return (
                StatusCode::TOO_MANY_REQUESTS,
                Json(json!({"error": "too many reveal requests, try again later"})),
            )
                .into_response();
        }
        count.count += 1;
    }

    let row = sqlx::query_as::<_, KeyHintRow>(
        "SELECT key_hint, key_encrypted FROM virtual_keys WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(&state.pg)
    .await;
    let row = match row {
        Ok(Some(r)) => r,
        Ok(None) => return (StatusCode::NOT_FOUND, Json(json!({"error": "not found"}))).into_response(),
        Err(e) => return server_error(e),
    };

    // hint-validierung: verhindert reveal via CSRF oder versehentliche triggers
    if row.key_hint.is_empty() || row.key_hint != req.key_hint.trim() {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({"error": "invalid key_hint"})),
        )
            .into_response();
    }

    let encrypted = match row.key_encrypted {
        Some(e) if !e.is_empty() => e,
        _ => {
            return (
                StatusCode::GONE,
                Json(json!({"error": "key not available (created before reveal feature)"})),
            )
                .into_response()
        }
    };

    match common::crypto::decrypt(&encrypted, &state.encryption_key) {
        Ok(key) => Json(json!({ "key": key })).into_response(),
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": "decryption failed (APP_SECRET changed?)"})),
        )
            .into_response(),
    }
}

/// Globaler reveal-rate-limit-zustand (fixed window, lazy-initialisiert).
struct RevealCounter {
    window_start: std::time::Instant,
    count: u32,
}

static REVEAL_COUNTER: once_cell::sync::Lazy<tokio::sync::Mutex<RevealCounter>> =
    once_cell::sync::Lazy::new(|| {
        tokio::sync::Mutex::new(RevealCounter {
            window_start: std::time::Instant::now(),
            count: 0,
        })
    });

// ============================================================
// Providers
// ============================================================

#[derive(sqlx::FromRow)]
struct ProviderRow {
    id: Uuid,
    name: String,
    kind: String,
    base_url: String,
    enabled: bool,
    created_at: chrono::DateTime<chrono::Utc>,
    metrics_url: Option<String>,
}

pub async fn list_providers(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if auth::require_session(&state, &headers).await.is_err() {
        return unauthorized();
    }
    let rows = sqlx::query_as::<_, ProviderRow>(
        "SELECT id, name, kind, base_url, enabled, created_at, metrics_url FROM providers ORDER BY created_at",
    )
    .fetch_all(&state.pg)
    .await;

    match rows {
        Ok(rows) => {
            let providers: Vec<_> = rows
                .into_iter()
                .map(|r| {
                    json!({
                        "id": r.id,
                        "name": r.name,
                        "kind": r.kind,
                        "base_url": r.base_url,
                        "enabled": r.enabled,
                        "created_at": r.created_at,
                        "metrics_url": r.metrics_url,
                    })
                })
                .collect();
            Json(json!({ "providers": providers })).into_response()
        }
        Err(e) => server_error(e),
    }
}

#[derive(Deserialize)]
pub struct CreateProviderRequest {
    pub name: String,
    pub kind: String,
    pub base_url: String,
    pub api_key: String,
    pub metrics_url: Option<String>,
}

pub async fn create_provider(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<CreateProviderRequest>,
) -> Response {
    if auth::require_session(&state, &headers).await.is_err() {
        return unauthorized();
    }

    // kind validieren
    let kind = match providers::ProviderKind::parse(&req.kind) {
        Ok(k) => k,
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": "kind must be one of: openai, anthropic, gemini, openai_compat"})),
            )
                .into_response();
        }
    };

    // metrics-url: wenn explizit angegeben uebernehmen, andernfalls automatisch
    // aus der base-url discovern (falls ein /metrics-endpoint erreichbar ist).
    let metrics_url = match req.metrics_url.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        Some(u) => Some(u.to_string()),
        None => discover_metrics_url(&state, &kind, &req.base_url, &req.api_key).await,
    };

    let encrypted = common::crypto::encrypt(&req.api_key, &state.encryption_key);
    let res = sqlx::query(
        r#"
        INSERT INTO providers (id, name, kind, base_url, api_key_encrypted, metrics_url)
        VALUES ($1, $2, $3, $4, $5, $6)
        "#,
    )
    .bind(Uuid::new_v4())
    .bind(req.name.trim())
    .bind(&req.kind)
    .bind(req.base_url.trim())
    .bind(&encrypted)
    .bind(metrics_url.as_deref())
    .execute(&state.pg)
    .await;

    match res {
        Ok(_) => {
            let _ = state.reload_routes().await;
            Json(json!({"ok": true})).into_response()
        }
        Err(e) => server_error(e),
    }
}

pub async fn delete_provider(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Response {
    if auth::require_session(&state, &headers).await.is_err() {
        return unauthorized();
    }
    let res = sqlx::query("DELETE FROM providers WHERE id = $1")
        .bind(id)
        .execute(&state.pg)
        .await;
    match res {
        Ok(_) => {
            let _ = state.reload_routes().await;
            Json(json!({"ok": true})).into_response()
        }
        Err(e) => server_error(e),
    }
}

#[derive(Deserialize)]
pub struct RenameProviderRequest {
    pub name: String,
}

/// Aktuelle provider-namen aus postgres: provider_id -> name.
/// Wird benutzt, um in clickhouse-logs/aggregate nach einem rename den
/// aktuellen namen anzuzeigen (logs speichern den namen zum zeitpunkt
/// des requests + seit neuestem die provider_id).
async fn current_provider_names(state: &AppState) -> HashMap<Uuid, String> {
    match sqlx::query_as::<_, (Uuid, String)>("SELECT id, name FROM providers")
        .fetch_all(&state.pg)
        .await
    {
        Ok(rows) => rows.into_iter().collect(),
        Err(e) => {
            tracing::error!("failed to load provider names: {e}");
            HashMap::new()
        }
    }
}

/// Valldiert ein model-capabilities-json (freie metadata wie attachment,
/// modalities, max_content_length). Bekannte strukturen werden geprueft,
/// unbekannte keys sind erlaubt (zukunftssicher). Ok -> Some(Value), Fehler
/// -> Some(fehlermeldung).
fn validate_capabilities(v: &Value) -> Result<(), String> {
    let Some(obj) = v.as_object() else {
        return Err("capabilities must be a JSON object".to_string());
    };
    for (key, val) in obj {
        match key.as_str() {
            "attachment" => {
                if !val.is_boolean() {
                    return Err("capabilities.attachment must be a boolean".to_string());
                }
            }
            "modalities" => {
                let Some(mods) = val.as_object() else {
                    return Err("capabilities.modalities must be an object".to_string());
                };
                const KNOWN: &[&str] = &["text", "image", "audio", "video"];
                for (dir, list) in mods {
                    if dir != "input" && dir != "output" {
                        return Err(format!(
                            "capabilities.modalities: unknown direction \"{dir}\" (expected input/output)"
                        ));
                    }
                    let Some(items) = list.as_array() else {
                        return Err(format!("capabilities.modalities.{dir} must be an array"));
                    };
                    for item in items {
                        match item.as_str() {
                            Some(s) if KNOWN.contains(&s) => {}
                            _ => {
                                return Err(format!(
                                    "capabilities.modalities.{dir}: unknown modality {:?} (known: {})",
                                    item,
                                    KNOWN.join(", ")
                                ))
                            }
                        }
                    }
                }
            }
            "max_content_length" | "max_output_tokens" => {
                if !val.is_u64() {
                    return Err(format!("capabilities.{key} must be a non-negative integer"));
                }
            }
            _ => {} // freie keys erlaubt
        }
    }
    Ok(())
}

/// Bereinigt ein link-feld: trimmt, leere -> None. Praefix "https://" wird
/// ergaenzt, wenn das schema fehlt; ungueltige URLs -> Err.
fn clean_link(raw: &Option<String>) -> Result<Option<String>, String> {
    let Some(v) = raw.as_deref().map(str::trim).filter(|s| !s.is_empty()) else {
        return Ok(None);
    };
    let url = if v.starts_with("http://") || v.starts_with("https://") {
        v.to_string()
    } else {
        format!("https://{v}")
    };
    if url.contains(' ') || !url.contains('.') {
        return Err(format!("invalid link: {v}"));
    }
    Ok(Some(url))
}

/// Benennt einen provider um (nur der anzeigename; verbindungsdaten bleiben).
/// Der name erscheint in logs als provider_name - historische logs bleiben
/// unberuehrt, neue requests loggen den neuen namen.
pub async fn rename_provider(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(req): Json<RenameProviderRequest>,
) -> Response {
    if auth::require_session(&state, &headers).await.is_err() {
        return unauthorized();
    }
    let name = req.name.trim().to_string();
    if name.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "name must not be empty"})),
        )
            .into_response();
    }

    let res = sqlx::query("UPDATE providers SET name = $1 WHERE id = $2")
        .bind(&name)
        .bind(id)
        .execute(&state.pg)
        .await;

    match res {
        Ok(res) => {
            if res.rows_affected() == 0 {
                return (
                    StatusCode::NOT_FOUND,
                    Json(json!({"error": "provider not found"})),
                )
                    .into_response();
            }
            let _ = state.reload_routes().await;
            Json(json!({"ok": true, "name": name})).into_response()
        }
        Err(e) => server_error(e),
    }
}

#[derive(Deserialize)]
pub struct DiscoverMetricsRequest {
    pub kind: String,
    pub base_url: String,
    pub api_key: String,
}

/// Entdeckt eine metrics-url fuer die angegebenen daten (ohne db-zugriff).
pub async fn discover_metrics(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<DiscoverMetricsRequest>,
) -> Response {
    if auth::require_session(&state, &headers).await.is_err() {
        return unauthorized();
    }
    let kind = match providers::ProviderKind::parse(&req.kind) {
        Ok(k) => k,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": format!("unbekannter provider kind: {e}")})),
            )
                .into_response();
        }
    };
    let url = discover_metrics_url(&state, &kind, &req.base_url, &req.api_key).await;
    Json(json!({ "metrics_url": url })).into_response()
}

#[derive(Deserialize)]
pub struct UpdateProviderRequest {
    pub name: Option<String>,
    pub kind: Option<String>,
    pub base_url: Option<String>,
    pub api_key: Option<String>,
    pub metrics_url: Option<String>,
    pub enabled: Option<bool>,
}

/// Aktualisiert einen provider. Felder, die nicht uebergeben werden (None),
/// bleiben unveraendert. `api_key`: leer = unveraendert. `metrics_url`: "" = leeren.
pub async fn update_provider(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(req): Json<UpdateProviderRequest>,
) -> Response {
    if auth::require_session(&state, &headers).await.is_err() {
        return unauthorized();
    }

    #[derive(sqlx::FromRow)]
    struct ProvRow {
        name: String,
        kind: String,
        base_url: String,
        api_key_encrypted: String,
        metrics_url: Option<String>,
        enabled: bool,
    }

    let existing = match sqlx::query_as::<_, ProvRow>(
        "SELECT name, kind, base_url, api_key_encrypted, metrics_url, enabled FROM providers WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(&state.pg)
    .await
    {
        Ok(Some(r)) => r,
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({"error": "provider not found"})),
            )
                .into_response();
        }
        Err(e) => return server_error(e),
    };

    let name = match &req.name {
        Some(n) if !n.trim().is_empty() => n.trim().to_string(),
        Some(_) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": "name must not be empty"})),
            )
                .into_response();
        }
        None => existing.name,
    };

    let kind_str = match &req.kind {
        Some(k) => {
            if providers::ProviderKind::parse(k).is_err() {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(json!({"error": "kind must be one of: openai, anthropic, gemini, openai_compat"})),
                )
                    .into_response();
            }
            k.trim().to_string()
        }
        None => existing.kind,
    };

    let base_url = match &req.base_url {
        Some(b) if !b.trim().is_empty() => b.trim().to_string(),
        Some(_) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": "base_url must not be empty"})),
            )
                .into_response();
        }
        None => existing.base_url,
    };

    // api-key: nur neu verschluesseln, wenn ein nicht-leerer wert uebergeben wurde
    let api_key_encrypted = match &req.api_key {
        Some(k) if !k.trim().is_empty() => common::crypto::encrypt(k.trim(), &state.encryption_key),
        _ => existing.api_key_encrypted,
    };

    // metrics_url: some("url") = setzen, some("") = leeren, none = unveraendert
    let metrics_url: Option<String> = match &req.metrics_url {
        Some(m) => {
            let t = m.trim().to_string();
            if t.is_empty() {
                None
            } else {
                Some(t)
            }
        }
        None => existing.metrics_url.clone(),
    };

    let enabled = req.enabled.unwrap_or(existing.enabled);

    let res = sqlx::query(
        "UPDATE providers SET name = $1, kind = $2, base_url = $3, api_key_encrypted = $4, metrics_url = $5, enabled = $6 WHERE id = $7",
    )
    .bind(&name)
    .bind(&kind_str)
    .bind(&base_url)
    .bind(&api_key_encrypted)
    .bind(metrics_url.as_deref())
    .bind(enabled)
    .bind(id)
    .execute(&state.pg)
    .await;

    match res {
        Ok(res) => {
            if res.rows_affected() == 0 {
                return (
                    StatusCode::NOT_FOUND,
                    Json(json!({"error": "provider not found"})),
                )
                    .into_response();
            }
            let _ = state.reload_routes().await;
            Json(json!({"ok": true})).into_response()
        }
        Err(e) => server_error(e),
    }
}

// ============================================================
// Capabilities-Sync: Upstream-Modellliste anfragen und fehlende
// Felder in die manuell gepflegten Capabilities mergen.
// Manuelle Werte haben immer Vorrang.
// ============================================================

/// Baut die models-list-URL und die Auth-Header je nach Provider-Kind.
/// Auth-Header je nach Provider-Kind (fuer models- und metrics-requests).
fn upstream_auth_headers(
    kind: &providers::ProviderKind,
    api_key: &str,
) -> Vec<(&'static str, String)> {
    match kind {
        providers::ProviderKind::Anthropic => vec![
            ("x-api-key", api_key.to_string()),
            ("anthropic-version", "2023-06-01".to_string()),
        ],
        providers::ProviderKind::Gemini => Vec::new(),
        // OpenAI & OpenAI-kompatibel
        _ => vec![("Authorization", format!("Bearer {api_key}"))],
    }
}

fn upstream_models_request(
    kind: &providers::ProviderKind,
    base_url: &str,
    api_key: &str,
) -> (String, Vec<(&'static str, String)>) {
    let base = base_url.trim_end_matches('/');
    let hdrs = upstream_auth_headers(kind, api_key);
    let url = match kind {
        providers::ProviderKind::Anthropic => format!("{base}/v1/models"),
        providers::ProviderKind::Gemini => format!("{base}/models?key={api_key}"),
        // OpenAI & OpenAI-kompatibel
        _ => format!("{base}/models"),
    };
    (url, hdrs)
}

/// Kandidaten-URLs fuer ein /metrics-endpoint aus einer base-url ableiten.
/// Probiert zunaechst `{base}/metrics` und, falls die base auf `/v1` endet,
/// auch die URL ohne dieses Suffix (z.B. vLLM: /v1/metrics -> /metrics).
fn metrics_url_candidates(base_url: &str) -> Vec<String> {
    let base = base_url.trim().trim_end_matches('/');
    if base.is_empty() {
        return Vec::new();
    }
    let mut out = vec![format!("{base}/metrics")];
    if let Some(stripped) = base.strip_suffix("/v1") {
        out.push(format!("{stripped}/metrics"));
    }
    out
}

/// Entdeckt eine Prometheus-/metrics-endpoint-URL aus der base-url, falls
/// erreichbar und der body wie metrics aussieht. Liefert None, wenn nichts
/// gefunden wurde.
async fn discover_metrics_url(
    state: &AppState,
    kind: &providers::ProviderKind,
    base_url: &str,
    api_key: &str,
) -> Option<String> {
    let hdrs = upstream_auth_headers(kind, api_key);
    for url in metrics_url_candidates(base_url) {
        let mut req = state
            .http
            .get(&url)
            .timeout(std::time::Duration::from_secs(5));
        for (k, v) in &hdrs {
            req = req.header(*k, v.as_str());
        }
        let resp = match req.send().await {
            Ok(r) => r,
            Err(_) => continue,
        };
        if !resp.status().is_success() {
            continue;
        }
        let body: String = match resp.text().await {
            Ok(b) => b,
            Err(_) => continue,
        };
        if common::metrics::looks_like_prometheus(&body) {
            return Some(url);
        }
    }
    None
}

/// Extrahiert capability-Werte aus einem einzelnen Upstream-Modell-Objekt.
fn upstream_caps(kind: &providers::ProviderKind, m: &Value) -> Value {
    let mut map = serde_json::Map::new();
    let max_content = match kind {
        providers::ProviderKind::Anthropic => m.get("max_input_tokens").and_then(|v| v.as_u64()),
        providers::ProviderKind::Gemini => m.get("inputTokenLimit").and_then(|v| v.as_u64()),
        _ => m
            .get("context_length")
            .or_else(|| m.get("max_model_len"))
            .or_else(|| m.get("max_input_tokens"))
            .and_then(|v| v.as_u64()),
    };
    let max_output = match kind {
        providers::ProviderKind::Anthropic => m.get("max_tokens").and_then(|v| v.as_u64()),
        providers::ProviderKind::Gemini => m.get("outputTokenLimit").and_then(|v| v.as_u64()),
        _ => m.get("max_output_tokens").and_then(|v| v.as_u64()),
    };
    if let Some(v) = max_content {
        map.insert("max_content_length".to_string(), json!(v));
    }
    if let Some(v) = max_output {
        map.insert("max_output_tokens".to_string(), json!(v));
    }
    Value::Object(map)
}

/// Liest das Array der Upstream-Modell-Objekte aus der Antwort.
fn parse_upstream_models(kind: &providers::ProviderKind, resp: &Value) -> Option<Vec<Value>> {
    let arr = match kind {
        providers::ProviderKind::Gemini => resp.get("models"),
        _ => resp.get("data"),
    }?;
    arr.as_array().cloned()
}

/// Liefert alle Namen, unter denen ein Upstream-Modell auffindbar ist.
fn upstream_model_names(kind: &providers::ProviderKind, m: &Value) -> Option<Vec<String>> {
    let raw = match kind {
        providers::ProviderKind::Gemini => m.get("name").and_then(|v| v.as_str()),
        _ => m.get("id").and_then(|v| v.as_str()),
    }?;
    let mut names = vec![raw.to_string()];
    if let Some(stripped) = raw.strip_prefix("models/") {
        names.push(stripped.to_string());
    }
    Some(names)
}

/// Merge: manuelle Werte (manual) haben Vorrang, Upstream-Werte ergaenzen
/// nur fehlende Felder.
fn merge_caps(manual: &Option<Value>, upstream: &Value) -> Value {
    let mut merged = upstream.clone();
    if let (Some(mobj), Some(uobj)) = (
        merged.as_object_mut(),
        manual.as_ref().and_then(|v| v.as_object()),
    ) {
        for (k, v) in uobj {
            mobj.insert(k.clone(), v.clone());
        }
    }
    merged
}

pub async fn refresh_provider_capabilities(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Response {
    if auth::require_session(&state, &headers).await.is_err() {
        return unauthorized();
    }

    #[derive(sqlx::FromRow)]
    struct ProvRow {
        kind: String,
        base_url: String,
        api_key_encrypted: String,
        metrics_url: Option<String>,
    }

    let prov = match sqlx::query_as::<_, ProvRow>(
        "SELECT kind, base_url, api_key_encrypted, metrics_url FROM providers WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(&state.pg)
    .await
    {
        Ok(Some(r)) => r,
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({"error": "provider not found"})),
            )
                .into_response();
        }
        Err(e) => return server_error(e),
    };

    let kind = match providers::ProviderKind::parse(&prov.kind) {
        Ok(k) => k,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": format!("unbekannter provider kind: {e}")})),
            )
                .into_response();
        }
    };

    let api_key = match common::crypto::decrypt(&prov.api_key_encrypted, &state.encryption_key) {
        Ok(k) => k,
        Err(e) => return server_error(e),
    };

    // Im Nachhinein: falls noch keine metrics-url gesetzt, automatisch discovern
    let mut metrics_discovered: Option<String> = None;
    let needs_discovery = prov
        .metrics_url
        .as_deref()
        .map(str::trim)
        .map(|s| s.is_empty())
        .unwrap_or(true);
    if needs_discovery {
        if let Some(url) = discover_metrics_url(&state, &kind, &prov.base_url, &api_key).await {
            let _ = sqlx::query("UPDATE providers SET metrics_url = $1 WHERE id = $2")
                .bind(&url)
                .bind(id)
                .execute(&state.pg)
                .await;
            metrics_discovered = Some(url);
        }
    }

    let (url, hdrs) = upstream_models_request(&kind, &prov.base_url, &api_key);
    let mut req = state
        .http
        .get(&url)
        .timeout(std::time::Duration::from_secs(15));
    for (k, v) in &hdrs {
        req = req.header(*k, v.as_str());
    }
    let resp_json: Value = match req.send().await {
        Ok(r) => match r.json::<Value>().await {
            Ok(v) => v,
            Err(e) => {
                return (
                    StatusCode::BAD_GATEWAY,
                    Json(json!({"error": format!("upstream response parse failed: {e}")})),
                )
                    .into_response();
            }
        },
        Err(e) => {
            return (
                StatusCode::BAD_GATEWAY,
                Json(json!({"error": format!("upstream request failed: {e}")})),
            )
                .into_response();
        }
    };

    let upstream_models = match parse_upstream_models(&kind, &resp_json) {
        Some(v) => v,
        None => {
            return (
                StatusCode::BAD_GATEWAY,
                Json(json!({"error": "upstream model list not found in response"})),
            )
                .into_response();
        }
    };

    let mut lookup: HashMap<String, Value> = HashMap::new();
    for m in &upstream_models {
        let caps = upstream_caps(&kind, m);
        if let Some(names) = upstream_model_names(&kind, m) {
            for name in names {
                lookup.insert(name, caps.clone());
            }
        }
    }

    #[derive(sqlx::FromRow)]
    struct ModelCapRow {
        id: Uuid,
        model_name: String,
        upstream_model: String,
        capabilities: Option<Value>,
    }

    let rows = match sqlx::query_as::<_, ModelCapRow>(
        "SELECT id, model_name, upstream_model, capabilities \
         FROM models WHERE provider_id = $1 AND enabled = TRUE",
    )
    .bind(id)
    .fetch_all(&state.pg)
    .await
    {
        Ok(r) => r,
        Err(e) => return server_error(e),
    };

    let mut updated = Vec::new();
    let mut unchanged = Vec::new();
    let mut not_found = Vec::new();

    for row in &rows {
        let upstream = match lookup.get(&row.upstream_model) {
            Some(c) => c,
            None => {
                not_found.push(row.upstream_model.clone());
                continue;
            }
        };
        let merged = merge_caps(&row.capabilities, upstream);
        let changed = match &row.capabilities {
            None => !merged.as_object().map(|o| o.is_empty()).unwrap_or(true),
            Some(old) => *old != merged,
        };
        if changed {
            let is_null = merged.as_object().map(|o| o.is_empty()).unwrap_or(true);
            let new_val: Option<Value> = if is_null {
                None
            } else {
                Some(merged.clone())
            };
            if let Err(e) = sqlx::query("UPDATE models SET capabilities = $1 WHERE id = $2")
                .bind(new_val)
                .bind(row.id)
                .execute(&state.pg)
                .await
            {
                return server_error(e);
            }
            let added = merged
                .as_object()
                .map(|o| {
                    let mut added_map = o.clone();
                    if let Some(old) = row.capabilities.as_ref().and_then(|v| v.as_object()) {
                        for k in old.keys() {
                            added_map.remove(k);
                        }
                    }
                    Value::Object(added_map)
                })
                .unwrap_or(json!({}));
            updated.push(json!({ "model_name": row.model_name, "added": added }));
        } else {
            unchanged.push(row.model_name.clone());
        }
    }

    let _ = state.reload_routes().await;

    Json(json!({
        "ok": true,
        "provider_id": id,
        "fetched_models": upstream_models.len(),
        "updated": updated,
        "unchanged": unchanged,
        "not_found_upstream": not_found,
        "metrics_url_discovered": metrics_discovered,
    }))
    .into_response()
}

// ============================================================
// Models
// ============================================================

#[derive(sqlx::FromRow)]
struct ModelRow {
    id: Uuid,
    model_name: String,
    upstream_model: String,
    input_price_per_million: f64,
    output_price_per_million: f64,
    enabled: bool,
    provider_id: Uuid,
    provider_name: String,
    quantization: Option<String>,
    notes: Option<String>,
    link: Option<String>,
    capabilities: Option<Value>,
}

pub async fn list_models(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if auth::require_session(&state, &headers).await.is_err() {
        return unauthorized();
    }
    let rows = sqlx::query_as::<_, ModelRow>(
        r#"
        SELECT m.id, m.model_name, m.upstream_model,
               m.input_price_per_million::float8 AS input_price_per_million,
               m.output_price_per_million::float8 AS output_price_per_million,
               m.enabled, m.provider_id, p.name AS provider_name,
               m.quantization, m.notes, m.link, m.capabilities
        FROM models m
        JOIN providers p ON p.id = m.provider_id
        ORDER BY m.model_name
        "#,
    )
    .fetch_all(&state.pg)
    .await;

    match rows {
        Ok(rows) => {
            let models: Vec<_> = rows
                .into_iter()
                .map(|r| {
                    json!({
                        "id": r.id,
                        "model_name": r.model_name,
                        "upstream_model": r.upstream_model,
                        "input_price_per_million": r.input_price_per_million,
                        "output_price_per_million": r.output_price_per_million,
                        "enabled": r.enabled,
                        "provider_id": r.provider_id,
                        "provider_name": r.provider_name,
                        "quantization": r.quantization,
                        "notes": r.notes,
                        "link": r.link,
                        "capabilities": r.capabilities,
                    })
                })
                .collect();
            Json(json!({ "models": models })).into_response()
        }
        Err(e) => server_error(e),
    }
}

#[derive(Deserialize)]
pub struct CreateModelRequest {
    pub provider_id: Uuid,
    pub model_name: String,
    pub upstream_model: String,
    pub input_price_per_million: Option<f64>,
    pub output_price_per_million: Option<f64>,
    pub quantization: Option<String>,
    pub notes: Option<String>,
    pub link: Option<String>,
    pub capabilities: Option<Value>,
}

pub async fn create_model(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<CreateModelRequest>,
) -> Response {
    if auth::require_session(&state, &headers).await.is_err() {
        return unauthorized();
    }
    if let Some(caps) = &req.capabilities {
        if let Err(e) = validate_capabilities(caps) {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": e})),
            )
                .into_response();
        }
    }
    let link = match clean_link(&req.link) {
        Ok(l) => l,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": e})),
            )
                .into_response();
        }
    };
    let res = sqlx::query(
        r#"
        INSERT INTO models (id, provider_id, model_name, upstream_model, input_price_per_million, output_price_per_million, quantization, notes, link, capabilities)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
        "#,
    )
    .bind(Uuid::new_v4())
    .bind(req.provider_id)
    .bind(req.model_name.trim())
    .bind(req.upstream_model.trim())
    .bind(req.input_price_per_million.unwrap_or(0.0))
    .bind(req.output_price_per_million.unwrap_or(0.0))
    .bind(req.quantization.as_deref().map(str::trim).filter(|s| !s.is_empty()))
    .bind(req.notes.as_deref().map(str::trim).filter(|s| !s.is_empty()))
    .bind(link)
    .bind(req.capabilities)
    .execute(&state.pg)
    .await;

    match res {
        Ok(_) => {
            let _ = state.reload_routes().await;
            Json(json!({"ok": true})).into_response()
        }
        Err(e) => server_error(e),
    }
}

#[derive(Deserialize)]
pub struct UpdateModelRequest {
    pub provider_id: Option<Uuid>,
    pub upstream_model: Option<String>,
    pub input_price_per_million: Option<f64>,
    pub output_price_per_million: Option<f64>,
    pub quantization: Option<String>,
    pub notes: Option<String>,
    pub link: Option<String>,
    pub enabled: Option<bool>,
    pub capabilities: Option<Value>,
}

/// Aktualisiert ein modell. model_name bleibt unveraenderlich (clients und
/// fallback-ketten referenzieren ihn); alles andere ist editierbar.
pub async fn update_model(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(req): Json<UpdateModelRequest>,
) -> Response {
    if auth::require_session(&state, &headers).await.is_err() {
        return unauthorized();
    }
    if let Some(caps) = &req.capabilities {
        if let Err(e) = validate_capabilities(caps) {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": e})),
            )
                .into_response();
        }
    }
    let link = match clean_link(&req.link) {
        Ok(l) => l,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": e})),
            )
                .into_response();
        }
    };

    // trim helper: leere strings -> NULL bei optionalen textfeldern
    let clean = |s: &Option<String>| {
        s.as_ref()
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
    };

    let res = sqlx::query(
        r#"
        UPDATE models SET
            provider_id = COALESCE($1, provider_id),
            upstream_model = COALESCE($2, upstream_model),
            input_price_per_million = COALESCE($3, input_price_per_million),
            output_price_per_million = COALESCE($4, output_price_per_million),
            quantization = $5,
            notes = $6,
            link = $7,
            enabled = COALESCE($8, enabled),
            capabilities = $9
        WHERE id = $10
        "#,
    )
    .bind(req.provider_id)
    .bind(req.upstream_model.as_deref().map(str::trim).filter(|s| !s.is_empty()))
    .bind(req.input_price_per_million)
    .bind(req.output_price_per_million)
    .bind(clean(&req.quantization))
    .bind(clean(&req.notes))
    .bind(link)
    .bind(req.enabled)
    .bind(req.capabilities)
    .bind(id)
    .execute(&state.pg)
    .await;

    match res {
        Ok(res) => {
            if res.rows_affected() == 0 {
                return (
                    StatusCode::NOT_FOUND,
                    Json(json!({"error": "model not found"})),
                )
                    .into_response();
            }
            let _ = state.reload_routes().await;
            Json(json!({"ok": true})).into_response()
        }
        Err(e) => server_error(e),
    }
}

pub async fn delete_model(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Response {
    if auth::require_session(&state, &headers).await.is_err() {
        return unauthorized();
    }
    let res = sqlx::query("DELETE FROM models WHERE id = $1")
        .bind(id)
        .execute(&state.pg)
        .await;
    match res {
        Ok(_) => {
            let _ = state.reload_routes().await;
            Json(json!({"ok": true})).into_response()
        }
        Err(e) => server_error(e),
    }
}

// ============================================================
// Fallbacks
// ============================================================

#[derive(sqlx::FromRow)]
struct FallbackRow {
    id: Uuid,
    model_name: String,
    fallback_model_name: String,
    priority: i32,
    enabled: bool,
}

pub async fn list_fallbacks(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if auth::require_session(&state, &headers).await.is_err() {
        return unauthorized();
    }
    let rows = sqlx::query_as::<_, FallbackRow>(
        "SELECT id, model_name, fallback_model_name, priority, enabled FROM fallbacks ORDER BY model_name, priority",
    )
    .fetch_all(&state.pg)
    .await;

    match rows {
        Ok(rows) => {
            let fallbacks: Vec<_> = rows
                .into_iter()
                .map(|r| {
                    json!({
                        "id": r.id,
                        "model_name": r.model_name,
                        "fallback_model_name": r.fallback_model_name,
                        "priority": r.priority,
                        "enabled": r.enabled,
                    })
                })
                .collect();
            Json(json!({ "fallbacks": fallbacks })).into_response()
        }
        Err(e) => server_error(e),
    }
}

#[derive(Deserialize)]
pub struct CreateFallbackRequest {
    pub model_name: String,
    pub fallback_model_name: String,
    pub priority: Option<i32>,
}

pub async fn create_fallback(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<CreateFallbackRequest>,
) -> Response {
    if auth::require_session(&state, &headers).await.is_err() {
        return unauthorized();
    }
    let res = sqlx::query(
        r#"
        INSERT INTO fallbacks (id, model_name, fallback_model_name, priority)
        VALUES ($1, $2, $3, $4)
        "#,
    )
    .bind(Uuid::new_v4())
    .bind(req.model_name.trim())
    .bind(req.fallback_model_name.trim())
    .bind(req.priority.unwrap_or(0))
    .execute(&state.pg)
    .await;

    match res {
        Ok(_) => {
            let _ = state.reload_routes().await;
            Json(json!({"ok": true})).into_response()
        }
        Err(e) => server_error(e),
    }
}

pub async fn delete_fallback(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Response {
    if auth::require_session(&state, &headers).await.is_err() {
        return unauthorized();
    }
    let res = sqlx::query("DELETE FROM fallbacks WHERE id = $1")
        .bind(id)
        .execute(&state.pg)
        .await;
    match res {
        Ok(_) => {
            let _ = state.reload_routes().await;
            Json(json!({"ok": true})).into_response()
        }
        Err(e) => server_error(e),
    }
}

// ============================================================
// Logs & Stats (ClickHouse)
// ============================================================

#[derive(Deserialize)]
pub struct LogsQuery {
    pub key_name: Option<String>,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub status: Option<u16>,
    pub hours: Option<u32>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
}

pub async fn list_logs(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<LogsQuery>,
) -> Response {
    if auth::require_session(&state, &headers).await.is_err() {
        return unauthorized();
    }

    let hours = q.hours.unwrap_or(24).min(24 * 30);
    let limit = q.limit.unwrap_or(50).min(500);
    let offset = q.offset.unwrap_or(0);

    // provider-namen aus postgres: nach einem rename zeigen auch historische
    // logs den aktuellen namen (aufloesung ueber provider_id)
    let provider_names = current_provider_names(&state).await;

    // ClickHouse named-bindings: platzhalter {name:Type} werden durch bind()-werte
    // in reihenfolge ersetzt. Wir bauen platzhalter + binds konsistent auf.
    let mut where_clauses = vec!["timestamp >= now() - INTERVAL ? HOUR".to_string()];
    if q.key_name.is_some() {
        where_clauses.push("key_name = ?".to_string());
    }
    if q.provider.is_some() {
        where_clauses.push("provider = ?".to_string());
    }
    if q.model.is_some() {
        where_clauses.push("(model = ? OR original_model = ?)".to_string());
    }
    if q.status.is_some() {
        where_clauses.push("status = ?".to_string());
    }

    let where_sql = where_clauses.join(" AND ");

    let sql = format!(
        r#"
        SELECT id, request_id, timestamp, key_name, provider, provider_name, provider_id,
               model, original_model, attempts_made, is_fallback, upstream_model, endpoint, status, error_type, is_stream,
               prompt_tokens, completion_tokens, cost_usd, duration_ms, first_byte_ms
        FROM yalr.request_logs
        WHERE {where_sql}
        ORDER BY timestamp DESC
        LIMIT {limit_} OFFSET {offset_}
        "#,
        limit_ = limit,
        offset_ = offset,
    );

    let count_sql = format!(
        r#"
        SELECT count()
        FROM yalr.request_logs
        WHERE {where_sql}
        "#,
    );

    let mut query = state.ch.query(&sql).bind(hours);
    if let Some(key) = &q.key_name {
        query = query.bind(key.clone());
    }
    if let Some(provider) = &q.provider {
        query = query.bind(provider.clone());
    }
    if let Some(model) = &q.model {
        // Filter "(model = ? OR original_model = ?)" -> zwei Binds pro Filter
        query = query.bind(model.clone()).bind(model.clone());
    }
    if let Some(status) = &q.status {
        query = query.bind(status);
    }

    let mut count_query = state.ch.query(&count_sql).bind(hours);
    if let Some(key) = &q.key_name {
        count_query = count_query.bind(key.clone());
    }
    if let Some(provider) = &q.provider {
        count_query = count_query.bind(provider.clone());
    }
    if let Some(model) = &q.model {
        count_query = count_query.bind(model.clone()).bind(model.clone());
    }
    if let Some(status) = &q.status {
        count_query = count_query.bind(status);
    }

    #[derive(clickhouse::Row, serde::Serialize, serde::Deserialize)]
    struct LogRow {
        #[serde(with = "clickhouse::serde::uuid")]
        id: Uuid,
        request_id: String,
        #[serde(with = "clickhouse::serde::chrono::datetime64::millis")]
        timestamp: chrono::DateTime<chrono::Utc>,
        key_name: String,
        provider: String,
        provider_name: String,
        #[serde(with = "clickhouse::serde::uuid::option")]
        provider_id: Option<Uuid>,
        model: String,
        original_model: String,
        attempts_made: u8,
        is_fallback: bool,
        upstream_model: String,
        endpoint: String,
        status: u16,
        error_type: String,
        is_stream: bool,
        prompt_tokens: u64,
        completion_tokens: u64,
        cost_usd: f64,
        duration_ms: u64,
        first_byte_ms: u64,
    }

    let total: u64 = match count_query.fetch_one::<u64>().await {
        Ok(t) => t,
        Err(e) => {
            tracing::error!("clickhouse count failed: {e}");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "query failed", "detail": e.to_string()})),
            )
                .into_response();
        }
    };

    match query.fetch_all::<LogRow>().await {
        Ok(rows) => {
            let logs: Vec<_> = rows
                .into_iter()
                .map(|r| {
                    json!({
                        "id": r.id,
                        "request_id": r.request_id,
                        "timestamp": r.timestamp,
                        "key_name": r.key_name,
                        "provider": r.provider,
                        "provider_name": r.provider_id
                            .and_then(|id| provider_names.get(&id).cloned())
                            .unwrap_or(r.provider_name.clone()),
                        "provider_id": r.provider_id,
                        "model": r.model,
                        "original_model": r.original_model,
                        "attempts_made": r.attempts_made,
                        "is_fallback": r.is_fallback,
                        "upstream_model": r.upstream_model,
                        "endpoint": r.endpoint,
                        "status": r.status,
                        "error_type": r.error_type,
                        "is_stream": r.is_stream,
                        "prompt_tokens": r.prompt_tokens,
                        "completion_tokens": r.completion_tokens,
                        "cost_usd": r.cost_usd,
                        "duration_ms": r.duration_ms,
                        "first_byte_ms": r.first_byte_ms,
                    })
                })
                .collect();
            Json(json!({ "logs": logs, "total": total })).into_response()
        }
        Err(e) => {
            tracing::error!("clickhouse query failed: {e}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "query failed", "detail": e.to_string()})),
            )
                .into_response()
        }
    }
}

pub async fn get_log(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Response {
    if auth::require_session(&state, &headers).await.is_err() {
        return unauthorized();
    }

    #[derive(clickhouse::Row, serde::Serialize, serde::Deserialize)]
    struct LogDetailRow {
        #[serde(with = "clickhouse::serde::uuid")]
        id: Uuid,
        request_id: String,
        #[serde(with = "clickhouse::serde::chrono::datetime64::millis")]
        timestamp: chrono::DateTime<chrono::Utc>,
        key_name: String,
        provider: String,
        provider_name: String,
        #[serde(with = "clickhouse::serde::uuid::option")]
        provider_id: Option<Uuid>,
        model: String,
        original_model: String,
        attempts_made: u8,
        is_fallback: bool,
        upstream_model: String,
        endpoint: String,
        status: u16,
        error_message: String,
        error_type: String,
        is_stream: bool,
        prompt_tokens: u64,
        completion_tokens: u64,
        cost_usd: f64,
        duration_ms: u64,
        first_byte_ms: u64,
        request_body: String,
        response_body: String,
        request_truncated: bool,
        response_truncated: bool,
    }

    let provider_names = current_provider_names(&state).await;
    let result = state
        .ch
        .query(
            r#"
            SELECT id, request_id, timestamp, key_name, provider, provider_name, provider_id,
                   model, original_model, attempts_made, is_fallback, upstream_model, endpoint, status, error_message, error_type,
                   is_stream, prompt_tokens, completion_tokens, cost_usd,
                   duration_ms, first_byte_ms, request_body, response_body,
                   request_truncated, response_truncated
            FROM yalr.request_logs WHERE id = ?
            "#,
        )
        .bind(id)
        .fetch_one::<LogDetailRow>()
        .await;

    match result {
        Ok(row) => {
            let mut value = serde_json::to_value(&row).unwrap_or_default();
            if let Some(pid) = row.provider_id {
                if let Some(name) = provider_names.get(&pid) {
                    if let Some(obj) = value.as_object_mut() {
                        obj.insert("provider_name".to_string(), json!(name));
                    }
                }
            }
            Json(value).into_response()
        }
        Err(_) => (StatusCode::NOT_FOUND, Json(json!({"error": "not found"}))).into_response(),
    }
}

// ============================================================
// Overview-Endpunkte (stats/timeseries/breakdown): Stunden-Parameter
// ============================================================

/// Obergrenze fuer `hours` in Stunden: 1 Jahr.
const HOURS_MAX: u32 = 24 * 365;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HoursRange {
    /// Zeitraum in Stunden (1..=HOURS_MAX).
    Hours(u32),
    /// "all" -> seit Aufzeichnung, kein unterer Zeitfilter.
    All,
}

/// Parsen des `hours`-Query-Parameters.
/// - fehlend/leer -> Default 24h
/// - "all" (case-insensitiv) -> seit Aufzeichnung
/// - positive ganze Zahl 1..=HOURS_MAX -> Zeitraum in Stunden
/// - alles andere (0, negativ, nicht-numerisch, >HOURS_MAX) -> Fehler
fn parse_hours_param(raw: Option<&str>) -> Result<HoursRange, &'static str> {
    let s = raw.unwrap_or("").trim();
    if s.is_empty() {
        return Ok(HoursRange::Hours(24));
    }
    if s.eq_ignore_ascii_case("all") {
        return Ok(HoursRange::All);
    }
    match s.parse::<u32>() {
        Ok(h) if (1..=HOURS_MAX).contains(&h) => Ok(HoursRange::Hours(h)),
        _ => Err("hours muss eine positive ganze Zahl (1-8760) sein oder 'all'"),
    }
}

/// Kleinstes Bucket aus [1,3,6,12,24,168,336,720] Stunden, bei dem
/// ceil(span/bucket) <= 720 Punkte bleibt. Fallback: groesstes Bucket (720).
fn pick_bucket_hours(span_hours: u32) -> u32 {
    const BUCKETS: [u32; 8] = [1, 3, 6, 12, 24, 168, 336, 720];
    for &b in &BUCKETS {
        // ceil-division overflow-sicher: (span-1)/b + 1
        let points = (span_hours - 1) / b + 1;
        if points <= 720 {
            return b;
        }
    }
    720
}

fn bad_request(msg: &str) -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(json!({"error": "bad request", "detail": msg})),
    )
        .into_response()
}

#[derive(Deserialize)]
pub struct StatsQuery {
    pub key_name: Option<String>,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub hours: Option<String>,
}

pub async fn stats(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<StatsQuery>,
) -> Response {
    if auth::require_session(&state, &headers).await.is_err() {
        return unauthorized();
    }
    let range = match parse_hours_param(q.hours.as_deref()) {
        Ok(r) => r,
        Err(msg) => return bad_request(msg),
    };

    let mut where_clauses: Vec<String> = Vec::new();
    // bei "all" (HoursRange::All) entfaellt der untere Zeitfilter komplett
    if let HoursRange::Hours(_) = range {
        where_clauses.push("timestamp >= now() - INTERVAL ? HOUR".to_string());
    }
    if q.key_name.is_some() {
        where_clauses.push("key_name = ?".to_string());
    }
    if q.provider.is_some() {
        where_clauses.push("provider = ?".to_string());
    }
    if q.model.is_some() {
        where_clauses.push("(model = ? OR original_model = ?)".to_string());
    }
    let where_sql = if where_clauses.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", where_clauses.join(" AND "))
    };

    let sql = format!(
        r#"
        SELECT
            count() AS total_requests,
            countIf(status >= 200 AND status < 300) AS success_requests,
            countIf(status >= 400) AS error_requests,
            sum(cost_usd) AS total_cost,
            sum(prompt_tokens) AS total_prompt_tokens,
            sum(completion_tokens) AS total_completion_tokens,
            sum(duration_ms) / greatest(count(), 1) AS avg_duration_ms,
            countIf(is_fallback) AS fallback_count,
            toFloat64(countIf(is_fallback AND status >= 200 AND status < 300))
                / greatest(countIf(status >= 200 AND status < 300), 1) AS fallback_rate
        FROM yalr.request_logs
        {where_sql}
        "#
    );

    let mut query = state.ch.query(&sql);
    if let HoursRange::Hours(h) = range {
        query = query.bind(h);
    }
    if let Some(key) = &q.key_name {
        query = query.bind(key.clone());
    }
    if let Some(provider) = &q.provider {
        query = query.bind(provider.clone());
    }
    if let Some(model) = &q.model {
        // Filter "(model = ? OR original_model = ?)" -> zwei Binds pro Filter
        query = query.bind(model.clone()).bind(model.clone());
    }

    #[derive(clickhouse::Row, serde::Serialize, serde::Deserialize)]
    struct StatsRow {
        total_requests: u64,
        success_requests: u64,
        error_requests: u64,
        total_cost: f64,
        total_prompt_tokens: u64,
        total_completion_tokens: u64,
        avg_duration_ms: f64,
        fallback_count: u64,
        fallback_rate: f64,
    }

    match query.fetch_one::<StatsRow>().await {
        Ok(row) => Json(serde_json::to_value(&row).unwrap_or_default()).into_response(),
        Err(e) => {
            tracing::error!("clickhouse stats failed: {e}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "query failed"})),
            )
                .into_response()
        }
    }
}

#[derive(Deserialize)]
pub struct TimeseriesQuery {
    pub key_name: Option<String>,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub hours: Option<String>,
}

/// Kosten/requests pro stunde (fuer charts).
pub async fn timeseries(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<TimeseriesQuery>,
) -> Response {
    if auth::require_session(&state, &headers).await.is_err() {
        return unauthorized();
    }
    let range = match parse_hours_param(q.hours.as_deref()) {
        Ok(r) => r,
        Err(msg) => return bad_request(msg),
    };

    // Spanne in Stunden bestimmen + ggf. unteren Zeitfilter setzen
    let mut where_clauses: Vec<String> = Vec::new();
    let span_hours: u32 = match range {
        HoursRange::Hours(h) => {
            where_clauses.push("timestamp >= now() - INTERVAL ? HOUR".to_string());
            h
        }
        HoursRange::All => {
            // kein unterer Zeitfilter; Spanne = seit Aufzeichnung.
            // min() über non-nullable DateTime64 liefert bei leerer Menge den
            // Epochen-Default statt NULL -> count() als sicheres Leer-Kriterium.
            #[derive(clickhouse::Row, serde::Serialize, serde::Deserialize)]
            struct MinRow {
                #[serde(with = "clickhouse::serde::chrono::datetime64::millis")]
                earliest: chrono::DateTime<chrono::Utc>,
                total: u64,
            }
            // dieselben Filter wie die Haupt-Query, aber ohne Zeitbedingung (es gibt bei "all" keine)
            let mut min_clauses: Vec<String> = Vec::new();
            if q.key_name.is_some() {
                min_clauses.push("key_name = ?".to_string());
            }
            if q.provider.is_some() {
                min_clauses.push("provider = ?".to_string());
            }
            if q.model.is_some() {
                min_clauses.push("(model = ? OR original_model = ?)".to_string());
            }
            let min_where = if min_clauses.is_empty() {
                String::new()
            } else {
                format!("WHERE {}", min_clauses.join(" AND "))
            };
            let min_sql = format!(
                r#"SELECT min(timestamp) AS earliest, count() AS total FROM yalr.request_logs {min_where}"#
            );
            let mut min_query = state.ch.query(&min_sql);
            if let Some(key) = &q.key_name {
                min_query = min_query.bind(key.clone());
            }
            if let Some(provider) = &q.provider {
                min_query = min_query.bind(provider.clone());
            }
            if let Some(model) = &q.model {
                // Filter "(model = ? OR original_model = ?)" -> zwei Binds pro Filter
                min_query = min_query.bind(model.clone()).bind(model.clone());
            }
            let min_row = match min_query.fetch_one::<MinRow>().await {
                Ok(r) => r,
                Err(e) => {
                    tracing::error!("clickhouse timeseries min failed: {e}");
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(json!({"error": "query failed"})),
                    )
                        .into_response();
                }
            };
            if min_row.total == 0 {
                // keine Aufzeichnung (für diese Filter) -> leere Response in der bisherigen Form
                return Json(json!({ "timeseries": [] })).into_response();
            }
            let now = chrono::Utc::now();
            let secs = now.signed_duration_since(min_row.earliest).num_seconds();
            // ceil auf Stunden, mindestens 1
            ((secs.max(0) + 3599) / 3600).max(1) as u32
        }
    };

    if q.key_name.is_some() {
        where_clauses.push("key_name = ?".to_string());
    }
    if q.provider.is_some() {
        where_clauses.push("provider = ?".to_string());
    }
    if q.model.is_some() {
        where_clauses.push("(model = ? OR original_model = ?)".to_string());
    }
    let where_sql = if where_clauses.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", where_clauses.join(" AND "))
    };

    let bucket = pick_bucket_hours(span_hours);
    // bucket ist ein u32 aus der festen Liste [1,3,6,12,24,168,336,720].
    // `INTERVAL ? HOUR` ist mit der clickhouse-Crate nicht bindbar, daher
    // wird der Wert direkt in den SQL-String formatiert. Keine
    // Injektionsgefahr: der Wert stammt ausschliesslich aus der festen Liste.
    let sql = format!(
        r#"
        SELECT
            toDateTime64(toStartOfInterval(timestamp, INTERVAL {bucket} HOUR), 3) AS bucket,
            count() AS requests,
            sum(cost_usd) AS cost,
            sum(duration_ms) / greatest(count(), 1) AS avg_duration_ms
        FROM yalr.request_logs
        {where_sql}
        GROUP BY bucket
        ORDER BY bucket
        "#
    );

    let mut query = state.ch.query(&sql);
    if let HoursRange::Hours(h) = range {
        query = query.bind(h);
    }
    if let Some(key) = &q.key_name {
        query = query.bind(key.clone());
    }
    if let Some(provider) = &q.provider {
        query = query.bind(provider.clone());
    }
    if let Some(model) = &q.model {
        // Filter "(model = ? OR original_model = ?)" -> zwei Binds pro Filter
        query = query.bind(model.clone()).bind(model.clone());
    }

    #[derive(clickhouse::Row, serde::Serialize, serde::Deserialize)]
    struct TsRow {
        #[serde(with = "clickhouse::serde::chrono::datetime64::millis")]
        bucket: chrono::DateTime<chrono::Utc>,
        requests: u64,
        cost: f64,
        avg_duration_ms: f64,
    }

    match query.fetch_all::<TsRow>().await {
        Ok(rows) => {
            let series: Vec<_> = rows
                .into_iter()
                .map(|r| {
                    json!({
                        "bucket": r.bucket,
                        "requests": r.requests,
                        "cost": r.cost,
                        "avg_duration_ms": r.avg_duration_ms,
                    })
                })
                .collect();
            Json(json!({ "timeseries": series })).into_response()
        }
        Err(e) => {
            tracing::error!("clickhouse timeseries failed: {e}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "query failed"})),
            )
                .into_response()
        }
    }
}

/// Kostenaufstellung pro key / model / provider (aggregiert).
#[derive(Deserialize)]
pub struct GroupByQuery {
    pub group_by: Option<String>,
    pub hours: Option<String>,
    pub key_name: Option<String>,
}

pub async fn breakdown(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<GroupByQuery>,
) -> Response {
    if auth::require_session(&state, &headers).await.is_err() {
        return unauthorized();
    }
    let range = match parse_hours_param(q.hours.as_deref()) {
        Ok(r) => r,
        Err(msg) => return bad_request(msg),
    };
    // group_col: einzelne spalte | model_provider: kombi aus beiden feldern
    let (group_col, with_provider) = match q.group_by.as_deref() {
        Some("provider") => ("provider", false),
        Some("model") => ("model", false),
        Some("model_provider") => ("model", true),
        _ => ("key_name", false),
    };

    #[derive(clickhouse::Row, serde::Serialize, serde::Deserialize)]
    struct GroupRow {
        group_name: String,
        provider_name: String,
        #[serde(with = "clickhouse::serde::uuid::option")]
        provider_id: Option<Uuid>,
        requests: u64,
        cost: f64,
        tokens: u64,
    }

    let mut where_clauses: Vec<String> = Vec::new();
    if let HoursRange::Hours(_) = range {
        where_clauses.push("timestamp >= now() - INTERVAL ? HOUR".to_string());
    }
    if q.key_name.is_some() {
        where_clauses.push("key_name = ?".to_string());
    }
    let where_sql = if where_clauses.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", where_clauses.join(" AND "))
    };

    let sql = if with_provider {
        format!(
            r#"
            SELECT
                {group_col} AS group_name,
                provider_name AS provider_name,
                any(provider_id) AS provider_id,
                count() AS requests,
                sum(cost_usd) AS cost,
                sum(prompt_tokens + completion_tokens) AS tokens
            FROM yalr.request_logs
            {where_sql}
            GROUP BY group_name, provider_name
            ORDER BY cost DESC
            LIMIT 100
            "#
        )
    } else {
        format!(
            r#"
            SELECT
                {group_col} AS group_name,
                '' AS provider_name,
                NULL AS provider_id,
                count() AS requests,
                sum(cost_usd) AS cost,
                sum(prompt_tokens + completion_tokens) AS tokens
            FROM yalr.request_logs
            {where_sql}
            GROUP BY group_name
            ORDER BY cost DESC
            LIMIT 100
            "#
        )
    };

    let mut query = state.ch.query(&sql);
    if let HoursRange::Hours(h) = range {
        query = query.bind(h);
    }
    if let Some(key) = &q.key_name {
        query = query.bind(key.clone());
    }

    let provider_names = current_provider_names(&state).await;

    match query.fetch_all::<GroupRow>().await {
        Ok(rows) => {
            // rename-aufloesung kann mehrere (group, provider)-zeilen auf denselben
            // namen abbilden - desshalb nach dem mappen nach group+name neu aggregieren
            let mut items: Vec<serde_json::Value> = Vec::new();
            for r in rows {
                let name = r
                    .provider_id
                    .and_then(|id| provider_names.get(&id).cloned())
                    .unwrap_or_else(|| r.provider_name.clone());
                match items.iter_mut().find(|it| {
                    it["group"] == r.group_name.as_str() && it["provider_name"] == name.as_str()
                }) {
                    Some(existing) => {
                        let prev_req = existing["requests"].as_u64().unwrap_or(0);
                        let prev_cost = existing["cost"].as_f64().unwrap_or(0.0);
                        let prev_tokens = existing["tokens"].as_u64().unwrap_or(0);
                        existing["requests"] = json!(prev_req + r.requests);
                        existing["cost"] = json!(prev_cost + r.cost);
                        existing["tokens"] = json!(prev_tokens + r.tokens);
                    }
                    None => items.push(json!({
                        "group": r.group_name,
                        "provider_name": name,
                        "requests": r.requests,
                        "cost": r.cost,
                        "tokens": r.tokens,
                    })),
                }
            }
            Json(json!({ "breakdown": items })).into_response()
        }
        Err(e) => {
            tracing::error!("clickhouse breakdown failed: {e}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "query failed"})),
            )
                .into_response()
        }
    }
}

// ============================================================
// Live Provider Stats (ClickHouse, kurzzeit-fenster)
// ============================================================

/// Akkumulierte Summen pro Provider-Name fuer die Sigma/Summe-Raten-Berechnung.
#[derive(Default, Clone, Copy, Debug, PartialEq)]
struct RateSums {
    completion: u64,
    decode_ms: u64,
    prompt: u64,
    prefill_ms: u64,
}

/// Summiert die vier Sums pro aufgeloestem Provider-Namen auf (Rename-Merge).
/// Eingabe: Iterator ueber (name, completion, decode_ms, prompt, prefill_ms).
fn merge_rate_sums<I>(rows: I) -> std::collections::HashMap<String, RateSums>
where
    I: IntoIterator<Item = (String, u64, u64, u64, u64)>,
{
    let mut map: std::collections::HashMap<String, RateSums> = std::collections::HashMap::new();
    for (name, completion, decode_ms, prompt, prefill_ms) in rows {
        let s = map.entry(name).or_default();
        s.completion += completion;
        s.decode_ms += decode_ms;
        s.prompt += prompt;
        s.prefill_ms += prefill_ms;
    }
    map
}

/// Guard-Division: decode_tps / prefill_tps aus Summen.
/// Liefert None, wenn Nenner oder Zäehler 0 ist.
fn rates_from_sums(s: &RateSums) -> (Option<f64>, Option<f64>) {
    let decode = if s.decode_ms > 0 && s.completion > 0 {
        Some(s.completion as f64 / s.decode_ms as f64 * 1000.0)
    } else {
        None
    };
    let prefill = if s.prefill_ms > 0 && s.prompt > 0 {
        Some(s.prompt as f64 / s.prefill_ms as f64 * 1000.0)
    } else {
        None
    };
    (decode, prefill)
}

/// Filtert das in-flight-Snapshot nach key_name, falls ein Filter gesetzt ist.
fn filter_in_flight(
    snapshot: Vec<ingest::InFlightReq>,
    key_name: &Option<String>,
) -> Vec<ingest::InFlightReq> {
    match key_name {
        Some(k) => snapshot.into_iter().filter(|r| &r.key_name == k).collect(),
        None => snapshot,
    }
}

#[derive(Deserialize)]
pub struct LiveStatsQuery {
    /// aggregations-fenster in sekunden (whitelist: 30, 60, 300, 3600)
    pub window: Option<u32>,
    pub key_name: Option<String>,
}

pub async fn live_stats(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<LiveStatsQuery>,
) -> Response {
    if auth::require_session(&state, &headers).await.is_err() {
        return unauthorized();
    }

    let window = match q.window {
        Some(30) | Some(60) | Some(300) | Some(3600) => q.window.unwrap(),
        _ => 60,
    };

    let mut where_clauses = vec!["timestamp >= now() - INTERVAL ? SECOND".to_string()];
    if q.key_name.is_some() {
        where_clauses.push("key_name = ?".to_string());
    }
    let where_sql = where_clauses.join(" AND ");

    let sql = format!(
        r#"
        SELECT
            provider,
            provider_name,
            anyIf(provider_id, provider_id IS NOT NULL) AS provider_id,
            count() AS reqs,
            countIf(status >= 400) AS errors,
            avg(nullIf(first_byte_ms, 0)) AS avg_ttft,
            quantile(0.5)(duration_ms) AS p50,
            quantile(0.95)(duration_ms) AS p95,
            sum(cost_usd) AS cost_usd
        FROM yalr.request_logs
        WHERE {where_sql}
        GROUP BY provider, provider_name
        ORDER BY reqs DESC
        "#
    );

    let mut query = state.ch.query(&sql).bind(window);
    if let Some(key) = &q.key_name {
        query = query.bind(key.clone());
    }

    // provider-metrics (falls metrics_url gesetzt): parallel abfragen,
    // mit kurzem timeout + mini-cache (verhindert hammern bei 1s-poll)
    let provider_metrics = fetch_provider_metrics(&state).await;

    // aktuelle namen aus postgres: logs koennen den namen zum zeitpunkt des
    // requests speichern (rename!), die queue-metrik-key ist der aktuelle name
    let provider_names = current_provider_names(&state).await;

    // Live-Raten (decode/prefill) via Sigma/Summe. Lookback ist fix 1 h
    // (LIVE_RATE_LOOKBACK), unabhaengig vom window-Parameter.
    // NUR aus Streaming-Requests:
    //   decode_tps_own  = sum(completion_tokens) / sum(dur - ttfb) * 1000
    //   prefill_tps_own = sum(prompt_tokens)     / sum(ttfb)      * 1000
    // mit Guards: is_stream, tokens>0, duration>ttfb bzw. ttfb>0, status<400.
    // NUR streaming-zahlen flussen mit; non-streaming-rows (first_byte_ms=0)
    // waeren sonst Gift fuer den Praefill-Nenner.
    const LIVE_RATE_LOOKBACK: u32 = 3600;
    let rate_clauses: Vec<String> = {
        let mut c = vec!["timestamp >= now() - INTERVAL ? SECOND".to_string()];
        if q.key_name.is_some() {
            c.push("key_name = ?".to_string());
        }
        c
    };
    let rate_where_sql = rate_clauses.join(" AND ");
    let rate_sql = format!(
        r#"
        SELECT
            provider,
            provider_name,
            anyIf(provider_id, provider_id IS NOT NULL) AS provider_id,
            sumIf(completion_tokens, is_stream = 1 AND completion_tokens > 0 AND duration_ms > first_byte_ms AND status < 400) AS completion_sum,
            sumIf(duration_ms - first_byte_ms, is_stream = 1 AND completion_tokens > 0 AND duration_ms > first_byte_ms AND status < 400) AS decode_ms_sum,
            sumIf(prompt_tokens, is_stream = 1 AND prompt_tokens > 0 AND first_byte_ms > 0 AND status < 400) AS prompt_sum,
            sumIf(first_byte_ms, is_stream = 1 AND prompt_tokens > 0 AND first_byte_ms > 0 AND status < 400) AS prefill_ms_sum
        FROM yalr.request_logs
        WHERE {rate_where_sql}
        GROUP BY provider, provider_name
        "#
    );
    let mut rate_query = state.ch.query(&rate_sql).bind(LIVE_RATE_LOOKBACK);
    if let Some(key) = &q.key_name {
        rate_query = rate_query.bind(key.clone());
    }

    #[derive(clickhouse::Row, serde::Serialize, serde::Deserialize)]
    struct RateSumsRow {
        provider: String,
        provider_name: String,
        #[serde(with = "clickhouse::serde::uuid::option")]
        provider_id: Option<Uuid>,
        completion_sum: u64,
        decode_ms_sum: u64,
        prompt_sum: u64,
        prefill_ms_sum: u64,
    }

    // rename-aufloesung: alle CH-zeilen desselben aktuellem Namens aufaddieren
    // (statt "neueste zeile" behalten), damit die Raten den Gesamt-Traffic
    // des Provider-Namens abbilden.
    let mut rate_sums_by_name: std::collections::HashMap<String, RateSums> =
        std::collections::HashMap::new();
    match rate_query.fetch_all::<RateSumsRow>().await {
        Ok(rate_rows) => {
            let tuples: Vec<(String, u64, u64, u64, u64)> = rate_rows
                .iter()
                .map(|r| {
                    let name = r
                        .provider_id
                        .and_then(|id| provider_names.get(&id).cloned())
                        .unwrap_or_else(|| r.provider_name.clone());
                    (name, r.completion_sum, r.decode_ms_sum, r.prompt_sum, r.prefill_ms_sum)
                })
                .collect();
            rate_sums_by_name = merge_rate_sums(tuples);
        }
        Err(e) => tracing::error!("clickhouse rate-sums query failed: {e}"),
    }

    #[derive(clickhouse::Row, serde::Serialize, serde::Deserialize)]
    struct LiveStatsRow {
        provider: String,
        provider_name: String,
        #[serde(with = "clickhouse::serde::uuid::option")]
        provider_id: Option<Uuid>,
        reqs: u64,
        errors: u64,
        avg_ttft: Option<f64>,
        p50: f64,
        p95: f64,
        cost_usd: f64,
    }

    match query.fetch_all::<LiveStatsRow>().await {
        Ok(rows) => {
            // ClickHouse gruppiert nach dem geloggten namen; die aufloesung auf
            // den aktuellen namen kann mehrere zeilen zusammenfuehren - wir
            // agggegieren deshalb in rust nach dem mappen neu.
            #[derive(Clone)]
            struct Agg {
                provider: String,
                provider_id: Option<Uuid>,
                reqs: u64,
                errors: u64,
                ttft_sum: f64,
                ttft_count: u64,
                durations: Vec<f64>,
                cost_usd: f64,
            }

            let mut by_name: Vec<(String, Agg)> = Vec::new();
            for r in rows {
                // aktueller name (rename-aufloesung), fallback: geloggter name
                let name = r
                    .provider_id
                    .and_then(|id| provider_names.get(&id).cloned())
                    .unwrap_or_else(|| r.provider_name.clone());
                let entry = match by_name.iter_mut().find(|(n, _)| *n == name) {
                    Some((_, e)) => e,
                    None => {
                        by_name.push((
                            name.clone(),
                            Agg {
                                provider: r.provider.clone(),
                                provider_id: r.provider_id,
                                reqs: 0,
                                errors: 0,
                                ttft_sum: 0.0,
                                ttft_count: 0,
                                durations: Vec::new(),
                                cost_usd: 0.0,
                            },
                        ));
                        &mut by_name.last_mut().unwrap().1
                    }
                };
                entry.reqs += r.reqs;
                entry.errors += r.errors;
                entry.cost_usd += r.cost_usd;
                entry.durations.push(r.p50);
                entry.durations.push(r.p95);
                if let Some(ttft) = r.avg_ttft {
                    entry.ttft_sum += ttft * r.reqs as f64;
                    entry.ttft_count += r.reqs;
                }
            }

            let mut providers: Vec<_> = by_name
                .into_iter()
                .map(|(name, a)| {
                    let metrics = provider_metrics.get(&name);
                    // live-raten aus fenster-gesummten streaming-zahlen (Sigma/Summe)
                    let sums = rate_sums_by_name.get(&name);
                    let (our_decode, our_prefill) = match sums {
                        Some(s) => rates_from_sums(s),
                        None => (None, None),
                    };
                    json!({
                        "provider": a.provider,
                        "provider_name": name,
                        "provider_id": a.provider_id,
                        "reqs": a.reqs,
                        "errors": a.errors,
                        "avg_ttft": if a.ttft_count > 0 {
                            Some(a.ttft_sum / a.ttft_count as f64)
                        } else {
                            None
                        },
                        "p50": a.durations.iter().cloned().fold(0.0f64, f64::max),
                        "p95": a.durations.iter().cloned().fold(0.0f64, f64::max),
                        // decode-rate: provider-metric (live) bevorzugen, sonst wert
                        // aus der letzten abgeschlossenen anfrage (live, fensterunabhaengig)
                        "decode_tps": metrics.and_then(|m| m.decode_tps),
                        "decode_tps_own": our_decode,
                        "decode_tps_live": metrics.and_then(|m| m.decode_tps_live),
                        "prefill_tps": our_prefill,
                        "prefill_tps_live": metrics.and_then(|m| m.prefill_tps_live),
                        "cost_usd": a.cost_usd,
                        "metrics_url": metrics.and_then(|m| m.metrics_url.clone()),
                        "queued": metrics.and_then(|m| m.queued),
                        "running": metrics.and_then(|m| m.running),
                        "kv_cache_usage": metrics.and_then(|m| m.kv_cache_usage),
                    })
                })
                .collect();

            // Union: alle aktiven (enabled) postgres-providers, die ohne
            // traffic im fenster sind, als null-zeile ergaenzen - auch ohne
            // metrics_url bzw. bei fetch-fehler, damit die zeile stabil
            // bleibt (queue-tiefe ist auch ohne requests relevant).
            // CH-zeilen geloeschter provider bleiben unangetastet (namen-fallback).
            let present: std::collections::HashSet<String> = providers
                .iter()
                .filter_map(|p| p["provider_name"].as_str().map(|s| s.to_string()))
                .collect();
            for (name, m) in &provider_metrics {
                if present.contains(name) {
                    continue;
                }
                providers.push(json!({
                    "provider": m.kind,
                    "provider_name": name,
                    "provider_id": m.provider_id,
                    "reqs": 0,
                    "errors": 0,
                    "avg_ttft": null,
                    "p50": 0.0,
                    "p95": 0.0,
                    "decode_tps": m.decode_tps,
                    "decode_tps_own": null,
                    "decode_tps_live": m.decode_tps_live,
                    "prefill_tps": null,
                    "prefill_tps_live": m.prefill_tps_live,
                    "cost_usd": 0.0,
                    "metrics_url": m.metrics_url,
                    "queued": m.queued,
                    "running": m.running,
                    "kv_cache_usage": m.kv_cache_usage,
                }));
            }

            // stabile anzeige: alphabetisch absteigend nach provider-name
            // (statt reqs, was bei jedem poll die reihenfolge springen lies);
            // zeilen ohne namen (leerer fallback) ans ende. NACH dem anhaengen
            // der metrics-nullzeilen, damit auch diese einsortiert werden.
            providers.sort_by(|a, b| {
                let an = a["provider_name"].as_str().unwrap_or_default();
                let bn = b["provider_name"].as_str().unwrap_or_default();
                match (an.is_empty(), bn.is_empty()) {
                    (true, true) => std::cmp::Ordering::Equal,
                    (true, false) => std::cmp::Ordering::Greater,
                    (false, true) => std::cmp::Ordering::Less,
                    (false, false) => bn.cmp(an),
                }
            });

            Json(json!({
                "window": window,
                "providers": providers,
                "in_flight": filter_in_flight(state.log_sink.in_flight_snapshot(), &q.key_name),
            }))
                .into_response()
        }
        Err(e) => {
            tracing::error!("clickhouse live_stats failed: {e}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "query failed"})),
            )
                .into_response()
        }
    }
}

/// Provider-metrics aus /metrics-endpoints (prometheus-textformat).
/// Deckt alle aktiven (enabled) provider ab - auch ohne metrics_url und bei
/// fetch-fehlern (dann alle werte None, damit die zeile stabil bleibt).
/// Cached 2s, damit der 1s-poll des dashboards die provider nicht bombardiert.
#[derive(Clone)]
struct ProviderMetricsEntry {
    /// id des postgres-providers (fuer die null-zeilen-union)
    provider_id: Uuid,
    /// provider-kind (z.b. "openai_compat"), entspricht dem ch-"provider"-feld
    kind: String,
    /// konfigurierte metrics-url; None, falls nicht gesetzt
    metrics_url: Option<String>,
    queued: Option<f64>,
    running: Option<f64>,
    /// kv-cache-auslastung (roh, summe ueber Ranks/Instanzen, kann > 1),
    /// falls der engine sie exportiert
    kv_cache_usage: Option<f64>,
    /// live decode-rate (token/s) aus dem vllm-histogramm, falls vorhanden
    decode_tps: Option<f64>,
    /// live decode-rate aus counter-delta (gen_tokens / elapsed)
    decode_tps_live: Option<f64>,
    /// live prefill-rate aus counter-delta (prompt_tokens / ttft)
    prefill_tps_live: Option<f64>,
    /// absolute generation-token-counter (vllm:generation_tokens_total, gelabelt summiert)
    gen_tokens_total: Option<f64>,
    /// absolute prompt-token-counter (vllm:prompt_tokens_total, gelabelt summiert)
    prompt_tokens_total: Option<f64>,
    /// Zeitpunkt des Fetchs fuer die Counter-Delta-Rate
    fetched_at: std::time::Instant,
    /// true = /metrics-Fetch erfolgreich (2xx + Prometheus-Format), sonst
    /// false (inkl. fehlende metrics_url / Timeout / nicht-Prometheus-Body)
    fetch_ok: bool,
}

static METRICS_CACHE: once_cell::sync::Lazy<tokio::sync::Mutex<Option<(std::time::Instant, std::collections::HashMap<String, ProviderMetricsEntry>)>>> =
    once_cell::sync::Lazy::new(|| tokio::sync::Mutex::new(None));

/// Letzter Counter-Stand pro Provider (Uuid) fuer die Delta-Rate-Berechnung.
/// Gen- und Prompt-Baseline unabhängig (None = Counter fehlte -> verworfen).
static PREV_SAMPLES: once_cell::sync::Lazy<
    tokio::sync::Mutex<
        std::collections::HashMap<Uuid, (std::time::Instant, Option<f64>, Option<f64>)>,
    >,
> = once_cell::sync::Lazy::new(|| tokio::sync::Mutex::new(std::collections::HashMap::new()));

/// Mini-Cache (1s TTL) NUR fuer den on-demand-Scrape-Pfad der
/// Upstream-Counter im /metrics-Handler: verhindert, dass schnelle Scrapes
/// (z.B. 1s-Poll) die Provider-/metrics-Endpoints fluten. Schreibt NICHT in
/// den 2s-METRICS_CACHE (dessen Eintraege tragen Delta-Raten vom
/// live_stats-Pfad) und faehrt PREV_SAMPLES nicht an.
type UpstreamScrapeCacheValue =
    (std::time::Instant, Vec<(String, ProviderMetricsEntry)>);

static UPSTREAM_SCRAPE_CACHE: once_cell::sync::Lazy<
    tokio::sync::Mutex<Option<UpstreamScrapeCacheValue>>,
> = once_cell::sync::Lazy::new(|| tokio::sync::Mutex::new(None));

/// Baseline- und Rate-Zustand pro Upstream-Provider (Uuid) fuer die
/// serverseitig berechneten Token-Raten im on-demand-Scrape-Pfad.
/// Gen- und Prompt-Baseline unabhaengig; last_rates werden bis zum
/// naechsten echt neueren Sample wiederholt.
#[derive(Clone)]
struct UpstreamRateState {
    /// (fetched_at, prompt_tokens_total) der letzten gueltigen Baseline
    prompt_baseline: Option<(std::time::Instant, f64)>,
    /// (fetched_at, gen_tokens_total) der letzten gueltigen Baseline
    gen_baseline: Option<(std::time::Instant, f64)>,
    /// zuletzt berechnete Prefill-Rate (token/s); None = noch keine
    prefill_tps: Option<f64>,
    /// zuletzt berechnete Decode-Rate (token/s); None = noch keine
    decode_tps: Option<f64>,
    /// Sample-Abstand in Sekunden des zuletzt gemessenen Decode-Fensters
    decode_window: Option<f64>,
}

/// Letzter Rate-Zustand pro Provider (Uuid) fuer den on-demand-Scrape-Pfad.
/// Getrennt von PREV_SAMPLES / UPSTREAM_SCRAPE_CACHE; Lock nur kurz halten
/// (kein .await in der kritischen Sektion).
static UPSTREAM_RATE_BASELINES: once_cell::sync::Lazy<
    tokio::sync::Mutex<HashMap<Uuid, UpstreamRateState>>,
> = once_cell::sync::Lazy::new(|| tokio::sync::Mutex::new(HashMap::new()));

/// Reine Kernfunktion: faehrt den Upstream-Rate-Zustand eines Providers
/// mit einem aktuellen Scrape-Sample voran. Keine Statics, voll testbar.
///
/// Regeln (provider_id-matching erfolgt auerhalb; hier nur Counter-Logik):
/// - Nur wenn cur.fetched_at NEUER als die jeweilige Counter-Baseline:
///   dieser Counter wird verarbeitet. Identischer/aelterer Sample: Baseline
///   und last_rates unveraendert (verhindert Fake-0 und TTL-Kanten-Race).
/// - Pro Counter unabhaengig: fehlender Counter (None) haelt die alte
///   Baseline, der andere ruckt normal.
/// - Reset (diff < 0) -> Rate None, Baseline trotzdem auf cur setzen;
///   last_rates dieses Counters -> None (sonst Rate nach Restart tot).
/// - MIN_ELAPSED-Floor (<1s) -> Rate None, Baseline fortschreiben,
///   last_rates unveraendert (transient, selbstheilend).
/// - Kaltstart (keine Baseline) -> Baseline seeden, Rate None.
fn advance_rate_state(
    old: Option<&UpstreamRateState>,
    cur: &ProviderMetricsEntry,
) -> UpstreamRateState {
    let mut s = old.cloned().unwrap_or(UpstreamRateState {
        prompt_baseline: None,
        gen_baseline: None,
        prefill_tps: None,
        decode_tps: None,
        decode_window: None,
    });
    let now = cur.fetched_at;

    // Prefill (Prompt-Counter)
    if let Some(v) = cur.prompt_tokens_total {
        match s.prompt_baseline {
            None => s.prompt_baseline = Some((now, v)), // Kaltstart
            Some((bt, bv)) if now > bt => {
                let rate = common::metrics::counter_delta_rate(Some((&bt, bv)), v, now);
                s.prompt_baseline = Some((now, v));
                match rate {
                    Some(r) => s.prefill_tps = Some(r),
                    None if v < bv => s.prefill_tps = None, // Reset
                    None => {} // MIN_ELAPSED: Rate unveraendert
                }
            }
            _ => {} // identisch/aelter: unveraendert
        }
    }

    // Decode (Generation-Counter)
    if let Some(v) = cur.gen_tokens_total {
        match s.gen_baseline {
            None => s.gen_baseline = Some((now, v)), // Kaltstart
            Some((bt, bv)) if now > bt => {
                let rate = common::metrics::counter_delta_rate(Some((&bt, bv)), v, now);
                s.gen_baseline = Some((now, v));
                match rate {
                    Some(r) => {
                        s.decode_tps = Some(r);
                        s.decode_window = Some(now.duration_since(bt).as_secs_f64());
                    }
                    None if v < bv => {
                        // Reset: Decode-Rate und Fenster toten
                        s.decode_tps = None;
                        s.decode_window = None;
                    }
                    None => {} // MIN_ELAPSED: Rate/Fenster unveraendert
                }
            }
            _ => {} // identisch/aelter: unveraendert
        }
    }

    s
}

/// Liefert die Provider-/metrics-Eintraege fuer den on-demand-Scrape mit
/// 1s-Cache. Cache frisch (<1s) -> geklonter Cache-Stand; sonst frischer
/// Fetch. PG-Fehler -> leerer Vec (debug statt warn: laeuft pro Scrape,
/// PG-down ist ueber die 30s-Upstream-Sektion + Age-Gauge sichtbar).
async fn upstream_scrape_entries(
    state: &AppState,
) -> Vec<(String, ProviderMetricsEntry)> {
    {
        let cache = UPSTREAM_SCRAPE_CACHE.lock().await;
        if let Some((at, entries)) = cache.as_ref() {
            if at.elapsed() < std::time::Duration::from_secs(1) {
                return entries.clone();
            }
        }
    }
    let rows = match load_enabled_providers(state).await {
        Ok(rows) => rows,
        Err(e) => {
            tracing::debug!("upstream-scrape-cache: provider-laden fehlgeschlagen: {e}");
            return Vec::new();
        }
    };
    let (entries, _from_cache) = fetch_and_parse_provider_metrics(state, rows).await;
    *UPSTREAM_SCRAPE_CACHE.lock().await = Some((std::time::Instant::now(), entries.clone()));
    entries
}

/// Ein aktiver (enabled) Provider aus Postgres fuer die Metrics-/Upstream-Sektionen.
#[derive(sqlx::FromRow)]
struct MetricsProviderRow {
    id: Uuid,
    name: String,
    kind: String,
    metrics_url: Option<String>,
}

/// Laedt alle enabled Provider aus Postgres (nur die PG-Query, ohne HTTP).
/// Bei PG-Fehler wird die Error durchgereicht und NICHT in eine leere Map
/// umgewandelt - der Aufrufer entscheidet (live_stats rendert wie bisher
/// leer, der 30s-Task haelt den alten Stand).
/// Die null-zeilen-union braucht auch provider ohne metrics_url (sonst
/// fehlen sie bei leerem traffic komplett).
async fn load_enabled_providers(
    state: &AppState,
) -> Result<Vec<MetricsProviderRow>, String> {
    sqlx::query_as::<_, MetricsProviderRow>(
        "SELECT id, name, kind, metrics_url FROM providers WHERE enabled = TRUE",
    )
    .fetch_all(&state.pg)
    .await
    .map_err(|e| e.to_string())
}

/// Frischen (max. 2s alten) METRICS_CACHE-Stand liefern, falls vorhanden.
/// Wird von beiden pfaden genutzt: live_stats (fruehzeitiger return ohne
/// pg-query, wie vor dem metrics-umbau) und dem 30s-task (nur lesezugriff,
/// kein fetch noetig).
async fn fresh_metrics_cache() -> Option<std::collections::HashMap<String, ProviderMetricsEntry>> {
    let cache = METRICS_CACHE.lock().await;
    if let Some((at, map)) = cache.as_ref() {
        if at.elapsed() < std::time::Duration::from_secs(2) {
            return Some(map.clone());
        }
    }
    None
}

/// Holt die /metrics-Endpunkte der Provider und parst die Werte (inkl.
/// fetch_ok). Liesst den geteilten 2s-METRICS_CACHE (Cache-Hit -> kein
/// Fetch), schreibt ihn aber NICHT: geschrieben wird nur vom
/// live_stats-Pfad, damit der 30s-Metrics-Task die PREV_SAMPLES-Baseline
/// der Delta-Raten nicht vorfaehrt.
/// Return: (Eintraege, aus-Cache)
async fn fetch_and_parse_provider_metrics(
    state: &AppState,
    rows: Vec<MetricsProviderRow>,
) -> (Vec<(String, ProviderMetricsEntry)>, bool) {
    // cache pruefen (2s frisch)
    if let Some(map) = fresh_metrics_cache().await {
        return (map.into_iter().collect(), true);
    }

    // parallel fetchen (kurzer timeout: das live-dashboard wartet nicht auf haengende endpoints)
    let client = &state.http;
    let futures: Vec<_> = rows
        .into_iter()
        .map(|row| {
            let metrics_url = row.metrics_url.filter(|u| !u.is_empty());
            let name = row.name;
            let provider_id = row.id;
            let kind = row.kind;
            async move {
                // bei fetch-fehler (timeout/nicht-2xx/parse) trotzdem einen
                // eintrag mit allen-none-werten erzeugen, damit die zeile im
                // dashboard stabil bleibt und nicht flackert
                let (queued, running, decode_tps, gen_tokens, prompt_tokens, kv_cache, fetch_ok) =
                    match &metrics_url {
                        Some(url) => {
                            let fetch = client
                                .get(url)
                                .timeout(std::time::Duration::from_millis(1500))
                                .send()
                                .await;
                            match fetch {
                                Ok(resp) if resp.status().is_success() => {
                                    // Body-Read mit hartem 2s-Timeout: ein
                                    // trickelnder Provider-Body darf den Pfad
                                    // nicht unbestimmt blockieren (die
                                    // read_timeout des Clients ist nur ein
                                    // Stall-Guard). Fixt auch den latenten
                                    // Blocker im live_stats-Pfad (bewusste
                                    // Verbesserung, keine semantische
                                    // Aenderung).
                                    match tokio::time::timeout(
                                        std::time::Duration::from_secs(2),
                                        resp.text(),
                                    )
                                    .await
                                    {
                                        Ok(Ok(body))
                                            if common::metrics::looks_like_prometheus(&body) =>
                                        {
                                            let (queued, running) =
                                                common::metrics::queue_metrics(&body);
                                            let decode_tps =
                                                common::metrics::decode_tps(&body);
                                            let (gen_tokens, prompt_tokens) =
                                                common::metrics::parse_token_counters(&body);
                                            let kv_cache =
                                                common::metrics::kv_cache_usage(&body);
                                            (
                                                queued,
                                                running,
                                                decode_tps,
                                                gen_tokens,
                                                prompt_tokens,
                                                kv_cache,
                                                true,
                                            )
                                        }
                                        // Timeout / Read-Fehler / 2xx mit
                                        // nicht-Prometheus-Body (HTML-Login
                                        // etc.): Fetch-Fehler, kein Erfolg
                                        _ => (None, None, None, None, None, None, false),
                                    }
                                }
                                _ => (None, None, None, None, None, None, false),
                            }
                        }
                        None => (None, None, None, None, None, None, false),
                    };
                (
                    name,
                    ProviderMetricsEntry {
                        provider_id,
                        kind,
                        metrics_url,
                        queued,
                        running,
                        kv_cache_usage: kv_cache,
                        decode_tps,
                        decode_tps_live: None,
                        prefill_tps_live: None,
                        gen_tokens_total: gen_tokens,
                        prompt_tokens_total: prompt_tokens,
                        fetched_at: std::time::Instant::now(),
                        fetch_ok,
                    },
                )
            }
        })
        .collect();
    let results = futures::future::join_all(futures).await;
    (results, false)
}

/// Provider-Metriken fuer live_stats: enabled Provider laden, /metrics-Werte
/// holen (geteilter 2s-Cache) und die Counter-Delta-Raten gegen die
/// PREV_SAMPLES-Baseline berechnen. Bei PG-Fehler leere Map, exakt wie
/// bisher (live_stats laeuft trotzdem). Der 30s-Metrics-Task nutzt
/// load_enabled_providers + fetch_and_parse_provider_metrics direkt, ohne
/// diesen Baseline-Pfad.
async fn fetch_provider_metrics(
    state: &AppState,
) -> std::collections::HashMap<String, ProviderMetricsEntry> {
    // cache zuerst pruefen (wie vor dem metrics-umbau): bei frischem cache
    // keine pg-query, gecachten stand inkl. delta-raten liefern
    if let Some(map) = fresh_metrics_cache().await {
        return map;
    }

    let rows = match load_enabled_providers(state).await {
        Ok(r) => r,
        Err(_) => return std::collections::HashMap::new(),
    };

    let (mut results, from_cache) = fetch_and_parse_provider_metrics(state, rows).await;

    if from_cache {
        // Cache-Hit: exakt den Stand des letzten echten fetchs liefern
        // (inkl. Delta-Raten), PREV_SAMPLES bleibt unangetastet
        return results.into_iter().collect();
    }

    // live counter-delta-rates: gegen den vorherigen echten fetch pro provider
    // vergleichen (zeitschluessel = fetched_at, keine cache-ttl-annahme).
    // lock wird nur kurz gehalten, kein .await in der kritischen sektion.
    {
        let mut prev = PREV_SAMPLES.lock().await;
        for (_, entry) in results.iter_mut() {
            let now = entry.fetched_at;
            // invariant: old_t ist genau dann Some, wenn auch die beiden
            // Counter-Komponenten Some sind (werden zusammen gespeichert/verworfen)
            let (old_t, old_gen, old_prompt) = match prev.get(&entry.provider_id) {
                Some((t, g, p)) => (Some(*t), *g, *p),
                None => (None, None, None),
            };

            // Gen-Baseline unabhaengig behandeln: vorhanden -> Delta + Update;
            // fehlend -> nur diese Komponente verwerfen (nicht die Prompt-Baseline).
            let gen_new = match (entry.gen_tokens_total, old_gen) {
                (Some(gen), Some(pgen)) => {
                    entry.decode_tps_live = old_t
                        .and_then(|t| common::metrics::counter_delta_rate(Some((&t, pgen)), gen, now));
                    Some(gen)
                }
                (Some(gen), None) => Some(gen),
                (None, _) => None,
            };

            let prompt_new = match (entry.prompt_tokens_total, old_prompt) {
                (Some(prompt), Some(pprompt)) => {
                    entry.prefill_tps_live = old_t
                        .and_then(|t| common::metrics::counter_delta_rate(Some((&t, pprompt)), prompt, now));
                    Some(prompt)
                }
                (Some(prompt), None) => Some(prompt),
                (None, _) => None,
            };

            match (gen_new, prompt_new) {
                // beide Counter fehlen: komplette Baseline verwerfen,
                // sonst muesste vom alten wert gemessen werden
                (None, None) => {
                    prev.remove(&entry.provider_id);
                }
                (g, p) => {
                    // baseline auf den aktuellen stand setzen (auch im reset-fall:
                    // das naechste sample startet beim neuen wert)
                    prev.insert(entry.provider_id, (now, g, p));
                }
            }
        }
    }

    let mut map = std::collections::HashMap::new();
    for (name, entry) in results {
        map.insert(name, entry);
    }

    // cache schreiben
    *METRICS_CACHE.lock().await = Some((std::time::Instant::now(), map.clone()));
    map
}

// ============================================================
// Settings
// ============================================================

#[derive(Deserialize)]
pub struct ChangePasswordRequest {
    pub current_password: String,
    pub new_password: String,
}

pub async fn change_password(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<ChangePasswordRequest>,
) -> Response {
    if auth::require_session(&state, &headers).await.is_err() {
        return unauthorized();
    }

    // eingeloggten user ueber session-token ermitteln
    let Some(token) = auth::extract_cookie(&headers, auth::SESSION_COOKIE) else {
        return unauthorized();
    };
    let token_hash = auth::hash_token(&token);

    #[derive(sqlx::FromRow)]
    struct SessionUserRow {
        admin_user_id: Uuid,
    }
    let session = sqlx::query_as::<_, SessionUserRow>(
        "SELECT admin_user_id FROM sessions WHERE token_hash = $1",
    )
    .bind(&token_hash)
    .fetch_optional(&state.pg)
    .await;
    let admin_user_id = match session {
        Ok(Some(s)) => s.admin_user_id,
        _ => return unauthorized(),
    };

    #[derive(sqlx::FromRow)]
    struct AdminPasswordRow {
        password_hash: String,
    }
    let user = sqlx::query_as::<_, AdminPasswordRow>(
        "SELECT password_hash FROM admin_users WHERE id = $1",
    )
    .bind(admin_user_id)
    .fetch_one(&state.pg)
    .await;

    let Ok(password_hash) = user.map(|u| u.password_hash) else {
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": "internal"}))).into_response();
    };

    if !bcrypt::verify(&req.current_password, &password_hash).unwrap_or(false) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({"error": "current password incorrect"})),
        )
            .into_response();
    }

    if req.new_password.len() < 8 {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "new password must be at least 8 characters"})),
        )
            .into_response();
    }

    let new_hash = match bcrypt::hash(&req.new_password, bcrypt::DEFAULT_COST) {
        Ok(h) => h,
        Err(e) => return server_error(e),
    };
    let res = sqlx::query("UPDATE admin_users SET password_hash = $1 WHERE id = $2")
        .bind(&new_hash)
        .bind(admin_user_id)
        .execute(&state.pg)
        .await;

    match res {
        Ok(_) => Json(json!({"ok": true})).into_response(),
        Err(e) => server_error(e),
    }
}

// ============================================================
// Live-Events (SSE)
// ============================================================

/// SSE-Stream mit Live-Request-Events (started/first_byte/completed).
/// Auth wie alle Dashboard-Endpunkte via Session-Cookie.
pub async fn live_events(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Response {
    if auth::require_session(&state, &headers).await.is_err() {
        return unauthorized();
    }

    use axum::response::sse::{Event, KeepAlive, Sse};
    use futures::StreamExt;
    use std::convert::Infallible;

    let rx = state.log_sink.subscribe();
    let stream = tokio_stream::wrappers::BroadcastStream::new(rx).filter_map(
        |msg| async move {
            match msg {
                Ok(event) => Some(Ok::<_, Infallible>(
                    Event::default()
                        .event(match &event {
                            ingest::LiveEvent::RequestStarted { .. } => "request_started",
                            ingest::LiveEvent::FirstByte { .. } => "first_byte",
                            ingest::LiveEvent::Completed { .. } => "completed",
                        })
                        .data(serde_json::to_string(&event).unwrap_or_default()),
                )),
                // lagged clients verlieren events (best-effort) - einfach weitermachen
                Err(_lagged) => None,
            }
        },
    );

    Sse::new(stream)
        .keep_alive(KeepAlive::new().interval(std::time::Duration::from_secs(15)))
        .into_response()
}

// ============================================================
// Prometheus-Metriken (GET /metrics)
// ============================================================

/// Formatiert einen f64-Wert ohne wissenschaftliche Notation.
/// Fixe 6 Nachkommastellen, danach trailing Zeros und ggf. den Punkt trimmen.
fn fmt_f64(v: f64) -> String {
    let mut s = format!("{:.6}", v);
    s = s.trim_end_matches('0').to_string();
    s = s.trim_end_matches('.').to_string();
    s
}

/// Escapet Label-Werte fuer Prometheus-Textformat (\\, \", \n).
fn escape_label(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            _ => out.push(c),
        }
    }
    out
}

/// Fuehrt die CH-Query fuer die 24h-Latenz-Sektion aus und rendert die
/// dazugehoerigen Prometheus-Textzeilen. Err bei Query-Fehler.
pub async fn build_latency_body(state: &AppState) -> Result<String, String> {
    let sql = r#"
        SELECT
            count() AS n,
            quantileDeterministic(0.5)(duration_ms, cityHash64(request_id)) / 1000 AS d_p50,
            quantileDeterministic(0.95)(duration_ms, cityHash64(request_id)) / 1000 AS d_p95,
            quantileDeterministic(0.5)(nullIf(first_byte_ms, 0), cityHash64(request_id)) / 1000 AS fb_p50,
            quantileDeterministic(0.95)(nullIf(first_byte_ms, 0), cityHash64(request_id)) / 1000 AS fb_p95
        FROM yalr.request_logs
        WHERE timestamp >= now() - INTERVAL 24 HOUR
    "#;

    #[derive(clickhouse::Row, serde::Deserialize)]
    struct SnapRow {
        n: u64,
        d_p50: f64,
        d_p95: f64,
        fb_p50: Option<f64>,
        fb_p95: Option<f64>,
    }

    let row = state
        .ch
        .query(sql)
        .fetch_one::<SnapRow>()
        .await
        .map_err(|e| e.to_string())?;

    let mut body = String::new();
    body.push_str(
        "# HELP yalr_request_duration_seconds Request duration in seconds (24h window, deterministic quantile)\n",
    );
    body.push_str("# TYPE yalr_request_duration_seconds gauge\n");
    if row.n > 0 {
        body.push_str(&format!(
            "yalr_request_duration_seconds{{quantile=\"0.5\"}} {}\n",
            fmt_f64(row.d_p50)
        ));
        body.push_str(&format!(
            "yalr_request_duration_seconds{{quantile=\"0.95\"}} {}\n",
            fmt_f64(row.d_p95)
        ));
    }

    body.push_str(
        "# HELP yalr_first_byte_seconds Time to first byte in seconds (24h window, deterministic quantile)\n",
    );
    body.push_str("# TYPE yalr_first_byte_seconds gauge\n");
    if row.n > 0 {
        if let Some(v) = row.fb_p50 {
            body.push_str(&format!(
                "yalr_first_byte_seconds{{quantile=\"0.5\"}} {}\n",
                fmt_f64(v)
            ));
        }
        if let Some(v) = row.fb_p95 {
            body.push_str(&format!(
                "yalr_first_byte_seconds{{quantile=\"0.95\"}} {}\n",
                fmt_f64(v)
            ));
        }
    }

    Ok(body)
}

/// Eine Provider-Gruppe der 60s-Live-Sektion (Werte bereits in Sekunden bzw.
/// Token/s; die ms->s-Konvertierung passiert in der CH-Query).
#[derive(Debug, Clone)]
pub struct LiveGroup {
    pub provider: String,
    pub provider_name: String,
    pub reqs: u64,
    pub errors: u64,
    pub cost_usd: f64,
    /// Durchschnittliches TTFT in Sekunden (None = kein First-Byte-Datum im Fenster).
    pub avg_ttft_s: Option<f64>,
    /// p50/p95 der Gesamtdauer in Sekunden (deterministische Quantile).
    pub p50_s: f64,
    pub p95_s: f64,
    /// Decode- bzw. Prefill-Rate in Token/s (1h-Lookback, nur Streaming; None ohne Daten).
    pub decode_tps: Option<f64>,
    pub prefill_tps: Option<f64>,
}

/// Ein Provider mit Upstream-Engine-Metriken (Rohwerte aus dessen /metrics-Endpoint).
#[derive(Debug, Clone)]
pub struct UpstreamGroup {
    pub provider: String,
    pub provider_name: String,
    /// Anzahl wartender Requests laut Engine-Report.
    pub queued: Option<f64>,
    /// Anzahl laufender Requests laut Engine-Report.
    pub running: Option<f64>,
    /// KV-Cache-Auslastung (Rohwert, Summe ueber alle Label-Serien, d.h.
    /// Ranks/Instanzen) — vllm:kv_cache_usage_perc / sglang:token_usage;
    /// kann > 1 sein (kein Clamp, wie im Dashboard).
    pub kv_cache_usage: Option<f64>,
    /// true = der /metrics-Fetch war erfolgreich (2xx + Prometheus-Format),
    /// sonst false (auch bei fehlender metrics_url, Timeout oder
    /// nicht-Prometheus-Body).
    pub fetch_ok: bool,
}

/// Rendert die 60s-Live-Metriken (Gauges) pro Provider-Gruppe.
/// Leere Eingabe -> leerer Body (Serien verschwinden, Prometheus-Staleness).
pub fn render_live_exposition(groups: &[LiveGroup]) -> String {
    let mut body = String::new();
    if groups.is_empty() {
        return body;
    }

    body.push_str(
        "# HELP yalr_live_requests LLM requests in the trailing 60s window (snapshot refreshed every 30s). Not cumulative; see yalr_requests_total for the since-process-start counter.\n",
    );
    body.push_str("# TYPE yalr_live_requests gauge\n");
    for g in groups {
        body.push_str(&format!(
            "yalr_live_requests{{provider=\"{}\", provider_name=\"{}\"}} {}\n",
            escape_label(&g.provider),
            escape_label(&g.provider_name),
            g.reqs
        ));
    }

    body.push_str(
        "# HELP yalr_live_errors Failed LLM requests (status >= 400) in the trailing 60s window (snapshot refreshed every 30s)\n",
    );
    body.push_str("# TYPE yalr_live_errors gauge\n");
    for g in groups {
        body.push_str(&format!(
            "yalr_live_errors{{provider=\"{}\", provider_name=\"{}\"}} {}\n",
            escape_label(&g.provider),
            escape_label(&g.provider_name),
            g.errors
        ));
    }

    body.push_str(
        "# HELP yalr_live_cost_usd Cost in USD in the trailing 60s window (snapshot refreshed every 30s)\n",
    );
    body.push_str("# TYPE yalr_live_cost_usd gauge\n");
    for g in groups {
        body.push_str(&format!(
            "yalr_live_cost_usd{{provider=\"{}\", provider_name=\"{}\"}} {}\n",
            escape_label(&g.provider),
            escape_label(&g.provider_name),
            fmt_f64(g.cost_usd)
        ));
    }

    body.push_str(
        "# HELP yalr_live_duration_seconds Request duration in seconds in the trailing 60s window (deterministic quantile, snapshot refreshed every 30s)\n",
    );
    body.push_str("# TYPE yalr_live_duration_seconds gauge\n");
    for g in groups {
        body.push_str(&format!(
            "yalr_live_duration_seconds{{provider=\"{}\", provider_name=\"{}\", quantile=\"0.5\"}} {}\n",
            escape_label(&g.provider),
            escape_label(&g.provider_name),
            fmt_f64(g.p50_s)
        ));
        body.push_str(&format!(
            "yalr_live_duration_seconds{{provider=\"{}\", provider_name=\"{}\", quantile=\"0.95\"}} {}\n",
            escape_label(&g.provider),
            escape_label(&g.provider_name),
            fmt_f64(g.p95_s)
        ));
    }

    body.push_str(
        "# HELP yalr_live_first_byte_seconds Average time to first byte in seconds in the trailing 60s window (snapshot refreshed every 30s)\n",
    );
    body.push_str("# TYPE yalr_live_first_byte_seconds gauge\n");
    for g in groups {
        if let Some(v) = g.avg_ttft_s {
            body.push_str(&format!(
                "yalr_live_first_byte_seconds{{provider=\"{}\", provider_name=\"{}\"}} {}\n",
                escape_label(&g.provider),
                escape_label(&g.provider_name),
                fmt_f64(v)
            ));
        }
    }

    body.push_str(
        "# HELP yalr_live_decode_tps Decode rate in tokens per second (1h lookback, streaming requests only, like the dashboard live view)\n",
    );
    body.push_str("# TYPE yalr_live_decode_tps gauge\n");
    for g in groups {
        if let Some(v) = g.decode_tps {
            body.push_str(&format!(
                "yalr_live_decode_tps{{provider=\"{}\", provider_name=\"{}\"}} {}\n",
                escape_label(&g.provider),
                escape_label(&g.provider_name),
                fmt_f64(v)
            ));
        }
    }

    body.push_str(
        "# HELP yalr_live_prefill_tps Prefill rate in tokens per second (1h lookback, streaming requests only, like the dashboard live view)\n",
    );
    body.push_str("# TYPE yalr_live_prefill_tps gauge\n");
    for g in groups {
        if let Some(v) = g.prefill_tps {
            body.push_str(&format!(
                "yalr_live_prefill_tps{{provider=\"{}\", provider_name=\"{}\"}} {}\n",
                escape_label(&g.provider),
                escape_label(&g.provider_name),
                fmt_f64(v)
            ));
        }
    }

    body
}

/// Rendert die Upstream-Engine-Metriken pro Provider.
/// Bei bekannter, aber leerer Provider-Liste werden trotzdem die
/// HELP/TYPE-Header und yalr_providers_enabled 0 gerendert (Datenzeilen der
/// anderen Familien bleiben absent). Fehlende Einzelwerte (None) werden
/// nicht gerendert (keine NaN-Zeilen).
pub fn render_upstream_exposition(groups: &[UpstreamGroup], providers_enabled: usize) -> String {
    let mut body = String::new();

    body.push_str(
        "# HELP yalr_upstream_queued Queued requests reported by the provider engine (snapshot refreshed every 30s)\n",
    );
    body.push_str("# TYPE yalr_upstream_queued gauge\n");
    for g in groups {
        if let Some(v) = g.queued {
            body.push_str(&format!(
                "yalr_upstream_queued{{provider=\"{}\", provider_name=\"{}\"}} {}\n",
                escape_label(&g.provider),
                escape_label(&g.provider_name),
                fmt_f64(v)
            ));
        }
    }

    body.push_str(
        "# HELP yalr_upstream_running Running requests reported by the provider engine (snapshot refreshed every 30s)\n",
    );
    body.push_str("# TYPE yalr_upstream_running gauge\n");
    for g in groups {
        if let Some(v) = g.running {
            body.push_str(&format!(
                "yalr_upstream_running{{provider=\"{}\", provider_name=\"{}\"}} {}\n",
                escape_label(&g.provider),
                escape_label(&g.provider_name),
                fmt_f64(v)
            ));
        }
    }

    body.push_str(
        "# HELP yalr_upstream_kv_cache_usage KV-cache usage reported by the provider engine (vllm:kv_cache_usage_perc or sglang:token_usage); raw value, summed over all labeled series (ranks/instances), may exceed 1 (snapshot refreshed every 30s)\n",
    );
    body.push_str("# TYPE yalr_upstream_kv_cache_usage gauge\n");
    for g in groups {
        if let Some(v) = g.kv_cache_usage {
            body.push_str(&format!(
                "yalr_upstream_kv_cache_usage{{provider=\"{}\", provider_name=\"{}\"}} {}\n",
                escape_label(&g.provider),
                escape_label(&g.provider_name),
                fmt_f64(v)
            ));
        }
    }

    body.push_str(
        "# HELP yalr_upstream_up Whether the provider /metrics endpoint was reachable on the last fetch (1=up, 0=down or no metrics_url configured)\n",
    );
    body.push_str("# TYPE yalr_upstream_up gauge\n");
    for g in groups {
        body.push_str(&format!(
            "yalr_upstream_up{{provider=\"{}\", provider_name=\"{}\"}} {}\n",
            escape_label(&g.provider),
            escape_label(&g.provider_name),
            if g.fetch_ok { 1 } else { 0 }
        ));
    }

    body.push_str("# HELP yalr_providers_enabled Number of enabled providers\n");
    body.push_str("# TYPE yalr_providers_enabled gauge\n");
    body.push_str(&format!("yalr_providers_enabled {}\n", providers_enabled));

    body
}

/// Rendert die Engine-Counter-Familien der Upstream-Tokenzähler aus den
/// on-demand gescrapten Provider-/metrics-Eintraegen. Absolute Rohcounter
/// (vllm/sglang), beim Scrape gesampelt, über Engine-Serien summiert —
/// aktualisieren sich während der Generation. None-Werte werden nicht
/// gerendert (kein NaN); Counter-Reset beim Provider-Restart ist moeglich
/// und normal (Consumer behandelt den Reset). Leerer Input -> leerer String.
/// Stabile Reihenfolge: alphabetisch absteigend nach provider_name (wie die
/// Upstream-Sektion).
fn render_upstream_token_counters(
    entries: &[(String, ProviderMetricsEntry)],
    rates: &[(Uuid, UpstreamRateState)],
) -> String {
    let mut body = String::new();
    if entries.is_empty() {
        return body;
    }
    let mut sorted: Vec<&(String, ProviderMetricsEntry)> = entries.iter().collect();
    sorted.sort_by(|a, b| b.0.cmp(&a.0));

    // Rate-State-Lookup nach provider_id (für die Gauge-Familien unten)
    let rate_map: HashMap<Uuid, &UpstreamRateState> =
        rates.iter().map(|(id, st)| (*id, st)).collect();

    body.push_str(
        "# HELP yalr_upstream_prompt_tokens_total Prompt tokens sampled at scrape time from the provider /metrics endpoint (vllm/sglang raw engine counters, summed over labeled series)\n",
    );
    body.push_str("# TYPE yalr_upstream_prompt_tokens_total counter\n");
    for (name, e) in &sorted {
        if let Some(v) = e.prompt_tokens_total {
            body.push_str(&format!(
                "yalr_upstream_prompt_tokens_total{{provider=\"{}\", provider_name=\"{}\"}} {}\n",
                escape_label(&e.kind),
                escape_label(name),
                fmt_f64(v)
            ));
        }
    }

    body.push_str(
        "# HELP yalr_upstream_generation_tokens_total Generation tokens sampled at scrape time from the provider /metrics endpoint (vllm/sglang raw engine counters, summed over labeled series)\n",
    );
    body.push_str("# TYPE yalr_upstream_generation_tokens_total counter\n");
    for (name, e) in &sorted {
        if let Some(v) = e.gen_tokens_total {
            body.push_str(&format!(
                "yalr_upstream_generation_tokens_total{{provider=\"{}\", provider_name=\"{}\"}} {}\n",
                escape_label(&e.kind),
                escape_label(name),
                fmt_f64(v)
            ));
        }
    }

    // Serverseitig berechnete Prefill-Rate (Gauge) aus dem Scrape-Pfad
    body.push_str(
        "# HELP yalr_upstream_prefill_tps Prefill-Durchsatz des Providers: Delta der Engine-Prompt-Counter zwischen den zwei jüngsten Engine-Samples, die der Scrape-Pfad gesehen hat, geteilt durch den Sample-Abstand. Wird bis zum nächsten neueren Sample wiederholt.\n",
    );
    body.push_str("# TYPE yalr_upstream_prefill_tps gauge\n");
    for (name, e) in &sorted {
        if let Some(st) = rate_map.get(&e.provider_id) {
            if let Some(v) = st.prefill_tps {
                body.push_str(&format!(
                    "yalr_upstream_prefill_tps{{provider=\"{}\", provider_name=\"{}\"}} {}\n",
                    escape_label(&e.kind),
                    escape_label(name),
                    fmt_f64(v)
                ));
            }
        }
    }

    // Serverseitig berechnete Decode-Rate (Gauge) aus dem Scrape-Pfad
    body.push_str(
        "# HELP yalr_upstream_decode_tps Decode-Durchsatz des Providers: Delta der Engine-Generation-Counter zwischen den zwei jüngsten Engine-Samples, die der Scrape-Pfad gesehen hat, geteilt durch den Sample-Abstand. Wird bis zum nächsten neueren Sample wiederholt.\n",
    );
    body.push_str("# TYPE yalr_upstream_decode_tps gauge\n");
    for (name, e) in &sorted {
        if let Some(st) = rate_map.get(&e.provider_id) {
            if let Some(v) = st.decode_tps {
                body.push_str(&format!(
                    "yalr_upstream_decode_tps{{provider=\"{}\", provider_name=\"{}\"}} {}\n",
                    escape_label(&e.kind),
                    escape_label(name),
                    fmt_f64(v)
                ));
            }
        }
    }

    // Decode-Fenster (Gauge): Sample-Abstand des zuletzt gemessenen Decode-Fensters
    body.push_str(
        "# HELP yalr_upstream_rate_window_seconds Sample-Abstand in Sekunden, über den die aktuelle Decode-Rate gemessen wurde. Macht das Messfenster sichtbar (poll-taktabhängig).\n",
    );
    body.push_str("# TYPE yalr_upstream_rate_window_seconds gauge\n");
    for (name, e) in &sorted {
        if let Some(st) = rate_map.get(&e.provider_id) {
            if st.decode_tps.is_some() {
                if let Some(v) = st.decode_window {
                    body.push_str(&format!(
                        "yalr_upstream_rate_window_seconds{{provider=\"{}\", provider_name=\"{}\"}} {}\n",
                        escape_label(&e.kind),
                        escape_label(name),
                        fmt_f64(v)
                    ));
                }
            }
        }
    }

    body
}

/// Wird on-demand im metrics_handler gerendert (scrape-aktuell, kein Snapshot).
pub fn render_in_flight_exposition(in_flight: &[ingest::InFlightReq]) -> String {
    let mut counts: std::collections::BTreeMap<(String, String), u64> =
        std::collections::BTreeMap::new();
    for r in in_flight {
        *counts
            .entry((r.provider.clone(), r.provider_name.clone()))
            .or_insert(0) += 1;
    }

    let mut body = String::new();
    body.push_str(
        "# HELP yalr_in_flight_requests Currently in-flight (running) LLM requests per provider\n",
    );
    body.push_str("# TYPE yalr_in_flight_requests gauge\n");
    for ((provider, provider_name), n) in &counts {
        body.push_str(&format!(
            "yalr_in_flight_requests{{provider=\"{}\", provider_name=\"{}\"}} {}\n",
            escape_label(provider),
            escape_label(provider_name),
            n
        ));
    }
    body
}

/// Fuehrt die CH-Queries fuer die 60s-Live-Sektion aus und rendert die
/// Prometheus-Textzeilen. Schema wie live_stats, aber OHNE Rename-Merge,
/// key_name-Filter und null-rows-Union. Err bei Query-Fehler.
pub async fn build_live_body(state: &AppState) -> Result<String, String> {
    let sql = r#"
        SELECT
            provider,
            provider_name,
            count() AS reqs,
            countIf(status >= 400) AS errors,
            avg(nullIf(first_byte_ms, 0)) / 1000 AS avg_ttft_s,
            quantileDeterministic(0.5)(duration_ms, cityHash64(request_id)) / 1000 AS p50_s,
            quantileDeterministic(0.95)(duration_ms, cityHash64(request_id)) / 1000 AS p95_s,
            sum(cost_usd) AS cost_usd
        FROM yalr.request_logs
        WHERE timestamp >= now() - INTERVAL 60 SECOND
        GROUP BY provider, provider_name
        ORDER BY reqs DESC
    "#;

    #[derive(clickhouse::Row, serde::Deserialize)]
    struct LiveRow {
        provider: String,
        provider_name: String,
        reqs: u64,
        errors: u64,
        avg_ttft_s: Option<f64>,
        p50_s: f64,
        p95_s: f64,
        cost_usd: f64,
    }

    let rows = state
        .ch
        .query(sql)
        .fetch_all::<LiveRow>()
        .await
        .map_err(|e| e.to_string())?;

    // 1h-Lookback fuer decode/prefill-TPS pro (provider, provider_name),
    // identische Guard-Klauseln wie in live_stats.
    let rate_sql = r#"
        SELECT
            provider,
            provider_name,
            sumIf(completion_tokens, is_stream = 1 AND completion_tokens > 0 AND duration_ms > first_byte_ms AND status < 400) AS completion_sum,
            sumIf(duration_ms - first_byte_ms, is_stream = 1 AND completion_tokens > 0 AND duration_ms > first_byte_ms AND status < 400) AS decode_ms_sum,
            sumIf(prompt_tokens, is_stream = 1 AND prompt_tokens > 0 AND first_byte_ms > 0 AND status < 400) AS prompt_sum,
            sumIf(first_byte_ms, is_stream = 1 AND prompt_tokens > 0 AND first_byte_ms > 0 AND status < 400) AS prefill_ms_sum
        FROM yalr.request_logs
        WHERE timestamp >= now() - INTERVAL 3600 SECOND
        GROUP BY provider, provider_name
    "#;

    #[derive(clickhouse::Row, serde::Deserialize)]
    struct LiveRateRow {
        provider: String,
        provider_name: String,
        completion_sum: u64,
        decode_ms_sum: u64,
        prompt_sum: u64,
        prefill_ms_sum: u64,
    }

    let rate_rows = state
        .ch
        .query(rate_sql)
        .fetch_all::<LiveRateRow>()
        .await
        .map_err(|e| e.to_string())?;

    // Rates pro Gruppe via rates_from_sums (bereits Token/s, keine 3600-Division noetig).
    let rates: std::collections::HashMap<(String, String), (Option<f64>, Option<f64>)> =
        rate_rows
            .iter()
            .map(|r| {
                let s = RateSums {
                    completion: r.completion_sum,
                    decode_ms: r.decode_ms_sum,
                    prompt: r.prompt_sum,
                    prefill_ms: r.prefill_ms_sum,
                };
                ((r.provider.clone(), r.provider_name.clone()), rates_from_sums(&s))
            })
            .collect();

    let groups: Vec<LiveGroup> = rows
        .into_iter()
        .map(|r| {
            let (decode_tps, prefill_tps) = rates
                .get(&(r.provider.clone(), r.provider_name.clone()))
                .cloned()
                .unwrap_or((None, None));
            LiveGroup {
                provider: r.provider,
                provider_name: r.provider_name,
                reqs: r.reqs,
                errors: r.errors,
                cost_usd: r.cost_usd,
                avg_ttft_s: r.avg_ttft_s,
                p50_s: r.p50_s,
                p95_s: r.p95_s,
                decode_tps,
                prefill_tps,
            }
        })
        .collect();

    Ok(render_live_exposition(&groups))
}

/// Baut die Upstream-Sektion aus den Provider-/metrics-Endpunkten (nutzt den
/// geteilten 2s-Cache). Laedt die enabled Provider direkt aus Postgres und
/// holt die /metrics-Werte OHNE den Delta-Raten-Pfad von live_stats
/// (PREV_SAMPLES wird nicht angefasst). Ein PG-Fehler wird als Err
/// durchgereicht, damit der Task den alten Stand behaelt und nicht "0
/// Provider" rendert.
pub async fn build_upstream_body(state: &AppState) -> Result<String, String> {
    let rows = load_enabled_providers(state).await?;
    let providers_enabled = rows.len();
    let (entries, _from_cache) = fetch_and_parse_provider_metrics(state, rows).await;

    let mut groups: Vec<UpstreamGroup> = entries
        .iter()
        .map(|(name, e)| UpstreamGroup {
            provider: e.kind.clone(),
            provider_name: name.clone(),
            queued: e.queued,
            running: e.running,
            kv_cache_usage: e.kv_cache_usage,
            fetch_ok: e.fetch_ok,
        })
        .collect();
    // stabile Reihenfolge: alphabetisch absteigend nach provider_name
    groups.sort_by(|a, b| b.provider_name.cmp(&a.provider_name));

    Ok(render_upstream_exposition(&groups, providers_enabled))
}

/// Rendert die counter-basierten Metriken (since process start, ohne CH).
pub fn render_counter_exposition(
    entries: &[ingest::CounterEntry],
    auth_failures: u64,
    dropped: u64,
) -> String {
    let mut body = String::new();

    body.push_str("# HELP yalr_requests_total Total LLM requests since process start (status not in 401, 402)\n");
    body.push_str("# TYPE yalr_requests_total counter\n");
    for e in entries {
        body.push_str(&format!(
            "yalr_requests_total{{provider=\"{}\", provider_name=\"{}\", model=\"{}\"}} {}\n",
            escape_label(&e.provider),
            escape_label(&e.provider_name),
            escape_label(&e.model),
            e.requests
        ));
    }

    body.push_str("# HELP yalr_errors_total Total failed LLM requests since process start (status >= 400; auth failures excluded)\n");
    body.push_str("# TYPE yalr_errors_total counter\n");
    for e in entries {
        body.push_str(&format!(
            "yalr_errors_total{{provider=\"{}\", provider_name=\"{}\", model=\"{}\"}} {}\n",
            escape_label(&e.provider),
            escape_label(&e.provider_name),
            escape_label(&e.model),
            e.errors
        ));
    }

    body.push_str("# HELP yalr_prompt_tokens_total Total prompt tokens since process start\n");
    body.push_str("# TYPE yalr_prompt_tokens_total counter\n");
    for e in entries {
        body.push_str(&format!(
            "yalr_prompt_tokens_total{{provider=\"{}\", provider_name=\"{}\", model=\"{}\"}} {}\n",
            escape_label(&e.provider),
            escape_label(&e.provider_name),
            escape_label(&e.model),
            e.prompt_tokens
        ));
    }

    body.push_str(
        "# HELP yalr_completion_tokens_total Total completion tokens since process start\n",
    );
    body.push_str("# TYPE yalr_completion_tokens_total counter\n");
    for e in entries {
        body.push_str(&format!(
            "yalr_completion_tokens_total{{provider=\"{}\", provider_name=\"{}\", model=\"{}\"}} {}\n",
            escape_label(&e.provider),
            escape_label(&e.provider_name),
            escape_label(&e.model),
            e.completion_tokens
        ));
    }

    body.push_str("# HELP yalr_cost_usd_total Total cost in USD since process start\n");
    body.push_str("# TYPE yalr_cost_usd_total counter\n");
    for e in entries {
        let cost = e.cost_usd_micros as f64 / 1_000_000.0;
        body.push_str(&format!(
            "yalr_cost_usd_total{{provider=\"{}\", provider_name=\"{}\", model=\"{}\"}} {}\n",
            escape_label(&e.provider),
            escape_label(&e.provider_name),
            escape_label(&e.model),
            fmt_f64(cost)
        ));
    }

    body.push_str("# HELP yalr_auth_failures_total Total auth failures (status 401/402) since process start\n");
    body.push_str("# TYPE yalr_auth_failures_total counter\n");
    body.push_str(&format!("yalr_auth_failures_total {}\n", auth_failures));

    body.push_str(
        "# HELP yalr_ingest_dropped_logs_total Total log entries dropped due to overflow or flush failure\n",
    );
    body.push_str("# TYPE yalr_ingest_dropped_logs_total counter\n");
    body.push_str(&format!(
        "yalr_ingest_dropped_logs_total {}\n",
        dropped
    ));

    body
}

/// Rendert die Snapshot-Metriken (clickhouse_up + Age des letzten Builds).
/// Vor dem ersten Snapshot-Bau (clickhouse_up == None) wird die
/// clickhouse_up-Zeile ausgelassen — noch ist kein Up-/Down-Zustand bekannt.
pub fn render_snapshot_exposition(snap: &common::state::MetricsSnapshot) -> String {
    let mut body = String::new();

    body.push_str("# HELP yalr_clickhouse_up Whether ClickHouse is reachable (1=up, 0=down)\n");
    body.push_str("# TYPE yalr_clickhouse_up gauge\n");
    if let Some(up) = snap.clickhouse_up {
        body.push_str(&format!(
            "yalr_clickhouse_up {}\n",
            if up { 1 } else { 0 }
        ));
    }

    push_age_metric(
        &mut body,
        "yalr_metrics_snapshot_age_seconds",
        "Age of the last successful latency snapshot in seconds",
        snap.latency_built_at,
    );
    push_age_metric(
        &mut body,
        "yalr_live_metrics_age_seconds",
        "Age of the last successful live-metrics snapshot in seconds",
        snap.live_built_at,
    );
    push_age_metric(
        &mut body,
        "yalr_upstream_metrics_age_seconds",
        "Age of the last successful upstream-metrics snapshot in seconds",
        snap.upstream_built_at,
    );

    body
}

/// Hängt HELP/TYPE + (falls vorhanden) die Age-Wertzeile einer
/// Snapshot-Metrik an (Dedup der drei Age-Blöcke).
fn push_age_metric(body: &mut String, metric: &str, help: &str, built_at: Option<DateTime<Utc>>) {
    body.push_str(&format!("# HELP {metric} {help}\n"));
    body.push_str(&format!("# TYPE {metric} gauge\n"));
    if let Some(built_at) = built_at {
        let age_s = Utc::now().signed_duration_since(built_at).num_seconds().max(0) as f64;
        body.push_str(&format!("{metric} {}\n", fmt_f64(age_s)));
    }
}

/// Konkatentiert die /metrics-Sektionen in fester Reihenfolge (reine
/// Funktion, testbar): Latenz (24h), Live (60s), Upstream (30s),
/// Upstream-Counter (on-demand), Snapshot-Metadaten, In-Flight, kumulative
/// Counter.
pub fn compose_metrics_body(
    latency: &str,
    live: &str,
    upstream: &str,
    upstream_counters: &str,
    snapshot: &str,
    in_flight: &str,
    counters: &str,
) -> String {
    let mut body = String::with_capacity(
        latency
            .len()
            + live
            .len()
            + upstream
            .len()
            + upstream_counters
            .len()
            + snapshot
            .len()
            + in_flight
            .len()
            + counters
            .len(),
    );
    body.push_str(latency);
    body.push_str(live);
    body.push_str(upstream);
    body.push_str(upstream_counters);
    body.push_str(snapshot);
    body.push_str(in_flight);
    body.push_str(counters);
    body
}

/// GET /metrics - Prometheus-Endpoint (kein Session-Schutz, optional Token).
pub async fn metrics_handler(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Some(token) = &state.metrics_token {
        let provided = headers
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            // Bearer-Präfix case-insensitiv, analog common::auth::extract_api_key
            .and_then(|v| {
                v.strip_prefix("Bearer ")
                    .or_else(|| v.strip_prefix("bearer "))
            });
        // Timing-sicherer Vergleich: beide Seiten SHA-256 hashen (fixlanger
        // Hex-Digest) und vergleichen — kein Plaintext-String-Vergleich.
        let expected = common::auth::hash_key(token);
        let ok = provided
            .map(|p| common::auth::hash_key(p) == expected)
            .unwrap_or(false);
        if !ok {
            return (StatusCode::UNAUTHORIZED, "unauthorized").into_response();
        }
    }

    let snap = {
        let g = state.metrics_snapshot.read().await;
        g.clone()
    };

    let (entries, auth_failures) = state.log_sink.snapshot_counters();
    let dropped = state.log_sink.dropped_count();
    let in_flight = state.log_sink.in_flight_snapshot();

    let snapshot_part = render_snapshot_exposition(&snap);
    let in_flight_part = render_in_flight_exposition(&in_flight);
    let counter_part = render_counter_exposition(&entries, auth_failures, dropped);
    // Upstream-Engine-Counter (on-demand, 1s-Scrape-Cache): PG-Fehler laesst
    // die Sektion leer (debug, da pro Scrape).
    let entries = upstream_scrape_entries(&state).await;
    // Rate-Baselines fortschreiben: Lock wird nur kurz zum Map-Read/-Write
    // gehalten, advance_rate_state laeuft sync auerhalb der kritischen Sektion.
    let rates: Vec<(Uuid, UpstreamRateState)> = {
        let mut old_states = HashMap::new();
        {
            let map = UPSTREAM_RATE_BASELINES.lock().await;
            for (_, e) in &entries {
                if !e.provider_id.is_nil() {
                    if let Some(st) = map.get(&e.provider_id) {
                        old_states.insert(e.provider_id, st.clone());
                    }
                }
            }
        }
        let mut rates = Vec::new();
        for (_, e) in &entries {
            if e.provider_id.is_nil() {
                continue; // kein Rate-Tracking ohne provider_id
            }
            let old = old_states.get(&e.provider_id);
            rates.push((e.provider_id, advance_rate_state(old, e)));
        }
        {
            let mut map = UPSTREAM_RATE_BASELINES.lock().await;
            for (id, st) in &rates {
                map.insert(*id, st.clone());
            }
        }
        rates
    };
    let upstream_counter_part = render_upstream_token_counters(&entries, &rates);
    // Sektionen in fester Reihenfolge: Latenz (24h), Live (60s), Upstream,
    // Upstream-Counter (on-demand), Snapshot-Metadaten, on-demand In-Flight,
    // kumulative Counter.
    let body = compose_metrics_body(
        &snap.latency_body,
        &snap.live_body,
        &snap.upstream_body,
        &upstream_counter_part,
        &snapshot_part,
        &in_flight_part,
        &counter_part,
    );

    (
        StatusCode::OK,
        [(axum::http::header::CONTENT_TYPE, "text/plain; version=0.0.4; charset=utf-8")],
        body,
    )
        .into_response()
}

#[cfg(test)]
mod metrics_tests {
    use super::{escape_label, fmt_f64};
    use chrono::Utc;

    #[test]
    fn test_fmt_f64_no_scientific_notation() {
        // 1e-7 liegt unter der Mikro-Präzision (6 Stellen) -> rundet auf 0;
        // entscheidend ist: keine wissenschaftliche Notation.
        assert_eq!(fmt_f64(0.0000001), "0");
        assert_eq!(fmt_f64(0.000001), "0.000001");
        assert_eq!(fmt_f64(0.001234), "0.001234");
        assert_eq!(fmt_f64(1.5), "1.5");
        assert_eq!(fmt_f64(2.0), "2");
        assert_eq!(fmt_f64(0.000123456), "0.000123");
    }

    #[test]
    fn test_escape_label() {
        assert_eq!(escape_label("a\\b"), "a\\\\b");
        assert_eq!(escape_label("a\"b"), "a\\\"b");
        assert_eq!(escape_label("a\nb"), "a\\nb");
        assert_eq!(escape_label("normal"), "normal");
    }

    #[test]
    fn test_escape_label_combined() {
        assert_eq!(escape_label("\\\"\n"), "\\\\\\\"\\n");
    }

    #[test]
    fn test_render_counter_exposition_format_and_escaping() {
        let entries = vec![ingest::CounterEntry {
            provider: "openai\n\"x\"".into(),
            provider_name: "DGX \"1\"\\cluster".into(),
            model: "gpt-4o".into(),
            requests: 3,
            errors: 1,
            prompt_tokens: 12,
            completion_tokens: 7,
            cost_usd_micros: 1_234_000,
        }];
        let body = super::render_counter_exposition(&entries, 2, 5);

        // HELP/TYPE vorhanden und korrekt geordnet
        assert!(body.contains("# HELP yalr_requests_total "));
        assert!(body.contains("# TYPE yalr_requests_total counter"));
        assert!(body.contains("# TYPE yalr_cost_usd_total counter"));
        assert!(body.contains("# TYPE yalr_auth_failures_total counter"));

        // Label-Escaping: \n und \" in provider, \\" und \\ in provider_name
        assert!(body.contains(r##"provider="openai\n\"x\""##));
        assert!(!body.contains("openai\n\"x\""));
        assert!(body.contains(r##"provider_name="DGX \"1\"\\cluster"##));

        // Werte + f64 ohne wissenschaftliche Notation (1.234 USD)
        assert!(body.contains(
            "yalr_requests_total{provider=\"openai\\n\\\"x\\\"\", provider_name=\"DGX \\\"1\\\"\\\\cluster\", model=\"gpt-4o\"} 3"
        ));
        assert!(body.contains("yalr_cost_usd_total{provider=\"openai\\n\\\"x\\\"\", provider_name=\"DGX \\\"1\\\"\\\\cluster\", model=\"gpt-4o\"} 1.234"));
        assert!(body.contains("yalr_auth_failures_total 2"));
        assert!(body.contains("yalr_ingest_dropped_logs_total 5"));
    }

    #[test]
    fn test_render_counter_exposition_cost_no_scientific() {
        let entries = vec![ingest::CounterEntry {
            provider: "p".into(),
            provider_name: "n".into(),
            model: "m".into(),
            requests: 1,
            errors: 0,
            prompt_tokens: 0,
            completion_tokens: 0,
            cost_usd_micros: 5,
        }];
        let body = super::render_counter_exposition(&entries, 0, 0);
        // 5 Mikro-USD = 0.000005, kein "5e-6"
        assert!(body.contains(
            "yalr_cost_usd_total{provider=\"p\", provider_name=\"n\", model=\"m\"} 0.000005"
        ));
        assert!(!body.contains("e-"));
    }

    #[test]
    fn test_render_counter_exposition_empty_provider_name() {
        // gateway-bucket: provider_name="" wird als leeres Label gerendert
        let entries = vec![ingest::CounterEntry {
            provider: "gateway".into(),
            provider_name: String::new(),
            model: String::new(),
            requests: 1,
            errors: 1,
            prompt_tokens: 0,
            completion_tokens: 0,
            cost_usd_micros: 0,
        }];
        let body = super::render_counter_exposition(&entries, 0, 0);
        assert!(body.contains(
            "yalr_requests_total{provider=\"gateway\", provider_name=\"\", model=\"\"} 1"
        ));
        assert!(body.contains(
            "yalr_errors_total{provider=\"gateway\", provider_name=\"\", model=\"\"} 1"
        ));
    }

    #[test]
    fn test_render_counter_exposition_empty() {
        let body = super::render_counter_exposition(&[], 0, 0);
        // Auch ohne Eintraege werden HELP/TYPE und globale Counter gerendert
        assert!(body.contains("# TYPE yalr_requests_total counter"));
        assert!(body.contains("yalr_auth_failures_total 0"));
    }

    #[test]
    fn test_render_snapshot_exposition_up_and_age() {
        // Default (noch kein Snapshot-Bau): clickhouse_up-Zeile fehlt komplett,
        // keine irrefuehrende 0.
        let snap = common::state::MetricsSnapshot::default();
        let body = super::render_snapshot_exposition(&snap);
        assert!(!body.contains("yalr_clickhouse_up 0"));
        assert!(!body.contains("yalr_clickhouse_up 1"));
        assert!(!body.contains("yalr_metrics_snapshot_age_seconds 0"));
        assert!(!body.contains("yalr_live_metrics_age_seconds 0"));
        assert!(!body.contains("yalr_upstream_metrics_age_seconds 0"));

        let mut snap = snap;
        snap.clickhouse_up = Some(true);
        snap.latency_built_at = Some(Utc::now());
        snap.live_built_at = Some(Utc::now());
        snap.upstream_built_at = Some(Utc::now());
        let body = super::render_snapshot_exposition(&snap);
        assert!(body.contains("yalr_clickhouse_up 1"));
        assert!(body.contains("yalr_metrics_snapshot_age_seconds "));
        assert!(body.contains("yalr_live_metrics_age_seconds "));
        assert!(body.contains("yalr_upstream_metrics_age_seconds "));

        snap.clickhouse_up = Some(false);
        snap.latency_built_at = None;
        snap.live_built_at = None;
        snap.upstream_built_at = None;
        let body = super::render_snapshot_exposition(&snap);
        assert!(body.contains("yalr_clickhouse_up 0"));
        // ohne built_at: keine Age-Zeilen
        assert!(!body.contains("yalr_metrics_snapshot_age_seconds 0"));
        assert!(!body.contains("yalr_live_metrics_age_seconds 0"));
        assert!(!body.contains("yalr_upstream_metrics_age_seconds 0"));
    }

    #[test]
    fn test_render_live_exposition_normal() {
        let groups = vec![super::LiveGroup {
            provider: "openai_compat".into(),
            provider_name: "DGX 1".into(),
            reqs: 10,
            errors: 2,
            cost_usd: 1.234567,
            // Werte kommen bereits in Sekunden aus der Query (ms / 1000)
            avg_ttft_s: Some(0.5),
            p50_s: 1.0,
            p95_s: 2.25,
            decode_tps: Some(120.5),
            prefill_tps: Some(480.25),
        }];
        let body = super::render_live_exposition(&groups);
        assert!(body.contains("# TYPE yalr_live_requests gauge"));
        assert!(body.contains("yalr_live_requests{provider=\"openai_compat\", provider_name=\"DGX 1\"} 10"));
        assert!(body.contains("yalr_live_errors{provider=\"openai_compat\", provider_name=\"DGX 1\"} 2"));
        assert!(body.contains("yalr_live_cost_usd{provider=\"openai_compat\", provider_name=\"DGX 1\"} 1.234567"));
        // quantile-Labels + Werte in Sekunden (ms->s in der Query)
        assert!(body.contains("yalr_live_duration_seconds{provider=\"openai_compat\", provider_name=\"DGX 1\", quantile=\"0.5\"} 1"));
        assert!(body.contains("yalr_live_duration_seconds{provider=\"openai_compat\", provider_name=\"DGX 1\", quantile=\"0.95\"} 2.25"));
        assert!(body.contains("yalr_live_first_byte_seconds{provider=\"openai_compat\", provider_name=\"DGX 1\"} 0.5"));
        assert!(body.contains("yalr_live_decode_tps{provider=\"openai_compat\", provider_name=\"DGX 1\"} 120.5"));
        assert!(body.contains("yalr_live_prefill_tps{provider=\"openai_compat\", provider_name=\"DGX 1\"} 480.25"));
    }

    #[test]
    fn test_render_live_exposition_empty() {
        // Leere Eingabe -> leerer Body (Prometheus-Staleness, Serien verschwinden)
        assert_eq!(super::render_live_exposition(&[]), "");
    }

    #[test]
    fn test_render_live_exposition_escaping_and_none() {
        let groups = vec![super::LiveGroup {
            provider: "p\n\"x\"".into(),
            provider_name: "N".into(),
            reqs: 1,
            errors: 0,
            cost_usd: 0.0,
            avg_ttft_s: None,
            p50_s: 0.0,
            p95_s: 0.0,
            decode_tps: None,
            prefill_tps: None,
        }];
        let body = super::render_live_exposition(&groups);
        // Label-Escaping: \n und \"
        assert!(body.contains(r##"provider="p\n\"x\""##));
        assert!(!body.contains("p\n\"x\""));
        // cost 0.0 -> "0" (fmt_f64 trimmt trailing Zeros)
        assert!(body.contains("yalr_live_cost_usd{provider=\"p\\n\\\"x\\\"\", provider_name=\"N\"} 0"));
        // None-Werte: keine Daten-Zeilen, aber HELP/TYPE vorhanden
        assert!(!body.contains("yalr_live_first_byte_seconds{"));
        assert!(!body.contains("yalr_live_decode_tps{"));
        assert!(!body.contains("yalr_live_prefill_tps{"));
        assert!(body.contains("# TYPE yalr_live_first_byte_seconds gauge"));
    }

    #[test]
    fn test_render_upstream_exposition_normal() {
        let groups = vec![
            super::UpstreamGroup {
                provider: "vllm".into(),
                provider_name: "DGX \"A\"".into(),
                queued: Some(3.0),
                running: Some(2.0),
                kv_cache_usage: Some(0.85),
                fetch_ok: true,
            },
            super::UpstreamGroup {
                provider: "openai_compat".into(),
                provider_name: "OpenAI".into(),
                queued: None,
                running: None,
                kv_cache_usage: None,
                fetch_ok: false,
            },
        ];
        let body = super::render_upstream_exposition(&groups, 5);
        assert!(body.contains("yalr_upstream_queued{provider=\"vllm\", provider_name=\"DGX \\\"A\\\"\"} 3"));
        assert!(body.contains("yalr_upstream_running{provider=\"vllm\", provider_name=\"DGX \\\"A\\\"\"} 2"));
        assert!(body.contains("yalr_upstream_kv_cache_usage{provider=\"vllm\", provider_name=\"DGX \\\"A\\\"\"} 0.85"));
        assert!(body.contains("yalr_upstream_up{provider=\"vllm\", provider_name=\"DGX \\\"A\\\"\"} 1"));
        assert!(body.contains("yalr_upstream_up{provider=\"openai_compat\", provider_name=\"OpenAI\"} 0"));
        // fehlende Werte (None) werden nicht gerendert, nie NaN
        assert!(!body.contains("yalr_upstream_queued{provider=\"openai_compat\""));
        assert!(!body.contains("NaN"));
        assert!(body.contains("yalr_providers_enabled 5"));
    }

    #[test]
    fn test_render_upstream_exposition_empty() {
        // Bekannte, aber leere Provider-Liste: Header + providers_enabled 0,
        // Datenzeilen der anderen Familien bleiben absent.
        let body = super::render_upstream_exposition(&[], 0);
        assert!(body.contains("# HELP yalr_providers_enabled"));
        assert!(body.contains("# TYPE yalr_providers_enabled gauge"));
        assert!(body.contains("yalr_providers_enabled 0"));
        assert!(body.contains("# TYPE yalr_upstream_queued gauge"));
        assert!(body.contains("# TYPE yalr_upstream_running gauge"));
        assert!(body.contains("# TYPE yalr_upstream_kv_cache_usage gauge"));
        assert!(body.contains("# TYPE yalr_upstream_up gauge"));
        assert!(!body.contains("yalr_upstream_queued{"));
        assert!(!body.contains("yalr_upstream_running{"));
        assert!(!body.contains("yalr_upstream_kv_cache_usage{"));
        assert!(!body.contains("yalr_upstream_up{"));
    }

    #[test]
    fn test_render_upstream_exposition_providers_enabled_mismatch() {
        // providers_enabled kommt aus der DB, nicht aus der Gruppen-Liste:
        // Provider ohne metrics_url taucht in yalr_upstream_up auf (0), nicht
        // in queued/running; enabled-Zaehl bleibt 2.
        let groups = vec![super::UpstreamGroup {
            provider: "openai_compat".into(),
            provider_name: "NoUrl".into(),
            queued: None,
            running: None,
            kv_cache_usage: None,
            fetch_ok: false,
        }];
        let body = super::render_upstream_exposition(&groups, 2);
        assert!(body.contains("yalr_providers_enabled 2"));
        assert!(body.contains("yalr_upstream_up{provider=\"openai_compat\", provider_name=\"NoUrl\"} 0"));
    }

    #[test]
    fn test_render_in_flight_exposition() {
        let mk = |id: &str, prov: &str, name: &str| ingest::InFlightReq {
            request_id: id.into(),
            provider: prov.into(),
            provider_name: name.into(),
            model: "m".into(),
            key_name: "k".into(),
            started_at_ms: 0,
            first_byte_ms: None,
        };
        let in_flight = vec![
            mk("a", "p1", "N1"),
            mk("b", "p1", "N1"),
            mk("c", "p2", "N2"),
            // Label-Werte, die Escaping erfordern (\n und ")
            mk("d", "p\n3", "N\"4"),
        ];
        let body = super::render_in_flight_exposition(&in_flight);
        // pro (provider, provider_name) gezählt
        assert!(body.contains("yalr_in_flight_requests{provider=\"p1\", provider_name=\"N1\"} 2"));
        assert!(body.contains("yalr_in_flight_requests{provider=\"p2\", provider_name=\"N2\"} 1"));
        // Label-Escaping: \n und " werden escaped, rohe Werte tauchen nie auf
        assert!(body.contains(r##"yalr_in_flight_requests{provider="p\n3", provider_name="N\"4"} 1"##));
        assert!(!body.contains("p\n3"));
        assert!(!body.contains("N\"4"));

        // leer: Header vorhanden, keine Daten-Zeilen
        let body = super::render_in_flight_exposition(&[]);
        assert!(body.contains("# TYPE yalr_in_flight_requests gauge"));
        assert!(!body.contains("yalr_in_flight_requests{"));
    }

    #[test]
    fn test_compose_metrics_body_order_and_empty_sections() {
        let body = super::compose_metrics_body(
            "yalr_request_duration_seconds{quantile=\"0.5\"} 1\n",
            "yalr_live_requests{provider=\"p\"} 2\n",
            "yalr_upstream_up{provider=\"p\"} 1\n",
            "yalr_upstream_prompt_tokens_total{provider=\"p\"} 5\n",
            "yalr_clickhouse_up 1\n",
            "yalr_in_flight_requests{provider=\"p\"} 3\n",
            "yalr_requests_total 4\n",
        );
        // alle Sektionen vorhanden
        assert!(body.contains("yalr_request_duration_seconds{quantile=\"0.5\"} 1"));
        assert!(body.contains("yalr_live_requests{provider=\"p\"} 2"));
        assert!(body.contains("yalr_upstream_up{provider=\"p\"} 1"));
        assert!(body.contains("yalr_upstream_prompt_tokens_total{provider=\"p\"} 5"));
        assert!(body.contains("yalr_clickhouse_up 1"));
        assert!(body.contains("yalr_in_flight_requests{provider=\"p\"} 3"));
        assert!(body.contains("yalr_requests_total 4"));
        // feste Reihenfolge: Latenz < Live < Upstream < Upstream-Counter <
        // Snapshot < In-Flight < Counter
        let pos = |s: &str| body.find(s).expect("Sektion fehlt");
        assert!(pos("yalr_request_duration_seconds") < pos("yalr_live_requests"));
        assert!(pos("yalr_live_requests") < pos("yalr_upstream_up"));
        assert!(pos("yalr_upstream_up") < pos("yalr_upstream_prompt_tokens_total"));
        assert!(pos("yalr_upstream_prompt_tokens_total") < pos("yalr_clickhouse_up"));
        assert!(pos("yalr_clickhouse_up") < pos("yalr_in_flight_requests"));
        assert!(pos("yalr_in_flight_requests") < pos("yalr_requests_total"));

        // Die drei neuen Gauge-Familien sitzen innerhalb der On-Demand-
        // Upstream-Counter-Sektion, nach den Counter-Familien.
        const SAMPLE: &str = r#"yalr_upstream_prompt_tokens_total{provider="p"} 5
yalr_upstream_generation_tokens_total{provider="p"} 7
yalr_upstream_prefill_tps{provider="p"} 20
yalr_upstream_decode_tps{provider="p"} 10
yalr_upstream_rate_window_seconds{provider="p"} 2.5
"#;
        let body = super::compose_metrics_body(
            "",
            "",
            "",
            SAMPLE,
            "yalr_clickhouse_up 1\n",
            "",
            "yalr_requests_total 4\n",
        );
        let pos2 = |s: &str| body.find(s).expect("Sektion fehlt");
        assert!(pos2("yalr_upstream_prompt_tokens_total") < pos2("yalr_upstream_prefill_tps"));
        assert!(pos2("yalr_upstream_generation_tokens_total") < pos2("yalr_upstream_prefill_tps"));
        assert!(pos2("yalr_upstream_prefill_tps") < pos2("yalr_upstream_decode_tps"));
        assert!(pos2("yalr_upstream_decode_tps") < pos2("yalr_upstream_rate_window_seconds"));
        assert!(pos2("yalr_upstream_rate_window_seconds") < pos2("yalr_clickhouse_up"));

        // leere Sektionen: Rest unveraendert in richtiger Reihenfolge
        let body = super::compose_metrics_body(
            "", "", "", "", "yalr_clickhouse_up 1\n", "", "yalr_requests_total 4\n",
        );
        assert_eq!(body, "yalr_clickhouse_up 1\nyalr_requests_total 4\n");
    }

    /// Test-Doppel: Entry nur mit den fuer die Upstream-Counter relevanten
    /// Feldern befuellt (provider_id/fetched_at sind fuer das reine
    /// Counter-Rendering irrelevant).
    fn upstream_entry(kind: &str, gen: Option<f64>, prompt: Option<f64>) -> super::ProviderMetricsEntry {
        super::ProviderMetricsEntry {
            provider_id: uuid::Uuid::nil(),
            kind: kind.to_string(),
            metrics_url: None,
            queued: None,
            running: None,
            kv_cache_usage: None,
            decode_tps: None,
            decode_tps_live: None,
            prefill_tps_live: None,
            gen_tokens_total: gen,
            prompt_tokens_total: prompt,
            fetched_at: std::time::Instant::now(),
            fetch_ok: true,
        }
    }

    /// Test-Doppel mit explizitem provider_id und fetched_at.
    fn upstream_entry_id(
        id: uuid::Uuid,
        kind: &str,
        gen: Option<f64>,
        prompt: Option<f64>,
        fetched_at: std::time::Instant,
    ) -> super::ProviderMetricsEntry {
        super::ProviderMetricsEntry {
            provider_id: id,
            kind: kind.to_string(),
            metrics_url: None,
            queued: None,
            running: None,
            kv_cache_usage: None,
            decode_tps: None,
            decode_tps_live: None,
            prefill_tps_live: None,
            gen_tokens_total: gen,
            prompt_tokens_total: prompt,
            fetched_at,
            fetch_ok: true,
        }
    }

    fn rate_state(
        prompt_baseline: Option<(std::time::Instant, f64)>,
        gen_baseline: Option<(std::time::Instant, f64)>,
        prefill_tps: Option<f64>,
        decode_tps: Option<f64>,
        decode_window: Option<f64>,
    ) -> super::UpstreamRateState {
        super::UpstreamRateState {
            prompt_baseline,
            gen_baseline,
            prefill_tps,
            decode_tps,
            decode_window,
        }
    }

    #[test]
    fn test_render_upstream_token_counters() {
        // leerer Input -> leerer String (auch keine HELP/TYPE-Zeilen)
        assert_eq!(super::render_upstream_token_counters(&[], &[]), "");

        // normales Rendering: beide Familien, Labels, stabile Reihenfolge
        // (absteigend nach provider_name), Zahlen via fmt_f64
        let entries = vec![
            ("Alpha".to_string(), upstream_entry("openai_compat", Some(100.0), Some(50.5))),
            ("Beta".to_string(), upstream_entry("openai_compat", Some(7.0), Some(3.0))),
        ];
        let body = super::render_upstream_token_counters(&entries, &[]);
        assert!(body.contains(
            "# HELP yalr_upstream_prompt_tokens_total Prompt tokens sampled at scrape time from the provider /metrics endpoint (vllm/sglang raw engine counters, summed over labeled series)"
        ));
        assert!(body.contains("# TYPE yalr_upstream_prompt_tokens_total counter"));
        assert!(body.contains(
            "# HELP yalr_upstream_generation_tokens_total Generation tokens sampled at scrape time from the provider /metrics endpoint (vllm/sglang raw engine counters, summed over labeled series)"
        ));
        assert!(body.contains("# TYPE yalr_upstream_generation_tokens_total counter"));
        assert!(body.contains(
            "yalr_upstream_generation_tokens_total{provider=\"openai_compat\", provider_name=\"Beta\"} 7"
        ));
        assert!(body.contains(
            "yalr_upstream_generation_tokens_total{provider=\"openai_compat\", provider_name=\"Alpha\"} 100"
        ));
        assert!(body.contains(
            "yalr_upstream_prompt_tokens_total{provider=\"openai_compat\", provider_name=\"Beta\"} 3"
        ));
        assert!(body.contains(
            "yalr_upstream_prompt_tokens_total{provider=\"openai_compat\", provider_name=\"Alpha\"} 50.5"
        ));
        // HELP/TYPE vor den Samples
        assert!(body.find("# TYPE yalr_upstream_generation_tokens_total counter")
            .unwrap()
            < body.find("yalr_upstream_generation_tokens_total{").unwrap());
        // absteigende Reihenfolge: Beta vor Alpha
        assert!(body.find("provider_name=\"Beta\"").unwrap()
            < body.find("provider_name=\"Alpha\"").unwrap());

        // None-Wert -> Serie fehlt (kein NaN), andere Familie bleibt
        let entries = vec![(
            "Gamma".to_string(),
            upstream_entry("openai_compat", None, Some(42.0)),
        )];
        let body = super::render_upstream_token_counters(&entries, &[]);
        assert!(!body.contains("generation_tokens_total{"));
        assert!(!body.contains("NaN"));
        assert!(body.contains(
            "yalr_upstream_prompt_tokens_total{provider=\"openai_compat\", provider_name=\"Gamma\"} 42"
        ));

        // Label-Escaping: \\, \", \\n in provider_name
        let entries = vec![(
            "Bad\"Name\nB".to_string(),
            upstream_entry("p\\k", Some(1.0), None),
        )];
        let body = super::render_upstream_token_counters(&entries, &[]);
        assert!(body.contains(r##"yalr_upstream_generation_tokens_total{provider="p\\k", provider_name="Bad\"Name\nB"} 1"##));
        assert!(!body.contains("Bad\"Name"));
    }

    #[test]
    fn test_render_upstream_token_counters_rates() {
        let id_a = uuid::Uuid::parse_str("11111111-1111-4111-8111-111111111111").unwrap();
        let id_b = uuid::Uuid::parse_str("22222222-2222-4222-8222-222222222222").unwrap();
        let t = std::time::Instant::now();
        let entries = vec![
            ("Alpha".to_string(), upstream_entry_id(id_a, "openai_compat", Some(100.0), Some(50.0), t)),
            ("Beta".to_string(), upstream_entry_id(id_b, "openai_compat", Some(7.0), Some(3.0), t)),
        ];
        // Alpha hat Raten, Beta keine -> Beta-Serien komplett abwesend
        let rates = vec![
            (id_a, rate_state(None, None, Some(20.0), Some(10.0), Some(3.5))),
            (id_b, rate_state(None, None, Some(4.0), None, Some(1.0))),
        ];
        let body = super::render_upstream_token_counters(&entries, &rates);
        // alle drei Gauge-Familien mit HELP/TYPE vorhanden
        assert!(body.contains("# HELP yalr_upstream_prefill_tps Prefill-Durchsatz des Providers: "));
        assert!(body.contains("# TYPE yalr_upstream_prefill_tps gauge"));
        assert!(body.contains("# HELP yalr_upstream_decode_tps Decode-Durchsatz des Providers: "));
        assert!(body.contains("# TYPE yalr_upstream_decode_tps gauge"));
        assert!(body.contains("# HELP yalr_upstream_rate_window_seconds Sample-Abstand in Sekunden, \u{00fc}ber den die aktuelle Decode-Rate gemessen wurde."));
        assert!(body.contains("# TYPE yalr_upstream_rate_window_seconds gauge"));
        // Alpha-Serien vorhanden
        assert!(body.contains(
            "yalr_upstream_prefill_tps{provider=\"openai_compat\", provider_name=\"Alpha\"} 20"
        ));
        assert!(body.contains(
            "yalr_upstream_decode_tps{provider=\"openai_compat\", provider_name=\"Alpha\"} 10"
        ));
        assert!(body.contains(
            "yalr_upstream_rate_window_seconds{provider=\"openai_compat\", provider_name=\"Alpha\"} 3.5"
        ));
        // Beta: prefill_tps vorhanden (4.0), decode_tps None -> keine
        // decode-Serie und kein Window (auch wenn decode_window Some waere)
        assert!(body.contains(
            "yalr_upstream_prefill_tps{provider=\"openai_compat\", provider_name=\"Beta\"} 4"
        ));
        assert!(!body.contains("decode_tps{provider=\"openai_compat\", provider_name=\"Beta\"}"));
        assert!(!body.contains("rate_window_seconds{provider=\"openai_compat\", provider_name=\"Beta\"}"));
        assert!(!body.contains("NaN"));
        // Position: die Gauge-Familien folgen auf die Counter-Familien
        let pos = |s: &str| body.find(s).expect("Familie fehlt");
        assert!(pos("yalr_upstream_prompt_tokens_total{") < pos("# HELP yalr_upstream_prefill_tps "));
        assert!(pos("yalr_upstream_generation_tokens_total{") < pos("# HELP yalr_upstream_prefill_tps "));
        assert!(pos("# TYPE yalr_upstream_prefill_tps gauge") < pos("yalr_upstream_prefill_tps{"));
        assert!(pos("# HELP yalr_upstream_prefill_tps ") < pos("# HELP yalr_upstream_decode_tps "));
        assert!(pos("# HELP yalr_upstream_decode_tps ") < pos("# HELP yalr_upstream_rate_window_seconds "));

        // Label-Escaping auch in den Rate-Familien
        let id_c = uuid::Uuid::parse_str("33333333-3333-4333-8333-333333333333").unwrap();
        let entries = vec![("Bad\"Name\nB".to_string(), upstream_entry_id(id_c, "p\\k", Some(1.0), None, t))];
        let rates = vec![(id_c, rate_state(None, None, Some(1.5), None, None))];
        let body = super::render_upstream_token_counters(&entries, &rates);
        assert!(body.contains(r##"yalr_upstream_prefill_tps{provider="p\\k", provider_name="Bad\"Name\nB"} 1.5"##));
        assert!(!body.contains("Bad\"Name"));

        // leerer Input -> leerer String (auch ohne Rates)
        assert_eq!(super::render_upstream_token_counters(&[], &[]), "");
    }

    #[test]
    fn test_advance_rate_state_cold_start() {
        let t = std::time::Instant::now();
        let entry = upstream_entry_id(uuid::Uuid::nil(), "k", Some(100.0), Some(50.0), t);
        let s = super::advance_rate_state(None, &entry);
        // Baselines gesetzt, Raten None
        assert_eq!(s.prompt_baseline, Some((t, 50.0)));
        assert_eq!(s.gen_baseline, Some((t, 100.0)));
        assert_eq!(s.prefill_tps, None);
        assert_eq!(s.decode_tps, None);
        assert_eq!(s.decode_window, None);
    }

    #[test]
    fn test_advance_rate_state_newer_sample_computes_rates() {
        let t0 = std::time::Instant::now();
        let t1 = t0 + std::time::Duration::from_secs(10);
        let old = rate_state(Some((t0, 100.0)), Some((t0, 50.0)), None, None, None);
        let entry = upstream_entry_id(uuid::Uuid::nil(), "k", Some(150.0), Some(300.0), t1);
        let s = super::advance_rate_state(Some(&old), &entry);
        // (300-100)/10s = 20, (150-50)/10s = 10, Fenster exakt 10s
        assert_eq!(s.prefill_tps, Some(20.0));
        assert_eq!(s.decode_tps, Some(10.0));
        assert_eq!(s.decode_window, Some(10.0));
        assert_eq!(s.prompt_baseline, Some((t1, 300.0)));
        assert_eq!(s.gen_baseline, Some((t1, 150.0)));
    }

    #[test]
    fn test_advance_rate_state_same_timestamp_repeats_rates_no_fake_zero() {
        let t0 = std::time::Instant::now();
        let old = rate_state(
            Some((t0, 100.0)),
            Some((t0, 50.0)),
            Some(20.0),
            Some(10.0),
            Some(10.0),
        );
        // Identisches fetched_at, andere Counter-Werte: keine Neuberechnung
        // (verhindert Fake-0 bei gleichem Timestamp / TTL-Kanten-Race).
        let entry = upstream_entry_id(uuid::Uuid::nil(), "k", Some(999.0), Some(888.0), t0);
        let s = super::advance_rate_state(Some(&old), &entry);
        assert_eq!(s.prefill_tps, Some(20.0));
        assert_eq!(s.decode_tps, Some(10.0));
        assert_eq!(s.decode_window, Some(10.0));
        // Baselines unveraendert
        assert_eq!(s.prompt_baseline, Some((t0, 100.0)));
        assert_eq!(s.gen_baseline, Some((t0, 50.0)));
    }

    #[test]
    fn test_advance_rate_state_older_sample_unchanged() {
        let t0 = std::time::Instant::now();
        let t_old = t0 - std::time::Duration::from_secs(5);
        let old = rate_state(
            Some((t0, 100.0)),
            Some((t0, 50.0)),
            Some(20.0),
            Some(10.0),
            Some(10.0),
        );
        let entry = upstream_entry_id(uuid::Uuid::nil(), "k", Some(150.0), Some(300.0), t_old);
        let s = super::advance_rate_state(Some(&old), &entry);
        assert_eq!(s.prefill_tps, Some(20.0));
        assert_eq!(s.decode_tps, Some(10.0));
        assert_eq!(s.decode_window, Some(10.0));
        assert_eq!(s.prompt_baseline, Some((t0, 100.0)));
        assert_eq!(s.gen_baseline, Some((t0, 50.0)));
    }

    #[test]
    fn test_advance_rate_state_reset_zeroes_rate_but_advances_baseline() {
        let t0 = std::time::Instant::now();
        let t1 = t0 + std::time::Duration::from_secs(10);
        let old = rate_state(
            Some((t0, 100.0)),
            Some((t0, 50.0)),
            Some(20.0),
            Some(10.0),
            Some(10.0),
        );
        // Gen-Counter zurueckgesetzt (20 < 50) -> Reset.
        // Prompt 400 statt 300, damit die neu berechnete Prefill-Rate
        // (400-100)/10 = 30 sich vom alten Wert 20 unterscheidet — die
        // Assertion waere sonst vaku (alt == neu berechnet).
        let entry = upstream_entry_id(uuid::Uuid::nil(), "k", Some(20.0), Some(400.0), t1);
        let s = super::advance_rate_state(Some(&old), &entry);
        // Prefill normal neu berechnet: (400-100)/10 = 30
        assert_eq!(s.prefill_tps, Some(30.0));
        // Decode-Reset: Rate + Fenster None, Baseline auf den neuen Wert.
        assert_eq!(s.decode_tps, None);
        assert_eq!(s.decode_window, None);
        assert_eq!(s.gen_baseline, Some((t1, 20.0)));
        assert_eq!(s.prompt_baseline, Some((t1, 400.0)));
    }

    #[test]
    fn test_advance_rate_state_one_counter_missing_only_other_advances() {
        let t0 = std::time::Instant::now();
        let t1 = t0 + std::time::Duration::from_secs(10);
        let old = rate_state(
            Some((t0, 100.0)),
            Some((t0, 50.0)),
            Some(20.0),
            Some(10.0),
            Some(10.0),
        );
        // Nur Gen vorhanden, Prompt None -> nur Gen-Baseline rueckt.
        // Gen 120 statt 150 und t1 20s statt 10s, damit die neu
        // berechnete Decode-Rate (120-50)/20 = 3.5 und das Fenster 20
        // sich vom alten Wert 10 unterscheiden — die Assertions waeren
        // sonst vaku (alt == neu berechnet).
        let t1 = t0 + std::time::Duration::from_secs(20);
        let entry = upstream_entry_id(uuid::Uuid::nil(), "k", Some(120.0), None, t1);
        let s = super::advance_rate_state(Some(&old), &entry);
        // Prompt unveraendert (Baseline + Rate)
        assert_eq!(s.prompt_baseline, Some((t0, 100.0)));
        assert_eq!(s.prefill_tps, Some(20.0));
        // Gen fortgeschritten und neu berechnet
        assert_eq!(s.gen_baseline, Some((t1, 120.0)));
        assert_eq!(s.decode_tps, Some(3.5));
        assert_eq!(s.decode_window, Some(20.0));
    }

    #[test]
    fn test_advance_rate_state_min_elapsed_keeps_rates_advances_baseline() {
        let t0 = std::time::Instant::now();
        let t1 = t0 + std::time::Duration::from_millis(500);
        let old = rate_state(
            Some((t0, 100.0)),
            Some((t0, 50.0)),
            Some(20.0),
            Some(10.0),
            Some(10.0),
        );
        // <1s -> MIN_ELAPSED-Floor: Rate wird nicht neu berechnet, Baseline
        // wird fortgeschrieben, letzte Rates bleiben (transient).
        let entry = upstream_entry_id(uuid::Uuid::nil(), "k", Some(150.0), Some(300.0), t1);
        let s = super::advance_rate_state(Some(&old), &entry);
        assert_eq!(s.prefill_tps, Some(20.0));
        assert_eq!(s.decode_tps, Some(10.0));
        assert_eq!(s.decode_window, Some(10.0));
        assert_eq!(s.prompt_baseline, Some((t1, 300.0)));
        assert_eq!(s.gen_baseline, Some((t1, 150.0)));
    }


    #[test]
    fn test_rates_from_sums_guard() {
        // beides vorhanden: decode und prefill werden gerechnet
        let s = super::RateSums {
            completion: 100,
            decode_ms: 500,
            prompt: 200,
            prefill_ms: 100,
        };
        let (d, p) = super::rates_from_sums(&s);
        assert_eq!(d, Some(100.0 / 500.0 * 1000.0));
        assert_eq!(p, Some(200.0 / 100.0 * 1000.0));

        // Nenner 0 -> None (Guard)
        let s = super::RateSums {
            completion: 100,
            decode_ms: 0,
            prompt: 0,
            prefill_ms: 0,
        };
        let (d, p) = super::rates_from_sums(&s);
        assert_eq!(d, None);
        assert_eq!(p, None);

        // Zaehler 0 -> None
        let s = super::RateSums::default();
        let (d, p) = super::rates_from_sums(&s);
        assert_eq!(d, None);
        assert_eq!(p, None);

        // nur decode vorhanden, prefill fehlt
        let s = super::RateSums {
            completion: 50,
            decode_ms: 100,
            prompt: 0,
            prefill_ms: 0,
        };
        let (d, p) = super::rates_from_sums(&s);
        assert_eq!(d, Some(500.0));
        assert_eq!(p, None);
    }

    #[test]
    fn test_merge_rate_sums_rename_and_empty() {
        // Leere Eingabe -> leeres Map
        let map = super::merge_rate_sums(Vec::<(String, u64, u64, u64, u64)>::new());
        assert!(map.is_empty());

        // Rename-Szenario: zwei CH-Zeilen (alter + neuer Name) mit gleicher provider_id
        // werden auf einen aufgeloesten Namen zusammengefasst und Summen addiert.
        let rows = vec![
            ("Neuer Name".to_string(), 10, 20, 30, 40),
            ("Alter Name".to_string(), 5, 7, 11, 13),
        ];
        let map = super::merge_rate_sums(rows.into_iter());
        assert_eq!(map.len(), 2);
        let neu = &map["Neuer Name"];
        assert_eq!(neu.completion, 10);
        let alt = &map["Alter Name"];
        assert_eq!(alt.completion, 5);

        // gleiche (aufgeloeste) Name -> Summen addieren
        let rows = vec![
            ("Provider X".to_string(), 10, 20, 30, 40),
            ("Provider X".to_string(), 5, 7, 11, 13),
        ];
        let map = super::merge_rate_sums(rows.into_iter());
        assert_eq!(map.len(), 1);
        let s = &map["Provider X"];
        assert_eq!(s.completion, 15);
        assert_eq!(s.decode_ms, 27);
        assert_eq!(s.prompt, 41);
        assert_eq!(s.prefill_ms, 53);
    }

    #[test]
    fn test_filter_in_flight() {
        let mk = |id: &str, key: &str| ingest::InFlightReq {
            request_id: id.into(),
            provider: "p".into(),
            provider_name: "P".into(),
            model: "m".into(),
            key_name: key.into(),
            started_at_ms: 0,
            first_byte_ms: None,
        };

        let snapshot = vec![mk("a", "key1"), mk("b", "key2"), mk("c", "key1")];

        // Ohne Filter: unverändert
        let result = super::filter_in_flight(snapshot.clone(), &None);
        assert_eq!(result.len(), 3);

        // Mit Filter: nur passende Einträge
        let result = super::filter_in_flight(snapshot.clone(), &Some("key1".into()));
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].request_id, "a");
        assert_eq!(result[1].request_id, "c");

        // Filter trifft nichts
        let result = super::filter_in_flight(snapshot, &Some("keyX".into()));
        assert!(result.is_empty());
    }

    #[test]
    fn test_parse_hours_param_all() {
        assert_eq!(super::parse_hours_param(Some("all")), Ok(super::HoursRange::All));
        assert_eq!(super::parse_hours_param(Some("ALL")), Ok(super::HoursRange::All));
        assert_eq!(super::parse_hours_param(Some("  all ")), Ok(super::HoursRange::All));
    }

    #[test]
    fn test_parse_hours_param_number() {
        assert_eq!(super::parse_hours_param(None), Ok(super::HoursRange::Hours(24)));
        assert_eq!(super::parse_hours_param(Some("")), Ok(super::HoursRange::Hours(24)));
        assert_eq!(super::parse_hours_param(Some("   ")), Ok(super::HoursRange::Hours(24)));
        assert_eq!(super::parse_hours_param(Some("24")), Ok(super::HoursRange::Hours(24)));
        assert_eq!(super::parse_hours_param(Some("1")), Ok(super::HoursRange::Hours(1)));
        assert_eq!(super::parse_hours_param(Some("8760")), Ok(super::HoursRange::Hours(8760)));
    }

    #[test]
    fn test_parse_hours_param_invalid() {
        assert!(super::parse_hours_param(Some("0")).is_err());
        assert!(super::parse_hours_param(Some("-5")).is_err());
        assert!(super::parse_hours_param(Some("8761")).is_err());
        assert!(super::parse_hours_param(Some("abc")).is_err());
        assert!(super::parse_hours_param(Some("1.5")).is_err());
    }

    #[test]
    fn test_pick_bucket_hours() {
        // Bis 720h Spanne bleibt 1h-Bucket (bestehendes Verhalten)
        assert_eq!(super::pick_bucket_hours(1), 1);
        assert_eq!(super::pick_bucket_hours(24), 1);
        assert_eq!(super::pick_bucket_hours(720), 1);
        // Ueber 720h -> groessere Bucket
        assert_eq!(super::pick_bucket_hours(721), 3);
        assert_eq!(super::pick_bucket_hours(8760), 24);
        // Sehr groessen Spannen fallen auf 720 zurueck
        assert_eq!(super::pick_bucket_hours(u32::MAX), 720);
    }

    #[test]
    fn test_pick_bucket_hours_boundaries() {
      // Invariante: kleinstes b aus [1,3,6,12,24,168,336,720] mit ceil(span/b) <= 720.
      // span = 720*b  -> b ; span = 720*b + 1 -> naechstgroesseres b (b=720: Fallback 720).
      const TRANSITIONS: [(u32, u32, u32); 8] = [
        (1, 720 * 1, 720 * 1 + 1),
        (3, 720 * 3, 720 * 3 + 1),
        (6, 720 * 6, 720 * 6 + 1),
        (12, 720 * 12, 720 * 12 + 1),
        (24, 720 * 24, 720 * 24 + 1),
        (168, 720 * 168, 720 * 168 + 1),
        (336, 720 * 336, 720 * 336 + 1),
        (720, 720 * 720, 720 * 720 + 1),
      ];
      const BUCKETS: [u32; 8] = [1, 3, 6, 12, 24, 168, 336, 720];
      for (i, (b, at, plusOne)) in TRANSITIONS.iter().enumerate() {
        assert_eq!(super::pick_bucket_hours(*at), *b);
        let expected_next = if i + 1 < BUCKETS.len() { BUCKETS[i + 1] } else { 720 };
        assert_eq!(super::pick_bucket_hours(*plusOne), expected_next);
      }
    }
}

