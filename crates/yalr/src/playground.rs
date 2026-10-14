//! Dashboard-Playground: schickt Requests ohne API-Key durch die normale
//! Proxy-Pipeline (`proxy_core`). Zugang nur mit Dashboard-Session; die
//! Antwort traegt zusaetzlich die `x-yalr-*`-Routing-Header.

use std::time::Instant;

use axum::extract::{Request, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;
use uuid::Uuid;

use common::state::{AppState, VirtualKey, PLAYGROUND_KEY_ID, PLAYGROUND_KEY_NAME};

use crate::proxy::{provider_override, proxy_core, CoreOpts, MAX_BODY_BYTES};

const CHAT: &str = "/v1/chat/completions";
const SPEECH: &str = "/v1/audio/speech";

/// POST /dashboard-api/playground/chat
pub async fn chat(State(state): State<AppState>, request: Request) -> Response {
    let session = dashboard::auth::require_session(&state, request.headers()).await;
    handle(&state, request, CHAT, session).await
}

/// POST /dashboard-api/playground/speech
pub async fn speech(State(state): State<AppState>, request: Request) -> Response {
    let session = dashboard::auth::require_session(&state, request.headers()).await;
    handle(&state, request, SPEECH, session).await
}

fn error(status: StatusCode, error_type: &str, message: &str) -> Response {
    (
        status,
        axum::Json(json!({"error": {"message": message, "type": error_type}})),
    )
        .into_response()
}

/// Fehler aus `require_session` -> Response: nur 401 ist "nicht eingeloggt",
/// alles andere (z.B. 500 bei DB-Fehler) behaelt seinen Status.
fn session_error(status: StatusCode) -> Response {
    if status == StatusCode::UNAUTHORIZED {
        error(status, "auth", "unauthorized")
    } else {
        error(status, "internal", "session check failed")
    }
}

/// Body-Lesefehler -> Response: Laengengrenze (413) vs. sonstiger Fehler (400).
fn body_error(err: &axum::Error) -> Response {
    if is_length_limit(err) {
        error(
            StatusCode::PAYLOAD_TOO_LARGE,
            "payload_too_large",
            &format!("request body exceeds {} MB", MAX_BODY_BYTES / (1024 * 1024)),
        )
    } else {
        error(StatusCode::BAD_REQUEST, "bad_request", "failed to read request body")
    }
}

/// `to_bytes` meldet die Grenze als `http_body_util::LengthLimitError` in der
/// Source-Kette.
fn is_length_limit(err: &axum::Error) -> bool {
    let mut cur: Option<&(dyn std::error::Error + 'static)> = Some(err);
    while let Some(e) = cur {
        if e.is::<http_body_util::LengthLimitError>() {
            return true;
        }
        cur = e.source();
    }
    false
}

/// Synthetischer Key: kein Budget, keine Scopes, nicht in der DB.
pub fn playground_key() -> VirtualKey {
    VirtualKey {
        id: PLAYGROUND_KEY_ID,
        name: PLAYGROUND_KEY_NAME.to_string(),
        key_hash: String::new(),
        budget_cents: None,
        enabled: true,
        allowed_providers: Vec::new(),
        allowed_models: Vec::new(),
    }
}

/// Header-Pruefungen nach der Session: JSON-Content-Type (415) und
/// Cross-Site-Schutz ueber `Sec-Fetch-Site` (403). `Some` = abgelehnt.
fn check_headers(headers: &HeaderMap) -> Option<Response> {
    let is_json = headers
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(';').next())
        .map(|v| v.trim().eq_ignore_ascii_case("application/json"))
        .unwrap_or(false);
    if !is_json {
        return Some(error(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "unsupported_media_type",
            "content-type must be application/json",
        ));
    }
    if let Some(site) = headers.get("sec-fetch-site") {
        let ok = site
            .to_str()
            .map(|s| s.eq_ignore_ascii_case("same-origin") || s.eq_ignore_ascii_case("none"))
            .unwrap_or(false);
        if !ok {
            return Some(error(StatusCode::FORBIDDEN, "forbidden", "cross-site request rejected"));
        }
    }
    None
}

/// Guard-Reihenfolge: Session (401) -> Header (415/403) -> Body (413) ->
/// `proxy_core`. `session` ist das Ergebnis von `require_session` (fuer Tests
/// injizierbar).
async fn handle(
    state: &AppState,
    request: Request,
    endpoint: &str,
    session: Result<(), StatusCode>,
) -> Response {
    if let Err(status) = session {
        return session_error(status);
    }
    let (parts, body) = request.into_parts();
    if let Some(resp) = check_headers(&parts.headers) {
        return resp;
    }
    let provider_override = provider_override(&parts.headers);

    // Body selbst lesen (keine Extractor-Grenze von 2 MB).
    let body = match axum::body::to_bytes(body, MAX_BODY_BYTES).await {
        Ok(b) => b,
        Err(e) => return body_error(&e),
    };

    proxy_core(
        state,
        playground_key(),
        endpoint.to_string(),
        body,
        Uuid::new_v4().to_string(),
        Instant::now(),
        CoreOpts { provider_override, expose_routing: true },
    )
    .await
}

#[cfg(test)]
pub(crate) async fn handle_for_test(
    state: &AppState,
    request: Request,
    endpoint: &str,
    session: Result<(), StatusCode>,
) -> Response {
    handle(state, request, endpoint, session).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_playground_key_is_unscoped_and_fixed() {
        let k = playground_key();
        assert_eq!(k.id, PLAYGROUND_KEY_ID);
        assert!(!k.id.is_nil());
        assert_eq!(k.name, "playground");
        assert!(k.enabled && k.budget_cents.is_none());
        assert!(k.allowed_models.is_empty() && k.allowed_providers.is_empty());
    }

    async fn json_of(resp: Response) -> serde_json::Value {
        let body = axum::body::to_bytes(resp.into_body(), 1 << 20).await.unwrap();
        serde_json::from_slice(&body).unwrap()
    }

    #[tokio::test]
    async fn test_session_error_mapping() {
        let resp = session_error(StatusCode::UNAUTHORIZED);
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(json_of(resp).await, json!({"error": {"message": "unauthorized", "type": "auth"}}));
        let resp = session_error(StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(
            json_of(resp).await,
            json!({"error": {"message": "session check failed", "type": "internal"}})
        );
    }

    #[tokio::test]
    async fn test_body_error_mapping() {
        // echte Laengengrenze aus to_bytes
        let err = axum::body::to_bytes(axum::body::Body::from(vec![0u8; 4]), 1).await.unwrap_err();
        assert!(is_length_limit(&err));
        let resp = body_error(&err);
        assert_eq!(resp.status(), StatusCode::PAYLOAD_TOO_LARGE);
        assert_eq!(json_of(resp).await["error"]["type"], "payload_too_large");
        // sonstiger Lesefehler (Stream bricht ab)
        let stream = futures::stream::iter(vec![Err::<bytes::Bytes, _>(std::io::Error::other("reset"))]);
        let err = axum::body::to_bytes(axum::body::Body::from_stream(stream), MAX_BODY_BYTES)
            .await
            .unwrap_err();
        assert!(!is_length_limit(&err));
        let resp = body_error(&err);
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        assert_eq!(
            json_of(resp).await,
            json!({"error": {"message": "failed to read request body", "type": "bad_request"}})
        );
    }

    #[test]
    fn test_check_headers() {
        let mut h = HeaderMap::new();
        assert_eq!(check_headers(&h).unwrap().status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
        h.insert("content-type", "text/plain".parse().unwrap());
        assert_eq!(check_headers(&h).unwrap().status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
        h.insert("content-type", "application/json; charset=utf-8".parse().unwrap());
        assert!(check_headers(&h).is_none());
        h.insert("sec-fetch-site", "same-origin".parse().unwrap());
        assert!(check_headers(&h).is_none());
        h.insert("sec-fetch-site", "none".parse().unwrap());
        assert!(check_headers(&h).is_none());
        h.insert("sec-fetch-site", "cross-site".parse().unwrap());
        assert_eq!(check_headers(&h).unwrap().status(), StatusCode::FORBIDDEN);
        h.insert("sec-fetch-site", "same-site".parse().unwrap());
        assert_eq!(check_headers(&h).unwrap().status(), StatusCode::FORBIDDEN);
    }
}
