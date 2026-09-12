//! Auth: Virtual-Key-Extraktion + Hashing.

use axum::http::HeaderMap;
use sha2::{Digest, Sha256};

use crate::state::VirtualKey;

/// Extrahiert den Bearer-Token aus dem Authorization-Header.
pub fn extract_api_key(headers: &HeaderMap) -> Option<String> {
    let auth = headers.get("authorization")?.to_str().ok()?;
    let token = auth
        .strip_prefix("Bearer ")
        .or_else(|| auth.strip_prefix("bearer "))?;
    let token = token.trim();
    if token.is_empty() { None } else { Some(token.to_string()) }
}

/// sha256-hex-hash eines Keys (keys werden nur als hash gespeichert).
pub fn hash_key(key: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(key.as_bytes());
    let digest = hasher.finalize();
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

/// Validiert einen API-Key gegen die DB (via state-cache).
pub async fn authenticate(
    state: &crate::state::AppState,
    headers: &HeaderMap,
) -> Result<VirtualKey, axum::http::StatusCode> {
    let key =
        extract_api_key(headers).ok_or(axum::http::StatusCode::UNAUTHORIZED)?;

    let hash = hash_key(&key);
    let vk = state
        .lookup_key(&hash)
        .await
        .ok_or(axum::http::StatusCode::UNAUTHORIZED)?;

    if state.is_budget_exceeded(&vk).await {
        return Err(axum::http::StatusCode::PAYMENT_REQUIRED);
    }

    Ok(vk)
}
