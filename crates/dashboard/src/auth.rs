//! Admin-Auth: Login, Sessions (Cookie), Bootstrap des initialen Admin-Users.

use axum::http::{HeaderMap, StatusCode};
use rand::Rng;
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use uuid::Uuid;

pub const SESSION_COOKIE: &str = "yalr_session";
const SESSION_TTL_HOURS: i64 = 24 * 7; // 7 Tage

#[derive(sqlx::FromRow)]
struct AdminUserRow {
    id: Uuid,
}

pub async fn ensure_admin_user(
    pg: &PgPool,
    username: &str,
    password: &str,
) -> anyhow::Result<()> {
    let existing: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM admin_users")
        .fetch_one(pg)
        .await?;
    if existing > 0 {
        return Ok(());
    }
    let hash = bcrypt::hash(password, bcrypt::DEFAULT_COST)?;
    sqlx::query("INSERT INTO admin_users (id, username, password_hash) VALUES ($1, $2, $3)")
        .bind(Uuid::new_v4())
        .bind(username)
        .bind(hash)
        .execute(pg)
        .await?;
    tracing::info!("initial admin user '{username}' created");
    Ok(())
}

pub fn hash_token(token: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    let digest = hasher.finalize();
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

/// Erstellt eine Session in der DB und gibt das klartext-cookie-token zurueck.
pub async fn create_session(pg: &PgPool, username: &str) -> anyhow::Result<String> {
    let user = sqlx::query_as::<_, AdminUserRow>(
        "SELECT id FROM admin_users WHERE username = $1",
    )
    .bind(username)
    .fetch_one(pg)
    .await?;

    // 32 random bytes -> 64 hex chars
    let mut raw = [0u8; 32];
    rand::thread_rng().fill(&mut raw);
    let token: String = raw.iter().map(|b| format!("{b:02x}")).collect();
    let token_hash = hash_token(&token);

    let expires = chrono::Utc::now() + chrono::Duration::hours(SESSION_TTL_HOURS);
    sqlx::query("INSERT INTO sessions (token_hash, admin_user_id, expires_at) VALUES ($1, $2, $3)")
        .bind(&token_hash)
        .bind(user.id)
        .bind(expires)
        .execute(pg)
        .await?;
    Ok(token)
}

/// Prueft Session-Cookie gegen die DB.
pub async fn validate_session(pg: &PgPool, headers: &HeaderMap) -> Result<bool, StatusCode> {
    let token = extract_cookie(headers, SESSION_COOKIE).ok_or(StatusCode::UNAUTHORIZED)?;
    let token_hash = hash_token(&token);

    #[derive(sqlx::FromRow)]
    struct SessionRow {
        expires_at: chrono::DateTime<chrono::Utc>,
    }
    let row = sqlx::query_as::<_, SessionRow>(
        "SELECT expires_at FROM sessions WHERE token_hash = $1",
    )
    .bind(&token_hash)
    .fetch_optional(pg)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    match row {
        Some(SessionRow { expires_at }) => Ok(expires_at > chrono::Utc::now()),
        None => Ok(false),
    }
}

/// Loescht eine Session (logout).
pub async fn delete_session(pg: &PgPool, headers: &HeaderMap) -> anyhow::Result<()> {
    if let Some(token) = extract_cookie(headers, SESSION_COOKIE) {
        let token_hash = hash_token(&token);
        sqlx::query("DELETE FROM sessions WHERE token_hash = $1")
            .bind(&token_hash)
            .execute(pg)
            .await?;
    }
    Ok(())
}

pub fn extract_cookie(headers: &HeaderMap, name: &str) -> Option<String> {
    let cookies = headers.get("cookie")?.to_str().ok()?;
    for pair in cookies.split(';') {
        let pair = pair.trim();
        if let Some((k, v)) = pair.split_once('=') {
            if k == name {
                return Some(v.to_string());
            }
        }
    }
    None
}

/// Middleware-aehnlicher helper: 401 wenn keine gueltige Session.
pub async fn require_session(
    state: &common::state::AppState,
    headers: &HeaderMap,
) -> Result<(), StatusCode> {
    match validate_session(&state.pg, headers).await {
        Ok(true) => Ok(()),
        Ok(false) => Err(StatusCode::UNAUTHORIZED),
        Err(e) => Err(e),
    }
}
